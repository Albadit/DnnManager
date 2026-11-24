//! Source Control's use cases: compare a project with an environment's server (`scan`), send files either way or
//! delete one on the server (`transfer`), and read both copies of a file for the diff (`read_both`). They see the
//! system through [`LocalFiles`], [`Remote`] and [`SyncState`], so every rule is tested without a server.
//!
//! What keeps the server safe:
//! - a server's host key is trusted only after the user said so, before any password is sent - and a different one
//!   later is refused;
//! - a file is checked again right before it is replaced: changed on either side since the comparison, it is left
//!   alone ("compare again");
//! - a conflict is never sent without a choice of side; a missing local file never deletes the server's - only
//!   [`Action::DeleteRemote`] does, for a file deleted here after a sync, after a question;
//! - replacing or deleting server files asks first, naming them, and protected ones are named apart;
//! - with backups on, the server's copy is saved here before it is replaced or deleted;
//! - a transfer that breaks off leaves the old file in place (written under a temporary name, then renamed) and
//!   its record as it was; a lost connection is opened again and the file tried again, up to three times.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::classify::{compare, worth_verifying};
use super::model::{Change, Entry, Environment, FileMeta, HistoryLine, PLAIN_FTP_ACCEPTED, Protocol, SyncRecord, TLS_CHECKED};
use super::paths::{IgnoreRules, project_path, remote_path};
use crate::operation::{CANCELLED, CancelToken, Operation};

// ─── What the use cases see the system through ───────────────────────────

/// Why a server call failed: the connection (worth opening again), a refusal (permissions), or another error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    Connection(String),
    Denied(String),
    Other(String),
    Cancelled,
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Connection(e) => write!(f, "the connection was lost: {e}"),
            RemoteError::Denied(e) => write!(f, "the server refused it: {e}"),
            RemoteError::Other(e) => f.write_str(e),
            RemoteError::Cancelled => f.write_str(CANCELLED),
        }
    }
}

/// A connected server. Paths are full server paths, `/`-separated.
pub trait RemoteFs: Send {
    /// Every file under `folder`, at any depth: its path from `folder` and its facts. Empty when there is no such
    /// folder.
    fn list(&mut self, folder: &str, cancel: &CancelToken) -> Result<Vec<(String, FileMeta)>, RemoteError>;
    /// A file's facts; None when there is none.
    fn stat(&mut self, path: &str) -> Result<Option<FileMeta>, RemoteError>;
    /// The file's bytes into `to`, a chunk at a time; `progress` gets the bytes so far.
    fn download(
        &mut self,
        path: &str,
        to: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<u64, RemoteError>;
    /// `from` as `path`, a chunk at a time: written beside it under a temporary name, then put in its place, so an
    /// upload that breaks off leaves the old file as it was. Its folders are made; its time set to `mtime`.
    fn upload(
        &mut self,
        from: &mut dyn Read,
        path: &str,
        mtime: i64,
        progress: &mut dyn FnMut(u64),
        cancel: &CancelToken,
    ) -> Result<(), RemoteError>;
    fn remove(&mut self, path: &str) -> Result<(), RemoteError>;
}

/// Opens connections to an environment's server.
pub trait Remote: Send + Sync {
    /// The server's host key fingerprint (`SHA256:…`), read without signing in.
    fn host_key(&self, env: &Environment) -> Result<String, String>;
    /// Signed in, with the server's key checked against `env.host_key` (refused when it differs).
    fn connect(&self, env: &Environment) -> Result<Box<dyn RemoteFs>, RemoteError>;
}

/// The project's folder.
pub trait LocalFiles: Send + Sync {
    /// Every file under `folder` (a project path, "" for all) that `ignore` doesn't match: its project path and facts.
    fn scan(&self, root: &Path, folder: &str, ignore: &IgnoreRules, cancel: &CancelToken)
    -> Result<Vec<(String, FileMeta)>, String>;
    fn stat(&self, root: &Path, path: &str) -> Option<FileMeta>;
    /// Its SHA-256, in lower-case hex.
    fn hash(&self, root: &Path, path: &str) -> Result<String, String>;
    fn open(&self, root: &Path, path: &str) -> Result<Box<dyn Read + Send>, String>;
    /// Writes `path` with what `fill` writes: under a temporary name, then put in place - an interrupted write
    /// leaves the old file. Its folders are made, its time set to `mtime`.
    fn write(
        &self,
        root: &Path,
        path: &str,
        mtime: i64,
        fill: &mut dyn FnMut(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String>;
}

/// What is kept between runs: each file's record of its last sync, the history, the trusted host key.
pub trait SyncState: Send + Sync {
    fn records(&self, env: &str) -> Result<HashMap<String, SyncRecord>, String>;
    fn set_record(&self, env: &str, path: &str, record: &SyncRecord) -> Result<(), String>;
    fn remove_record(&self, env: &str, path: &str) -> Result<(), String>;
    fn add_history(&self, env: &str, line: &HistoryLine) -> Result<(), String>;
    fn trust_host_key(&self, env: &str, fingerprint: &str) -> Result<(), String>;
    /// The server's files as the last comparison listed them (project paths) and when (Unix seconds) - so the page
    /// shows it again without connecting.
    fn save_listing(&self, env: &str, at: i64, files: &[(String, FileMeta)]) -> Result<(), String>;
    /// One file of that listing changed by a transfer: its facts now, or None when it is gone.
    fn update_listing(&self, env: &str, path: &str, meta: Option<&FileMeta>) -> Result<(), String>;
    fn listing(&self, env: &str) -> Result<Option<Listing>, String>;
}

/// The server's files as a comparison listed them, and when (Unix seconds).
pub type Listing = (i64, Vec<(String, FileMeta)>);

/// What the use cases are given.
#[derive(Clone)]
pub struct SyncDeps {
    pub local: Arc<dyn LocalFiles>,
    pub remote: Arc<dyn Remote>,
    pub state: Arc<dyn SyncState>,
    /// The project's folder.
    pub project_dir: PathBuf,
    /// Where copies of server files go before they are replaced or deleted (`backups\<project>\server`).
    pub backups: PathBuf,
    /// Now, in Unix seconds.
    pub now: fn() -> i64,
    /// Between attempts after a lost connection (the tests make it nothing).
    pub retry_pause: Duration,
}

// ─── Connecting ────────────────────────────────────────────────────────────

/// Signed in to the environment's server - first asking to trust it when it isn't yet (before any password is
/// sent): an SFTP server's key by its fingerprint; plain FTP after a warning that nothing is encrypted. FTPS needs no
/// question: its certificate is checked against the system's roots at every connection. `env` gets it trusted.
fn connect(deps: &SyncDeps, env: &mut Environment, op: &mut Operation) -> Result<Box<dyn RemoteFs>, String> {
    if env.host_key.is_none() && env.protocol == Protocol::Ftps {
        deps.state.trust_host_key(&env.id, TLS_CHECKED)?;
        env.host_key = Some(TLS_CHECKED.into());
    }
    if env.host_key.is_none() && env.protocol == Protocol::Ftp {
        let question = format!(
            "{} uses plain FTP: the password and every file go over the network unencrypted - anyone between here and {} can read them, and change them. Use SFTP or FTPS if the server has either. Connect with plain FTP anyway?",
            env.name, env.host
        );
        if !op.prompt.confirm(&question, "Connect unencrypted", "Cancel", true) {
            return Err(CANCELLED.into());
        }
        deps.state.trust_host_key(&env.id, PLAIN_FTP_ACCEPTED)?;
        env.host_key = Some(PLAIN_FTP_ACCEPTED.into());
    }
    if env.host_key.is_none() {
        let key = deps.remote.host_key(env).map_err(|e| format!("Could not reach {}:{}: {e}", env.host, env.port))?;
        let question = format!(
            "This is the first connection to {}:{}. Its key's fingerprint is\n{key}\nTrust this server? Check the fingerprint with the server's administrator if you can - DNN Manager will refuse a different key from now on.",
            env.host, env.port
        );
        if !op.prompt.confirm(&question, "Trust and connect", "Cancel", false) {
            return Err(CANCELLED.into());
        }
        deps.state.trust_host_key(&env.id, &key)?;
        env.host_key = Some(key);
    }
    deps.remote.connect(env).map_err(|e| format!("Could not sign in to {}: {e}", env.host))
}

// ─── Scan ──────────────────────────────────────────────────────────────────

/// What a comparison found: every file on either side (unchanged ones too), sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub entries: Vec<Entry>,
}

/// The most files whose content is read from the server in one comparison to tell a copied file from a changed one,
/// and the largest such file.
const VERIFY_FILES: usize = 200;
const VERIFY_BYTES: u64 = 2 * 1024 * 1024;

/// Compares the project with the environment's server: both sides read (only what the mappings hold, without what
/// is ignored), matched, and classified against the records of the last sync.
pub fn scan(deps: &SyncDeps, env: &Environment, op: &mut Operation) -> Result<Scan, String> {
    let mut env = env.clone();
    op.report.plan(&["Connect", "Read the project", "Read the server", "Compare"]);
    op.report.fact("Server", &env.label());
    op.report.step("Connect");
    let mut fs = connect(deps, &mut env, op)?;
    op.report.success(&format!("Signed in to {} as {}", env.host, env.user));

    let ignore = IgnoreRules::for_environment(&env);
    let mappings = env.effective_mappings();
    // A file belongs to the mapping with the longest folder that holds it.
    let ours = |index: usize, path: &str| remote_path(&mappings, path).is_some_and(|(i, _)| i == index);

    op.report.step("Read the project");
    let local = read_project(deps, &env, &op.cancel)?;
    op.report.info(&format!("{} files here", local.len()));

    op.report.step("Read the server");
    let mut remote = BTreeMap::new();
    for (index, mapping) in mappings.iter().enumerate() {
        op.cancel.check()?;
        let listed = fs.list(&mapping.remote, &op.cancel).map_err(|e| format!("Could not read {}: {e}", mapping.remote))?;
        for (relative, meta) in listed {
            let path = project_path(mapping, &relative);
            if !ignore.matches(&path) && ours(index, &path) {
                remote.insert(path, meta);
            }
        }
    }
    op.report.info(&format!("{} files on the server", remote.len()));

    op.report.step("Compare");
    let listed: Vec<(String, FileMeta)> = remote.iter().map(|(p, m)| (p.clone(), m.clone())).collect();
    deps.state.save_listing(&env.state_key(), (deps.now)(), &listed)?;
    let mut records = deps.state.records(&env.state_key())?;
    let mut entries = compare(&local, &remote, &records);
    verify_same_size(deps, &env, fs.as_mut(), &mut entries, &mut records, op)?;
    let counts = super::classify::counts(&entries);
    if counts.is_empty() {
        op.report.success("The project and the server are the same");
    } else {
        let text: Vec<String> = counts.iter().map(|(c, n)| format!("{n} {}", c.name().to_lowercase())).collect();
        op.report.success(&text.join(", "));
    }
    Ok(Scan { entries })
}

/// The project's files the environment's mappings hold, without what is ignored.
fn read_project(deps: &SyncDeps, env: &Environment, cancel: &CancelToken) -> Result<BTreeMap<String, FileMeta>, String> {
    let ignore = IgnoreRules::for_environment(env);
    let mappings = env.effective_mappings();
    let mut local = BTreeMap::new();
    for (index, mapping) in mappings.iter().enumerate() {
        cancel.check()?;
        let found = deps.local.scan(&deps.project_dir, mapping.local.trim_matches('/'), &ignore, cancel)?;
        local.extend(
            found.into_iter().filter(|(p, _)| remote_path(&mappings, p).is_some_and(|(i, _)| i == index)),
        );
    }
    Ok(local)
}

/// The comparison again without the server: the project read anew, the server as the last comparison read it
/// (`last`, its entries). For changes made here between two Refreshes - a file saved, added or deleted - without a
/// connection; what changed on the server since shows at the next Refresh.
pub fn reclassify_local(deps: &SyncDeps, env: &Environment, last: &[Entry], cancel: &CancelToken) -> Result<Vec<Entry>, String> {
    let local = read_project(deps, env, cancel)?;
    let remote: BTreeMap<String, FileMeta> =
        last.iter().filter_map(|e| e.remote.clone().map(|m| (e.path.clone(), m))).collect();
    let records = deps.state.records(&env.state_key())?;
    Ok(compare(&local, &remote, &records))
}

/// The last comparison again without connecting: the server's side as it was listed then (and kept up by every
/// transfer since), the project's read now - with when the server was listed. None before a first comparison.
pub fn cached(deps: &SyncDeps, env: &Environment, cancel: &CancelToken) -> Result<Option<(i64, Vec<Entry>)>, String> {
    let Some((at, listed)) = deps.state.listing(&env.state_key())? else { return Ok(None) };
    let local = read_project(deps, env, cancel)?;
    let remote: BTreeMap<String, FileMeta> = listed.into_iter().collect();
    let records = deps.state.records(&env.state_key())?;
    Ok(Some((at, compare(&local, &remote, &records))))
}

/// Never-synced files of the same size whose times differ: read from the server (the smaller ones, up to a number)
/// and compared by content - the same bytes are recorded as synced, so they show as unchanged from now on.
fn verify_same_size(
    deps: &SyncDeps,
    env: &Environment,
    fs: &mut dyn RemoteFs,
    entries: &mut [Entry],
    records: &mut HashMap<String, SyncRecord>,
    op: &mut Operation,
) -> Result<(), String> {
    let mappings = env.effective_mappings();
    let mut same = 0;
    let candidates: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            worth_verifying(&entries[i], records.get(&entries[i].path))
                && entries[i].remote.as_ref().is_some_and(|r| r.size <= VERIFY_BYTES)
        })
        .take(VERIFY_FILES)
        .collect();
    for i in candidates {
        op.cancel.check()?;
        let entry = &mut entries[i];
        let Some((_, remote)) = remote_path(&mappings, &entry.path) else { continue };
        let mut bytes = Vec::new();
        if fs.download(&remote, &mut bytes, &mut |_| {}, &op.cancel).is_err() {
            continue;
        }
        let Ok(local_hash) = deps.local.hash(&deps.project_dir, &entry.path) else { continue };
        if sha256_hex(&bytes) == local_hash
            && let (Some(l), Some(r)) = (entry.local.clone(), entry.remote.clone())
        {
            let record = SyncRecord {
                local: l.with_hash(local_hash.clone()),
                remote: r.with_hash(local_hash),
                synced: (deps.now)(),
            };
            deps.state.set_record(&env.state_key(), &entry.path, &record)?;
            records.insert(entry.path.clone(), record);
            entry.change = Change::Unchanged;
            same += 1;
        }
    }
    if same > 0 {
        op.report.info(&format!("{same} files had other times but the same content - now recorded as the same"));
    }
    Ok(())
}

// ─── Transfer ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The project's copy to the server (Upload, or keep the local side of a conflict).
    Upload,
    /// The server's copy to the project (Download, Revert local, or keep the server's side of a conflict).
    Download,
    /// Delete the server's copy of a file deleted here after a sync.
    DeleteRemote,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Upload => "Upload",
            Action::Download => "Download",
            Action::DeleteRemote => "Delete on server",
        }
    }
}

/// One file to send, as the comparison saw it - it is sent only if both sides are still so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferItem {
    pub path: String,
    pub action: Action,
    pub change: Change,
    pub local: Option<FileMeta>,
    pub remote: Option<FileMeta>,
    /// The user chose a side of this conflict (or chose to discard local changes): it may replace the other.
    pub resolved: bool,
}

impl TransferItem {
    pub fn from_entry(entry: &Entry, action: Action, resolved: bool) -> Self {
        Self {
            path: entry.path.clone(),
            action,
            change: entry.change,
            local: entry.local.clone(),
            remote: entry.remote.clone(),
            resolved,
        }
    }
}

/// What a transfer did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferResult {
    pub done: Vec<String>,
    /// Each file left alone, with why.
    pub skipped: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
}

/// Why `item` can't be sent as asked - None when it can.
pub fn refusal(item: &TransferItem) -> Option<String> {
    let ok = match item.action {
        Action::Upload => item.local.is_some() && (item.change.uploads() || item.resolved),
        Action::Download => item.remote.is_some() && (item.change.downloads() || item.resolved),
        Action::DeleteRemote => item.change == Change::Deleted && item.remote.is_some(),
    };
    if ok {
        return None;
    }
    Some(match (item.change, item.action) {
        (Change::Conflict, _) => "it changed on both sides - choose which one to keep".into(),
        (_, Action::DeleteRemote) => "only a file deleted here after a sync can be deleted on the server".into(),
        (_, Action::Upload) if item.local.is_none() => "it isn't in the project".into(),
        (_, Action::Download) if item.remote.is_none() => "it isn't on the server".into(),
        (change, action) => format!("{} isn't for a file that is {}", action.name(), change.name().to_lowercase()),
    })
}

/// A list of paths for a question: the first ten, then how many more.
fn listed(paths: &[&str]) -> String {
    let mut text: Vec<String> = paths.iter().take(10).map(|p| format!("  {p}")).collect();
    if paths.len() > 10 {
        text.push(format!("  … and {} more", paths.len() - 10));
    }
    text.join("\n")
}

/// Sends files to or from the environment's server, or deletes them there, as `items` says - each only when both
/// sides are still as the comparison saw them. Asks first for what replaces or deletes - unless `previewed`: the user
/// confirmed the deployment preview ([`super::plan::preview`]) of these items, which named every one. Files that fail don't stop
/// the others; the result says which.
pub fn transfer(
    deps: &SyncDeps,
    env: &Environment,
    items: &[TransferItem],
    previewed: bool,
    op: &mut Operation,
) -> Result<TransferResult, String> {
    let mut env = env.clone();
    let mut result = TransferResult::default();
    op.report.plan(&["Check", "Connect", "Transfer"]);
    op.report.fact("Server", &env.label());
    op.report.step("Check");
    let mut todo: Vec<&TransferItem> = Vec::new();
    for item in items {
        match refusal(item) {
            Some(why) => {
                op.report.warn(&format!("{}: left alone - {why}", item.path));
                result.skipped.push((item.path.clone(), why));
            }
            None => todo.push(item),
        }
    }
    if todo.is_empty() {
        op.report.info("Nothing to send.");
        return Ok(result);
    }

    // The questions, before anything is sent.
    let protected = IgnoreRules::protected(&env);
    let paths = |pick: &dyn Fn(&TransferItem) -> bool| -> Vec<&str> {
        todo.iter().filter(|i| pick(i)).map(|i| i.path.as_str()).collect()
    };
    // Asked here unless the user confirmed the preview, which named all of it.
    if !previewed {
        let deletions = paths(&|i| i.action == Action::DeleteRemote);
        if !deletions.is_empty()
            && !op.prompt.confirm(
                &format!("Delete {} file(s) on {} ({})?\n{}", deletions.len(), env.name, env.host, listed(&deletions)),
                "Delete on server",
                "Cancel",
                true,
            )
        {
            return Err(CANCELLED.into());
        }
        let replaced = paths(&|i| i.action == Action::Upload && i.remote.is_some());
        if !replaced.is_empty()
            && !op.prompt.confirm(
                &format!("Replace {} file(s) on {} ({})?\n{}", replaced.len(), env.name, env.host, listed(&replaced)),
                "Replace",
                "Cancel",
                true,
            )
        {
            return Err(CANCELLED.into());
        }
        let guarded = paths(&|i| i.action != Action::Download && protected.matches(&i.path));
        if !guarded.is_empty()
            && !op.prompt.confirm(
                &format!(
                    "{} of these are protected on {} - configuration or what the live site made. Send them anyway?\n{}",
                    guarded.len(),
                    env.name,
                    listed(&guarded)
                ),
                "Send protected files",
                "Cancel",
                true,
            )
        {
            return Err(CANCELLED.into());
        }
        let discarded = paths(&|i| i.action == Action::Download && i.local.is_some());
        if !discarded.is_empty()
            && !op.prompt.confirm(
                &format!(
                    "Replace {} file(s) in the project with the server's copy? Their changes here are lost.\n{}",
                    discarded.len(),
                    listed(&discarded)
                ),
                "Replace here",
                "Cancel",
                true,
            )
        {
            return Err(CANCELLED.into());
        }
    }

    op.report.step("Connect");
    let mut fs = connect(deps, &mut env, op)?;
    op.report.success(&format!("Signed in to {}", env.host));
    op.undo.cannot_undo("the files already sent - each one is whole, or left as it was");

    op.report.step("Transfer");
    let mappings = env.effective_mappings();
    let stamp = (deps.now)();
    for (n, item) in todo.iter().enumerate() {
        op.cancel.check()?;
        let Some((_, remote)) = remote_path(&mappings, &item.path) else {
            result.skipped.push((item.path.clone(), "no mapping holds it".into()));
            continue;
        };
        op.report.info(&format!("{} {} ({} of {})", item.action.name(), item.path, n + 1, todo.len()));
        let outcome = with_retries(deps, &env, &mut fs, op, |fs, op| send_one(deps, &env, fs, item, &remote, stamp, op));
        let line = |outcome: &str| HistoryLine {
            at: (deps.now)(),
            action: item.action.name().into(),
            path: item.path.clone(),
            outcome: outcome.into(),
        };
        match outcome {
            Ok(Sent::Done) => {
                deps.state.add_history(&env.state_key(), &line("Done"))?;
                result.done.push(item.path.clone());
            }
            Ok(Sent::Changed(why)) => {
                op.report.warn(&format!("{}: left alone - {why}", item.path));
                deps.state.add_history(&env.state_key(), &line(&format!("Left alone: {why}")))?;
                result.skipped.push((item.path.clone(), why));
            }
            Err(e) if e == CANCELLED => return Err(e),
            Err(e) => {
                op.report.fail(&format!("{}: {e}", item.path));
                deps.state.add_history(&env.state_key(), &line(&format!("Failed: {e}")))?;
                result.failed.push((item.path.clone(), e));
            }
        }
    }
    let summary = format!(
        "{} sent{}{}",
        result.done.len(),
        if result.skipped.is_empty() { String::new() } else { format!(", {} left alone", result.skipped.len()) },
        if result.failed.is_empty() { String::new() } else { format!(", {} failed", result.failed.len()) },
    );
    if result.failed.is_empty() {
        op.report.success(&summary);
        Ok(result)
    } else {
        Err(format!("{summary} - see each file above"))
    }
}

/// How one file went.
enum Sent {
    Done,
    /// Changed since the comparison: left alone.
    Changed(String),
}

/// `send` again after a lost connection - connected anew, up to three tries in all.
fn with_retries(
    deps: &SyncDeps,
    env: &Environment,
    fs: &mut Box<dyn RemoteFs>,
    op: &mut Operation,
    send: impl Fn(&mut dyn RemoteFs, &mut Operation) -> Result<Sent, RemoteError>,
) -> Result<Sent, String> {
    let mut attempt = 1;
    loop {
        match send(fs.as_mut(), op) {
            Ok(sent) => return Ok(sent),
            Err(RemoteError::Cancelled) => return Err(CANCELLED.into()),
            Err(RemoteError::Connection(e)) if attempt < 3 => {
                op.report.warn(&format!("The connection was lost ({e}) - trying again ({} of 3)", attempt + 1));
                std::thread::sleep(deps.retry_pause * attempt);
                op.cancel.check()?;
                match deps.remote.connect(env) {
                    Ok(again) => *fs = again,
                    Err(e) => return Err(format!("Could not connect again: {e}")),
                }
                attempt += 1;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn changed_since(now: Option<&FileMeta>, then: Option<&FileMeta>) -> bool {
    match (now, then) {
        (None, None) => false,
        (Some(a), Some(b)) => !a.same_as(b),
        _ => true,
    }
}

fn local_error(e: String) -> RemoteError {
    RemoteError::Other(e)
}

/// One file: both sides checked again, the server's copy backed up when it is replaced or deleted, sent, and the
/// record written only once it is all done.
fn send_one(
    deps: &SyncDeps,
    env: &Environment,
    fs: &mut dyn RemoteFs,
    item: &TransferItem,
    remote: &str,
    stamp: i64,
    op: &mut Operation,
) -> Result<Sent, RemoteError> {
    let root = &deps.project_dir;
    let local_now = deps.local.stat(root, &item.path);
    let remote_now = fs.stat(remote)?;
    if changed_since(remote_now.as_ref(), item.remote.as_ref()) {
        return Ok(Sent::Changed("it changed on the server since the comparison - compare again".into()));
    }
    if changed_since(local_now.as_ref(), item.local.as_ref()) {
        return Ok(Sent::Changed("it changed here since the comparison - compare again".into()));
    }
    let cancel = op.cancel.clone();
    // The server's copy, before it is replaced or deleted.
    if env.backup_remote && remote_now.is_some() && item.action != Action::Download {
        let folder = deps.backups.join(&env.id).join(stamp.to_string());
        let mut failed = None;
        deps.local
            .write(&folder, &item.path, remote_now.as_ref().map_or(0, |m| m.mtime), &mut |to| {
                fs.download(remote, to, &mut |_| {}, &cancel).map(|_| ()).map_err(|e| {
                    let text = e.to_string();
                    failed = Some(e);
                    text
                })
            })
            .map_err(|e| failed.take().unwrap_or(RemoteError::Other(format!("Its backup failed: {e}"))))?;
    }
    match item.action {
        Action::Upload => {
            let meta = local_now.ok_or_else(|| RemoteError::Other("it isn't in the project any more".into()))?;
            if meta.size <= DIFF_BYTES && still_merging(deps, &item.path) {
                return Ok(Sent::Changed(
                    "it still has merge markers (<<<<<<<) - finish the merge, save it, then Refresh".into(),
                ));
            }
            let hash = deps.local.hash(root, &item.path).map_err(local_error)?;
            let mut file = deps.local.open(root, &item.path).map_err(local_error)?;
            fs.upload(&mut file, remote, meta.mtime, &mut |_| {}, &cancel)?;
            let sent = fs.stat(remote)?.ok_or_else(|| RemoteError::Other("the server doesn't show it after the upload".into()))?;
            if sent.size != meta.size {
                return Err(RemoteError::Other(format!("the server has {} bytes of {}", sent.size, meta.size)));
            }
            deps.state.update_listing(&env.state_key(), &item.path, Some(&sent)).map_err(local_error)?;
            let record = SyncRecord { local: meta.with_hash(hash.clone()), remote: sent.with_hash(hash), synced: (deps.now)() };
            deps.state.set_record(&env.state_key(), &item.path, &record).map_err(local_error)?;
        }
        Action::Download => {
            let meta = remote_now.ok_or_else(|| RemoteError::Other("it isn't on the server any more".into()))?;
            let mut failed = None;
            deps.local
                .write(root, &item.path, meta.mtime, &mut |to| {
                    fs.download(remote, to, &mut |_| {}, &cancel).map(|_| ()).map_err(|e| {
                        let text = e.to_string();
                        failed = Some(e);
                        text
                    })
                })
                .map_err(|e| failed.take().unwrap_or(RemoteError::Other(e)))?;
            let local = deps
                .local
                .stat(root, &item.path)
                .ok_or_else(|| RemoteError::Other("the downloaded file isn't there".into()))?;
            if local.size != meta.size {
                return Err(RemoteError::Other(format!("{} of {} bytes arrived", local.size, meta.size)));
            }
            let hash = deps.local.hash(root, &item.path).map_err(local_error)?;
            let record = SyncRecord { local: local.with_hash(hash.clone()), remote: meta.with_hash(hash), synced: (deps.now)() };
            deps.state.set_record(&env.state_key(), &item.path, &record).map_err(local_error)?;
        }
        Action::DeleteRemote => {
            fs.remove(remote)?;
            deps.state.update_listing(&env.state_key(), &item.path, None).map_err(local_error)?;
            deps.state.remove_record(&env.state_key(), &item.path).map_err(local_error)?;
        }
    }
    Ok(Sent::Done)
}

/// Whether a text file of the project still holds a merge's markers.
fn still_merging(deps: &SyncDeps, path: &str) -> bool {
    let mut bytes = Vec::new();
    deps.local
        .open(&deps.project_dir, path)
        .is_ok_and(|mut f| f.read_to_end(&mut bytes).is_ok())
        && super::diff::text_of(&bytes).is_some_and(|t| super::diff::has_markers(&t))
}

// ─── Merging ──────────────────────────────────────────────────────────────

/// Starts merging a conflict by hand: the project's file becomes both copies with git's markers around each
/// difference ([`super::diff::merge_markers`]) - the project's own copy kept first in the backups folder. The user
/// edits it, saves, Refreshes, and keeps it (uploads it); an upload is refused while markers are left. The file's
/// full path, for an editor.
pub fn start_merge(deps: &SyncDeps, env: &Environment, path: &str, op: &mut Operation) -> Result<PathBuf, String> {
    op.report.plan(&["Read both copies", "Write the merge"]);
    op.report.fact("Server", &env.label());
    op.report.step("Read both copies");
    let (remote, local) = read_both(deps, env, path, &op.cancel)?;
    let text = |b: &Option<Vec<u8>>| match b {
        None => Some(String::new()),
        Some(bytes) => super::diff::text_of(bytes),
    };
    let (Some(server_text), Some(project_text)) = (text(&remote), text(&local)) else {
        return Err("Only a text file can be merged here - keep one side instead.".into());
    };
    op.cancel.check()?;
    op.report.step("Write the merge");
    let stamp = (deps.now)();
    if let Some(bytes) = &local {
        let folder = deps.backups.join(&env.id).join(format!("{stamp}-project"));
        deps.local.write(&folder, path, 0, &mut |to| to.write_all(bytes).map_err(|e| e.to_string()))?;
        op.report.info(&format!("The project's copy is kept in {}", folder.display()));
    }
    let merged = super::diff::merge_markers(&server_text, &project_text, &env.name);
    deps.local.write(&deps.project_dir, path, stamp, &mut |to| to.write_all(merged.as_bytes()).map_err(|e| e.to_string()))?;
    deps.state.add_history(
        &env.state_key(),
        &HistoryLine { at: stamp, action: "Merge".into(), path: path.into(), outcome: "Started - markers written".into() },
    )?;
    op.report.success(&format!(
        "{path} has both copies now, between <<<<<<< and >>>>>>> - keep what belongs, save, Refresh, then Keep mine."
    ));
    Ok(path.split('/').fold(deps.project_dir.clone(), |p, part| p.join(part)))
}

// ─── The diff's two sides ───────────────────────────────────────────────────

/// The largest file read for the diff.
pub const DIFF_BYTES: u64 = 4 * 1024 * 1024;

/// A file's two copies: the server's and the project's, None where there is none.
pub type BothCopies = (Option<Vec<u8>>, Option<Vec<u8>>);

/// Both copies of a file for the diff: the server's and the project's (None where there is none), each only up to
/// [`DIFF_BYTES`] - a larger one is an error. The server's key must have been trusted already (by a comparison).
pub fn read_both(deps: &SyncDeps, env: &Environment, path: &str, cancel: &CancelToken) -> Result<BothCopies, String> {
    if env.host_key.is_none() {
        return Err("Refresh first: the server's key isn't trusted yet.".into());
    }
    let root = &deps.project_dir;
    let local = match deps.local.stat(root, path) {
        Some(m) if m.size > DIFF_BYTES => return Err("The file is too large to compare here.".into()),
        Some(_) => {
            let mut bytes = Vec::new();
            deps.local.open(root, path)?.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            Some(bytes)
        }
        None => None,
    };
    let Some((_, remote)) = remote_path(&env.effective_mappings(), path) else {
        return Ok((None, local));
    };
    let mut fs = deps.remote.connect(env).map_err(|e| e.to_string())?;
    let remote_bytes = match fs.stat(&remote).map_err(|e| e.to_string())? {
        Some(m) if m.size > DIFF_BYTES => return Err("The file is too large to compare here.".into()),
        Some(_) => {
            let mut bytes = Vec::new();
            fs.download(&remote, &mut bytes, &mut |_| {}, cancel).map_err(|e| e.to_string())?;
            Some(bytes)
        }
        None => None,
    };
    Ok((remote_bytes, local))
}

/// The SHA-256 of `bytes` in lower-case hex - as [`LocalFiles::hash`] gives it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
