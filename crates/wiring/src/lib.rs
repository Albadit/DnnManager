//! The use cases joined to what they act on. A use case lives in `dnnm-application` and sees the system through the
//! traits it defines; this crate implements those traits with the IIS, SQL, DNN and Windows crates, and gives the app
//! one function per use case to run as an operation: `fn(&Services, request, &mut Operation) -> Result<(), String>`.
//! See .docs/gpui-port/porting-guide.md.

pub mod clear_site_cache;
pub mod remove_project;
pub mod change_database_connection;
pub mod change_host_password;
pub mod database_inspector;
pub mod setup_project;
pub mod edit_app_pool;
pub mod edit_bindings;
pub mod rename_project;
pub mod backups;
pub mod export_for_deployment;
pub mod export_project;
pub mod restore_backup;
pub mod upgrade_dnn;
pub mod clone_project;
pub mod host_project;
pub mod hosting;
pub mod sql_container;
pub mod open_with;
pub mod source_control;
use std::sync::Arc;

use dnnm_application::iis::IisManager;
use dnnm_application::settings::Settings;
use dnnm_infrastructure::app_db::AppDatabase;

/// What every use case may use: IIS, DNN Manager's database and the settings as they are when it starts.
#[derive(Clone)]
pub struct Services {
    pub iis: Arc<dyn IisManager>,
    pub db: AppDatabase,
    pub settings: Settings,
}
