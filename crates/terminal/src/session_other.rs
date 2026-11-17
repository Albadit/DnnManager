//! A shell session off Windows: the terminal runs shells in a Windows pseudo console (ConPTY), so here none starts -
//! and none is offered ([`crate::find_shells`] finds none), which leaves the panel without its Terminal tab.

use futures::channel::mpsc::UnboundedReceiver;

use crate::buffer::TerminalBuffer;

/// What the session's threads report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A chunk of what the program printed (UTF-8, possibly ending inside a character).
    Output(Vec<u8>),
    /// The shell ended by itself (e.g. `exit`).
    Exited,
}

/// A session that never starts.
pub struct TerminalSession {
    buffer: TerminalBuffer,
}

impl TerminalSession {
    pub fn start(
        _command_line: &str,
        _working_directory: &str,
        _columns: usize,
        _rows: usize,
        _as_administrator: bool,
    ) -> Result<TerminalSession, String> {
        Err("The terminal is only available on Windows in this build.".into())
    }

    pub fn buffer(&self) -> &TerminalBuffer {
        &self.buffer
    }

    pub fn is_administrator(&self) -> bool {
        false
    }

    pub fn process_id(&self) -> u32 {
        0
    }

    pub fn take_events(&mut self) -> Option<UnboundedReceiver<SessionEvent>> {
        None
    }

    pub fn apply_output(&mut self, _bytes: &[u8]) {}

    pub fn write(&self, _text: &str) {}

    pub fn resize(&mut self, _columns: usize, _rows: usize) -> bool {
        false
    }

    pub fn close(&mut self) {}
}
