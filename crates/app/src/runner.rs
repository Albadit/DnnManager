//! Runs a use case as the one operation (`OperationRunner`): on a thread of its own, its lines streamed to the UI as
//! they come, its questions shown as dialogs while it waits for the answer, Cancel and "Stop undoing" passed in, and
//! its undo run on a failure or a cancel (`dnnm_application::operation::run`).

use std::sync::mpsc;
use std::time::SystemTime;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::SharedString;

use dnnm_application::operation::{self, CancelToken, Operation, Outcome, Prompt, Reporter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Stage,
    Info,
    Success,
    Warn,
    Fail,
}

/// One line of the Output tab.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: SystemTime,
    pub kind: LineKind,
    pub text: SharedString,
}

/// A question from the running operation, waiting for the user (`MessageDialog`).
pub struct PromptRequest {
    pub question: SharedString,
    /// The answers, first is the yes; `danger` paints it red and makes the last the default.
    pub answers: Vec<SharedString>,
    pub danger: bool,
    reply: mpsc::Sender<Option<usize>>,
}

impl PromptRequest {
    /// A question the UI asks itself (not an operation's): the answer arrives on the receiver.
    pub fn new(
        question: impl Into<SharedString>,
        answers: Vec<SharedString>,
        danger: bool,
    ) -> (Self, mpsc::Receiver<Option<usize>>) {
        let (reply, wait) = mpsc::channel();
        (Self { question: question.into(), answers, danger, reply }, wait)
    }

    /// The user's answer - its index, or None when the dialog was closed.
    pub fn answer(self, choice: Option<usize>) {
        let _ = self.reply.send(choice);
    }
}

/// What the operation's thread tells the UI.
pub enum Message {
    Line(LogLine),
    Fact(SharedString, SharedString),
    Prompt(PromptRequest),
    /// The undo started: Cancel becomes "Stop undoing".
    Undoing,
    /// What would be undone now changed (kept on disk while the operation runs).
    Pending(Vec<String>),
    Done(Outcome),
}

struct ChannelReporter {
    tx: UnboundedSender<Message>,
}

impl ChannelReporter {
    fn line(&mut self, kind: LineKind, text: &str) {
        match kind {
            LineKind::Fail => log::error!("{text}"),
            LineKind::Warn => log::warn!("{text}"),
            _ => log::info!("{text}"),
        }
        if kind == LineKind::Stage && text == "Undo" {
            let _ = self.tx.unbounded_send(Message::Undoing);
        }
        let line = LogLine { at: SystemTime::now(), kind, text: text.to_string().into() };
        let _ = self.tx.unbounded_send(Message::Line(line));
    }
}

impl Reporter for ChannelReporter {
    fn step(&mut self, title: &str) {
        self.line(LineKind::Stage, title);
    }
    fn info(&mut self, text: &str) {
        self.line(LineKind::Info, text);
    }
    fn success(&mut self, text: &str) {
        self.line(LineKind::Success, text);
    }
    fn warn(&mut self, text: &str) {
        self.line(LineKind::Warn, text);
    }
    fn fail(&mut self, text: &str) {
        self.line(LineKind::Fail, text);
    }
    fn fact(&mut self, name: &str, value: &str) {
        let _ = self.tx.unbounded_send(Message::Fact(name.to_string().into(), value.to_string().into()));
    }
}

pub struct ChannelPrompt {
    tx: UnboundedSender<Message>,
}

impl ChannelPrompt {
    /// A prompt whose questions arrive as [`Message::Prompt`] on the receiver - for work outside an operation.
    pub fn detached() -> (Self, UnboundedReceiver<Message>) {
        let (tx, rx) = unbounded();
        (Self { tx }, rx)
    }
}

impl ChannelPrompt {
    fn ask(&self, question: &str, answers: Vec<SharedString>, danger: bool) -> Option<usize> {
        let (reply, wait) = mpsc::channel();
        let request = PromptRequest { question: question.to_string().into(), answers, danger, reply };
        if self.tx.unbounded_send(Message::Prompt(request)).is_err() {
            return None;
        }
        // The window closed while asking: the safe answer.
        wait.recv().unwrap_or(None)
    }
}

impl Prompt for ChannelPrompt {
    fn confirm(&self, question: &str, yes: &str, no: &str, danger: bool) -> bool {
        self.ask(question, vec![yes.to_string().into(), no.to_string().into()], danger) == Some(0)
    }

    fn choose(&self, question: &str, answers: &[&str]) -> Option<usize> {
        self.ask(question, answers.iter().map(|a| a.to_string().into()).collect(), false)
    }
}

/// Cancel and "Stop undoing" of the running operation.
#[derive(Clone)]
pub struct Controls {
    pub cancel: CancelToken,
    pub skip: CancelToken,
}

/// Starts `work` on a thread of its own; the receiver yields its messages, `Done` last.
pub fn start(
    work: impl FnOnce(&mut Operation) -> Result<(), String> + Send + 'static,
) -> (Controls, UnboundedReceiver<Message>) {
    let (tx, rx) = unbounded();
    let controls = Controls { cancel: CancelToken::new(), skip: CancelToken::new() };
    let (cancel, skip) = (controls.cancel.clone(), controls.skip.clone());
    std::thread::Builder::new()
        .name("operation".into())
        .spawn(move || {
            let mut reporter = ChannelReporter { tx: tx.clone() };
            let prompt = ChannelPrompt { tx: tx.clone() };
            let pending = tx.clone();
            let watch: Box<dyn Fn(Vec<String>) + Send> = Box::new(move |left| {
                let _ = pending.unbounded_send(Message::Pending(left));
            });
            // A panic in a use case is a failure of the operation, not of the app.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation::run_watched(&mut reporter, &prompt, cancel, &skip, Some(watch), work)
            }))
            .unwrap_or_else(|panic| {
                let text = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unexpected error".into());
                log::error!("operation panicked: {text}");
                Outcome::Failed(format!("Something went wrong: {text}"))
            });
            let _ = tx.unbounded_send(Message::Done(outcome));
        })
        .expect("a thread for the operation");
    (controls, rx)
}
