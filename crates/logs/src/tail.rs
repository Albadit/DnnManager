//! A log being shown (`ILogTail`, `FileLogTail`, `WholeFileTail` in `SiteLogs.cs`): its newest lines first, then what
//! is written to it. A large file is never read whole: only its end, then what is added.
//!
//! The C# tails run their own timers and raise events on a background thread; the port's are pulled instead - the app
//! calls [`LogTail::start`] once and [`LogTail::poll`] every [`LogTail::poll_every`] on its background executor, and
//! simply stops calling `poll` while the window is minimized (the C# `Paused`). A file is read on from where it was,
//! so no line is lost.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::clock::LocalDateTime;

/// Lines read at first (`LogsView.InitialLines`).
pub const INITIAL_LINES: usize = 5000;
/// The most lines the view keeps - older ones drop off the top as new ones come (`LogsView.MaxLines`).
pub const MAX_LINES: usize = 50_000;

/// What a read of a log brought.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TailUpdate {
    /// What was shown is out of date - the file was rewritten: clear it before adding `lines` (`ILogTail.Replaced`).
    pub replaced: bool,
    /// New lines, in order.
    pub lines: Vec<String>,
}

impl TailUpdate {
    fn lines(lines: Vec<String>) -> Self {
        Self { replaced: false, lines }
    }
}

/// A log being shown. Blocking: call it off the UI thread.
pub trait LogTail: Send {
    /// Its last `lines` lines (an event log: about as many entries' worth).
    fn start(&mut self, lines: usize) -> TailUpdate;
    /// What was written since the last call; empty when nothing was.
    fn poll(&mut self) -> TailUpdate;
    /// How often the app should call `poll`.
    fn poll_every(&self) -> Duration;
}

/// Opens a file to read, shared with its writer - which may also rename or delete it meanwhile.
pub(crate) fn open_shared(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        options.share_mode(7);
    }
    options.open(path)
}

// More than this written since the last read (a busy log while the window was minimized): only its last lines are
// read - the rest would be read, split and handed on only for the view to drop it.
const MAX_CATCH_UP: u64 = 4 * 1024 * 1024;

/// The end of a text file, then what is appended to it - looked at every second, the file shared with its writer.
pub struct FileTail {
    path: PathBuf,
    position: u64,
    // A line still being written stays until its line break arrives.
    partial: Vec<u8>,
    keep_lines: usize,
}

impl FileTail {
    pub fn new(path: PathBuf) -> Self {
        Self { path, position: 0, partial: Vec::new(), keep_lines: 1000 }
    }

    fn read_from(&mut self, file: &mut File, position: u64) -> std::io::Result<Vec<String>> {
        file.seek(SeekFrom::Start(position))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // Where the reading ended - not the length now, which may already include lines written meanwhile.
        self.position = position + bytes.len() as u64;
        let bytes = if position == 0 { bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes) } else { &bytes[..] };
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        self.partial.extend_from_slice(bytes);
        let Some(last) = self.partial.iter().rposition(|&b| b == b'\n') else { return Ok(Vec::new()) };
        let rest = self.partial.split_off(last + 1);
        let complete = std::mem::replace(&mut self.partial, rest);
        Ok(split_lines(&complete[..last]))
    }

    fn try_poll(&mut self) -> std::io::Result<Vec<String>> {
        let mut file = open_shared(&self.path)?;
        let length = file.metadata()?.len();
        if length < self.position {
            // Rewritten from the start (rolled over): read it anew.
            self.partial.clear();
            let mut lines = vec!["--- the file was emptied or replaced; reading it from the start ---".to_string()];
            lines.extend(self.read_from(&mut file, 0)?);
            Ok(lines)
        } else if length - self.position > MAX_CATCH_UP {
            let from = start_of_last_lines(&mut file, length, self.keep_lines)?;
            self.partial.clear();
            let skipped = from.saturating_sub(self.position) as f64 / 1_048_576.0;
            let mut lines =
                vec![format!("--- {} MB written meanwhile skipped; its last lines follow ---", n1(skipped))];
            lines.extend(self.read_from(&mut file, from)?);
            Ok(lines)
        } else if length > self.position {
            let position = self.position;
            self.read_from(&mut file, position)
        } else {
            Ok(Vec::new())
        }
    }
}

impl LogTail for FileTail {
    fn start(&mut self, lines: usize) -> TailUpdate {
        self.keep_lines = lines.max(1);
        let result = open_shared(&self.path).and_then(|mut file| {
            let length = file.metadata()?.len();
            let from = start_of_last_lines(&mut file, length, lines)?;
            self.read_from(&mut file, from)
        });
        TailUpdate::lines(result.unwrap_or_else(|e| vec![format!("Can't read {}: {e}", self.path.display())]))
    }

    fn poll(&mut self) -> TailUpdate {
        // Being rotated or locked for a moment - looked at again in a second.
        TailUpdate::lines(self.try_poll().unwrap_or_default())
    }

    fn poll_every(&self) -> Duration {
        Duration::from_secs(1)
    }
}

/// Back from the end in blocks until enough line breaks are found - a large file is not read whole.
fn start_of_last_lines(file: &mut File, length: u64, lines: usize) -> std::io::Result<u64> {
    const BLOCK: u64 = 64 * 1024;
    let mut buffer = vec![0u8; BLOCK as usize];
    let mut found = 0;
    let mut position = length;
    while position > 0 {
        let size = BLOCK.min(position);
        position -= size;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut buffer[..size as usize])?;
        for i in (0..size as usize).rev() {
            // The line break that ends the last line doesn't start one.
            if buffer[i] != b'\n' || position + i as u64 == length - 1 {
                continue;
            }
            found += 1;
            if found >= lines {
                return Ok(position + i as u64 + 1);
            }
        }
    }
    Ok(0)
}

fn split_lines(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == b'\n')
        .map(|line| String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line)).into_owned())
        .collect()
}

/// .NET's `N1` in the user's culture: "1,234.5".
fn n1(value: f64) -> String {
    let (decimal, group) = dnnm_infrastructure::locale::number_separators().unwrap_or(('.', ','));
    format_n1(value, decimal, group)
}

fn format_n1(value: f64, decimal: char, group: char) -> String {
    let text = format!("{value:.1}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, "0"));
    let digits: Vec<char> = whole.chars().collect();
    let mut grouped = String::new();
    for (i, digit) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(group);
        }
        grouped.push(*digit);
    }
    format!("{grouped}{decimal}{fraction}")
}

/// A small file that is rewritten rather than added to - the hosts file: read whole, then looked at every 2 s; when its
/// size or time differ it is read whole again, `replaced` and after a line saying when.
pub struct WholeFileTail {
    path: PathBuf,
    seen: Option<(u64, SystemTime)>,
}

impl WholeFileTail {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: None }
    }

    fn read(&mut self, changed: bool) -> TailUpdate {
        let result = (|| {
            let metadata = std::fs::metadata(&self.path)?;
            let now = (metadata.len(), metadata.modified()?);
            if changed && Some(now) == self.seen {
                return Ok(None);
            }
            let mut bytes = Vec::new();
            open_shared(&self.path)?.read_to_end(&mut bytes)?;
            let text = decode(&bytes);
            let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
            if lines.last().is_some_and(String::is_empty) {
                lines.pop();
            }
            self.seen = Some(now);
            if changed {
                let at = LocalDateTime::from_system_time(now.1).hms();
                lines.insert(0, format!("--- changed at {at}; read again ---"));
            }
            Ok::<_, std::io::Error>(Some(lines))
        })();
        match result {
            Ok(Some(lines)) => TailUpdate { replaced: changed, lines },
            Ok(None) => TailUpdate::default(),
            // Being written for a moment: looked at again at the next poll. Said only when it can't be read at all.
            Err(e) if !changed => TailUpdate::lines(vec![format!("Can't read {}: {e}", self.path.display())]),
            Err(_) => TailUpdate::default(),
        }
    }
}

impl LogTail for WholeFileTail {
    /// All of it: the lines asked for don't matter.
    fn start(&mut self, _lines: usize) -> TailUpdate {
        self.read(false)
    }

    fn poll(&mut self) -> TailUpdate {
        self.read(true)
    }

    fn poll_every(&self) -> Duration {
        Duration::from_secs(2)
    }
}

/// UTF-8, or the UTF-16 a byte order mark says (`detectEncodingFromByteOrderMarks`).
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if le { u16::from_le_bytes(c) } else { u16::from_be_bytes(c) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn starts_with_the_last_lines_then_follows_what_is_appended() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        append(&file, "\u{FEFF}one\r\ntwo\r\nthree\r\n");
        let mut tail = FileTail::new(file.clone());

        assert_eq!(tail.start(2).lines, ["two", "three"]);
        assert!(tail.poll().lines.is_empty());

        append(&file, "four\r\nfi");
        assert_eq!(tail.poll().lines, ["four"], "A line still being written waits for its line break");
        append(&file, "ve\nsix\n");
        assert_eq!(tail.poll().lines, ["five", "six"]);
    }

    #[test]
    fn the_whole_of_a_short_file_and_its_bom_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        append(&file, "\u{FEFF}one\ntwo");
        let mut tail = FileTail::new(file.clone());
        assert_eq!(tail.start(10).lines, ["one"]);
        append(&file, "\n");
        assert_eq!(tail.poll().lines, ["two"]);
    }

    #[test]
    fn a_file_emptied_is_read_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        append(&file, "one\ntwo\nthree\n");
        let mut tail = FileTail::new(file.clone());
        tail.start(5);
        std::fs::write(&file, "new\n").unwrap();
        assert_eq!(tail.poll().lines, ["--- the file was emptied or replaced; reading it from the start ---", "new"]);
    }

    #[test]
    fn much_written_meanwhile_only_its_last_lines_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.log");
        append(&file, "first\n");
        let mut tail = FileTail::new(file.clone());
        tail.start(3);
        let line = format!("{}\n", "x".repeat(1023));
        append(&file, &line.repeat(5 * 1024));
        append(&file, "last\n");
        let update = tail.poll();
        assert_eq!(update.lines.len(), 4, "The note and the last three lines");
        assert!(
            update.lines[0].starts_with("--- 5")
                && update.lines[0].ends_with(" MB written meanwhile skipped; its last lines follow ---")
        );
        assert_eq!(update.lines[3], "last");
    }

    #[test]
    fn a_missing_file_says_so() {
        let mut tail = FileTail::new(PathBuf::from(r"C:\nowhere\none.log"));
        let lines = tail.start(5).lines;
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with(r"Can't read C:\nowhere\none.log: "), "{lines:?}");
        assert!(tail.poll().lines.is_empty(), "Polling stays quiet");
    }

    #[test]
    fn the_last_lines_are_found_across_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.log");
        let text: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        append(&file, &text);
        let lines = FileTail::new(file).start(INITIAL_LINES).lines;
        assert_eq!(lines.len(), INITIAL_LINES);
        assert_eq!(lines[0], "line 15000");
        assert_eq!(lines[INITIAL_LINES - 1], "line 19999");
    }

    #[test]
    fn the_hosts_file_rewritten_in_the_middle_is_shown_anew() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hosts");
        std::fs::write(&file, "127.0.0.1 localhost\r\n# begin\r\n127.0.0.1 shop.dnndev.me\r\n# end\r\n").unwrap();
        let mut tail = WholeFileTail::new(file.clone());

        let first = tail.start(10);
        assert!(!first.replaced);
        assert_eq!(first.lines, ["127.0.0.1 localhost", "# begin", "127.0.0.1 shop.dnndev.me", "# end"]);
        assert_eq!(tail.poll(), TailUpdate::default(), "Unchanged: nothing");

        std::fs::write(&file, "127.0.0.1 localhost\r\n# begin\r\n# end\r\n").unwrap();
        // The size differs, whatever the clock's resolution.
        let second = tail.poll();
        assert!(second.replaced);
        assert!(second.lines[0].starts_with("--- changed at ") && second.lines[0].ends_with("; read again ---"));
        assert_eq!(second.lines[1..], ["127.0.0.1 localhost", "# begin", "# end"]);
    }

    #[test]
    fn utf16_with_a_bom_is_decoded() {
        assert_eq!(decode(&[0xFF, 0xFE, b'a', 0, b'b', 0]), "ab");
        assert_eq!(decode("\u{FEFF}é".as_bytes()), "é");
    }

    #[test]
    fn n1_groups_thousands() {
        assert_eq!(format_n1(1234.56, ',', '.'), "1.234,6");
        assert_eq!(format_n1(5.0, '.', ','), "5.0");
        assert_eq!(format_n1(123456.0, '.', ','), "123,456.0");
    }
}
