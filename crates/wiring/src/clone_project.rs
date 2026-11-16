//! "Clone…" of the project menu (`Pages/Projects/ProjectMenu.cs` `Clone`, `UseCases/CloneProjectUseCase.cs`): the
//! texts the app shows, the name the InputDialog suggests and checks, and the entry point.
//!
//! The app: the menu item [`CLONE_MENU_ITEM`] is enabled when the row's folder exists and no operation runs; it opens
//! `dnnm_dialogs::input::InputDialog` with [`clone_question`], [`suggested_clone_name`], [`CLONE_OK`] and
//! [`clone_name_problem`] as its check; on OK it runs [`clone_project`] with [`clone_request`] as an operation titled
//! [`clone_title`], and on success shows [`clone_done`] as a success toast.

use std::path::Path;

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::clone_project::{self as use_case, CloneRequest};
use dnnm_domain::validate_project_name;

use crate::Services;
use crate::hosting::hosting_deps;

/// The project menu's item.
pub const CLONE_MENU_ITEM: &str = "Clone…";
/// The InputDialog's OK button.
pub const CLONE_OK: &str = "Clone";

/// The InputDialog's question.
pub fn clone_question(source: &str) -> String {
    format!("Clone '{source}' - its files, its database and an IIS website of its own - as a new project named:")
}

/// The operation's title.
pub fn clone_title(source: &str, target: &str) -> String {
    format!("Clone '{source}' → '{target}'")
}

/// The success toast.
pub fn clone_done(source: &str, target: &str) -> String {
    format!("'{target}' is ready - a copy of '{source}' with its own database and IIS site.")
}

/// What is wrong with `name` for the clone - None when it is fine: a project name, no project folder of that name in
/// `projects_folder`, no site of that name among `site_names` (the sites the window shows).
pub fn clone_name_problem(name: &str, projects_folder: &Path, site_names: &[String]) -> Option<String> {
    if let Err(e) = validate_project_name(name) {
        return Some(e);
    }
    if projects_folder.join(name).is_dir() {
        return Some(format!("A project named '{name}' already exists."));
    }
    // The clone's IIS site would replace one of that name.
    if site_names.iter().any(|s| s.eq_ignore_ascii_case(name)) {
        return Some(format!("IIS already has a site named '{name}'."));
    }
    None
}

/// "shop_copy", or "shop_copy2"… when that is taken.
pub fn suggested_clone_name(source: &str, problem: impl Fn(&str) -> Option<String>) -> String {
    let mut suggested = format!("{source}_copy");
    let mut n = 2;
    while problem(&suggested).is_some() && n < 100 {
        suggested = format!("{source}_copy{n}");
        n += 1;
    }
    suggested
}

/// The request for cloning the site in `source_directory` as `target`: files, database and IIS site, the source's
/// backup written to `%TEMP%\dnnmanager_clone_<target>_<yyyyMMddHHmmss>.bak` on its server (a server on this PC uses
/// its own backup folder instead).
pub fn clone_request(source_directory: &Path, target: &str) -> CloneRequest {
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let bak = std::env::temp_dir().join(format!("dnnmanager_clone_{target}_{stamp}.bak"));
    CloneRequest::new(target, source_directory, &bak.display().to_string())
}

/// Runs Clone (`CloneProjectUseCase`).
pub fn clone_project(services: &Services, request: CloneRequest, op: &mut Operation) -> Result<(), String> {
    let deps = hosting_deps(services)?;
    use_case::clone_project(&deps, &request, op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_checked_as_the_menu_checks_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("shop_copy")).unwrap();
        let sites = vec!["Shop_Copy2".to_string()];
        let problem = |n: &str| clone_name_problem(n, dir.path(), &sites);
        assert_eq!(problem("shop_copy").as_deref(), Some("A project named 'shop_copy' already exists."));
        assert_eq!(problem("shop_copy2").as_deref(), Some("IIS already has a site named 'shop_copy2'."));
        assert_eq!(problem("-x").as_deref(), Some("Project name must start with a letter or a digit."));
        assert_eq!(problem("shop_copy3"), None);
        assert_eq!(suggested_clone_name("shop", problem), "shop_copy3");
    }

    #[test]
    fn texts_and_request() {
        assert_eq!(
            clone_question("shop"),
            "Clone 'shop' - its files, its database and an IIS website of its own - as a new project named:"
        );
        assert_eq!(clone_title("shop", "shop_copy"), "Clone 'shop' → 'shop_copy'");
        assert_eq!(
            clone_done("shop", "shop_copy"),
            "'shop_copy' is ready - a copy of 'shop' with its own database and IIS site."
        );
        let req = clone_request(Path::new(r"C:\DNN\shop"), "shop_copy");
        assert!(req.create_iis_site && req.copy_files && req.seed_database);
        let bak = Path::new(&req.source_backup_server_path).file_name().unwrap().to_string_lossy().into_owned();
        assert!(bak.starts_with("dnnmanager_clone_shop_copy_") && bak.ends_with(".bak") && bak.len() == 45, "{bak}");
    }
}
