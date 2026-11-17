//! A site's logs for the Logs tab, and DNN Manager's own daily log file.
//!
//! - Logs tab: [`catalog`] lists a site's (or DNN Manager's) logs, [`view`] groups them for the picker and colours a
//!   line, a source's [`tail`] reads its newest lines and follows it, [`search`] is the Find bar.
//! - DNN Manager's log: [`daily_file`] writes `Documents\DnnManager\logs\dnnmanager-yyyyMMdd.log`, [`logger`] sends the
//!   `log` crate's warnings and errors there, [`run_log`] an operation's lines and how it ended.

pub mod catalog;
pub mod clock;
pub mod daily_file;
pub mod event_log;
pub mod logger;
pub mod run_log;
pub mod search;
pub mod tail;
pub mod view;
