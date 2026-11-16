//! SQL Server for DNN Manager - the C#'s `DnnManager.Infrastructure/Sql`, `Docker` and `Dnn/MembershipPasswords`,
//! with the records of `DnnManager.Application/Abstractions` they work with.
//!
//! - [`provisioner::DatabaseProvisioner`] - site databases on any SQL Server (`IDatabaseProvisioner`)
//! - [`tester`] - signing in as a site does, and the Projects table's look at each site's database
//! - [`container::SqlServerService`] - the shared container, through sqlcmd inside it (`ISqlServerService`)
//! - [`docker`] - docker's status, the compose file and `docker compose up` (`IDockerComposeService`)
//! - [`remote_backup`] - BACKUP DATABASE on another server (`IRemoteSqlBackupService`)
//! - [`sqlpackage`] - .bacpac export and import (`IBacpacService`)
//! - [`localdb`] - LocalDB versions, instances and a site's file database
//! - [`membership`] - ASP.NET membership password hashes
//!
//! Every call is synchronous: it blocks the (worker) thread it is called on. SQL Server is reached with tiberius on a
//! small tokio runtime of this crate's own ([`client`]).

pub mod address;
pub mod client;
pub mod connection_strings;
pub mod container;
pub mod dirs;
pub mod docker;
pub mod localdb;
pub mod membership;
pub mod model;
pub mod programs;
pub mod provisioner;
pub mod remote_backup;
pub mod sql_text;
pub mod sqlpackage;
pub mod tester;

pub use address::SqlServerAddress;
pub use model::*;
