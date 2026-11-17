//! Whether an address answers - the status code it gives, or why it couldn't be reached
//! (`src/DnnManager.Infrastructure/Github/HttpConnectivityChecker.cs`, `IHttpConnectivityChecker`).

use std::time::Duration;

use dnnm_application::operation::{CANCELLED, CancelToken};

/// GET `url` within `timeout_seconds`: its HTTP status, redirects followed as HttpClient follows them.
pub fn check(url: &str, timeout_seconds: u64, cancel: &CancelToken) -> Result<u16, String> {
    cancel.check()?;
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(timeout_seconds)))
        .build()
        .new_agent();
    match agent.get(url).call() {
        Ok(response) => Ok(response.status().as_u16()),
        Err(_) if cancel.is_cancelled() => Err(CANCELLED.into()),
        // Its own time limit. Not a failure yet: DNN compiles the site on its first request.
        Err(ureq::Error::Timeout(_)) => {
            Err(format!("no answer within {timeout_seconds} seconds - a site's first start can take a few minutes"))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_answer_is_its_status_and_nothing_listening_is_an_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        assert_eq!(check(&format!("http://127.0.0.1:{port}/"), 10, &CancelToken::new()), Ok(503));
        server.join().unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(check("http://127.0.0.1:1/", 1, &cancel), Err(CANCELLED.to_string()));
    }
}
