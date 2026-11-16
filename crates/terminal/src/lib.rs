//! The bottom panel's Terminal (the C# `Infrastructure/Terminal`, `Presentation/Terminal`,
//! `Presentation/Services/TerminalService.cs` and the terminal parts of `Controls/TerminalPanel.xaml(.cs)`).
//!
//! GPUI-independent: [`TerminalSession`] (a shell in a Windows pseudo console, unelevated unless asked), the VT
//! parser and screen [`TerminalBuffer`], and the shells [`find_shells`] offers. On top: [`TerminalView`] draws a
//! session and takes its keys; [`TerminalPanelView`] is the shown terminal with the list of terminals beside it.
//! The shells run in ConPTY, a Windows API: on macOS and Linux the views compile, no shell is found and none starts
//! (`session_other.rs`), so the panel has no Terminal tab there.

pub mod buffer;
#[cfg(windows)]
pub mod environment;
pub mod panel;
#[cfg(windows)]
mod pty;
#[cfg(windows)]
pub mod session;
#[cfg(not(windows))]
#[path = "session_other.rs"]
pub mod session;
pub mod shells;
#[cfg(windows)]
mod token;
#[cfg(windows)]
mod trust;
pub mod view;

pub use buffer::TerminalBuffer;
pub use panel::{PanelColors, TerminalPanelEvent, TerminalPanelView};
pub use session::{SessionEvent, TerminalSession};
pub use shells::{Shells, TerminalShell, find_shells};
pub use view::{TerminalStyle, TerminalView, TerminalViewEvent};
