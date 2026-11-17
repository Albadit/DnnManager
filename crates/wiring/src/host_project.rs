//! New project's import (`Pages/SetupPage.xaml.cs` `Run_Click`, `UseCases/ImportProjectUseCase.cs`) and hosting an
//! existing folder (`UseCases/HostExistingProjectUseCase.cs`, which the WPF app runs only as Import's last part).
//!
//! The app: New project's "Import" runs [`import_project`] as an operation titled [`import_title`]; on success it
//! shows [`import_done`] as a success toast with the action [`IMPORT_OPEN_SITE`] opening [`import_site_url`].

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::host_project::{self as use_case, HostExistingRequest, ImportRequest};

use crate::Services;
use crate::hosting::hosting_deps;

/// The success toast's action.
pub const IMPORT_OPEN_SITE: &str = "Open site";

/// The operation's title.
pub fn import_title(project: &str) -> String {
    format!("Import '{project}'")
}

/// The success toast.
pub fn import_done(project: &str) -> String {
    format!("'{project}' is imported - it is on the Projects page.")
}

/// Where [`IMPORT_OPEN_SITE`] goes: the host name and port the page chose (the port 80 when none).
pub fn import_site_url(request: &ImportRequest) -> String {
    let host = request.host_name.as_deref().unwrap_or_default();
    let alias = dnnm_application::use_cases::provisioner::alias_for(host, request.port.unwrap_or(80));
    format!("http://{alias}")
}

/// Runs Import (`ImportProjectUseCase`): the zip extracted into a new project folder, its IIS site, its database
/// restored from the backup.
pub fn import_project(services: &Services, request: ImportRequest, op: &mut Operation) -> Result<(), String> {
    let deps = hosting_deps(services)?;
    use_case::import_project(&deps, &request, op)
}

/// Runs Host existing (`HostExistingProjectUseCase`): the IIS website, the local database, or both for a folder that
/// already holds a DNN site.
pub fn host_existing_project(
    services: &Services,
    request: HostExistingRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let deps = hosting_deps(services)?;
    use_case::host_existing_project(&deps, &request, op)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn texts_and_the_sites_address() {
        assert_eq!(import_title("shop"), "Import 'shop'");
        assert_eq!(import_done("shop"), "'shop' is imported - it is on the Projects page.");
        let req = ImportRequest {
            project_name: "shop".into(),
            zip_path: PathBuf::from("site.zip"),
            backup_file: PathBuf::from("shop.bacpac"),
            host_name: Some("shop.dnndev.me".into()),
            port: Some(8080),
        };
        assert_eq!(import_site_url(&req), "http://shop.dnndev.me:8080");
        assert_eq!(import_site_url(&ImportRequest { port: None, ..req }), "http://shop.dnndev.me");
    }
}
