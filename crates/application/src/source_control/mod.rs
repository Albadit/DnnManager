//! Source Control: a project's files against an environment's server over SFTP - what differs, the diff of a file,
//! and sending files either way without ever overwriting or deleting the server's by accident. The page is
//! `dnnm_dialogs::source_control_page`; the server `dnnm-remote`; the project's files and the records
//! `dnnm-infrastructure::source_control`.

pub mod classify;
pub mod diff;
pub mod highlight;
pub mod model;
pub mod paths;
pub mod plan;
pub mod sync;
