//! What changed: each file's two sides against what they were at the last sync. The record is what tells a file
//! deleted here (it was synced, the server still has it unchanged) from one that is only on the server (never
//! synced) - only the first may ever be deleted there, and only on request.

use std::collections::{BTreeMap, HashMap};

use super::model::{Change, Entry, FileMeta, SyncRecord};

/// The change for one file; None when it is on neither side.
pub fn classify(local: Option<&FileMeta>, remote: Option<&FileMeta>, record: Option<&SyncRecord>) -> Option<Change> {
    let local_changed = |l: &FileMeta, r: &SyncRecord| !l.same_as(&r.local);
    let remote_changed = |m: &FileMeta, r: &SyncRecord| !m.same_as(&r.remote);
    Some(match (local, remote, record) {
        (None, None, _) => return None,
        (Some(_), None, None) => Change::Added,
        (Some(l), None, Some(r)) => {
            if local_changed(l, r) {
                Change::Conflict
            } else {
                Change::DeletedOnServer
            }
        }
        (None, Some(_), None) => Change::RemoteOnly,
        (None, Some(m), Some(r)) => {
            if remote_changed(m, r) {
                Change::Conflict
            } else {
                Change::Deleted
            }
        }
        (Some(l), Some(m), Some(r)) => match (local_changed(l, r), remote_changed(m, r)) {
            (false, false) => Change::Unchanged,
            (true, false) => Change::Modified,
            (false, true) => Change::Incoming,
            (true, true) if same_content(l, m) => Change::Unchanged,
            (true, true) => Change::Conflict,
        },
        // Never synced, on both sides: the same file when the content is; else the newer side's change.
        (Some(l), Some(m), None) => {
            if same_content(l, m) {
                Change::Unchanged
            } else if m.mtime > l.mtime + 2 {
                Change::Incoming
            } else {
                Change::Modified
            }
        }
    })
}

/// Whether two files hold the same bytes: by their hashes when both are known, else by size and time.
fn same_content(a: &FileMeta, b: &FileMeta) -> bool {
    a.same_as(b)
}

/// Whether a never-synced file on both sides may well be the same despite its times (copied without them): the same
/// size and no hash of the server's copy yet - worth reading it to know.
pub fn worth_verifying(entry: &Entry, record: Option<&SyncRecord>) -> bool {
    record.is_none()
        && matches!(entry.change, Change::Modified | Change::Incoming)
        && match (&entry.local, &entry.remote) {
            (Some(l), Some(r)) => l.size == r.size && r.hash.is_none(),
            _ => false,
        }
}

/// Both sides of the project and the records, as entries sorted by path - every file on either side, matched
/// without regard to case (as Windows and IIS do). A file's path is as the project spells it when it has it.
pub fn compare(
    local: &BTreeMap<String, FileMeta>,
    remote: &BTreeMap<String, FileMeta>,
    records: &HashMap<String, SyncRecord>,
) -> Vec<Entry> {
    let mut paths: BTreeMap<String, String> = BTreeMap::new();
    for path in remote.keys().chain(local.keys()) {
        // The project's spelling wins: local keys come last.
        paths.insert(path.to_lowercase(), path.clone());
    }
    let local_by_key: HashMap<String, &FileMeta> = local.iter().map(|(p, m)| (p.to_lowercase(), m)).collect();
    let remote_by_key: HashMap<String, &FileMeta> = remote.iter().map(|(p, m)| (p.to_lowercase(), m)).collect();
    let records_by_key: HashMap<String, &SyncRecord> = records.iter().map(|(p, r)| (p.to_lowercase(), r)).collect();
    paths
        .into_iter()
        .filter_map(|(key, path)| {
            let (l, r) = (local_by_key.get(&key).copied(), remote_by_key.get(&key).copied());
            let change = classify(l, r, records_by_key.get(&key).copied())?;
            Some(Entry { path, local: l.cloned(), remote: r.cloned(), change })
        })
        .collect()
}

/// How many entries have each change, in the order the list shows them (unchanged files left out).
pub fn counts(entries: &[Entry]) -> Vec<(Change, usize)> {
    let mut counts: BTreeMap<Change, usize> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.change != Change::Unchanged) {
        *counts.entry(e.change).or_default() += 1;
    }
    counts.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(size: u64, mtime: i64) -> FileMeta {
        FileMeta::new(size, mtime)
    }

    fn record(local: FileMeta, remote: FileMeta) -> SyncRecord {
        SyncRecord { local, remote, synced: 0 }
    }

    #[test]
    fn never_synced_files() {
        assert_eq!(classify(Some(&m(1, 10)), None, None), Some(Change::Added));
        assert_eq!(classify(None, Some(&m(1, 10)), None), Some(Change::RemoteOnly), "never a deletion");
        assert_eq!(classify(Some(&m(1, 10)), Some(&m(1, 10)), None), Some(Change::Unchanged));
        assert_eq!(classify(Some(&m(1, 100)), Some(&m(2, 10)), None), Some(Change::Modified), "here is newer");
        assert_eq!(classify(Some(&m(1, 10)), Some(&m(2, 100)), None), Some(Change::Incoming), "the server is newer");
        assert_eq!(
            classify(Some(&m(1, 10).with_hash("h")), Some(&m(1, 900).with_hash("h")), None),
            Some(Change::Unchanged),
            "the same bytes, whatever the times"
        );
        assert_eq!(classify(None, None, None), None);
    }

    #[test]
    fn synced_files_tell_who_changed() {
        let r = record(m(5, 100), m(5, 100));
        assert_eq!(classify(Some(&m(5, 100)), Some(&m(5, 100)), Some(&r)), Some(Change::Unchanged));
        assert_eq!(classify(Some(&m(6, 200)), Some(&m(5, 100)), Some(&r)), Some(Change::Modified));
        assert_eq!(classify(Some(&m(5, 100)), Some(&m(7, 300)), Some(&r)), Some(Change::Incoming));
        assert_eq!(classify(Some(&m(6, 200)), Some(&m(7, 300)), Some(&r)), Some(Change::Conflict), "both changed");
        assert_eq!(
            classify(Some(&m(6, 200).with_hash("x")), Some(&m(6, 300).with_hash("x")), Some(&r)),
            Some(Change::Unchanged),
            "both changed the same way"
        );
    }

    #[test]
    fn a_deletion_is_only_a_deletion_after_a_sync() {
        let r = record(m(5, 100), m(5, 100));
        assert_eq!(classify(None, Some(&m(5, 100)), Some(&r)), Some(Change::Deleted));
        assert_eq!(classify(None, Some(&m(9, 400)), Some(&r)), Some(Change::Conflict), "deleted here, changed there");
        assert_eq!(classify(Some(&m(5, 100)), None, Some(&r)), Some(Change::DeletedOnServer));
        assert_eq!(classify(Some(&m(8, 500)), None, Some(&r)), Some(Change::Conflict), "changed here, deleted there");
    }

    #[test]
    fn both_sides_are_matched_without_regard_to_case() {
        let local = BTreeMap::from([
            ("Portals/0/Skin.css".to_string(), m(1, 10)),
            ("new.js".to_string(), m(1, 10)),
        ]);
        let remote = BTreeMap::from([
            ("portals/0/skin.css".to_string(), m(1, 10)),
            ("only-there.txt".to_string(), m(1, 10)),
        ]);
        let entries = compare(&local, &remote, &HashMap::new());
        let summary: Vec<(&str, Change)> = entries.iter().map(|e| (e.path.as_str(), e.change)).collect();
        assert_eq!(
            summary,
            [("new.js", Change::Added), ("only-there.txt", Change::RemoteOnly), ("Portals/0/Skin.css", Change::Unchanged)]
        );
        assert_eq!(counts(&entries), [(Change::Added, 1), (Change::RemoteOnly, 1)]);
    }

    /// A large project: both sides of 20 000 files and their records compared quickly.
    #[test]
    fn twenty_thousand_files_are_compared_quickly() {
        let paths: Vec<String> = (0..20_000).map(|i| format!("Portals/{}/f{i}.css", i % 9)).collect();
        let local: BTreeMap<String, FileMeta> = paths.iter().map(|p| (p.clone(), m(10, 100))).collect();
        let remote: BTreeMap<String, FileMeta> = paths.iter().map(|p| (p.to_uppercase(), m(10, 100))).collect();
        let records: HashMap<String, SyncRecord> =
            paths.iter().step_by(2).map(|p| (p.clone(), record(m(10, 100), m(10, 100)))).collect();
        let started = std::time::Instant::now();
        let entries = compare(&local, &remote, &records);
        assert_eq!(entries.len(), 20_000, "matched without regard to case");
        assert!(entries.iter().all(|e| e.change == Change::Unchanged));
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn a_same_size_file_without_a_record_is_worth_reading() {
        let e = Entry { path: "a".into(), local: Some(m(5, 10)), remote: Some(m(5, 900)), change: Change::Incoming };
        assert!(worth_verifying(&e, None));
        assert!(!worth_verifying(&e, Some(&record(m(5, 10), m(5, 10)))));
        let bigger = Entry { remote: Some(m(6, 900)), ..e.clone() };
        assert!(!worth_verifying(&bigger, None), "a different size is a different file");
    }
}
