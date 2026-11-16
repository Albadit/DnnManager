//! A shell running in a pseudo console, with the screen its output is drawn into - ports
//! `Presentation/Terminal/TerminalSession.cs`.
//!
//! The output is read on a thread of its own and sent as [`SessionEvent`]s; the owner (the view, on the UI thread)
//! applies them in batches with [`TerminalSession::apply_output`], so a program that prints a lot doesn't queue one
//! UI update per chunk. Writes go one after the other on a thread of their own (a program that isn't reading would
//! block the pipe).

use std::io::{Read, Write};
use std::sync::mpsc;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::buffer::TerminalBuffer;
use crate::pty::{self, PseudoConsole};

/// What the session's threads report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A chunk of what the program printed (UTF-8, possibly ending inside a character).
    Output(Vec<u8>),
    /// The shell ended by itself (e.g. `exit`).
    Exited,
}

pub struct TerminalSession {
    console: PseudoConsole,
    buffer: TerminalBuffer,
    decoder: Utf8Decoder,
    writer: mpsc::Sender<Vec<u8>>,
    events: Option<UnboundedReceiver<SessionEvent>>,
    is_administrator: bool,
    closed: bool,
}

impl TerminalSession {
    /// Starts `command_line` in `working_directory` - with DNN Manager's administrator rights when
    /// `as_administrator`, otherwise as the signed-in user, without them; when that can't be, the error says so and
    /// nothing starts.
    pub fn start(
        command_line: &str,
        working_directory: &str,
        columns: usize,
        rows: usize,
        as_administrator: bool,
    ) -> Result<TerminalSession, String> {
        let buffer = TerminalBuffer::new(columns, rows);
        let mut console = PseudoConsole::start(
            command_line,
            working_directory,
            buffer.columns() as u16,
            buffer.rows() as u16,
            as_administrator,
        )?;
        let (events_tx, events) = unbounded();
        let (writer, writes) = mpsc::channel::<Vec<u8>>();

        if let Some(output) = console.take_output() {
            let events_tx = events_tx.clone();
            std::thread::Builder::new()
                .name("terminal output".into())
                .spawn(move || read_output(output, events_tx))
                .map_err(|e| e.to_string())?;
        }
        if let Some(mut input) = console.take_input() {
            std::thread::Builder::new()
                .name("terminal input".into())
                .spawn(move || {
                    for bytes in writes {
                        // The shell is gone: nothing more to write to.
                        if input.write_all(&bytes).and_then(|_| input.flush()).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        let process = console.process_handle()?;
        std::thread::Builder::new()
            .name("terminal exit".into())
            .spawn(move || {
                pty::wait_for_exit(&process);
                let _ = events_tx.unbounded_send(SessionEvent::Exited);
            })
            .map_err(|e| e.to_string())?;

        Ok(TerminalSession {
            console,
            buffer,
            decoder: Utf8Decoder::default(),
            writer,
            events: Some(events),
            is_administrator: as_administrator,
            closed: false,
        })
    }

    pub fn buffer(&self) -> &TerminalBuffer {
        &self.buffer
    }

    /// The shell runs with administrator rights - what is typed in it does too.
    pub fn is_administrator(&self) -> bool {
        self.is_administrator
    }

    /// The shell's process ID.
    pub fn process_id(&self) -> u32 {
        self.console.process_id()
    }

    /// The session's events - taken once, by whoever applies them.
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<SessionEvent>> {
        self.events.take()
    }

    /// Applies what the shell printed to the screen, and answers what it asked (the cursor position…).
    pub fn apply_output(&mut self, bytes: &[u8]) {
        if self.closed {
            return;
        }
        let text = self.decoder.decode(bytes);
        self.buffer.feed(&text);
        for reply in self.buffer.take_replies() {
            self.write(&reply);
        }
    }

    /// Sends `text` to the shell as if it was typed.
    pub fn write(&self, text: &str) {
        if !self.closed && !text.is_empty() {
            let _ = self.writer.send(text.as_bytes().to_vec());
        }
    }

    /// A new size, in characters; false when it is the size it has.
    pub fn resize(&mut self, columns: usize, rows: usize) -> bool {
        if self.closed || (columns == self.buffer.columns() && rows == self.buffer.rows()) {
            return false;
        }
        self.buffer.resize(columns, rows);
        self.console.resize(self.buffer.columns() as u16, self.buffer.rows() as u16);
        true
    }

    /// Ends the shell and everything running in it.
    pub fn close(&mut self) {
        if !std::mem::replace(&mut self.closed, true) {
            self.console.close();
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn read_output(mut output: std::fs::File, events: UnboundedSender<SessionEvent>) {
    let mut bytes = vec![0u8; 8192];
    loop {
        match output.read(&mut bytes) {
            // The console was closed.
            Ok(0) | Err(_) => break,
            Ok(read) => {
                // Nobody listens any more: the output is still read, so closing the console isn't held up.
                let _ = events.unbounded_send(SessionEvent::Output(bytes[..read].to_vec()));
            }
        }
    }
}

/// UTF-8 to text, keeping the bytes of a character that is split over two reads.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut rest: &[u8] = &self.pending;
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    text.push_str(valid);
                    rest = &[];
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    // SAFETY: from_utf8 said these bytes are valid.
                    text.push_str(unsafe { std::str::from_utf8_unchecked(valid) });
                    match error.error_len() {
                        // Cut off at the end: kept for the next read.
                        None => {
                            rest = after;
                            break;
                        }
                        Some(bad) => {
                            text.push('\u{FFFD}');
                            rest = &after[bad..];
                        }
                    }
                }
            }
        }
        self.pending = rest.to_vec();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn decoder_keeps_a_split_character() {
        let mut decoder = Utf8Decoder::default();
        let euro = "€".as_bytes();
        assert_eq!(decoder.decode(&[b'a', euro[0]]), "a");
        assert_eq!(decoder.decode(&euro[1..]), "€");
        assert_eq!(decoder.decode(&[0xFF, b'b']), "\u{FFFD}b");
    }

    /// Starts `cmd.exe /c echo hi` in a pseudo console, as the unelevated terminal does it (a restricted token also
    /// when the tests aren't elevated): the output arrives, then the exit. Changes nothing on the system.
    #[test]
    fn a_shell_runs_in_a_pseudo_console_and_its_output_arrives() {
        pty::drop_rights_always(true);
        let system = crate::environment::system_folders().system;
        let cmd = crate::environment::join(&system, "cmd.exe");
        let mut session =
            TerminalSession::start(&format!("\"{cmd}\" /d /c echo hi"), &system, 80, 25, false).expect("started");
        assert!(session.process_id() > 0);
        let mut events = session.take_events().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut exited = false;
        let screen = |s: &TerminalSession| {
            (0..s.buffer().total_lines()).map(|i| crate::buffer::line_text(s.buffer().line(i))).collect::<Vec<_>>()
        };
        while Instant::now() < deadline && !(exited && screen(&session).iter().any(|l| l.trim_end() == "hi")) {
            match events.try_recv() {
                Ok(SessionEvent::Output(bytes)) => session.apply_output(&bytes),
                Ok(SessionEvent::Exited) => exited = true,
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        assert!(exited, "the shell didn't exit");
        assert!(screen(&session).iter().any(|l| l.trim_end() == "hi"), "{:?}", screen(&session));
        assert!(session.resize(100, 30));
        assert!(!session.resize(100, 30));
        session.close();
    }
}
