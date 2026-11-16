//! Files DNN Manager handles with administrator rights - the C# `DnnManager.Infrastructure/Files` folder and the
//! Application's `SafePath` and `BackupFilter`.
//!
//! Not ported here: `DailyLogFile` / `DailyLogFileLogger` (DNN Manager's own log - the logs crate's) and
//! `ProjectScaffolder` (the DNN site's `.gitignore` - DNN-specific).

pub mod backup_filter;
pub mod copier;
pub mod file_locks;
pub mod folder_guard;
pub mod private_temp;
pub mod redirection;
pub mod safe_path;
pub mod safe_zip;
