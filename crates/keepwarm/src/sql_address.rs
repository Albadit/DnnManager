//! The part of `SqlServerAddress.cs` keep warm needs: where a TCP connection reaches the SQL Server a site's
//! web.config names - a cold DNN can't start without its database, so a warm-up waits for it.

/// The port SQL Server's default instance listens on.
pub const DEFAULT_PORT: u16 = 1433;

/// A SQL Server address as a connection string writes it (`localhost,1433`, `.\SQLEXPRESS`, `tcp:db.example.com`,
/// `[::1],1433`, `(localdb)\MSSQLLocalDB`), taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlServerAddress {
    /// The prefix without its colon (`tcp`, `np`, `lpc`), empty when there is none.
    pub protocol: String,
    /// The server's name or address alone: no prefix, port, instance or IPv6 brackets.
    pub host: String,
    pub port: Option<u16>,
    /// The instance after the backslash; empty for the default instance.
    pub instance: String,
    pub is_local_db: bool,
}

impl SqlServerAddress {
    pub fn parse(server: Option<&str>) -> Self {
        let mut s = server.unwrap_or_default().trim().to_string();
        if s.to_ascii_lowercase().starts_with("(localdb)") {
            let instance = s.find('\\').map(|i| s[i + 1..].trim().to_string()).unwrap_or_default();
            return Self { protocol: String::new(), host: "(localdb)".into(), port: None, instance, is_local_db: true };
        }

        let mut protocol = String::new();
        if let Some(colon) = s.find(':')
            && colon > 0
            && colon < 6
            && !s.starts_with('[')
            && s[..colon].chars().all(char::is_alphabetic)
        {
            protocol = s[..colon].to_lowercase();
            s = s[colon + 1..].trim().to_string();
        }

        // A named pipe: np:\\server\pipe\… - or a pipe's path alone.
        if protocol == "np" || s.starts_with(r"\\") {
            let path = s.trim_start_matches('\\');
            let host = path.split('\\').next().unwrap_or_default().to_string();
            return Self { protocol: "np".into(), host, port: None, instance: String::new(), is_local_db: false };
        }

        let (host, rest) = if let Some(stripped) = s.strip_prefix('[') {
            match stripped.find(']') {
                Some(close) => (stripped[..close].to_string(), stripped[close + 1..].to_string()),
                None => (s.trim_matches('[').to_string(), String::new()),
            }
        } else {
            match s.find([',', '\\']) {
                Some(cut) => (s[..cut].to_string(), s[cut..].to_string()),
                None => (s.clone(), String::new()),
            }
        };

        let comma = rest.find(',');
        let instance_part = comma.map_or(rest.as_str(), |c| &rest[..c]);
        let instance = instance_part.strip_prefix('\\').map(|i| i.trim().to_string()).unwrap_or_default();
        let port = comma.and_then(|c| rest[c + 1..].trim().parse().ok());
        Self { protocol, host: host.trim().to_string(), port, instance, is_local_db: false }
    }

    /// Where a TCP connection reaches it: the host (`.` and `(local)` as 127.0.0.1) and the port (1433 when none is
    /// given). None for what isn't reached that way - a named instance, LocalDB, a pipe.
    pub fn tcp_endpoint(&self) -> Option<(String, u16)> {
        if self.is_local_db || !self.instance.is_empty() || self.protocol == "np" || self.protocol == "lpc" {
            return None;
        }
        let host = if self.host == "." || self.host == "(local)" { "127.0.0.1".to_string() } else { self.host.clone() };
        if host.is_empty() { None } else { Some((host, self.port.unwrap_or(DEFAULT_PORT))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp(s: Option<&str>) -> Option<(String, u16)> {
        SqlServerAddress::parse(s).tcp_endpoint()
    }

    #[test]
    fn a_warm_up_waits_for_the_server_the_sites_web_config_names() {
        assert_eq!(tcp(Some("localhost,1433")), Some(("localhost".into(), 1433)));
        assert_eq!(tcp(Some("tcp:127.0.0.1, 1444")), Some(("127.0.0.1".into(), 1444)));
        assert_eq!(tcp(Some("sql.example.com")), Some(("sql.example.com".into(), 1433)));
        assert_eq!(tcp(Some("(local)")), Some(("127.0.0.1".into(), 1433)));
        assert_eq!(tcp(Some("[::1],1433")), Some(("::1".into(), 1433)));
        assert_eq!(tcp(Some(r".\SQLEXPRESS")), None);
        assert_eq!(tcp(Some(r"(localdb)\MSSQLLocalDB")), None);
        assert_eq!(tcp(Some(r"np:\\.\pipe\sql\query")), None);
        assert_eq!(tcp(None), None);
    }
}
