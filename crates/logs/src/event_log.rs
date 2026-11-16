//! Windows event log entries that match a filter (`EventLogTail` in `SiteLogs.cs`): the newest ones, then each new one.
//!
//! The C# reads them with `EventLogReader` and is pushed new ones by an `EventLogWatcher`. The port uses the same
//! Windows API underneath (wevtapi: `EvtQuery`, `EvtNext`, `EvtRender`, `EvtFormatMessage`) and pulls instead of
//! being pushed: `poll` asks for the entries with a record number above the newest one seen, so the app drives every
//! log the same way and nothing arrives on a thread of Windows' own.

use std::time::Duration;

use crate::catalog::EventLogFilter;
use crate::tail::{LogTail, TailUpdate};

/// The most entries looked through at first: a busy log can hold many of these providers' entries about others.
const MAX_SCANNED: usize = 5000;

/// One entry, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEntry {
    pub record_id: u64,
    /// "2026-10-01 14:03:22" in local time.
    pub time: String,
    /// "Error", "Warning"… as Windows names the level - or "Level 4".
    pub level: String,
    pub provider: String,
    pub event_id: u32,
    pub message: String,
}

/// "2026-10-01 14:03:22  ERROR  ASP.NET 4.0.30319.0 (1309)", then the message, indented, then an empty line.
pub fn format_entry(entry: &EventEntry) -> Vec<String> {
    let mut lines =
        vec![format!("{}  {}  {} ({})", entry.time, entry.level.to_uppercase(), entry.provider, entry.event_id)];
    for line in entry.message.split('\n') {
        let line = line.trim_end_matches('\r');
        if !line.is_empty() {
            lines.push(format!("    {line}"));
        }
    }
    lines.push(String::new());
    lines
}

/// The XPath query of the filter's providers - and, when following, of the entries after `after`.
pub fn query(filter: &EventLogFilter, after: Option<u64>) -> String {
    let providers = filter.providers.iter().map(|p| format!("@Name='{}'", p.replace('\'', "&apos;")));
    let providers = providers.collect::<Vec<_>>().join(" or ");
    match after {
        Some(id) => format!("*[System[Provider[{providers}] and (EventRecordID > {id})]]"),
        None => format!("*[System[Provider[{providers}]]]"),
    }
}

/// Whether the entry is about what the filter looks for: its message mentions the text, ignoring case.
pub fn matches(filter: &EventLogFilter, message: &str) -> bool {
    filter.message_contains.as_ref().is_none_or(|text| message.to_lowercase().contains(&text.to_lowercase()))
}

/// The newest entries of a filter, then those written after them.
pub struct EventLogTail {
    filter: EventLogFilter,
    // The newest record looked at - matching or not; None until the first read worked.
    newest: Option<u64>,
}

impl EventLogTail {
    pub fn new(filter: EventLogFilter) -> Self {
        Self { filter, newest: None }
    }

    fn cannot_read(&self, error: &str) -> TailUpdate {
        TailUpdate {
            replaced: false,
            lines: vec![format!("Can't read the {} event log: {error}", self.filter.log_name)],
        }
    }
}

impl LogTail for EventLogTail {
    fn start(&mut self, lines: usize) -> TailUpdate {
        // Newest first, as many as are wanted (each entry is a few lines), then in the order they happened.
        let budget = (lines / 6).max(20);
        match platform::read(&self.filter, None, budget, MAX_SCANNED) {
            Ok((mut entries, newest)) => {
                self.newest = Some(newest.unwrap_or(0));
                entries.reverse();
                let text: Vec<String> = entries.iter().flat_map(format_entry).collect();
                let text = if text.is_empty() {
                    vec![format!(
                        "No entries yet - {} event log, {}. New ones show up here.",
                        self.filter.log_name,
                        self.filter.providers.join(", ")
                    )]
                } else {
                    text
                };
                TailUpdate { replaced: false, lines: text }
            }
            Err(error) => self.cannot_read(&error),
        }
    }

    fn poll(&mut self) -> TailUpdate {
        // The first read failed: it said so, and there is nothing to follow.
        let Some(after) = self.newest else { return TailUpdate::default() };
        match platform::read(&self.filter, Some(after), usize::MAX, usize::MAX) {
            Ok((entries, newest)) => {
                if let Some(newest) = newest {
                    self.newest = Some(newest.max(after));
                }
                TailUpdate { replaced: false, lines: entries.iter().flat_map(format_entry).collect() }
            }
            // The log being cleared or Windows busy for a moment: looked at again at the next poll.
            Err(_) => TailUpdate::default(),
        }
    }

    fn poll_every(&self) -> Duration {
        Duration::from_secs(1)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{EventEntry, EventLogFilter};

    pub fn read(
        _: &EventLogFilter,
        _: Option<u64>,
        _: usize,
        _: usize,
    ) -> Result<(Vec<EventEntry>, Option<u64>), String> {
        Err("event logs are only on Windows".into())
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;

    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
    use windows::Win32::System::EventLog::*;
    use windows::core::{HSTRING, PCWSTR};

    use super::{EventEntry, EventLogFilter, matches};
    use crate::clock::{LocalDateTime, from_file_time};

    /// Closes the handle when dropped.
    struct Handle(EVT_HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle wevtapi gave, closed once.
            unsafe {
                let _ = EvtClose(self.0);
            }
        }
    }

    /// The matching entries of the filter - newest first, or after `after` oldest first - at most `wanted`, looking
    /// through `max_scanned` at most; and the newest record number looked at.
    pub fn read(
        filter: &EventLogFilter,
        after: Option<u64>,
        wanted: usize,
        max_scanned: usize,
    ) -> Result<(Vec<EventEntry>, Option<u64>), String> {
        let direction = if after.is_some() { EvtQueryForwardDirection } else { EvtQueryReverseDirection };
        let path = HSTRING::from(filter.log_name.as_str());
        let xpath = HSTRING::from(super::query(filter, after).as_str());
        // SAFETY: strings live across the call; the handles are closed by `Handle`.
        let (results, system, user) = unsafe {
            let results = EvtQuery(None, &path, &xpath, EvtQueryChannelPath.0 | direction.0).map_err(message)?;
            let system = EvtCreateRenderContext(None, EvtRenderContextSystem.0).map_err(message)?;
            let user = EvtCreateRenderContext(None, EvtRenderContextUser.0).map_err(message)?;
            (Handle(results), Handle(system), Handle(user))
        };
        let mut publishers: HashMap<String, Option<Handle>> = HashMap::new();
        let mut entries = Vec::new();
        let mut newest = None;
        let mut scanned = 0;
        'batches: loop {
            let mut batch = [0isize; 32];
            let mut returned = 0u32;
            // SAFETY: the array's length is passed with it; each returned handle is wrapped at once.
            if let Err(e) = unsafe { EvtNext(results.0, &mut batch, 5000, 0, &mut returned) } {
                if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
                    break;
                }
                return Err(message(e));
            }
            let events: Vec<Handle> = batch[..returned as usize].iter().map(|&h| Handle(EVT_HANDLE(h))).collect();
            for event in &events {
                scanned += 1;
                if scanned > max_scanned || entries.len() >= wanted {
                    break 'batches;
                }
                let Some(mut entry) = system_values(&system, event) else { continue };
                newest = Some(newest.map_or(entry.record_id, |n: u64| n.max(entry.record_id)));
                let publisher = publishers
                    .entry(entry.provider.clone())
                    .or_insert_with(|| open_publisher(&entry.provider))
                    .as_ref()
                    .map(|h| h.0);
                entry.message = format_message(publisher, event, EvtFormatMessageEvent.0)
                    .unwrap_or_else(|| user_values(&user, event));
                if let Some(level) = format_message(publisher, event, EvtFormatMessageLevel.0) {
                    entry.level = level;
                }
                if matches(filter, &entry.message) {
                    entries.push(entry);
                }
            }
        }
        Ok((entries, newest))
    }

    fn message(error: windows::core::Error) -> String {
        error.message().trim_end().to_string()
    }

    fn open_publisher(provider: &str) -> Option<Handle> {
        let name = HSTRING::from(provider);
        // SAFETY: the name lives across the call.
        unsafe { EvtOpenPublisherMetadata(None, &name, PCWSTR::null(), 0, 0) }.ok().map(Handle)
    }

    /// The event's values in `context`, as wevtapi lays them out: an array of EVT_VARIANT and what they point to.
    fn render(context: &Handle, event: &Handle) -> Option<(Vec<u64>, usize)> {
        let (mut used, mut count) = (0u32, 0u32);
        // SAFETY: first asks for the size, then renders into a buffer of it, 8-byte aligned for the variants.
        unsafe {
            if let Err(e) = EvtRender(Some(context.0), event.0, EvtRenderEventValues.0, 0, None, &mut used, &mut count)
                && e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
            {
                return None;
            }
            let mut buffer = vec![0u64; (used as usize).div_ceil(8)];
            EvtRender(
                Some(context.0),
                event.0,
                EvtRenderEventValues.0,
                (buffer.len() * 8) as u32,
                Some(buffer.as_mut_ptr().cast()),
                &mut used,
                &mut count,
            )
            .ok()?;
            Some((buffer, count as usize))
        }
    }

    fn variants(buffer: &[u64], count: usize) -> &[EVT_VARIANT] {
        let count = count.min(buffer.len() * 8 / size_of::<EVT_VARIANT>());
        // SAFETY: wevtapi wrote `count` EVT_VARIANTs at the start of the buffer, which is aligned for them.
        unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), count) }
    }

    fn system_values(context: &Handle, event: &Handle) -> Option<EventEntry> {
        let (buffer, count) = render(context, event)?;
        let values = variants(&buffer, count);
        let get = |id: EVT_SYSTEM_PROPERTY_ID| values.get(id.0 as usize);
        // SAFETY: each union field is read only for the type wevtapi says it holds.
        unsafe {
            let provider = get(EvtSystemProviderName)
                .filter(|v| v.Type == EvtVarTypeString.0 as u32 && !v.Anonymous.StringVal.is_null())
                .map(|v| v.Anonymous.StringVal.to_string().unwrap_or_default())
                .unwrap_or_default();
            let event_id = get(EvtSystemEventID).map_or(0, |v| u32::from(v.Anonymous.UInt16Val));
            let level = get(EvtSystemLevel).map_or(0, |v| v.Anonymous.ByteVal);
            let time = get(EvtSystemTimeCreated).map_or(0, |v| v.Anonymous.FileTimeVal);
            let record_id = get(EvtSystemEventRecordId).map_or(0, |v| v.Anonymous.UInt64Val);
            Some(EventEntry {
                record_id,
                time: LocalDateTime::from_system_time(from_file_time(time)).ymd_hms(),
                level: format!("Level {level}"),
                provider,
                event_id,
                message: String::new(),
            })
        }
    }

    /// The entry's data values joined by spaces - what the C# shows when the provider's message can't be formatted.
    fn user_values(context: &Handle, event: &Handle) -> String {
        let Some((buffer, count)) = render(context, event) else { return String::new() };
        let mut parts = Vec::new();
        for v in variants(&buffer, count) {
            let kind = EVT_VARIANT_TYPE(v.Type as i32);
            // SAFETY: each union field is read only for the type wevtapi says it holds.
            let text = unsafe {
                if kind == EvtVarTypeString && !v.Anonymous.StringVal.is_null() {
                    v.Anonymous.StringVal.to_string().unwrap_or_default()
                } else if kind == EvtVarTypeUInt16 {
                    v.Anonymous.UInt16Val.to_string()
                } else if kind == EvtVarTypeUInt32 {
                    v.Anonymous.UInt32Val.to_string()
                } else if kind == EvtVarTypeInt32 {
                    v.Anonymous.Int32Val.to_string()
                } else if kind == EvtVarTypeUInt64 {
                    v.Anonymous.UInt64Val.to_string()
                } else if kind == EvtVarTypeInt64 {
                    v.Anonymous.Int64Val.to_string()
                } else {
                    continue;
                }
            };
            parts.push(text);
        }
        parts.join(" ")
    }

    fn format_message(publisher: Option<EVT_HANDLE>, event: &Handle, flags: u32) -> Option<String> {
        let publisher = publisher?;
        let mut used = 0u32;
        // SAFETY: first asks for the size, then formats into a buffer of it.
        unsafe {
            if let Err(e) = EvtFormatMessage(Some(publisher), Some(event.0), 0, None, flags, None, &mut used)
                && e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
            {
                return None;
            }
            let mut buffer = vec![0u16; used as usize];
            EvtFormatMessage(Some(publisher), Some(event.0), 0, None, flags, Some(&mut buffer), &mut used).ok()?;
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(contains: Option<&str>) -> EventLogFilter {
        EventLogFilter {
            log_name: "Application".into(),
            providers: vec!["ASP.NET 4.0.30319.0".into(), "ASP.NET 2.0.50727.0".into()],
            message_contains: contains.map(String::from),
        }
    }

    #[test]
    fn the_query_names_the_providers_and_when_following_the_records_after() {
        assert_eq!(
            query(&filter(None), None),
            "*[System[Provider[@Name='ASP.NET 4.0.30319.0' or @Name='ASP.NET 2.0.50727.0']]]"
        );
        assert!(query(&filter(None), Some(42)).ends_with("] and (EventRecordID > 42)]]"));
    }

    #[test]
    fn an_entry_is_its_header_its_message_indented_and_an_empty_line() {
        let entry = EventEntry {
            record_id: 1,
            time: "2026-10-01 14:03:22".into(),
            level: "Error".into(),
            provider: "ASP.NET 4.0.30319.0".into(),
            event_id: 1309,
            message: "Event code: 3005\r\n\r\nApplication: /LM/W3SVC/2/ROOT\r\n".into(),
        };
        assert_eq!(
            format_entry(&entry),
            [
                "2026-10-01 14:03:22  ERROR  ASP.NET 4.0.30319.0 (1309)",
                "    Event code: 3005",
                "    Application: /LM/W3SVC/2/ROOT",
                ""
            ]
        );
    }

    #[test]
    fn the_message_must_mention_the_text_ignoring_case() {
        assert!(matches(&filter(Some("/LM/W3SVC/2/")), "Application: /lm/w3svc/2/ROOT"));
        assert!(!matches(&filter(Some("/LM/W3SVC/2/")), "Application: /LM/W3SVC/21/ROOT"));
        assert!(matches(&filter(None), "anything"));
    }

    // Reads the real Application log, read-only: run with `cargo test -p dnnm-logs -- --ignored`.
    #[test]
    #[ignore = "reads this computer's Application event log"]
    fn reads_the_real_application_log() {
        let filter = EventLogFilter {
            log_name: "Application".into(),
            providers: vec!["Microsoft-Windows-Security-SPP".into(), "Windows Error Reporting".into()],
            message_contains: None,
        };
        let mut tail = EventLogTail::new(filter);
        let first = tail.start(120);
        println!("{}", first.lines.join("\n"));
        assert!(!first.lines.is_empty());
        assert!(!first.lines[0].starts_with("Can't read"), "{:?}", first.lines[0]);
        assert!(tail.newest.is_some());
        let _ = tail.poll();
    }
}
