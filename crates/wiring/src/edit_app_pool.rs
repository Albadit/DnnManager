//! Edit app pool joined to IIS (`EditAppPoolUseCase`, `ProjectEdits.EditAppPool`), with the texts the app offers it
//! with (`ProjectView.xaml`, `MainWindow.Commands.cs`).

use dnnm_application::iis::IisPoolSettings;
use dnnm_application::operation::Operation;
use dnnm_application::use_cases::edit_app_pool::{self as use_case, EditAppPoolRequest};

use crate::Services;

/// The command palette's entry (`MainWindow.Commands.cs`).
pub const COMMAND_ID: &str = "project.editAppPool";
pub const COMMAND: &str = "Edit app pool…";
pub const COMMAND_KEYWORDS: &str = "application pool clr pipeline identity 32-bit idle iis";
/// The Details' IIS tab (`ProjectView.xaml`).
pub const BUTTON: &str = "Edit app pool…";
pub const BUTTON_TOOLTIP: &str = ".NET CLR version, pipeline, 32-bit, identity, idle time-out, start mode";

/// The operation's title.
pub fn title(name: &str) -> String {
    format!("Edit the app pool of '{name}'")
}

/// The toast once it is done.
pub fn done_toast(name: &str) -> String {
    format!("The app pool of '{name}' was changed and recycled.")
}

/// What the dialog starts from: the site's app pool's name and its settings. The error is what the app shows in an
/// error dialog instead (`Dialogs.Error`). Reads IIS: call it off the UI thread.
pub fn current_pool(services: &Services, site: &str) -> Result<(String, IisPoolSettings), String> {
    services
        .iis
        .site_details(site)
        .and_then(|d| d.pool)
        .map(|pool| (pool.name.clone(), IisPoolSettings::from_pool(&pool)))
        .ok_or_else(|| format!("'{site}' has no app pool to edit."))
}

/// Runs Edit app pool.
pub fn edit_app_pool(services: &Services, request: EditAppPoolRequest, op: &mut Operation) -> Result<(), String> {
    use_case::edit_app_pool(&services.iis, &request, op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(title("shop"), "Edit the app pool of 'shop'");
        assert_eq!(done_toast("shop"), "The app pool of 'shop' was changed and recycled.");
    }
}
