//! Sends keep-warm requests - `KeepWarmRequester.cs`: a plain GET to the site on this machine ([`LocalSiteTarget`]) -
//! no browser, no proxy, no redirect followed by itself. Each site keeps the cookies it was given, so DNN doesn't hand
//! out a new anonymous cookie (or session) for every request. Only the start of a response is read. A page of DNN's
//! installer is never requested, whatever asks for it.

use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::TlsConfig;
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

use crate::model::{KeepWarmOutcome, KeepWarmOutcomeKind, KeepWarmRequestKind};
use crate::plan::LocalSiteTarget;
use crate::rules;
use crate::settings::is_installer_path;

pub const USER_AGENT: &str = "DnnManager-KeepWarm";

// Read to the end so the page is rendered whole (and compiled), but not more than this of it.
const MAX_BODY_BYTES: usize = 256 * 1024;
// What is looked at for DNN's installer.
const BODY_START_BYTES: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
// Minutes go by between two requests to a site: a connection isn't kept open for the next one.
const IDLE_CONNECTION: Duration = Duration::from_secs(10);

/// Sends the requests and keeps each site's cookies. Shared by the service's request threads.
#[derive(Default)]
pub struct KeepWarmRequester {
    // Per site, by hand: every site is on this machine, and a cookie jar would hand one site's cookies to the others.
    cookies: Mutex<HashMap<String, Vec<(String, String)>>>,
}

/// Connects every request to the target's address, whatever host name the address in the request has: the request
/// is for `http(s)://<host header>/…`, so the Host header and the TLS server name are the site's - as the C#
/// HttpClient does with a Host header set - while the connection goes to this machine, whatever DNS says.
#[derive(Debug)]
struct FixedAddress(SocketAddr);

impl Resolver for FixedAddress {
    fn resolve(&self, _uri: &Uri, _config: &Config, _timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut addresses = self.empty();
        addresses.push(self.0);
        Ok(addresses)
    }
}

impl KeepWarmRequester {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests `path` of `site` - and, for a warm-up, the same site's pages it redirects to - and says what the answers
    /// mean. `throttled`: DNN Manager is in efficiency mode (times measured can be longer than the site took).
    pub fn send(
        &self,
        site: &str,
        target: &LocalSiteTarget,
        path: &str,
        kind: KeepWarmRequestKind,
        throttled: &AtomicBool,
    ) -> KeepWarmOutcome {
        let limit = if kind == KeepWarmRequestKind::WarmUp { rules::WARM_UP_TIMEOUT } else { rules::PING_TIMEOUT };
        let watch = Instant::now();
        let mut was_throttled = throttled.load(Ordering::Relaxed);
        let mut first_answer: Option<Duration> = None;
        let mut requests = 0u32;
        let mut current = path.to_string();

        let outcome = |kind, reason: String, requests, was_throttled, timed_out| KeepWarmOutcome {
            elapsed: watch.elapsed(),
            reason: Some(reason),
            requests,
            throttled: was_throttled,
            timed_out,
            ..KeepWarmOutcome::of(kind)
        };
        let timed_out = |requests, was_throttled| {
            outcome(
                KeepWarmOutcomeKind::NotAnswering,
                format!("the site didn't answer within {}", rules::span(limit)),
                requests,
                was_throttled,
                true,
            )
        };

        let Some(ip) = target.connect_ip() else {
            return outcome(
                KeepWarmOutcomeKind::NotAnswering,
                format!("the site can't be reached: {} is no address", target.connect_host),
                0,
                was_throttled,
                false,
            );
        };
        let agent = agent(SocketAddr::new(ip, target.port));

        let mut hop = 0;
        loop {
            // The last safeguard: whatever the settings or a redirect say - checked as asked for and as it would be sent.
            let sent_path = Url::parse(&target.uri_for(&current)).map(|u| u.path().to_string());
            if is_installer_path(&current) || sent_path.as_deref().map_or(true, is_installer_path) {
                return outcome(
                    KeepWarmOutcomeKind::Failed,
                    format!("{current} is a page of DNN's installer - keep warm never requests it"),
                    requests,
                    was_throttled,
                    false,
                );
            }
            let Some(remaining) = limit.checked_sub(watch.elapsed()).filter(|r| !r.is_zero()) else {
                return timed_out(requests, was_throttled);
            };

            let mut request = agent
                .get(target.display_url(&current))
                .header("Host", &target.host_header)
                .header("Accept", "text/html,*/*");
            if let Some(cookies) = self.cookie_header(site) {
                request = request.header("Cookie", cookies);
            }
            requests += 1;
            let response = match request.config().timeout_global(Some(remaining)).build().call() {
                Ok(response) => response,
                Err(ureq::Error::Timeout(_)) => return timed_out(requests, was_throttled),
                Err(e) => {
                    let refused = matches!(&e, ureq::Error::ConnectionFailed)
                        || matches!(&e, ureq::Error::Io(io) if io.kind() == ErrorKind::ConnectionRefused);
                    let reason = if refused {
                        format!("nothing answers on {}:{}", target.connect_host, target.port)
                    } else {
                        format!("the site can't be reached: {}", first_line(&e.to_string()))
                    };
                    return outcome(KeepWarmOutcomeKind::NotAnswering, reason, requests, was_throttled, false);
                }
            };
            let answered = *first_answer.get_or_insert(watch.elapsed());
            was_throttled |= throttled.load(Ordering::Relaxed);
            let status = response.status().as_u16();

            // Windows' HTTP.sys answered, not the site: IIS has no running site for this address - it was stopped, or its
            // app pool is.
            let from_http_sys = response.headers().get_all("server").iter().filter_map(|v| v.to_str().ok()).any(|v| {
                v.split_whitespace()
                    .any(|p| p.split('/').next().unwrap_or_default().eq_ignore_ascii_case("Microsoft-HTTPAPI"))
            });
            if from_http_sys {
                return KeepWarmOutcome {
                    elapsed: answered,
                    status_code: Some(status),
                    reason: Some(format!(
                        "nothing in IIS answers for {} (HTTP {status}) - the site or its app pool may be stopped",
                        target.host_header
                    )),
                    requests,
                    throttled: was_throttled,
                    ..KeepWarmOutcome::of(KeepWarmOutcomeKind::NotAnswering)
                };
            }

            self.keep_cookies(site, response.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()));
            let location = response.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
            let body = match read_start(response.into_body().into_reader()) {
                Ok(body) => body,
                Err(e) if is_timeout(&e) => return timed_out(requests, was_throttled),
                Err(e) => {
                    return outcome(
                        KeepWarmOutcomeKind::NotAnswering,
                        format!("the request failed: {e}"),
                        requests,
                        was_throttled,
                        false,
                    );
                }
            };
            let Ok(requested) = Url::parse(&target.display_url(&current)) else {
                return outcome(
                    KeepWarmOutcomeKind::NotAnswering,
                    format!("the request failed: {} is no address", target.display_url(&current)),
                    requests,
                    was_throttled,
                    false,
                );
            };
            let verdict = rules::classify(kind, &requested, status, location.as_deref(), &body);
            if let Some(next) = verdict.follow_path
                && hop < rules::MAX_REDIRECTS
            {
                current = next;
                hop += 1;
                continue;
            }
            return KeepWarmOutcome {
                elapsed: answered,
                status_code: Some(status),
                reason: verdict.reason,
                note: verdict.note,
                requests,
                throttled: was_throttled,
                ..KeepWarmOutcome::of(verdict.kind)
            };
        }
    }

    /// Forgets `site`'s cookies - its worker process is gone, and so are the sessions they were for.
    pub fn forget_cookies(&self, site: &str) {
        if let Ok(mut cookies) = self.cookies.lock() {
            cookies.remove(&site.to_lowercase());
        }
    }

    fn cookie_header(&self, site: &str) -> Option<String> {
        let cookies = self.cookies.lock().ok()?;
        let jar = cookies.get(&site.to_lowercase()).filter(|j| !j.is_empty())?;
        Some(jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
    }

    // name=value of each Set-Cookie - its attributes don't matter for one site on one machine; one that is deleted
    // (empty, or expired) goes.
    fn keep_cookies<'a>(&self, site: &str, values: impl Iterator<Item = &'a str>) {
        let Ok(mut cookies) = self.cookies.lock() else { return };
        let mut values = values.peekable();
        if values.peek().is_none() {
            return;
        }
        let jar = cookies.entry(site.to_lowercase()).or_default();
        for value in values {
            let first = value.split(';').next().unwrap_or_default();
            let Some((name, val)) = first.split_once('=') else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let lower = value.to_ascii_lowercase();
            let deleted = val.is_empty()
                || lower.contains("max-age=0")
                || lower.contains("expires=thu, 01-jan-1970")
                || lower.contains("expires=mon, 01-jan-1900");
            let at = jar.iter().position(|(k, _)| k == name);
            if deleted {
                if let Some(i) = at {
                    jar.remove(i);
                }
            } else if val.len() <= 4096 {
                // A handful at most, and none of them large.
                match at {
                    Some(i) => jar[i].1 = val.trim().to_string(),
                    None if jar.len() < 20 => jar.push((name.to_string(), val.trim().to_string())),
                    None => {}
                }
            }
        }
    }
}

fn agent(address: SocketAddr) -> ureq::Agent {
    let config = Config::builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .max_idle_age(IDLE_CONNECTION)
        // A local site's certificate is usually self-signed, or for another name - it is this machine either way.
        .tls_config(TlsConfig::builder().disable_verification(true).build())
        .build();
    ureq::Agent::with_parts(config, DefaultConnector::new(), FixedAddress(address))
}

/// Reads the body to its end (at most [`MAX_BODY_BYTES`]) and returns its start as text.
fn read_start(mut reader: impl Read) -> std::io::Result<String> {
    let mut buffer = [0u8; 16 * 1024];
    let mut start = Vec::with_capacity(BODY_START_BYTES);
    let mut total = 0;
    while total < MAX_BODY_BYTES {
        let want = buffer.len().min(MAX_BODY_BYTES - total);
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        if start.len() < BODY_START_BYTES {
            let take = read.min(BODY_START_BYTES - start.len());
            start.extend_from_slice(&buffer[..take]);
        }
        total += read;
    }
    Ok(String::from_utf8_lossy(&start).into_owned())
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
        || e.get_ref()
            .and_then(|inner| inner.downcast_ref::<ureq::Error>())
            .is_some_and(|u| matches!(u, ureq::Error::Timeout(_)))
}

fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or_default().trim()
}

/// Whether SQL Server answers on `host`:`port` - a TCP connection, nothing more. A cold DNN can't start without its
/// database, so a warm-up waits for it.
pub fn sql_answers(host: &str, port: u16) -> bool {
    let limit = Duration::from_secs(3);
    let Ok(addresses) = (host, port).to_socket_addrs() else { return false };
    addresses.into_iter().any(|a| TcpStream::connect_timeout(&a, limit).is_ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;

    /// What a test server answers for a path.
    #[derive(Clone, Default)]
    pub(crate) struct Answer {
        pub status: u16,
        pub location: Option<String>,
        pub body: String,
        pub set_cookie: Option<String>,
        pub server: Option<String>,
    }

    pub(crate) fn answer(status: u16) -> Answer {
        Answer { status, ..Default::default() }
    }

    #[derive(Clone, Debug)]
    pub(crate) struct Seen {
        pub path: String,
        pub host: Option<String>,
        pub cookie: Option<String>,
        pub user_agent: Option<String>,
    }

    /// HTTP/1.1 on 127.0.0.1 with an answer per path - and what each request was, in order.
    pub(crate) struct TinyServer {
        pub port: u16,
        pub requests: Arc<Mutex<Vec<Seen>>>,
        stop: Arc<AtomicBool>,
    }

    impl TinyServer {
        pub(crate) fn start(answer: impl Fn(&str) -> Answer + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (seen, stopped, answer) = (requests.clone(), stop.clone(), Arc::new(answer));
            thread::spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (seen, answer) = (seen.clone(), answer.clone());
                            thread::spawn(move || serve(stream, &seen, &*answer));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
            Self { port, requests, stop }
        }

        pub(crate) fn seen(&self) -> Vec<Seen> {
            self.requests.lock().unwrap().clone()
        }

        pub(crate) fn paths(&self) -> Vec<String> {
            self.seen().into_iter().map(|s| s.path).collect()
        }

        pub(crate) fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Drop for TinyServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn serve(stream: TcpStream, seen: &Mutex<Vec<Seen>>, answer: &dyn Fn(&str) -> Answer) {
        stream.set_nonblocking(false).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let path = line.split(' ').nth(1).unwrap_or_default().to_string();
        let mut headers = HashMap::new();
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = header.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        seen.lock().unwrap().push(Seen {
            path: path.clone(),
            host: headers.get("host").cloned(),
            cookie: headers.get("cookie").cloned(),
            user_agent: headers.get("user-agent").cloned(),
        });
        let a = answer(&path);
        let mut head =
            format!("HTTP/1.1 {} Answer\r\nContent-Length: {}\r\nConnection: close\r\n", a.status, a.body.len());
        for (name, value) in [("Location", &a.location), ("Set-Cookie", &a.set_cookie), ("Server", &a.server)] {
            if let Some(value) = value {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        head.push_str("\r\n");
        let mut stream = stream;
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(a.body.as_bytes());
        let _ = stream.flush();
    }

    fn ping(r: &KeepWarmRequester, site: &str, target: &LocalSiteTarget, path: &str) -> KeepWarmOutcome {
        r.send(site, target, path, KeepWarmRequestKind::Ping, &AtomicBool::new(false))
    }

    fn warm_up(r: &KeepWarmRequester, target: &LocalSiteTarget, path: &str) -> KeepWarmOutcome {
        r.send("shop", target, path, KeepWarmRequestKind::WarmUp, &AtomicBool::new(false))
    }

    #[test]
    fn sends_the_host_name_and_keeps_each_sites_cookies() {
        let server = TinyServer::start(|path| match path {
            "/KeepAlive.aspx" => Answer {
                status: 200,
                body: "ok".into(),
                set_cookie: Some(".ASPXANONYMOUS=abc; path=/; HttpOnly".into()),
                ..Default::default()
            },
            _ => answer(404),
        });
        let r = KeepWarmRequester::new();
        let host = format!("shop.dnndev.me:{}", server.port);
        let target = LocalSiteTarget::new("http", "127.0.0.1", server.port, &host);

        let first = ping(&r, "shop", &target, "/KeepAlive.aspx");
        assert_eq!((first.kind, first.requests, first.status_code), (KeepWarmOutcomeKind::Ok, 1, Some(200)));
        ping(&r, "shop", &target, "/KeepAlive.aspx");
        ping(&r, "blog", &target, "/KeepAlive.aspx");

        let seen = server.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].host.as_deref(), Some(host.as_str()));
        assert_eq!(seen[0].user_agent.as_deref(), Some(USER_AGENT));
        assert_eq!(seen[0].cookie, None, "Nothing to send the first time.");
        assert_eq!(seen[1].cookie.as_deref(), Some(".ASPXANONYMOUS=abc"), "The cookie DNN gave.");
        assert_eq!(seen[2].cookie, None, "One site's cookies aren't another's.");

        r.forget_cookies("SHOP");
        ping(&r, "shop", &target, "/KeepAlive.aspx");
        assert_eq!(server.seen().last().unwrap().cookie, None, "Gone with its worker process.");
    }

    #[test]
    fn a_warm_up_follows_the_site_and_stops_at_the_installer_or_an_error() {
        let server = TinyServer::start(|path| match path {
            "/" => Answer { status: 302, location: Some("/Home".into()), ..Default::default() },
            "/Home" => Answer { status: 200, body: "<html>home</html>".into(), ..Default::default() },
            "/new" => {
                Answer { status: 302, location: Some("/Install/InstallWizard.aspx".into()), ..Default::default() }
            }
            "/broken" => Answer {
                status: 302,
                location: Some("/ErrorPage.aspx?status=500&error=Connection+To+The+Database+Failed".into()),
                ..Default::default()
            },
            "/down" => answer(503),
            _ => answer(404),
        });
        let r = KeepWarmRequester::new();
        let target = LocalSiteTarget::new("http", "127.0.0.1", server.port, &format!("shop.dnndev.me:{}", server.port));

        let home = warm_up(&r, &target, "/");
        assert_eq!(
            (home.kind, home.requests),
            (KeepWarmOutcomeKind::Ok, 2),
            "The redirect to the same site was followed."
        );
        assert_eq!(server.paths(), vec!["/", "/Home"]);

        let installer = warm_up(&r, &target, "/new");
        assert_eq!((installer.kind, installer.requests), (KeepWarmOutcomeKind::Ok, 1), "Never into the installer.");
        assert!(!server.paths().iter().any(|p| p.to_ascii_lowercase().starts_with("/install")));

        let broken = warm_up(&r, &target, "/broken");
        assert_eq!(broken.kind, KeepWarmOutcomeKind::Failed);
        assert!(broken.reason.unwrap().contains("Connection To The Database Failed"));

        assert_eq!(ping(&r, "shop", &target, "/down").kind, KeepWarmOutcomeKind::NotAnswering);
        assert_eq!(ping(&r, "shop", &target, "/KeepAlive.aspx").kind, KeepWarmOutcomeKind::PingPathMissing);
    }

    #[test]
    fn says_when_nothing_answers() {
        // A port nothing listens on: one that was just free.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = KeepWarmRequester::new();
        let outcome =
            ping(&r, "shop", &LocalSiteTarget::new("http", "127.0.0.1", port, "shop.dnndev.me"), "/KeepAlive.aspx");
        assert_eq!(outcome.kind, KeepWarmOutcomeKind::NotAnswering);
        assert_eq!(outcome.reason, Some(format!("nothing answers on 127.0.0.1:{port}")));
        assert!(!sql_answers("127.0.0.1", port), "Nor does a SQL Server there.");
    }

    #[test]
    fn never_requests_the_installer_and_knows_when_iis_itself_answers() {
        let server = TinyServer::start(|path| match path {
            "/stopped" => Answer { status: 404, server: Some("Microsoft-HTTPAPI/2.0".into()), ..Default::default() },
            _ => answer(200),
        });
        let r = KeepWarmRequester::new();
        let target = LocalSiteTarget::new("http", "127.0.0.1", server.port, "shop.dnndev.me");

        let installer = warm_up(&r, &target, "/x/../Install/Install.aspx");
        assert_eq!(installer.kind, KeepWarmOutcomeKind::Failed);
        assert_eq!(
            installer.reason.as_deref(),
            Some("/x/../Install/Install.aspx is a page of DNN's installer - keep warm never requests it")
        );
        assert_eq!(server.count(), 0, "Not sent at all - whatever asked for it.");

        let stopped = ping(&r, "shop", &target, "/stopped");
        assert_eq!(stopped.kind, KeepWarmOutcomeKind::NotAnswering, "HTTP.sys's own 404 isn't the site's.");
        assert_eq!(
            stopped.reason.as_deref(),
            Some("nothing in IIS answers for shop.dnndev.me (HTTP 404) - the site or its app pool may be stopped")
        );
    }

    #[test]
    fn doesnt_follow_a_redirect_into_the_installer_however_it_is_written() {
        let server = TinyServer::start(|path| match path {
            "/" => Answer {
                status: 302,
                location: Some("http://shop.dnndev.me//Install/InstallWizard.aspx".into()),
                ..Default::default()
            },
            _ => answer(200),
        });
        let r = KeepWarmRequester::new();
        let outcome = warm_up(&r, &LocalSiteTarget::new("http", "127.0.0.1", server.port, "shop.dnndev.me"), "/");
        assert_eq!(outcome.kind, KeepWarmOutcomeKind::Ok);
        assert_eq!(server.paths(), vec!["/"]);
    }

    #[test]
    fn keeps_a_handful_of_cookies_and_drops_deleted_ones() {
        let r = KeepWarmRequester::new();
        r.keep_cookies("shop", ["a=1; path=/", "b=2", "=x", "c"].into_iter());
        assert_eq!(r.cookie_header("shop").as_deref(), Some("a=1; b=2"));
        r.keep_cookies("shop", ["a=; path=/", "b=3; max-age=0"].into_iter());
        assert_eq!(r.cookie_header("shop"), None);
    }
}
