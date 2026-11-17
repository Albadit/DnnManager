//! What a site's folder tells about it: the DNN version and the database (`ServerStateMonitor.ReadFolder`), and its
//! size. The problems are worded as the WPF app words them.

use std::path::Path;

use dnnm_application::project::ProjectState;

use crate::web_config::{DatabaseKind, is_inside, is_local_db, read_database_connection};
use crate::{dnn_install, web_config};

/// Fills in what the folder says - DNN version, database, whether it is a DNN Manager project.
pub fn read_folder(project: &mut ProjectState, projects_folder: &Path) {
    let directory = project.directory.clone();
    project.in_projects_folder = !projects_folder.as_os_str().is_empty() && is_inside(&directory, projects_folder);
    project.dnn_version = None;
    project.database_name = None;
    project.database_server = None;
    project.database_is_file = false;
    let mut problem = None;
    if directory.as_os_str().is_empty() {
        problem = Some("IIS gives the site no physical path".to_string());
    } else if !directory.is_dir() {
        problem = Some(format!("the site's folder isn't there: {}", directory.display()));
    } else {
        project.dnn_version = dnn_install::dnn_version(&directory);
        let config = directory.join("web.config");
        if !config.is_file() {
            problem = Some("the site's folder has no web.config".into());
        } else {
            match read_database_connection(&config) {
                Err(e) => problem = Some(format!("web.config: {e}")),
                Ok(c) if c.kind == DatabaseKind::LocalDbFile && !is_local_db(&c.server) => {
                    problem = Some(
                        "web.config still has DNN's own connection - DNN isn't installed into a database yet".into(),
                    )
                }
                Ok(c) if c.database.is_empty() => problem = Some("web.config's SiteSqlServer names no database".into()),
                Ok(c) => {
                    project.database_is_file = c.kind == DatabaseKind::LocalDbFile;
                    project.database_name = Some(c.database);
                    project.database_server = Some(c.server);
                }
            }
        }
    }
    // Only a DNN site (or a project's folder) is expected to have a database - any other site simply has none.
    if project.dnn_version.is_none() && !project.in_projects_folder {
        problem = None;
    }
    project.database_problem = problem;
}

/// The size of everything in a folder, not following links (a junction could point anywhere, even at itself).
pub fn folder_size(directory: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_type().is_symlink() || web_config::has_link(&dir, &entry.path()) {
                continue;
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    directory.is_dir().then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_application::iis::IisSiteRuntime;

    fn project(directory: &Path) -> ProjectState {
        ProjectState::from_site(
            "x".into(),
            IisSiteRuntime {
                id: 1,
                state: "Started".into(),
                app_pool: "x".into(),
                app_pool_state: None,
                worker_process_ids: vec![],
                bindings: vec![],
                physical_path: directory.to_path_buf(),
            },
        )
    }

    #[test]
    fn a_project_folder_without_web_config_says_so() {
        let base = tempfile::tempdir().unwrap();
        let site = base.path().join("x");
        std::fs::create_dir(&site).unwrap();
        let mut p = project(&site);
        read_folder(&mut p, base.path());
        assert!(p.in_projects_folder);
        assert_eq!(p.database_problem.as_deref(), Some("the site's folder has no web.config"));
    }

    #[test]
    fn a_plain_site_outside_the_projects_folder_has_no_problem() {
        let base = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut p = project(other.path());
        read_folder(&mut p, base.path());
        assert!(!p.in_projects_folder);
        assert_eq!(p.database_problem, None);
    }

    #[test]
    fn reads_the_database_from_web_config() {
        let base = tempfile::tempdir().unwrap();
        let site = base.path().join("x");
        std::fs::create_dir(&site).unwrap();
        std::fs::write(
            site.join("web.config"),
            r#"<configuration><connectionStrings><add name="SiteSqlServer" connectionString="Server=localhost,1433;Database=x;uid=sa;pwd=p" /></connectionStrings></configuration>"#,
        )
        .unwrap();
        let mut p = project(&site);
        read_folder(&mut p, base.path());
        assert_eq!(p.database_name.as_deref(), Some("x"));
        assert_eq!(p.database_server.as_deref(), Some("localhost,1433"));
        assert_eq!(p.database_problem, None);
    }

    #[test]
    fn sums_file_sizes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("f"), [0u8; 100]).unwrap();
        std::fs::write(dir.path().join("g"), [0u8; 23]).unwrap();
        assert_eq!(folder_size(dir.path()), Some(123));
        assert_eq!(folder_size(&dir.path().join("missing")), None);
    }
}
