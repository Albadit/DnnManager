//! A SQL Server address as a connection string writes it (`localhost,1433`, `.\SQLEXPRESS`, `tcp:db.example.com`,
//! `[::1],1433`, `(localdb)\MSSQLLocalDB`), taken apart (`Application/Abstractions/SqlServerAddress.cs`). Where it
//! points decides what DNN Manager may do to a database without asking.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::model::DatabaseServerOptions;

/// The port SQL Server's default instance listens on.
pub const DEFAULT_PORT: u16 = 1433;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlServerAddress {
    /// The prefix without its colon (`tcp`, `np`, `lpc`), empty when there is none.
    pub protocol: String,
    /// The server's name or address alone: no prefix, port, instance or IPv6 brackets.
    pub host: String,
    /// The port after the comma.
    pub port: Option<u16>,
    /// The instance after the backslash (`SQLEXPRESS`); empty for the default instance.
    pub instance: String,
    /// A LocalDB instance: `(localdb)\…`.
    pub is_local_db: bool,
}

impl SqlServerAddress {
    pub fn parse(server: &str) -> Self {
        let mut s = server.trim().to_string();
        if starts_with_ignore_case(&s, "(localdb)") {
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

        // A named pipe: np:\\server\pipe\… - or a pipe's path alone, which SqlClient opens as one too: its server is
        // the part after the two backslashes, not an empty host (which would be this PC).
        if protocol == "np" || s.starts_with(r"\\") {
            let path = s.trim_start_matches('\\');
            let host = path.find('\\').map_or(path, |end| &path[..end]);
            return Self {
                protocol: "np".into(),
                host: host.into(),
                port: None,
                instance: String::new(),
                is_local_db: false,
            };
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
        let port = comma.and_then(|c| rest[c + 1..].trim().parse::<u16>().ok());
        Self { protocol, host: host.trim().to_string(), port, instance, is_local_db: false }
    }

    /// The host part: without `tcp:`/`np:`, the port and the instance.
    pub fn host_of(server: &str) -> String {
        Self::parse(server).host
    }

    /// Whether `server` is this PC: localhost, `.`, `(local)`, LocalDB, its name or one of its addresses.
    pub fn is_on_this_machine(server: &str) -> bool {
        Self::parse(server).on_this_machine()
    }

    pub fn on_this_machine(&self) -> bool {
        self.is_local_db || is_local_host(&self.host)
    }

    /// Where a TCP connection reaches it: the host (`.` and `(local)` as 127.0.0.1) and the port (1433 when none is
    /// given). None for what isn't reached that way - a named instance, LocalDB, a pipe.
    pub fn tcp_endpoint(&self) -> Option<(String, u16)> {
        if self.is_local_db || !self.instance.is_empty() || self.protocol == "np" || self.protocol == "lpc" {
            return None;
        }
        let host = if self.host == "." || self.host == "(local)" { "127.0.0.1" } else { self.host.as_str() };
        if host.is_empty() { None } else { Some((host.to_string(), self.port.unwrap_or(DEFAULT_PORT))) }
    }

    /// The shared SQL Server container at `container_host` publishing `published_port`.
    pub fn is_container(&self, container_host: &str, published_port: u16) -> bool {
        self.is_container_host(container_host) && self.port.unwrap_or(DEFAULT_PORT) == published_port
    }

    /// The container's host, on whatever port: its address from the settings, or this PC - default instance only.
    pub fn is_container_host(&self, container_host: &str) -> bool {
        !self.is_local_db
            && self.instance.is_empty()
            && (self.protocol.is_empty() || self.protocol == "tcp")
            && (self.host.eq_ignore_ascii_case(container_host.trim()) || is_local_host(&self.host))
    }

    /// The same SQL Server as `other`, however each writes it.
    pub fn same_server_as(&self, other: &SqlServerAddress) -> bool {
        if self.is_local_db || other.is_local_db {
            return self.is_local_db && other.is_local_db && self.instance.eq_ignore_ascii_case(&other.instance);
        }
        if !self.instance.eq_ignore_ascii_case(&other.instance) {
            return false;
        }
        // A named instance without a port is found through SQL Browser: the instance says which one it is.
        let same_port = (!self.instance.is_empty() && self.port.is_none() && other.port.is_none())
            || self.port.unwrap_or(DEFAULT_PORT) == other.port.unwrap_or(DEFAULT_PORT);
        if !same_port {
            return false;
        }
        (self.on_this_machine() && other.on_this_machine()) || self.host.eq_ignore_ascii_case(&other.host)
    }

    /// Whether DNN Manager may sign in to `server` with the user's Windows account without asking: a server on this
    /// PC, or the SQL Server chosen in Settings → Database server - not just any a site's web.config names.
    pub fn may_sign_in_as_user(server: &str, chosen: &DatabaseServerOptions) -> bool {
        if Self::is_on_this_machine(server) {
            return true;
        }
        !chosen.is_container()
            && !chosen.is_local_db_file()
            && !chosen.server.is_empty()
            && Self::parse(server).same_server_as(&Self::parse(&chosen.server))
    }

    /// The question asked before signing in to `server` as the user, when [`Self::may_sign_in_as_user`] isn't so.
    pub fn sign_in_question(server: &str, why: &str) -> String {
        format!(
            "{why} signs in to {server} with your Windows account. That server is named by the site's web.config - not on \
             this PC, and not the one in Settings → Database server - and gets your Windows sign-in.\n\nSign in to it as you?"
        )
    }
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn is_local_host(host: &str) -> bool {
    if host.is_empty() || host == "." || host == "(local)" || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let local = local_names();
    // Its name: the NetBIOS one or its full DNS name - not any name that merely starts with it.
    if local.names.iter().any(|n| n.eq_ignore_ascii_case(host)) {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(address) => address.is_loopback() || local.addresses.contains(&address),
        Err(_) => false,
    }
}

#[derive(Clone, Default)]
struct LocalNames {
    names: Vec<String>,
    addresses: HashSet<IpAddr>,
}

/// This PC's names and addresses. The C# reads them again on Windows' address-change event; this keeps them for a
/// short while instead - the monitor asks every few seconds, and reading every adapter each time costs.
const LOCAL_NAMES_KEPT: Duration = Duration::from_secs(30);

fn local_names() -> LocalNames {
    static KEPT: Mutex<Option<(Instant, LocalNames)>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, names)) = kept.as_ref()
        && at.elapsed() < LOCAL_NAMES_KEPT
    {
        return names.clone();
    }
    let names = read_local_names();
    *kept = Some((Instant::now(), names.clone()));
    names
}

#[cfg(windows)]
fn read_local_names() -> LocalNames {
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};
    use windows::Win32::System::SystemInformation::{
        COMPUTER_NAME_FORMAT, ComputerNameDnsFullyQualified, ComputerNameDnsHostname, ComputerNameNetBIOS,
        GetComputerNameExW,
    };
    use windows::core::PWSTR;

    fn name(format: COMPUTER_NAME_FORMAT) -> Option<String> {
        let mut size = 0u32;
        unsafe {
            let _ = GetComputerNameExW(format, None, &mut size);
            let mut buffer = vec![0u16; size as usize + 1];
            GetComputerNameExW(format, Some(PWSTR(buffer.as_mut_ptr())), &mut size).ok()?;
            Some(String::from_utf16_lossy(&buffer[..size as usize]))
        }
    }

    let mut local = LocalNames::default();
    for format in [ComputerNameNetBIOS, ComputerNameDnsHostname, ComputerNameDnsFullyQualified] {
        if let Some(n) = name(format).filter(|n| !n.is_empty()) {
            local.names.push(n);
        }
    }
    if let Ok(n) = std::env::var("COMPUTERNAME") {
        local.names.push(n);
    }

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    for _ in 0..3 {
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe { GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(first), &mut size) };
        if result == 111 {
            // ERROR_BUFFER_OVERFLOW: `size` says how much it needs.
            continue;
        }
        if result != 0 {
            break;
        }
        let mut adapter = first as *const IP_ADAPTER_ADDRESSES_LH;
        unsafe {
            while !adapter.is_null() {
                let mut unicast = (*adapter).FirstUnicastAddress;
                while !unicast.is_null() {
                    let sockaddr = (*unicast).Address.lpSockaddr;
                    if !sockaddr.is_null() {
                        if (*sockaddr).sa_family == AF_INET {
                            let v4 = &*(sockaddr as *const SOCKADDR_IN);
                            local.addresses.insert(IpAddr::from(u32::from_be(v4.sin_addr.S_un.S_addr).to_be_bytes()));
                        } else if (*sockaddr).sa_family == AF_INET6 {
                            let v6 = &*(sockaddr as *const SOCKADDR_IN6);
                            local.addresses.insert(IpAddr::from(v6.sin6_addr.u.Byte));
                        }
                    }
                    unicast = (*unicast).Next;
                }
                adapter = (*adapter).Next;
            }
        }
        break;
    }
    local
}

#[cfg(not(windows))]
fn read_local_names() -> LocalNames {
    LocalNames::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_of_this_pc_are_here() {
        for server in [
            "localhost,1433",
            "127.0.0.1,1433",
            r".\SQLEXPRESS",
            "(local)",
            r"(LocalDB)\MSSQLLocalDB",
            "tcp:localhost,1433",
            "[::1],1433",
            ".",
        ] {
            assert!(SqlServerAddress::is_on_this_machine(server), "{server}");
        }
        assert!(SqlServerAddress::is_on_this_machine(r"np:\\.\pipe\sql\query"));
    }

    #[cfg(windows)]
    #[test]
    fn this_pcs_name_is_here() {
        let name = std::env::var("COMPUTERNAME").unwrap();
        assert!(SqlServerAddress::is_on_this_machine(&format!(r"{name}\SQLEXPRESS")));
        assert!(SqlServerAddress::is_on_this_machine(&name));
    }

    #[test]
    fn other_servers_are_not() {
        for server in [
            "staging-sql.example.com",
            "tcp:shop.database.windows.net,1433",
            "203.0.113.7,1433",
            r"\\evil.example\pipe\sql\query",
        ] {
            assert!(!SqlServerAddress::is_on_this_machine(server), "{server}");
        }
        assert_eq!(SqlServerAddress::parse(r"\\evil.example\pipe\sql\query").host, "evil.example");
    }

    #[test]
    fn host_is_the_servers_name_alone() {
        for (server, host) in [
            ("tcp:db.example.com,1433", "db.example.com"),
            (r"SERVER\INSTANCE", "SERVER"),
            ("localhost", "localhost"),
            ("[::1],1433", "::1"),
            (r"np:\\SQLBOX\pipe\sql\query", "SQLBOX"),
        ] {
            assert_eq!(SqlServerAddress::host_of(server), host, "{server}");
        }
    }

    #[test]
    fn the_container_is_the_default_instance_here_at_its_port() {
        for (server, port, container) in [
            ("localhost,1433", 1433, true),
            ("localhost", 1433, true),
            (".", 1433, true),
            ("127.0.0.1,14330", 14330, true),
            ("localhost", 14330, false),
            (".", 14330, false),
            ("localhost,1433", 14330, false),
            (r".\SQLEXPRESS", 1433, false),
            (r"(localdb)\MSSQLLocalDB", 1433, false),
            ("staging-sql.example.com,1433", 1433, false),
        ] {
            assert_eq!(SqlServerAddress::parse(server).is_container("localhost", port), container, "{server}");
        }
        assert!(SqlServerAddress::parse("10.0.0.5,1433").is_container("10.0.0.5", 1433));
    }

    #[test]
    fn one_server_written_two_ways_is_the_same() {
        for (a, b) in [
            (".", "localhost,1433"),
            ("(local)", "127.0.0.1"),
            (r".\SQLEXPRESS", r"localhost\sqlexpress"),
            (r"(localdb)\MSSQLLocalDB", r"(LocalDB)\mssqllocaldb"),
            ("tcp:db.example.com,1433", "DB.example.com"),
        ] {
            assert!(SqlServerAddress::parse(a).same_server_as(&SqlServerAddress::parse(b)), "{a} / {b}");
        }
        for (a, b) in [
            (".", r".\SQLEXPRESS"),
            ("localhost,1433", "localhost,14330"),
            ("localhost", "db.example.com"),
            (r"(localdb)\MSSQLLocalDB", "."),
        ] {
            assert!(!SqlServerAddress::parse(a).same_server_as(&SqlServerAddress::parse(b)), "{a} / {b}");
        }
    }

    #[test]
    fn signing_in_as_the_user_only_here_or_to_the_chosen_server() {
        let chosen = DatabaseServerOptions { kind: "sqlServer".into(), server: "db.example.com".into() };
        assert!(SqlServerAddress::may_sign_in_as_user(r".\SQLEXPRESS", &chosen));
        assert!(SqlServerAddress::may_sign_in_as_user("db.example.com", &chosen));
        assert!(!SqlServerAddress::may_sign_in_as_user("evil.example", &chosen));
        assert!(!SqlServerAddress::may_sign_in_as_user(r"\\evil.example\pipe\sql\query", &chosen));
        let container = DatabaseServerOptions { kind: "container".into(), server: "db.example.com".into() };
        assert!(!SqlServerAddress::may_sign_in_as_user("db.example.com", &container));
        assert!(SqlServerAddress::sign_in_question("evil.example", "Opening it").contains("evil.example"));
    }

    #[test]
    fn tcp_endpoints() {
        assert_eq!(SqlServerAddress::parse(".").tcp_endpoint(), Some(("127.0.0.1".into(), 1433)));
        assert_eq!(SqlServerAddress::parse("tcp:db,14330").tcp_endpoint(), Some(("db".into(), 14330)));
        assert_eq!(SqlServerAddress::parse(r".\SQLEXPRESS").tcp_endpoint(), None);
        assert_eq!(SqlServerAddress::parse(r"(localdb)\x").tcp_endpoint(), None);
    }
}
