//! DNN releases on GitHub, DNN packages, DNN's unattended install and upgrade, and the upgrade path - the C#
//! `DnnManager.Infrastructure/Github`, `DnnManager.Infrastructure/Dnn` and `DnnManager.Application/Upgrades`.

pub mod connectivity;
pub mod format;
pub mod install;
pub mod install_output;
pub mod install_template;
pub mod installer;
pub mod log_files;
pub mod metadata;
pub mod packages;
pub mod releases;
pub mod safe_zip;
pub mod site_http;
pub mod site_inspector;
pub mod site_sql;
pub mod upgrade_checks;
pub mod upgrades;
pub mod version;
mod xml;
