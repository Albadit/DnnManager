//! DNN Manager's system services - the C# `DnnManager.Infrastructure` folders `Hosts`, `Settings` (secrets), `Files`,
//! `Startup`, `Prereq`, `Updates` and `Processes` (trusted programs, efficiency mode), and the Presentation's
//! `SingleInstance` / `RunningMarker`.
//!
//! Most of it is Windows only, as IIS is. On macOS and Linux the hosts file, the prerequisites and the update check
//! are the same code; the rest is [`other`]: the same names, saying the feature is Windows-only.

#[cfg(windows)]
pub mod acl;
#[cfg(windows)]
pub mod files;
pub mod hosts;
#[cfg(windows)]
pub mod power;
pub mod prereq;
#[cfg(windows)]
pub mod secrets;
#[cfg(windows)]
pub mod single_instance;
#[cfg(windows)]
pub mod startup;
#[cfg(windows)]
pub mod trusted;
#[cfg(windows)]
pub mod update_helper;
pub mod updates;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub use other::{files, secrets, single_instance, startup, trusted, update_helper};
#[cfg(not(windows))]
#[path = "files/backup_filter.rs"]
#[doc(hidden)]
pub mod files_backup_filter;

#[cfg(test)]
mod test_support;
