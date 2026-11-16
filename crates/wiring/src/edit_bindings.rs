//! Edit host names joined to IIS and SQL Server (`EditBindingsUseCase`, `ProjectEdits.EditBindings`), with the texts
//! the app offers it with (`ProjectView.xaml`, `MainWindow.Commands.cs`).

use std::sync::Arc;

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::edit_bindings::{
    self as use_case, EditBindingsDeps, EditBindingsRequest, HttpBindingEdit,
};

use crate::Services;
use crate::rename_project::SqlSiteDatabases;

/// The command palette's entry (`MainWindow.Commands.cs`).
pub const COMMAND_ID: &str = "project.editBindings";
pub const COMMAND: &str = "Edit host names…";
pub const COMMAND_KEYWORDS: &str = "bindings port hostname domain alias iis";
/// The Details' IIS tab (`ProjectView.xaml`).
pub const BUTTON: &str = "Edit host names…";
pub const BUTTON_TOOLTIP: &str = "The host names and ports the site answers on - DNN's portal aliases follow";

/// The operation's title.
pub fn title(name: &str) -> String {
    format!("Edit the host names of '{name}'")
}

/// The toast once it is done: "'shop' answers on shop.dnndev.me, *:8080."
pub fn done_toast(name: &str, bindings: &[HttpBindingEdit]) -> String {
    let list: Vec<String> = bindings.iter().map(HttpBindingEdit::shown).collect();
    format!("'{name}' answers on {}.", list.join(", "))
}

/// Runs Edit host names. It may ask (another project's database, a sign-in elsewhere) through `op.prompt`.
pub fn edit_bindings(services: &Services, request: EditBindingsRequest, op: &mut Operation) -> Result<(), String> {
    let deps = EditBindingsDeps { iis: services.iis.clone(), databases: Arc::new(SqlSiteDatabases::new(services)) };
    use_case::edit_bindings(&deps, &request, op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        let bindings = [
            HttpBindingEdit { old_host: None, old_port: None, host: "a.me".into(), port: 80 },
            HttpBindingEdit { old_host: None, old_port: None, host: "b.me".into(), port: 8080 },
            HttpBindingEdit { old_host: None, old_port: None, host: String::new(), port: 81 },
        ];
        assert_eq!(done_toast("shop", &bindings), "'shop' answers on a.me, b.me:8080, *:81.");
        assert_eq!(title("shop"), "Edit the host names of 'shop'");
    }
}
