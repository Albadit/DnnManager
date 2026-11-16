//! What every use case runs inside (`OperationRunner`, `OperationUndo`, `IProgressReporter`, `IUserPrompt` of the WPF
//! app): it reports its stages, notes before each step how to take it back, honours Cancel, and asks the user through
//! a prompt. On a failure or a cancel the runner undoes what was noted, last step first, each step within its time
//! limit (ADR 0001).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// SQL and IIS undo steps get a minute, file steps 30 s, the whole undo 5 minutes (`OperationUndo`).
pub const STEP_LIMIT: Duration = Duration::from_secs(60);
pub const FILE_STEP_LIMIT: Duration = Duration::from_secs(30);
pub const TOTAL_LIMIT: Duration = Duration::from_secs(5 * 60);
/// How long a step whose time is up gets to end once it is told to stop.
const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// A file kept in memory to be put back is a configuration file, not a backup.
const MAX_KEPT_FILE: u64 = 10 * 1024 * 1024;

/// Cancel: set once, seen by every clone. A step polls it between pieces of work, or passes it on.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Err("Cancelled.") once cancelled - for `?` between a use case's steps.
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
    }
}

/// The error a cancelled operation ends with; the runner tells it apart from a failure by it.
pub const CANCELLED: &str = "Cancelled.";

/// How a use case tells what it does (`IProgressReporter`): the Output tab and the log.
pub trait Reporter: Send {
    /// A new stage of the operation ("Create IIS site").
    fn step(&mut self, title: &str);
    fn info(&mut self, text: &str);
    fn success(&mut self, text: &str);
    fn warn(&mut self, text: &str);
    fn fail(&mut self, text: &str);
    /// The stages it will go through, before it starts; optional.
    fn plan(&mut self, _stages: &[&str]) {}
    /// One fact for the Output's header ("Database: dnn10"); optional.
    fn fact(&mut self, _name: &str, _value: &str) {}
    /// An address the user may want to open; optional.
    fn link(&mut self, _url: &str) {}
}

/// How a use case asks the user (`IUserPrompt`) - a dialog on the UI thread; the operation waits for the answer.
pub trait Prompt: Send + Sync {
    /// Yes or no. `danger`: the yes deletes or replaces something (the dialog shows it so and No is the default).
    fn confirm(&self, question: &str, yes: &str, no: &str, danger: bool) -> bool;
    /// One of several answers; None when the dialog was closed.
    fn choose(&self, question: &str, answers: &[&str]) -> Option<usize>;
}

type UndoFn = Box<dyn FnOnce(&CancelToken) -> Result<(), String> + Send>;

struct UndoStep {
    what: String,
    run: UndoFn,
    limit: Duration,
}

/// What an operation has made, noted before each step, to take back on a failure or a cancel (`OperationUndo`).
#[derive(Default)]
pub struct Undo {
    steps: Vec<UndoStep>,
    lost: Vec<String>,
    kept_files: Vec<PathBuf>,
    /// Told what would be undone each time that changes (`OperationUndo.Changed`) - the runner keeps it on disk, so a
    /// start after DNN Manager ended mid-operation can say what may be half done.
    watcher: Option<Box<dyn Fn(Vec<String>) + Send>>,
}

impl Undo {
    /// Calls `watch` with [`Undo::pending`] whenever it changes.
    pub fn watch(&mut self, watch: impl Fn(Vec<String>) + Send + 'static) {
        self.watcher = Some(Box::new(watch));
    }

    fn changed(&self) {
        if let Some(watch) = &self.watcher {
            watch(self.pending());
        }
    }
}

impl Undo {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.lost.is_empty()
    }

    /// What would be undone now, last step first, then what can't be put back.
    pub fn pending(&self) -> Vec<String> {
        self.steps
            .iter()
            .rev()
            .map(|s| s.what.clone())
            .chain(self.lost.iter().map(|l| format!("Can't be put back: {l}")))
            .collect()
    }

    /// Notes how to take back the step that is about to run.
    pub fn add(
        &mut self,
        what: impl Into<String>,
        limit: Duration,
        undo: impl FnOnce(&CancelToken) -> Result<(), String> + Send + 'static,
    ) {
        self.steps.push(UndoStep { what: what.into(), run: Box::new(undo), limit });
        self.changed();
    }

    /// A step that can't be taken back (a dropped database): said when the operation is undone.
    pub fn cannot_undo(&mut self, what: impl Into<String>) {
        self.lost.push(what.into());
        self.changed();
    }

    /// Leaves everything as it is, to look into (a DNN install that failed).
    pub fn keep(&mut self) {
        self.steps.clear();
        self.lost.clear();
        self.changed();
    }

    /// The folder doesn't exist yet: the undo deletes it again. Nothing is noted when it already exists.
    pub fn delete_folder_on_undo(&mut self, folder: &Path) {
        if folder.exists() {
            return;
        }
        let folder = folder.to_path_buf();
        self.add(format!("Delete {}", folder.display()), FILE_STEP_LIMIT, move |cancel| delete_folder(&folder, cancel));
    }

    /// Keeps the file as it is now (or that it isn't there), to put it back on undo. Each file only once.
    pub fn restore_file_on_undo(&mut self, file: &Path) {
        if self.kept_files.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(file.as_os_str())) {
            return;
        }
        self.kept_files.push(file.to_path_buf());
        let path = file.to_path_buf();
        if !file.exists() {
            self.add(format!("Delete {}", file.display()), FILE_STEP_LIMIT, move |_| {
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            });
            return;
        }
        if std::fs::metadata(file).map(|m| m.len() > MAX_KEPT_FILE).unwrap_or(false) {
            self.cannot_undo(format!("{} was overwritten.", file.display()));
            return;
        }
        match std::fs::read(file) {
            Ok(before) => {
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.add(format!("Put {name} back as it was"), FILE_STEP_LIMIT, move |_| {
                    std::fs::write(&path, &before).map_err(|e| e.to_string())
                });
            }
            Err(e) => self.cannot_undo(format!("{} (could not keep a copy of it: {e})", file.display())),
        }
    }

    /// Undoes everything noted, last step first; true when all of it was undone. `skip` stops the undo ("Stop
    /// undoing"); a step past its time limit is told to stop and named as not done.
    pub fn run(&mut self, reporter: &mut dyn Reporter, skip: &CancelToken) -> bool {
        let mut all_done = true;
        let total = self.steps.iter().map(|s| s.limit).max().map_or(TOTAL_LIMIT, |m| m.max(TOTAL_LIMIT));
        let started = Instant::now();
        let steps = std::mem::take(&mut self.steps);
        for step in steps.into_iter().rev() {
            let over = started.elapsed() >= total;
            if skip.is_cancelled() || over {
                all_done = false;
                reporter.warn(&format!("Not undone: {}", step.what));
                continue;
            }
            let what = step.what.clone();
            let left = total.saturating_sub(started.elapsed());
            match run_step(step, left, skip) {
                Ok(()) => reporter.success(&what),
                Err(e) => {
                    all_done = false;
                    reporter.fail(&format!("{what}: {e}"));
                }
            }
        }
        for lost in self.lost.drain(..) {
            all_done = false;
            reporter.warn(&format!("Can't be put back: {lost}"));
        }
        self.kept_files.clear();
        self.changed();
        all_done
    }
}

/// Runs one undo step on a thread of its own, so a step that hangs can't keep the undo - or quitting - from ending.
fn run_step(step: UndoStep, total_left: Duration, skip: &CancelToken) -> Result<(), String> {
    let limit = step.limit.min(total_left);
    let token = CancelToken::new();
    let (done, wait) = mpsc::channel();
    let step_token = token.clone();
    let run = step.run;
    std::thread::spawn(move || {
        let result = run(&step_token);
        let _ = done.send(result);
    });
    let deadline = Instant::now() + limit;
    loop {
        let slice = Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now()));
        match wait.recv_timeout(slice) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the step ended without a result".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if skip.is_cancelled() || Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    // A moment for a step that honours its token to end - not more: it may ignore it.
    token.cancel();
    if let Ok(result) = wait.recv_timeout(CANCEL_GRACE)
        && result.is_ok()
    {
        return result;
    }
    if skip.is_cancelled() {
        Err("not done - the undo was stopped.".into())
    } else if limit < step.limit {
        Err(format!("not done - the undo took longer than {} in all.", duration(TOTAL_LIMIT)))
    } else {
        Err(format!("didn't finish within {} - left as it is.", duration(limit)))
    }
}

fn duration(d: Duration) -> String {
    if d.as_secs() < 120 { format!("{} seconds", d.as_secs()) } else { format!("{} minutes", d.as_secs() / 60) }
}

/// Deletes a folder made by the operation, trying again while something still has a file open (10 × 0.5 s).
fn delete_folder(folder: &Path, cancel: &CancelToken) -> Result<(), String> {
    for attempt in 1.. {
        if !folder.exists() {
            return Ok(());
        }
        match std::fs::remove_dir_all(folder) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= 10 || cancel.is_cancelled() => {
                return Err(format!("{e} Delete the folder yourself once nothing uses it."));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    Ok(())
}

/// What a use case gets: its reporter, its undo list, Cancel and the prompt.
pub struct Operation<'a> {
    pub report: &'a mut dyn Reporter,
    pub undo: Undo,
    pub cancel: CancelToken,
    pub prompt: &'a dyn Prompt,
}

impl<'a> Operation<'a> {
    pub fn new(report: &'a mut dyn Reporter, prompt: &'a dyn Prompt, cancel: CancelToken) -> Self {
        Self { report, undo: Undo::default(), cancel, prompt }
    }
}

/// How an operation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Failed(String),
    Cancelled,
}

/// Runs a use case and, when it fails or is cancelled, undoes what it noted (`OperationRunner.RunAsync`). `skip`
/// stops the undo itself ("Stop undoing").
pub fn run(
    report: &mut dyn Reporter,
    prompt: &dyn Prompt,
    cancel: CancelToken,
    skip: &CancelToken,
    work: impl FnOnce(&mut Operation) -> Result<(), String>,
) -> Outcome {
    run_watched(report, prompt, cancel, skip, None, work)
}

/// [`run`], with `watch` told what would be undone whenever that changes (the unfinished-operation record).
pub fn run_watched(
    report: &mut dyn Reporter,
    prompt: &dyn Prompt,
    cancel: CancelToken,
    skip: &CancelToken,
    watch: Option<Box<dyn Fn(Vec<String>) + Send>>,
    work: impl FnOnce(&mut Operation) -> Result<(), String>,
) -> Outcome {
    let mut op = Operation::new(report, prompt, cancel.clone());
    if let Some(watch) = watch {
        op.undo.watch(watch);
    }
    let result = work(&mut op);
    let Operation { report, mut undo, .. } = op;
    match result {
        Ok(()) => Outcome::Finished,
        Err(error) => {
            let cancelled = cancel.is_cancelled() || error == CANCELLED;
            if !cancelled {
                report.fail(&error);
            }
            if !undo.is_empty() {
                report.step("Undo");
                if !undo.run(report, skip) {
                    report.warn("Not everything could be undone - what is left is named above.");
                }
            }
            if cancelled { Outcome::Cancelled } else { Outcome::Failed(error) }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Lines(pub Vec<String>);

    impl Reporter for Lines {
        fn step(&mut self, t: &str) {
            self.0.push(format!("## {t}"));
        }
        fn info(&mut self, t: &str) {
            self.0.push(t.into());
        }
        fn success(&mut self, t: &str) {
            self.0.push(format!("ok {t}"));
        }
        fn warn(&mut self, t: &str) {
            self.0.push(format!("warn {t}"));
        }
        fn fail(&mut self, t: &str) {
            self.0.push(format!("! {t}"));
        }
    }

    /// Answers every question the same way and records them.
    pub struct Answer(pub bool, pub Mutex<Vec<String>>);

    impl Answer {
        pub fn yes() -> Self {
            Self(true, Mutex::new(vec![]))
        }
        pub fn no() -> Self {
            Self(false, Mutex::new(vec![]))
        }
    }

    impl Prompt for Answer {
        fn confirm(&self, q: &str, _: &str, _: &str, _: bool) -> bool {
            self.1.lock().unwrap().push(q.into());
            self.0
        }
        fn choose(&self, q: &str, _: &[&str]) -> Option<usize> {
            self.1.lock().unwrap().push(q.into());
            self.0.then_some(0)
        }
    }

    #[test]
    fn a_failure_undoes_last_step_first() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut lines = Lines::default();
        let (a, b) = (order.clone(), order.clone());
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            op.undo.add("Remove the site", STEP_LIMIT, move |_| {
                a.lock().unwrap().push("site");
                Ok(())
            });
            op.undo.add("Drop the database", STEP_LIMIT, move |_| {
                b.lock().unwrap().push("db");
                Ok(())
            });
            op.undo.cannot_undo("the old database");
            Err("boom".into())
        });
        assert_eq!(outcome, Outcome::Failed("boom".into()));
        assert_eq!(*order.lock().unwrap(), vec!["db", "site"]);
        assert_eq!(
            lines.0,
            vec![
                "! boom",
                "## Undo",
                "ok Drop the database",
                "ok Remove the site",
                "warn Can't be put back: the old database",
                "warn Not everything could be undone - what is left is named above."
            ]
        );
    }

    #[test]
    fn success_and_keep_undo_nothing() {
        let mut lines = Lines::default();
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let outcome = run(&mut lines, &Answer::yes(), CancelToken::new(), &CancelToken::new(), |op| {
            op.undo.add("x", STEP_LIMIT, move |_| {
                r.store(true, Ordering::SeqCst);
                Ok(())
            });
            op.undo.keep();
            Err("install failed".into())
        });
        assert_eq!(outcome, Outcome::Failed("install failed".into()));
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(lines.0, vec!["! install failed"]);
    }

    #[test]
    fn a_cancel_is_cancelled_and_still_undoes() {
        let cancel = CancelToken::new();
        let mut lines = Lines::default();
        let c = cancel.clone();
        let outcome = run(&mut lines, &Answer::yes(), cancel, &CancelToken::new(), |op| {
            op.undo.add("Delete the folder", FILE_STEP_LIMIT, |_| Ok(()));
            c.cancel();
            op.cancel.check()
        });
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(lines.0, vec!["## Undo", "ok Delete the folder"]);
    }

    #[test]
    fn a_step_past_its_limit_is_left_and_named() {
        let mut undo = Undo::default();
        undo.add("Hang", Duration::from_millis(200), |cancel| {
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err("stopped".into())
        });
        let mut lines = Lines::default();
        assert!(!undo.run(&mut lines, &CancelToken::new()));
        assert_eq!(lines.0, vec!["! Hang: didn't finish within 0 seconds - left as it is."]);
    }

    #[test]
    fn the_watcher_sees_every_change() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let mut undo = Undo::default();
        undo.watch(move |pending| s.lock().unwrap().push(pending.len()));
        undo.add("a", STEP_LIMIT, |_| Ok(()));
        undo.cannot_undo("b");
        undo.run(&mut Lines::default(), &CancelToken::new());
        assert_eq!(*seen.lock().unwrap(), vec![1, 2, 0]);
    }

    #[test]
    fn stop_undoing_skips_what_is_left() {
        let skip = CancelToken::new();
        skip.cancel();
        let mut undo = Undo::default();
        undo.add("a", STEP_LIMIT, |_| Ok(()));
        let mut lines = Lines::default();
        assert!(!undo.run(&mut lines, &skip));
        assert_eq!(lines.0, vec!["warn Not undone: a"]);
    }

    #[test]
    fn files_and_folders_are_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("web.config");
        std::fs::write(&config, "before").unwrap();
        let new_file = dir.path().join("new.txt");
        let new_folder = dir.path().join("made");
        let mut undo = Undo::default();
        undo.restore_file_on_undo(&config);
        undo.restore_file_on_undo(&config);
        undo.restore_file_on_undo(&new_file);
        undo.delete_folder_on_undo(&new_folder);
        undo.delete_folder_on_undo(dir.path());
        assert_eq!(undo.pending().len(), 3, "each file once; an existing folder isn't deleted");
        std::fs::write(&config, "after").unwrap();
        std::fs::write(&new_file, "x").unwrap();
        std::fs::create_dir(&new_folder).unwrap();
        assert!(undo.run(&mut Lines::default(), &CancelToken::new()));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "before");
        assert!(!new_file.exists() && !new_folder.exists() && dir.path().exists());
    }
}
