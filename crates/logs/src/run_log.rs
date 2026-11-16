//! What an operation run writes to the day's log file - the file side of `ActivityLog`
//! (`DnnManager.Presentation/Services/ActivityLog.cs`). The app's Output model gets the same calls; this keeps the
//! run's state the file's lines depend on and writes them:
//!
//! - every line as it is - a warning `[warning] …`, an error `[error] …` with what lies behind it and what to do
//!   indented under it (`        detail`, `        → hint`); in-place progress lines are left out;
//! - an operation's title and its stages are the Output tab's, not the file's: the file has how it ended -
//!   `[success] Clone complete (1m 02s)`, `[error] Install stopped at Database (12s)`,
//!   `[cancelled] Install was cancelled (3s)`;
//! - an operation that only asked and was told no leaves no trace.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dnnm_application::operation::Reporter;

use crate::daily_file::DailyLogFile;

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Finished,
    Failed,
    Cancelled,
}

struct Run {
    title: String,
    started: Instant,
    // The running stage's name; None between stages and after the closing step.
    current: Option<(String, Instant)>,
    // After its closing step ("Clone complete"): what follows is a note, not a stage.
    closing: bool,
    closing_title: Option<String>,
    // The stage it stopped at - failed, or running when it was cancelled.
    stopped_at: Option<String>,
    // The errors written, so the run's error message isn't written twice.
    errors: Vec<String>,
}

#[derive(Default)]
struct State {
    run: Option<Run>,
    // An operation that has begun but done nothing yet - it may first ask. It shows with its first step, line or end.
    pending_title: Option<String>,
    // When a cancel was asked for: the stage running then was cut short.
    cancel_at: Option<Instant>,
}

/// The day's log file's view of the operations. Shared by the app's Output and the operations' threads.
pub struct RunLog {
    file: Arc<DailyLogFile>,
    state: Mutex<State>,
}

impl RunLog {
    pub fn new(file: Arc<DailyLogFile>) -> Self {
        Self { file, state: Mutex::new(State::default()) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An operation begins (`BeginRun`, `Header`); one still going on ends as finished.
    pub fn begin_run(&self, title: &str) {
        let mut state = self.lock();
        if state.run.is_some() {
            self.end_now(&mut state, RunStatus::Finished, None);
        }
        state.cancel_at = None;
        state.pending_title = Some(title.to_string());
    }

    /// The user said no to the operation's question: before it did anything it leaves no trace; after, it ends as
    /// cancelled.
    pub fn drop_run(&self) {
        let mut state = self.lock();
        if state.pending_title.take().is_some() {
            return;
        }
        if let Some(title) = state.run.as_ref().map(|r| r.title.clone()) {
            let message = format!("{title} - not done: the answer was no.");
            self.end_now(&mut state, RunStatus::Cancelled, Some(&message));
        }
    }

    /// The operation ended - finished, failed (with `message` as its error) or cancelled.
    pub fn end_run(&self, status: RunStatus, message: Option<&str>) {
        let mut state = self.lock();
        show_pending(&mut state);
        if state.run.is_some() {
            self.end_now(&mut state, status, message);
        } else if let Some(message) = message {
            let level = if status == RunStatus::Failed { Level::Error } else { Level::Info };
            self.add_line(&mut state, level, message, &[], None);
        }
    }

    /// A cancel was asked for: the stage running now is cut short.
    pub fn cancelling(&self) {
        let mut state = self.lock();
        show_pending(&mut state);
        state.cancel_at.get_or_insert_with(Instant::now);
        self.add_line(&mut state, Level::Info, "Cancelling…", &[], None);
    }

    /// A stage begins: "Step 3: Testing database connection…" is the stage "Testing database connection".
    pub fn step(&self, title: &str) {
        self.step_named(title, &stage_name(title));
    }

    pub fn step_named(&self, title: &str, name: &str) {
        let mut state = self.lock();
        show_pending(&mut state);
        if state.run.is_none() {
            self.add_line(&mut state, Level::Info, title, &[], None);
            return;
        }
        let cancel_at = state.cancel_at;
        let run = state.run.as_mut().expect("checked above");
        end_current(run, cancel_at);
        if is_closing(title) {
            run.closing = true;
            run.closing_title = Some(title.to_string());
            return;
        }
        // A "… complete" step that wasn't the end: what came after it was a stage of its own.
        if run.closing {
            run.closing_title = None;
        }
        run.closing = false;
        run.current = Some((name.to_string(), Instant::now()));
    }

    pub fn info(&self, text: &str) {
        self.line(Level::Info, text);
    }

    pub fn success(&self, text: &str) {
        self.line(Level::Info, text);
    }

    pub fn warn(&self, text: &str) {
        self.line(Level::Warn, text);
    }

    /// An error; a message of several lines is split into the error, what lies behind it and a hint.
    pub fn fail(&self, text: &str) {
        let (text, details, hint) = split_error(text);
        let mut state = self.lock();
        self.add_line(&mut state, Level::Error, &text, &details, hint.as_deref());
    }

    pub fn fail_with(&self, text: &str, details: &[String], hint: Option<&str>) {
        let mut state = self.lock();
        self.add_line(&mut state, Level::Error, text, details, hint);
    }

    /// An address the user may want to open.
    pub fn link(&self, url: &str) {
        let mut state = self.lock();
        show_pending(&mut state);
        self.file.append(&format!("Open {url}"));
    }

    /// Something going on in the background (keep warm): the log file has it - the tab shows its state elsewhere.
    pub fn background(&self, text: &str, is_warning: bool) {
        if is_warning { self.file.append(&format!("[warning] {text}")) } else { self.file.append(text) }
    }

    fn line(&self, level: Level, text: &str) {
        let mut state = self.lock();
        self.add_line(&mut state, level, text, &[], None);
    }

    fn add_line(&self, state: &mut State, level: Level, text: &str, details: &[String], hint: Option<&str>) {
        show_pending(state);
        if let Some(run) = state.run.as_mut() {
            // Before its first step, a line starts a stage of its own.
            if !run.closing && run.current.is_none() {
                run.current = Some(("Start".into(), Instant::now()));
            }
            if level == Level::Error {
                run.errors.push(text.to_string());
            }
        }
        self.file.append(&match level {
            Level::Warn => format!("[warning] {text}"),
            Level::Error => format!("[error] {text}"),
            Level::Info => text.to_string(),
        });
        for detail in details {
            self.file.append(&format!("        {detail}"));
        }
        if let Some(hint) = hint {
            self.file.append(&format!("        → {hint}"));
        }
    }

    fn end_now(&self, state: &mut State, status: RunStatus, message: Option<&str>) {
        if status == RunStatus::Failed
            && let Some(message) = message
            && !state.run.as_ref().is_some_and(|r| r.errors.iter().any(|e| e == message))
        {
            let (text, details, hint) = split_error(message);
            self.add_line(state, Level::Error, &text, &details, hint.as_deref());
        }
        let cancel_at = state.cancel_at;
        let Some(mut run) = state.run.take() else { return };
        match status {
            RunStatus::Failed => {
                if let Some((name, _)) = run.current.take() {
                    run.stopped_at = Some(name);
                }
            }
            // What runs after a cancel - putting things back - ends as it went; what ran when it came was cut short.
            RunStatus::Cancelled => {
                if let Some((name, started)) = run.current.take()
                    && cancel_at.is_none_or(|cancel| started < cancel)
                {
                    run.stopped_at = Some(name);
                }
            }
            RunStatus::Finished => {}
        }
        let duration = duration_text(run.started.elapsed());
        self.file.append(&match status {
            RunStatus::Finished => {
                let what = run.closing_title.clone().unwrap_or_else(|| format!("{} finished", run.title));
                format!("[success] {what} ({duration})")
            }
            RunStatus::Cancelled => {
                let what = message.map_or_else(|| format!("{} was cancelled", run.title), str::to_string);
                format!("[cancelled] {what} ({duration})")
            }
            RunStatus::Failed => match &run.stopped_at {
                Some(at) => format!("[error] {} stopped at {at} ({duration})", run.title),
                None => format!("[error] {} failed ({duration})", run.title),
            },
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Info,
    Warn,
    Error,
}

fn show_pending(state: &mut State) {
    if let Some(title) = state.pending_title.take() {
        state.run = Some(Run {
            title,
            started: Instant::now(),
            current: None,
            closing: false,
            closing_title: None,
            stopped_at: None,
            errors: Vec::new(),
        });
    }
}

/// The running stage ends - cut short when it started before a cancel was asked for.
fn end_current(run: &mut Run, cancel_at: Option<Instant>) {
    if let Some((name, started)) = run.current.take()
        && cancel_at.is_some_and(|cancel| started < cancel)
    {
        run.stopped_at = Some(name);
    }
}

/// The run's lines also reach the file from an operation's thread through the application's `Reporter`.
pub struct RunLogReporter(pub Arc<RunLog>);

impl Reporter for RunLogReporter {
    fn step(&mut self, title: &str) {
        self.0.step(title);
    }
    fn info(&mut self, text: &str) {
        self.0.info(text);
    }
    fn success(&mut self, text: &str) {
        self.0.success(text);
    }
    fn warn(&mut self, text: &str) {
        self.0.warn(text);
    }
    fn fail(&mut self, text: &str) {
        self.0.fail(text);
    }
    fn link(&mut self, url: &str) {
        self.0.link(url);
    }
}

/// "Step 3: Testing database connection" → "Testing database connection".
pub fn strip_step_number(title: &str) -> &str {
    let lower = title.get(..5).map(str::to_ascii_lowercase);
    if lower.as_deref() != Some("step ") {
        return title;
    }
    let rest = &title[5..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !rest[digits..].starts_with(':') {
        return title;
    }
    rest[digits + 1..].trim_start()
}

/// A stage's name: its title without the step number and the trailing "…", "." and spaces.
pub fn stage_name(title: &str) -> String {
    strip_step_number(title).trim_end_matches(['…', '.', ' ']).to_string()
}

/// "Clone complete", "Setup complete", "Done": the end of the stages, not one of them.
pub fn is_closing(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.ends_with(" complete") || lower == "done"
}

/// An error message of several lines: the first is the error, the others what lies behind it - and one starting with
/// "→" or "Hint:" what to do about it.
pub fn split_error(message: &str) -> (String, Vec<String>, Option<String>) {
    let lines: Vec<&str> = message.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() <= 1 {
        return (message.trim().to_string(), Vec::new(), None);
    }
    let mut hint = None;
    let mut details = Vec::new();
    for line in &lines[1..] {
        if line.starts_with('→') {
            hint = Some(line.trim_start_matches(['→', ' ']).to_string());
        } else if line.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("hint:")) {
            hint = Some(line[5..].trim().to_string());
        } else {
            details.push(line.to_string());
        }
    }
    (lines[0].to_string(), details, hint)
}

/// "57s", "1m 02s", "1h 03m" - a whole run (`OutputFormat.Long`).
pub fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, secs / 60 % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn with_log(work: impl FnOnce(&RunLog)) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let file = DailyLogFile::open(dir.path().to_path_buf());
        work(&RunLog::new(Arc::clone(&file)));
        file.close();
        let path = dir.path().join(crate::daily_file::file_name(&crate::clock::LocalDateTime::now()));
        let mut text = String::new();
        if let Ok(mut f) = crate::tail::open_shared(&path) {
            f.read_to_string(&mut text).unwrap();
        }
        // Without the time before each line.
        text.lines().map(|l| l[9..].to_string()).collect()
    }

    #[test]
    fn a_run_writes_its_lines_and_how_it_ended_not_its_title_and_stages() {
        let lines = with_log(|log| {
            log.begin_run("Install dnn10");
            log.step("Step 1: Create IIS site…");
            log.info("Site created.");
            log.warn("Port 80 is taken");
            log.step("Install complete");
            log.success("Open it at http://dnn10.dnndev.me");
            log.link("http://dnn10.dnndev.me");
            log.end_run(RunStatus::Finished, None);
        });
        assert_eq!(
            lines,
            [
                "Site created.",
                "[warning] Port 80 is taken",
                "Open it at http://dnn10.dnndev.me",
                "Open http://dnn10.dnndev.me",
                "[success] Install complete (0s)"
            ]
        );
    }

    #[test]
    fn a_failure_names_the_stage_and_its_error_once_with_details_and_hint() {
        let lines = with_log(|log| {
            log.begin_run("Install dnn10");
            log.step("Step 2: Database");
            log.fail("Login failed for user 'sa'.");
            log.end_run(RunStatus::Failed, Some("Login failed for user 'sa'."));
            log.begin_run("Clone");
            log.end_run(RunStatus::Failed, Some("Copy failed\nAccess denied\n→ Close the files"));
        });
        assert_eq!(
            lines,
            [
                "[error] Login failed for user 'sa'.",
                "[error] Install dnn10 stopped at Database (0s)",
                "[error] Copy failed",
                "        Access denied",
                "        → Close the files",
                // As the C#: the error line opens a "Start" stage, which is where it stopped.
                "[error] Clone stopped at Start (0s)"
            ]
        );
    }

    #[test]
    fn a_run_told_no_before_it_did_anything_leaves_no_trace() {
        let lines = with_log(|log| {
            log.begin_run("Remove shop");
            log.drop_run();
            log.begin_run("Stop IIS");
            log.info("Stopping");
            log.drop_run();
            log.begin_run("Upgrade");
            log.step("Backup");
            log.cancelling();
            log.end_run(RunStatus::Cancelled, None);
        });
        assert_eq!(
            lines,
            [
                "Stopping",
                "[cancelled] Stop IIS - not done: the answer was no. (0s)",
                "Cancelling…",
                "[cancelled] Upgrade was cancelled (0s)"
            ]
        );
    }

    #[test]
    fn a_new_run_ends_the_one_going_on() {
        let lines = with_log(|log| {
            log.begin_run("A");
            log.info("a");
            log.begin_run("B");
            log.background("keep warm: shop", true);
        });
        assert_eq!(lines, ["a", "[success] A finished (0s)", "[warning] keep warm: shop"]);
    }

    #[test]
    fn helpers() {
        assert_eq!(stage_name("Step 3: Testing database connection…"), "Testing database connection");
        assert_eq!(strip_step_number("step 12:x"), "x");
        assert_eq!(strip_step_number("Step x: y"), "Step x: y");
        assert!(is_closing("Clone complete") && is_closing("done") && !is_closing("Completed"));
        assert_eq!(duration_text(Duration::from_secs(57)), "57s");
        assert_eq!(duration_text(Duration::from_secs(62)), "1m 02s");
        assert_eq!(duration_text(Duration::from_secs(3780)), "1h 03m");
        assert_eq!(split_error("one line "), ("one line".to_string(), vec![], None));
        assert_eq!(split_error("a\r\nb\r\nHint: c"), ("a".to_string(), vec!["b".to_string()], Some("c".to_string())));
    }
}
