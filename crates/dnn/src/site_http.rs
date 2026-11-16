//! Requests to a DNN site on this PC: to 127.0.0.1 on the site's port with the site's host name, so they don't depend
//! on the host name resolving - the IIS binding routes them. `Session` is a DNN site seen as a browser sees it -
//! following redirects, keeping cookies, and signing in through DNN's own login form
//! (`src/DnnManager.Infrastructure/Dnn/DnnHttpSession.cs`).

use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use url::Url;

use crate::install_output::html_decode;

/// The cookie DNN sets for a signed-in user.
pub const AUTH_COOKIE: &str = ".DOTNETNUKE";

/// The most of a page read: DNN's pages are far smaller.
const MAX_PAGE: u64 = 32 * 1024 * 1024;

/// An agent for this PC's sites: no proxy, no redirects followed (they are looked at), every status an answer.
pub fn local_agent(connect: Duration, total: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(connect))
        .timeout_global(total)
        .user_agent("DnnManager-NET")
        .build()
        .new_agent()
}

/// The address to ask on this PC for `uri` - its path and query on 127.0.0.1 and `port`.
fn local_url(port: u16, uri: &Url) -> String {
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("http://127.0.0.1:{port}{}{query}", uri.path())
}

/// The `Host` header for `uri`: its host, and its port when it isn't http's.
fn host_header(uri: &Url) -> String {
    let host = uri.host_str().unwrap_or_default();
    match uri.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// GET `path_and_query` from the site answering on `alias` (`host[:port]`), on this PC's `port`.
pub fn get(
    agent: &ureq::Agent,
    port: u16,
    alias: &str,
    path_and_query: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    agent
        .get(format!("http://127.0.0.1:{port}{path_and_query}"))
        .header("Host", alias)
        .call()
        .map_err(|e| e.to_string())
}

/// A body read whole, as text.
pub fn read_text(response: ureq::http::Response<ureq::Body>) -> Result<String, String> {
    let mut bytes = Vec::new();
    response.into_body().into_reader().take(MAX_PAGE).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Where a redirect sends to, resolved against `base`; None without a `Location`.
pub fn location(response: &ureq::http::Response<ureq::Body>, base: &Url) -> Option<Url> {
    let location = response.headers().get("location")?.to_str().ok()?;
    base.join(location).ok()
}

/// A page as the session got it: where it ended up, its status, its HTML and the requests on the way.
#[derive(Debug, Clone)]
pub struct Page {
    pub uri: Url,
    pub status: u16,
    pub html: String,
    pub hops: Vec<String>,
}

impl Page {
    pub fn at_installer(&self) -> bool {
        self.uri.path().to_ascii_lowercase().starts_with("/install")
    }

    pub fn at_login(&self) -> bool {
        self.uri.path().to_ascii_lowercase().contains("/login")
            || self.uri.query().is_some_and(|q| q.to_ascii_lowercase().contains("ctl=login"))
    }

    pub fn at_error_page(&self) -> bool {
        self.uri.path().to_ascii_lowercase().contains("errorpage")
            || self.uri.query().is_some_and(|q| q.to_ascii_lowercase().contains("error="))
    }
}

/// How signing in went: signed in, whether the Persona Bar shows, and what was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub signed_in: bool,
    pub persona_bar: bool,
    pub detail: String,
}

/// A DNN site on this PC seen as a browser sees it.
pub struct Session {
    root: Url,
    port: u16,
    agent: ureq::Agent,
    cookies: Vec<(String, String)>,
}

static INPUT_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<input\b[^>]*>").expect("valid"));
static ANCHOR_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<a\b[^>]*>").expect("valid"));
static FORM_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<form\b[^>]*>").expect("valid"));
static ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([\w:\-\$\.]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"']+))"#).expect("valid"));
static POST_BACK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"__doPostBack\(\s*['"]([^'"]+)['"]"#).expect("valid"));
static RESET_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(resetToken=)[^&]+").expect("valid"));

type Attributes = HashMap<String, String>;

fn attributes(tag: &str) -> Attributes {
    let mut found = Attributes::new();
    for m in ATTRIBUTE.captures_iter(tag) {
        let value = m.get(2).or(m.get(3)).or(m.get(4)).map(|v| v.as_str()).unwrap_or_default();
        found.entry(m[1].to_lowercase()).or_insert_with(|| html_decode(value));
    }
    found
}

fn tags(pattern: &Regex, html: &str) -> Vec<Attributes> {
    pattern.find_iter(html).map(|m| attributes(m.as_str())).collect()
}

fn path_and_query(uri: &Url) -> String {
    format!("{}{}", uri.path(), uri.query().map(|q| format!("?{q}")).unwrap_or_default())
}

fn mask(path_and_query: &str) -> String {
    RESET_TOKEN.replace_all(path_and_query, "${1}***").into_owned()
}

/// The anti-forgery token DNN put in a page (`__RequestVerificationToken`) - what its Web API wants in a header.
pub fn verification_token(html: &str) -> Option<String> {
    tags(&INPUT_TAG, html)
        .into_iter()
        .find(|i| i.get("name").map(String::as_str) == Some("__RequestVerificationToken"))
        .and_then(|i| i.get("value").cloned())
}

impl Session {
    /// `alias`: the `host[:port][/child]` to ask for, e.g. `shop.dnndev.me`; `port`: the port the site's binding
    /// listens on, on this PC.
    pub fn new(alias: &str, port: u16, timeout: Duration) -> Result<Self, String> {
        let root = Url::parse(&format!("http://{}/", alias.trim_end_matches('/'))).map_err(|e| e.to_string())?;
        Ok(Self { root, port, agent: local_agent(Duration::from_secs(20), Some(timeout)), cookies: Vec::new() })
    }

    /// The site's root as requested (`http://alias/`).
    pub fn root(&self) -> &Url {
        &self.root
    }

    /// Whether DNN set its auth cookie.
    pub fn has_cookie(&self, name: &str) -> bool {
        self.cookies.iter().any(|(n, _)| n == name)
    }

    /// GET `relative` (to the site's root), following redirects - stopping at DNN's installer.
    pub fn get(&mut self, relative: &str) -> Result<Page, String> {
        let uri = self.root.join(relative.trim_start_matches('/')).map_err(|e| e.to_string())?;
        self.follow(uri, None)
    }

    fn send(
        &mut self,
        uri: &Url,
        form: Option<&[(String, String)]>,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let url = local_url(self.port, uri);
        let cookie = self.cookies.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ");
        let response = match form {
            Some(fields) => {
                let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish();
                let mut request = self
                    .agent
                    .post(&url)
                    .header("Host", host_header(uri))
                    .header("Content-Type", "application/x-www-form-urlencoded");
                if !cookie.is_empty() {
                    request = request.header("Cookie", &cookie);
                }
                request.send(body.as_bytes())
            }
            None => {
                let mut request = self.agent.get(&url).header("Host", host_header(uri));
                if !cookie.is_empty() {
                    request = request.header("Cookie", &cookie);
                }
                request.call()
            }
        }
        .map_err(|e| e.to_string())?;
        for set in response.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()) {
            self.keep_cookie(set);
        }
        Ok(response)
    }

    fn keep_cookie(&mut self, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else { return };
        let (name, value) = (name.trim().to_string(), value.trim().to_string());
        // A cookie set to expire is taken away - DNN signs out so.
        let expired = parts.any(|p| {
            let p = p.trim().to_ascii_lowercase();
            p == "max-age=0" || p.starts_with("expires=") && p.contains("1970")
        });
        self.cookies.retain(|(n, _)| *n != name);
        if !expired && !value.is_empty() {
            self.cookies.push((name, value));
        }
    }

    fn follow(&mut self, mut uri: Url, form: Option<Vec<(String, String)>>) -> Result<Page, String> {
        let mut hops = Vec::new();
        let mut form = form;
        for _ in 0..10 {
            let posting = form.is_some();
            let method = if posting { "POST" } else { "GET" };
            let response = self.send(&uri, form.as_deref())?;
            let status = response.status().as_u16();
            let next = location(&response, &uri);
            let html = read_text(response)?;
            if let Some(next) = next.filter(|_| (300..400).contains(&status)) {
                // Somewhere else than this site (another host name): followed on this PC all the same, under that name.
                hops.push(format!("{method} {} -> {status} {}", path_and_query(&uri), mask(&path_and_query(&next))));
                if next.path().to_ascii_lowercase().starts_with("/install") {
                    return Ok(Page { uri: next, status, html, hops });
                }
                uri = next;
                if uri.scheme() == "https" {
                    let mut plain = format!("http://{}{}", uri.host_str().unwrap_or_default(), path_and_query(&uri));
                    if let Some(port) = self.root.port() {
                        plain = format!("http://{}:{port}{}", uri.host_str().unwrap_or_default(), path_and_query(&uri));
                    }
                    uri = Url::parse(&plain).map_err(|e| e.to_string())?;
                }
                form = None;
                continue;
            }
            hops.push(format!("{method} {} -> {status}", mask(&path_and_query(&uri))));
            return Ok(Page { uri, status, html, hops });
        }
        hops.push("too many redirects".into());
        Ok(Page { uri, status: 0, html: String::new(), hops })
    }

    /// Signs in as `user`: signed in when DNN set its auth cookie, didn't send the user to change the password, and the
    /// home page then shows a signed-in user. `persona_bar`: it shows DNN's Persona Bar.
    pub fn sign_in(&mut self, user: &str, password: &str) -> Result<SignIn, String> {
        let home = self.get("")?;
        let login_link = tags(&ANCHOR_TAG, &home.html)
            .into_iter()
            .find(|a| a.get("id").is_some_and(|id| id.to_lowercase().contains("loginlink")))
            .and_then(|a| a.get("href").cloned());
        let target = match login_link.filter(|l| !l.is_empty() && !l.to_ascii_lowercase().starts_with("javascript")) {
            Some(link) => home.uri.join(&link).map_err(|e| e.to_string())?,
            None => self.root.join("Login").map_err(|e| e.to_string())?,
        };
        let page = self.follow(target, None)?;

        let inputs = tags(&INPUT_TAG, &page.html);
        let field = |suffix: &str| inputs.iter().filter_map(|i| i.get("name")).find(|n| n.ends_with(suffix)).cloned();
        let (Some(user_field), Some(password_field)) = (field("$txtUsername"), field("$txtPassword")) else {
            return Ok(SignIn {
                signed_in: false,
                persona_bar: false,
                detail: format!("No login form on {} (HTTP {}).", path_and_query(&page.uri), page.status),
            });
        };
        let button =
            tags(&ANCHOR_TAG, &page.html).into_iter().find(|a| a.get("id").is_some_and(|id| id.ends_with("_cmdLogin")));
        let event_target = button
            .and_then(|b| b.get("href").and_then(|href| POST_BACK.captures(href).map(|m| m[1].to_string())))
            .unwrap_or_else(|| user_field.replace("$txtUsername", "$cmdLogin"));

        let mut fields: Vec<(String, String)> = Vec::new();
        for input in &inputs {
            let Some(name) = input.get("name").filter(|n| !n.is_empty()) else { continue };
            let kind = input.get("type").map(|t| t.to_lowercase()).unwrap_or_else(|| "text".into());
            if matches!(kind.as_str(), "submit" | "button" | "image" | "reset" | "file") {
                continue;
            }
            if matches!(kind.as_str(), "checkbox" | "radio") && !input.contains_key("checked") {
                continue;
            }
            fields.push((name.clone(), input.get("value").cloned().unwrap_or_default()));
        }
        let mut set = |name: &str, value: &str| {
            fields.retain(|(n, _)| n != name);
            fields.push((name.to_string(), value.to_string()));
        };
        set("__EVENTTARGET", &event_target);
        set("__EVENTARGUMENT", "");
        set(&user_field, user);
        set(&password_field, password);
        let action = tags(&FORM_TAG, &page.html)
            .into_iter()
            .next()
            .and_then(|f| f.get("action").filter(|a| !a.is_empty()).cloned())
            .and_then(|a| page.uri.join(&a).ok())
            .unwrap_or_else(|| page.uri.clone());

        let posted = self.follow(action, Some(fields))?;
        let cookie_set = self.has_cookie(AUTH_COOKIE);
        let forced = posted.hops.iter().any(|h| {
            let h = h.to_lowercase();
            h.contains("passwordreset") || h.contains("forced=true")
        });
        let on_login = posted.at_login() || posted.html.contains("$txtPassword");
        let mut markers = Vec::new();
        if cookie_set {
            let after = self.get("")?;
            let html = after.html.to_lowercase();
            markers.extend(
                ["personaBar-iframe", "dnn.personaBar", "Logoff", "Logout", "ctl=logoff"]
                    .into_iter()
                    .filter(|m| html.contains(&m.to_lowercase())),
            );
        }
        let signed_in = cookie_set && !forced && !on_login && !markers.is_empty();
        Ok(SignIn {
            signed_in,
            persona_bar: markers.iter().any(|m| m.to_lowercase().contains("personabar")),
            detail: format!(
                "auth cookie {}{}{}, after: {}",
                if cookie_set { "set" } else { "not set" },
                if forced { ", sent to change the password" } else { "" },
                if on_login { ", back on the login page" } else { "" },
                path_and_query(&posted.uri)
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_read_as_dnn_writes_them() {
        let html = r#"<form method="post" action="/Login?returnurl=%2f" id="Form">
            <input type="hidden" name="__VIEWSTATE" value="a&amp;b" />
            <input name='dnn$ctr$Login$txtUsername' type='text'>
            <input type="password" NAME="dnn$ctr$Login$txtPassword" value="">
            <input type="hidden" name="__RequestVerificationToken" value="tok123" />
            <a id="dnn_ctr_Login_cmdLogin" href="javascript:__doPostBack('dnn$ctr$Login$cmdLogin','')">Login</a>"#;
        let inputs = tags(&INPUT_TAG, html);
        assert_eq!(inputs[0].get("value").map(String::as_str), Some("a&b"));
        assert_eq!(inputs[2].get("name").map(String::as_str), Some("dnn$ctr$Login$txtPassword"));
        assert_eq!(verification_token(html).as_deref(), Some("tok123"));
        let anchor = &tags(&ANCHOR_TAG, html)[0];
        assert_eq!(&POST_BACK.captures(anchor.get("href").unwrap()).unwrap()[1], "dnn$ctr$Login$cmdLogin");
        assert_eq!(tags(&FORM_TAG, html)[0].get("action").map(String::as_str), Some("/Login?returnurl=%2f"));
    }

    #[test]
    fn pages_are_told_apart_and_reset_tokens_masked() {
        let page = |u: &str| Page { uri: Url::parse(u).unwrap(), status: 200, html: String::new(), hops: vec![] };
        assert!(page("http://x/Install/Install.aspx").at_installer());
        assert!(page("http://x/Default.aspx?ctl=Login").at_login());
        assert!(page("http://x/ErrorPage.aspx").at_error_page());
        assert!(!page("http://x/Home").at_login());
        assert_eq!(mask("/x?resetToken=abc&y=1"), "/x?resetToken=***&y=1");
    }

    #[test]
    fn cookies_are_kept_and_taken_away() {
        let mut session = Session::new("shop.dnndev.me", 80, Duration::from_secs(5)).unwrap();
        session.keep_cookie(".DOTNETNUKE=abc; path=/; HttpOnly");
        assert!(session.has_cookie(AUTH_COOKIE));
        session.keep_cookie(".DOTNETNUKE=; expires=Thu, 01-Jan-1970 00:00:00 GMT; path=/");
        assert!(!session.has_cookie(AUTH_COOKIE));
        assert_eq!(host_header(&Url::parse("http://shop.dnndev.me:8081/x").unwrap()), "shop.dnndev.me:8081");
        assert_eq!(
            local_url(8081, &Url::parse("http://shop.dnndev.me:8081/a?b=1").unwrap()),
            "http://127.0.0.1:8081/a?b=1"
        );
    }
}
