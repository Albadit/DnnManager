//! DNN Manager's block in the Windows hosts file - `Hosts/HostsFile.cs` and the sync of `Hosts/HostsFileService.cs`.
//!
//! The host names of the IIS sites (`mysite.dnndev.me`, or a custom domain like `shop.test`) are kept in the hosts file
//! so the sites open without internet, and a custom domain opens at all. The block, between its two marker lines, is
//! DNN Manager's alone and written as a whole; every other line of the file stays byte for byte as it was. The lines
//! are kept when DNN Manager closes: the sites open offline after a restart too.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dnnm_application::iis::IisSiteRuntime;

/// One line of DNN Manager's block: `127.0.0.1 mysite.dnndev.me`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostsEntry {
    pub address: String,
    pub host: String,
}

impl HostsEntry {
    pub fn new(address: impl Into<String>, host: impl Into<String>) -> Self {
        Self { address: address.into(), host: host.into() }
    }
}

pub const BEGIN: &str =
    "# DNN Manager: local sites - begin. Kept by DNN Manager from its IIS sites; lines between these two are replaced.";
pub const END: &str = "# DNN Manager: local sites - end";

// What the markers are known by - a marker line with its comment changed is still one.
const BEGIN_PREFIX: &str = "# DNN Manager: local sites - begin";
const END_PREFIX: &str = "# DNN Manager: local sites - end";

/// What a binding on every address is reached at - what dnndev.me's public DNS answers too.
pub const LOOPBACK: &str = "127.0.0.1";

/// `%SystemRoot%\System32\drivers\etc\hosts`.
pub fn default_path() -> PathBuf {
    dnnm_infrastructure::process::system32(r"drivers\etc\hosts")
}

/// The entries `sites` need: the host name of each web binding to the address the binding listens on ([`LOOPBACK`]
/// for one on every address). Sorted by host name; a name with several bindings gets one entry, loopback first.
pub fn entries_for(sites: &[IisSiteRuntime]) -> Vec<HostsEntry> {
    let mut by_host: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for binding in sites.iter().flat_map(|s| &s.bindings).filter(|b| b.is_web()) {
        if let (Some(host), Some(address)) = (host_of(&binding.host), address_of(&binding.address)) {
            by_host.entry(host).or_default().push(address);
        }
    }
    by_host
        .into_iter()
        .map(|(host, mut addresses)| {
            addresses.sort_by(|a, b| (a != LOOPBACK, a).cmp(&(b != LOOPBACK, b)));
            HostsEntry { address: addresses.swap_remove(0), host }
        })
        .collect()
}

// Every name a site on this PC is bound to - a real domain too, which is how a copy of a live site is tried out
// locally. Not "localhost" (Windows answers it itself), an IP address, or a wildcard binding (the hosts file has none).
fn host_of(host: &str) -> Option<String> {
    let name = host.trim().trim_end_matches('.').to_lowercase();
    if name.is_empty() || name == "localhost" || !is_dns_name(&name) {
        return None;
    }
    // An international name (café.test) as Windows looks it up: in punycode (xn--caf-dma.test) - the file is ASCII.
    if name.is_ascii() { Some(name) } else { idna::domain_to_ascii(&name).ok().filter(|n| n.is_ascii()) }
}

/// `Uri.CheckHostName(name) == UriHostNameType.Dns`: labels of letters, digits, '-' and '_', none empty or longer
/// than 63, not an IP address.
fn is_dns_name(name: &str) -> bool {
    if name.len() > 255 || name.parse::<IpAddr>().is_ok() {
        return false;
    }
    name.split('.').all(|label| {
        let mut chars = label.chars();
        let first_ok = chars.next().is_some_and(char::is_alphanumeric);
        first_ok && label.chars().count() <= 63 && chars.all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    })
}

// "*" listens on every address; IIS writes an IPv6 one in brackets.
fn address_of(address: &str) -> Option<String> {
    let a = address.trim();
    if a.is_empty() || a == "*" {
        return Some(LOOPBACK.to_string());
    }
    a.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

/// `text` with DNN Manager's block holding `entries` - where the block was, or at the end; without the block when there
/// are none. A name the file already maps outside the block is left out: the user's own line wins.
pub fn with_entries(text: &str, entries: &[HostsEntry]) -> String {
    // Split on \n alone, each line keeping its \r: joined with \n again, the lines that stay are as they were.
    let mut lines: Vec<&str> = text.split('\n').collect();
    let begin = lines.iter().position(|l| content(l).starts_with(BEGIN_PREFIX));
    let end = begin
        .and_then(|b| lines.iter().skip(b + 1).position(|l| content(l).starts_with(END_PREFIX)).map(|i| i + b + 1));
    // A begin without an end (the end line deleted by hand): only the marker goes - lines after it may be the user's.
    let block_length = match (begin, end) {
        (None, _) => 0,
        (Some(_), None) => 1,
        (Some(b), Some(e)) => e - b + 1,
    };

    let start = begin.unwrap_or(usize::MAX);
    let mapped: HashSet<String> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < start || *i >= start.saturating_add(block_length))
        .flat_map(|(_, l)| names_on(l))
        .map(|n| n.to_lowercase())
        .collect();
    let wanted: Vec<&HostsEntry> = entries.iter().filter(|e| !mapped.contains(&e.host.to_lowercase())).collect();

    let cr = if text.contains("\r\n") || !text.contains('\n') { "\r" } else { "" };
    let block: Vec<String> = if wanted.is_empty() {
        Vec::new()
    } else {
        std::iter::once(BEGIN.to_string())
            .chain(wanted.iter().map(|e| format!("{:<15} {}", e.address, e.host)))
            .chain(std::iter::once(END.to_string()))
            .map(|l| l + cr)
            .collect()
    };

    if let Some(begin) = begin {
        lines.drain(begin..begin + block_length);
        // The empty line put before the block goes with it.
        if block.is_empty() && begin > 0 && lines[begin - 1].trim().is_empty() {
            lines.remove(begin - 1);
        } else {
            for (i, line) in block.iter().enumerate() {
                lines.insert(begin + i, line);
            }
        }
        return lines.join("\n");
    }
    if block.is_empty() {
        return text.to_string();
    }
    // At the end, after an empty line, ending with a line break as the file does.
    let head =
        if text.is_empty() { String::new() } else { format!("{}{cr}\n{cr}\n", text.trim_end_matches(['\r', '\n'])) };
    head + &block.join("\n") + "\n"
}

// A UTF-8 byte order mark as Latin-1 reads it: an editor that saved the file as "UTF-8 with BOM" put it before line 1.
const BOM: &str = "\u{EF}\u{BB}\u{BF}";

// A line without the spaces around it - and the first without its byte order mark.
fn content(line: &str) -> &str {
    let text = line.trim();
    text.strip_prefix(BOM).map_or(text, str::trim)
}

// The names a hosts line maps: everything after the address, up to a comment.
fn names_on(line: &str) -> Vec<&str> {
    let line = content(line);
    let line = line.split('#').next().unwrap_or_default();
    line.split_whitespace().skip(1).map(|n| n.trim_end_matches('.')).collect()
}

/// The copy of the hosts file as it was before DNN Manager last changed it - beside it, as `hosts.dnnmanager.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".dnnmanager.bak");
    PathBuf::from(name)
}

// Latin-1 turns every byte into one character and back: the rest of the file - whatever its encoding, a BOM included -
// is written back exactly as it was read. What DNN Manager adds is ASCII.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn latin1_bytes(text: &str) -> Vec<u8> {
    text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect()
}

/// Puts `entries` into the hosts file at `path` ([`with_entries`]), read and written through one handle so no other
/// writer comes in between. What it held before is copied to [`backup_path`] first: a write cut off leaves the file to
/// be put back from it. False when it already held them.
pub fn write(path: &Path, entries: &[HostsEntry]) -> std::io::Result<bool> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        options.share_mode(FILE_SHARE_READ);
    }
    let mut file = options.open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = latin1(&bytes);
    let next = with_entries(&text, entries);
    if next == text {
        return Ok(false);
    }

    // Written to a file of its own, then moved over the backup - never a half-written backup of a whole file.
    let backup = backup_path(path);
    let mut staged = backup.clone().into_os_string();
    staged.push(".new");
    std::fs::write(&staged, &bytes)?;
    std::fs::rename(&staged, &backup)?;

    let written = latin1_bytes(&next);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&written)?;
    file.set_len(written.len() as u64)?;
    file.sync_all()?;
    Ok(true)
}

// Antivirus or the DNS client may have the file open for a moment.
const RETRY_AFTER: [Duration; 3] = [Duration::from_millis(200), Duration::from_millis(500), Duration::from_secs(1)];

/// Keeps the hosts file in step with the sites (`HostsFileService`'s job): call [`HostsKeeper::sync`] with the sites
/// once IIS has been read completely, and again whenever a site is added, removed or gets other bindings. The file is
/// written only when the names change. The C# service's feed and its 0.5 s settle are the caller's (the store's).
pub struct HostsKeeper {
    path: PathBuf,
    // What the file was last made to hold; None until then, or after a write failed.
    written: Option<Vec<HostsEntry>>,
    last_problem: Option<String>,
}

/// A line for the activity log: the message, and whether it is a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub message: String,
    pub warning: bool,
}

impl HostsKeeper {
    /// For the hosts file at `path` ([`default_path`] in the app, a file of their own in tests).
    pub fn new(path: PathBuf) -> Self {
        Self { path, written: None, last_problem: None }
    }

    /// Writes the file when the names `sites` need differ from what it was last made to hold. Returns what is worth a
    /// line in the activity log, if anything. A failure is said once per problem, not at every change.
    pub fn sync(&mut self, sites: &[IisSiteRuntime]) -> Option<Notice> {
        let entries = entries_for(sites);
        if self.written.as_ref() == Some(&entries) {
            return None;
        }
        let previous = self.written.take();
        let mut attempt = 0;
        loop {
            match write(&self.path, &entries) {
                Ok(changed) => {
                    let notice = changed.then(|| report(previous.as_deref(), &entries));
                    self.written = Some(entries);
                    self.last_problem = None;
                    return notice;
                }
                Err(e) if is_sharing_problem(&e) && attempt < RETRY_AFTER.len() => {
                    std::thread::sleep(RETRY_AFTER[attempt]);
                    attempt += 1;
                }
                Err(e) => {
                    // Tried again with the next change of the sites.
                    self.written = None;
                    let problem = format!(
                        "The hosts file ({}) can't be written: {} Sites under dnndev.me only open while this PC has internet, and custom domains not at all.",
                        self.path.display(),
                        io_message(&e)
                    );
                    if self.last_problem.as_deref() == Some(problem.as_str()) {
                        return None;
                    }
                    self.last_problem = Some(problem.clone());
                    return Some(Notice { message: problem, warning: true });
                }
            }
        }
    }
}

fn is_sharing_problem(e: &std::io::Error) -> bool {
    // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, ERROR_USER_MAPPED_FILE (`FileInUse`).
    matches!(e.raw_os_error(), Some(32 | 33 | 1224))
}

// .NET's IOException messages end with a full stop; the sentence after it follows.
fn io_message(e: &std::io::Error) -> String {
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text).trim().to_string();
    if text.ends_with('.') { text } else { format!("{text}.") }
}

/// The activity log's line for a write: the first of this start, or what was added and removed.
pub fn report(before: Option<&[HostsEntry]>, now: &[HostsEntry]) -> Notice {
    let names = |entries: &[&HostsEntry]| {
        entries
            .iter()
            .map(|e| if e.address == LOOPBACK { e.host.clone() } else { format!("{} ({})", e.host, e.address) })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let message = match before {
        // The first write of this start: the file held what the sites were before, or what was edited by hand.
        None if !now.is_empty() => {
            format!("Hosts file: {} - these sites open without internet.", names(&now.iter().collect::<Vec<_>>()))
        }
        None => "Hosts file: DNN Manager's lines removed - no site uses them.".to_string(),
        Some(before) => {
            let old: BTreeSet<&HostsEntry> = before.iter().collect();
            let new: BTreeSet<&HostsEntry> = now.iter().collect();
            let added: Vec<&HostsEntry> = now.iter().filter(|e| !old.contains(e)).collect();
            let removed: Vec<&HostsEntry> = before.iter().filter(|e| !new.contains(e)).collect();
            let mut parts = Vec::new();
            if !added.is_empty() {
                parts.push(format!("{} added", names(&added)));
            }
            if !removed.is_empty() {
                parts.push(format!("{} removed", names(&removed)));
            }
            format!("Hosts file: {}.", parts.join("; "))
        }
    };
    Notice { message, warning: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_application::iis::IisBinding;

    const WINDOWS_HOSTS: &str = "# Copyright (c) 1993-2009 Microsoft Corp.\r\n#\r\n# localhost name resolution is handled within DNS itself.\r\n#\t127.0.0.1       localhost\r\n#\t::1             localhost\r\n";

    fn site(bindings: Vec<IisBinding>) -> IisSiteRuntime {
        IisSiteRuntime {
            id: 1,
            state: "Started".into(),
            app_pool: "shop".into(),
            app_pool_state: Some("Started".into()),
            worker_process_ids: vec![],
            bindings,
            physical_path: PathBuf::from(r"C:\DNN\shop"),
        }
    }

    fn http(host: &str) -> IisBinding {
        http_at(host, 80, "*")
    }

    fn http_at(host: &str, port: u16, address: &str) -> IisBinding {
        IisBinding {
            protocol: "http".into(),
            address: address.into(),
            port: Some(port),
            host: host.into(),
            has_certificate: false,
        }
    }

    fn local(host: &str) -> HostsEntry {
        HostsEntry::new(LOOPBACK, host)
    }

    #[test]
    fn entries_take_every_host_name_of_a_site_custom_domains_too() {
        let entries = entries_for(&[
            site(vec![
                http("Shop.DnnDev.Me"),
                IisBinding {
                    protocol: "https".into(),
                    address: "*".into(),
                    port: Some(443),
                    host: "shop.dnndev.me".into(),
                    has_certificate: true,
                },
            ]),
            site(vec![http_at("blog.dnndev.me", 8080, "*"), http("klant.local"), http("www.customer.com.")]),
            site(vec![
                http(""),
                http("*.shop.test"),
                http("localhost"),
                http("192.168.1.20"),
                http("bad name"),
                IisBinding {
                    protocol: "net.tcp".into(),
                    address: "".into(),
                    port: None,
                    host: "tcp.dnndev.me".into(),
                    has_certificate: false,
                },
            ]),
        ]);
        assert_eq!(
            entries,
            vec![local("blog.dnndev.me"), local("klant.local"), local("shop.dnndev.me"), local("www.customer.com")]
        );
    }

    #[test]
    fn entries_point_to_the_address_the_binding_listens_on() {
        let entries = entries_for(&[
            site(vec![http_at("lan.dnndev.me", 80, "192.168.1.20")]),
            site(vec![http_at("six.dnndev.me", 80, "[::1]")]),
            site(vec![http_at("both.dnndev.me", 80, "192.168.1.20"), http_at("both.dnndev.me", 8080, "*")]),
        ]);
        assert_eq!(
            entries,
            vec![
                local("both.dnndev.me"),
                HostsEntry::new("192.168.1.20", "lan.dnndev.me"),
                HostsEntry::new("::1", "six.dnndev.me")
            ]
        );
    }

    #[test]
    fn an_international_name_is_written_in_punycode() {
        assert_eq!(entries_for(&[site(vec![http("café.test")])]), vec![local("xn--caf-dma.test")]);
    }

    #[test]
    fn adds_the_block_at_the_end_and_takes_it_out_again() {
        let with = with_entries(WINDOWS_HOSTS, &[local("blog.dnndev.me"), local("shop.dnndev.me")]);
        assert_eq!(
            with,
            format!(
                "{WINDOWS_HOSTS}\r\n{BEGIN}\r\n127.0.0.1       blog.dnndev.me\r\n127.0.0.1       shop.dnndev.me\r\n{END}\r\n"
            )
        );
        assert_eq!(
            with_entries(&with, &[local("blog.dnndev.me"), local("shop.dnndev.me")]),
            with,
            "written once, it stays"
        );
        assert_eq!(with_entries(&with, &[]), WINDOWS_HOSTS, "without sites the file is as it was");
        assert_eq!(with_entries(WINDOWS_HOSTS, &[]), WINDOWS_HOSTS);
    }

    #[test]
    fn replaces_the_block_where_it_is() {
        let file = with_entries(WINDOWS_HOSTS, &[local("old.dnndev.me")]) + "10.0.0.9 nas\r\n";
        let next = with_entries(&file, &[local("new.dnndev.me")]);
        assert_eq!(
            next,
            format!("{WINDOWS_HOSTS}\r\n{BEGIN}\r\n127.0.0.1       new.dnndev.me\r\n{END}\r\n10.0.0.9 nas\r\n")
        );
    }

    #[test]
    fn leaves_out_a_name_the_user_maps_themselves() {
        let file = format!("{WINDOWS_HOSTS}10.0.0.5   shop.dnndev.me   # the test server\r\n");
        let next = with_entries(&file, &[local("blog.dnndev.me"), local("shop.dnndev.me")]);
        assert!(next.contains("127.0.0.1       blog.dnndev.me"));
        assert!(!next.contains("127.0.0.1       shop.dnndev.me"), "the user's own line wins");
        assert_eq!(with_entries(&file, &[local("shop.dnndev.me")]), file, "nothing left to add: no block at all");
    }

    #[test]
    fn keeps_the_files_line_endings() {
        assert_eq!(
            with_entries("127.0.0.1 localhost\n", &[local("shop.dnndev.me")]),
            format!("127.0.0.1 localhost\n\n{BEGIN}\n127.0.0.1       shop.dnndev.me\n{END}\n")
        );
        assert_eq!(
            with_entries("", &[local("shop.dnndev.me")]),
            format!("{BEGIN}\r\n127.0.0.1       shop.dnndev.me\r\n{END}\r\n"),
            "an empty file gets Windows' line breaks"
        );
    }

    #[test]
    fn a_begin_without_an_end_keeps_the_lines_after_it() {
        let file = format!("{WINDOWS_HOSTS}{BEGIN}\r\n127.0.0.1       shop.dnndev.me\r\n10.0.0.9 nas\r\n");
        assert_eq!(
            with_entries(&file, &[local("shop.dnndev.me")]),
            format!("{WINDOWS_HOSTS}127.0.0.1       shop.dnndev.me\r\n10.0.0.9 nas\r\n")
        );
    }

    #[test]
    fn finds_its_block_after_a_byte_order_mark() {
        let text = format!("{BOM}{BEGIN}\r\n127.0.0.1       old.dnndev.me\r\n{END}\r\n");
        let next = with_entries(&text, &[local("new.dnndev.me")]);
        assert!(!next.contains("old.dnndev.me"), "{next}");
        assert_eq!(next.matches(END).count(), 1, "one block: {next}");
    }

    #[test]
    fn write_keeps_the_rest_of_the_file_byte_for_byte_and_a_backup_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts");
        // Not ASCII and not UTF-8: whatever the bytes, they stay.
        let mut original = WINDOWS_HOSTS.as_bytes().to_vec();
        original.extend_from_slice(b"10.0.0.1 caf\xE9.lan\r\n");
        std::fs::write(&path, &original).unwrap();

        assert!(write(&path, &[local("shop.dnndev.me")]).unwrap());
        assert!(!write(&path, &[local("shop.dnndev.me")]).unwrap(), "already there: not written again");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..original.len()], &original[..]);
        assert!(String::from_utf8_lossy(&bytes).ends_with(&format!("127.0.0.1       shop.dnndev.me\r\n{END}\r\n")));
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), original);

        assert!(write(&path, &[local("blog.dnndev.me")]).unwrap());
        assert!(String::from_utf8_lossy(&std::fs::read(backup_path(&path)).unwrap()).contains("shop.dnndev.me"));
        assert!(write(&path, &[]).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn the_keeper_writes_only_changes_and_says_what_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts");
        std::fs::write(&path, WINDOWS_HOSTS).unwrap();
        let mut keeper = HostsKeeper::new(path.clone());

        let shop = site(vec![http("shop.dnndev.me"), http("blog.local.test")]);
        let first = keeper.sync(std::slice::from_ref(&shop)).unwrap();
        assert_eq!(first.message, "Hosts file: blog.local.test, shop.dnndev.me - these sites open without internet.");
        assert_eq!(keeper.sync(std::slice::from_ref(&shop)), None, "nothing changed");

        let store = site(vec![http("store.dnndev.me")]);
        let second = keeper.sync(std::slice::from_ref(&store)).unwrap();
        assert_eq!(second.message, "Hosts file: store.dnndev.me added; blog.local.test, shop.dnndev.me removed.");
        let third = keeper.sync(&[]).unwrap();
        assert_eq!(third.message, "Hosts file: store.dnndev.me removed.");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WINDOWS_HOSTS);
    }

    #[test]
    fn the_keeper_says_a_failure_once() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file should be: it can't be opened as one.
        let path = dir.path().join("hosts");
        std::fs::create_dir(&path).unwrap();
        let mut keeper = HostsKeeper::new(path);
        let notice = keeper.sync(&[site(vec![http("shop.dnndev.me")])]).unwrap();
        assert!(notice.warning);
        assert!(notice.message.contains("only open while this PC has internet"), "{}", notice.message);
        assert_eq!(keeper.sync(&[site(vec![http("blog.dnndev.me")])]), None, "said once, not at every change");
    }
}
