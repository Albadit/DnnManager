//! Export ▸ For deployment… (`ProjectEdits.ExportForDeployment`, `ExportForDeploymentUseCase.PrepareWebConfig` and
//! `WebConfigService.SetDebug` / `EnableHttpsRedirectRules`): the package for the live server, its web.config changed
//! in a copy taken out of the zip in DNN Manager's private temporary folder and put back into the zip.
//!
//! Deviation: the C# changes web.config as an XML tree (`XDocument`) and saves it whole; the port changes only the
//! attributes and comments concerned in the file's text, so the rest stays byte for byte as it was. The zip, which
//! the `zip` crate can't change in place, is written anew beside itself with every other entry copied as it is.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dnnm_application::operation::Operation;
use dnnm_application::use_cases::backups::{Stamp, is_inside};
use dnnm_application::use_cases::export_for_deployment::{
    self as use_case, DeployWebConfig, ExportForDeploymentDeps, ExportForDeploymentRequest, PreparedWebConfig,
};
use dnnm_infrastructure::web_config::load_site_xml;

use crate::Services;
use crate::backups::{CopierFiles, SqlDatabases, SqlPackage, SqlSettings, deployments_root};

/// Where the dialog suggests the package goes: `Documents\DnnManager\deployments\<project>_<yyyyMMdd_HHmmss>`.
pub fn default_folder(site: &str, now: Stamp) -> PathBuf {
    deployments_root().join(format!("{site}_{}", now.folder_suffix()))
}

/// Whether the dialog offers the database: the site's web.config names one that isn't a LocalDB file. Reads the
/// site's web.config.
pub fn has_database(services: &Services, directory: &Path) -> bool {
    crate::change_database_connection::database_of(&services.settings, directory)
        .is_some_and(|d| d.kind != dnnm_application::use_cases::database_connection::DatabaseKind::LocalDbFile)
}

/// Runs Export for deployment; the app titles it [`crate::backups::texts::deployment_title`] and shows
/// [`crate::backups::texts::deployment_done`] with Open folder (the request's output folder) when it is done.
pub fn export_for_deployment(
    services: &Services,
    request: ExportForDeploymentRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let temp_folder = dnnm_system::files::private_temp::path().map_err(|e| e.to_string())?;
    let bacpacs = SqlPackage(SqlSettings::new(services));
    // This PC's addresses of the site - left out of the package's portal aliases.
    let local_hosts = services
        .iis
        .site_details(&request.project_name)
        .map(|site| site.bindings.into_iter().map(|b| b.host).filter(|h| !h.is_empty()).collect())
        .unwrap_or_default();
    let deps = ExportForDeploymentDeps {
        files: &CopierFiles,
        web_config: &ZipWebConfig { temp_root: temp_folder.clone() },
        bacpacs: &bacpacs,
        databases: Arc::new(SqlDatabases),
        temp_folder,
        now: crate::backups::now(),
        hostname_suffix: services.settings.get("projects.hostnameSuffix").unwrap_or("dnndev.me").trim().trim_matches('.').into(),
        local_hosts,
    };
    use_case::export_for_deployment(&deps, &request, op)
}

/// The zip's web.config changed in a folder of its own under `temp_root`, deleted again afterwards.
pub(crate) struct ZipWebConfig {
    pub temp_root: PathBuf,
}

impl DeployWebConfig for ZipWebConfig {
    fn prepare(&self, zip: &Path, connection_string: &str, https: bool) -> Result<PreparedWebConfig, String> {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let temp = self.temp_root.join(format!("dnnmanager-deploy-{}-{unique:x}", std::process::id()));
        let result = prepare_in(zip, connection_string, https, &temp);
        let _ = std::fs::remove_dir_all(&temp);
        result
    }
}

/// The index of the zip entry `name`, whatever its capitals and separators - as Windows finds a file.
fn entry_index(names: &[String], name: &str) -> Option<usize> {
    let wanted = name.replace('\\', "/");
    names
        .iter()
        .position(|n| *n == wanted)
        .or_else(|| names.iter().position(|n| n.replace('\\', "/").eq_ignore_ascii_case(&wanted)))
}

fn prepare_in(zip_path: &Path, connection_string: &str, https: bool, temp: &Path) -> Result<PreparedWebConfig, String> {
    let error = |e: &dyn std::fmt::Display| e.to_string();
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(zip_path).map_err(|e| error(&e))?).map_err(|e| error(&e))?;
    let names: Vec<String> =
        (0..archive.len()).map(|i| archive.name_for_index(i).and_then(|n| n.ok()).unwrap_or_default().to_string()).collect();
    let Some(web_config_index) = entry_index(&names, "web.config") else {
        return Err("The site has no web.config.".into());
    };
    std::fs::create_dir_all(temp).map_err(|e| error(&e))?;
    let web_config = temp.join("web.config");
    let mut extract = |index: usize, to: &Path| -> Result<(), String> {
        let mut entry = archive.by_index(index).map_err(|e| error(&e))?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| error(&e))?;
        std::fs::write(to, bytes).map_err(|e| error(&e))
    };
    extract(web_config_index, &web_config)?;
    // The entries as found, whatever their capitals: these are replaced - by name, a Web.config would stay next to the
    // new one, with the local connection string in it.
    let mut entries = vec![(web_config_index, web_config.clone())];
    for source in config_sources(&web_config)? {
        // connectionStrings and appSettings may both come from one file: it is put back once.
        let Some(index) = entry_index(&names, &source) else { continue };
        if entries.iter().any(|(i, _)| *i == index) {
            continue;
        }
        let path = temp.join(source.replace('/', "\\"));
        // A rooted configSource (C:\...) would leave the temporary folder.
        if !is_inside(&path, temp) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| error(&e))?;
        }
        extract(index, &path)?;
        entries.push((index, path));
    }

    crate::change_database_connection::write_site_sql_server(&web_config, connection_string)
        .map_err(|e| format!("Could not set the connection string: {e}"))?;
    let debug_off = set_debug_off(&web_config).is_ok();
    let rules_enabled = if https { enable_https_redirect_rules(&web_config).unwrap_or_default() } else { Vec::new() };

    // Every other entry copied as it is, the changed ones written anew under their own names.
    let mut tmp = zip_path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = (|| -> Result<(), String> {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&tmp).map_err(|e| error(&e))?);
        for index in 0..names.len() {
            if entries.iter().any(|(i, _)| *i == index) {
                continue;
            }
            writer.raw_copy_file(archive.by_index_raw(index).map_err(|e| error(&e))?).map_err(|e| error(&e))?;
        }
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (index, file) in &entries {
            writer.start_file(names[*index].as_str(), options).map_err(|e| error(&e))?;
            writer.write_all(&std::fs::read(file).map_err(|e| error(&e))?).map_err(|e| error(&e))?;
        }
        writer.finish().map_err(|e| error(&e))?;
        Ok(())
    })();
    drop(archive);
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, zip_path).map_err(|e| error(&e))) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(PreparedWebConfig { debug_off, rules_enabled })
}

fn read_text(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| e.to_string())
}

/// The text without a byte order mark, for the XML parser.
fn xml(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

/// The files web.config's connectionStrings and appSettings are read from, as zip entry names. The site's web.config
/// (its app pool can change it): no DTD, nothing fetched from elsewhere.
fn config_sources(web_config: &Path) -> Result<Vec<String>, String> {
    let text = read_text(web_config)?;
    let doc = load_site_xml(xml(&text))?;
    let root = doc.root_element();
    Ok(["connectionStrings", "appSettings"]
        .iter()
        .filter_map(|section| root.children().find(|n| n.has_tag_name(*section)))
        .filter_map(|n| n.attribute("configSource").map(str::to_string))
        .filter(|s| !s.is_empty() && !s.contains(".."))
        .map(|s| s.replace('\\', "/").trim_start_matches('/').to_string())
        .collect())
}

/// Replaces byte ranges of `text` (none overlapping), last first, and writes the file whole or not at all.
fn save_edits(file: &Path, text: &str, mut edits: Vec<(std::ops::Range<usize>, String)>) -> Result<(), String> {
    if edits.is_empty() {
        return Ok(());
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    let mut text = text.to_string();
    for (range, piece) in edits {
        text.replace_range(range, &piece);
    }
    let temp = PathBuf::from(format!("{}.dnnmanager.tmp", file.display()));
    let written = std::fs::write(&temp, text.as_bytes()).and_then(|()| std::fs::rename(&temp, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

/// `system.web/compilation debug="false"` (`SetDebug(path, false)`); nothing when there is no such element.
pub(crate) fn set_debug_off(web_config: &Path) -> Result<(), String> {
    let text = read_text(web_config)?;
    let bom = text.len() - xml(&text).len();
    let doc = load_site_xml(xml(&text))?;
    let compilation = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("system.web"))
        .and_then(|web| web.children().find(|n| n.has_tag_name("compilation")));
    let Some(compilation) = compilation else { return Ok(()) };
    let edit = match compilation.attributes().find(|a| a.name() == "debug" && a.namespace().is_none()) {
        Some(debug) => {
            let at = debug.range_value();
            (at.start + bom..at.end + bom, "false".to_string())
        }
        None => {
            let at = bom + compilation.range().start + "<compilation".len();
            (at..at, " debug=\"false\"".to_string())
        }
    };
    save_edits(web_config, &text, vec![edit])
}

/// Switches the HTTPS redirect rules back on that DNN Manager switched off for local development (marked by its
/// comment before them): `enabled="false"` and the comment go, with the line the comment had to itself. Their names.
pub(crate) fn enable_https_redirect_rules(web_config: &Path) -> Result<Vec<String>, String> {
    if !web_config.is_file() {
        return Ok(Vec::new());
    }
    let text = read_text(web_config)?;
    let bom = text.len() - xml(&text).len();
    let doc = load_site_xml(xml(&text))?;
    let mut edits = Vec::new();
    let mut names = Vec::new();
    let rules = doc
        .descendants()
        .filter(|n| n.has_tag_name("system.webServer"))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rewrite")))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rules")))
        .flat_map(|n| n.children().filter(|c| c.has_tag_name("rule")));
    for rule in rules {
        let Some(enabled) = rule.attributes().find(|a| a.name() == "enabled" && a.namespace().is_none()) else {
            continue;
        };
        if !is_https_redirect(rule) || !enabled.value().eq_ignore_ascii_case("false") {
            continue;
        }
        // The nearest node before the rule that isn't whitespace is DNN Manager's comment.
        let mut previous = rule.prev_sibling();
        while let Some(node) = previous.filter(|n| n.is_text() && n.text().is_some_and(|t| t.trim().is_empty())) {
            previous = node.prev_sibling();
        }
        let Some(comment) =
            previous.filter(|n| n.is_comment() && n.text().is_some_and(|t| t.contains("Disabled by DNN Manager")))
        else {
            continue;
        };
        // The attribute with the space before it.
        let attribute = enabled.range();
        let start = text[..attribute.start + bom].trim_end_matches([' ', '\t', '\r', '\n']).len();
        edits.push((start..attribute.end + bom, String::new()));
        // The comment, and the line it had to itself: the whitespace right after it when the rule follows that.
        let after = comment.next_sibling();
        let end = match after {
            Some(indent) if indent.is_text() && indent.next_sibling() == Some(rule) => indent.range().end,
            _ => comment.range().end,
        };
        edits.push((comment.range().start + bom..end + bom, String::new()));
        names.push(rule.attribute("name").unwrap_or("(unnamed)").to_string());
    }
    save_edits(web_config, &text, edits)?;
    Ok(names)
}

/// A rule that redirects to an https:// address (typically "HTTP to HTTPS redirect").
fn is_https_redirect(rule: roxmltree::Node) -> bool {
    rule.children().find(|c| c.has_tag_name("action")).is_some_and(|action| {
        action.attribute("type").is_some_and(|t| t.eq_ignore_ascii_case("Redirect"))
            && action.attribute("url").is_some_and(|u| {
                let u = u.trim_start();
                u.len() >= 8 && u.is_char_boundary(8) && u[..8].eq_ignore_ascii_case("https://")
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_CONFIG: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<configuration>\r\n  <connectionStrings>\r\n    <add name=\"SiteSqlServer\" connectionString=\"Server=localhost,1433;Database=shop;uid=sa;pwd=local\" providerName=\"System.Data.SqlClient\" />\r\n  </connectionStrings>\r\n  <appSettings>\r\n    <add key=\"SiteSqlServer\" value=\"Server=localhost,1433;Database=shop;uid=sa;pwd=local\" />\r\n  </appSettings>\r\n  <system.web>\r\n    <compilation debug=\"true\" targetFramework=\"4.8\" />\r\n  </system.web>\r\n  <system.webServer>\r\n    <rewrite>\r\n      <rules>\r\n        <!-- Disabled by DNN Manager for local development (no HTTPS locally). Re-enable (remove enabled=\"false\") before deploying to production. -->\r\n        <rule name=\"HTTP to HTTPS\" enabled=\"false\" stopProcessing=\"true\">\r\n          <action type=\"Redirect\" url=\"https://{HTTP_HOST}/{R:1}\" />\r\n        </rule>\r\n        <rule name=\"Own\" enabled=\"false\">\r\n          <action type=\"Redirect\" url=\"https://x\" />\r\n        </rule>\r\n      </rules>\r\n    </rewrite>\r\n  </system.webServer>\r\n</configuration>\r\n";

    #[test]
    fn debug_and_the_https_rules_are_changed_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web.config");
        std::fs::write(&file, WEB_CONFIG).unwrap();
        set_debug_off(&file).unwrap();
        assert_eq!(enable_https_redirect_rules(&file).unwrap(), ["HTTP to HTTPS"]);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("<compilation debug=\"false\" targetFramework=\"4.8\" />"));
        assert!(text.contains("<rules>\r\n        <rule name=\"HTTP to HTTPS\" stopProcessing=\"true\">"), "{text}");
        assert!(text.contains("<rule name=\"Own\" enabled=\"false\">"), "a rule switched off by the site stays off");
        assert!(!text.contains("Disabled by DNN Manager"));
        // Without a debug attribute, one is added.
        std::fs::write(
            &file,
            "<configuration><system.web><compilation targetFramework=\"4.8\"/></system.web></configuration>",
        )
        .unwrap();
        set_debug_off(&file).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("<compilation debug=\"false\" targetFramework"));
    }

    fn zip_with(path: &Path, files: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in files {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    fn zip_entries(path: &Path) -> Vec<(String, String)> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut entries: Vec<(String, String)> = (0..archive.len())
            .map(|i| {
                let mut entry = archive.by_index(i).unwrap();
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                (entry.name().unwrap().to_string(), text)
            })
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn the_zips_web_config_and_its_config_source_are_made_ready_for_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("shop.zip");
        let web_config = "<configuration>\r\n  <connectionStrings configSource=\"App_Config\\cs.config\" />\r\n  <system.web><compilation debug=\"true\" /></system.web>\r\n</configuration>";
        let cs = "<connectionStrings>\r\n  <add name=\"SiteSqlServer\" connectionString=\"Server=.;Database=shop;uid=sa;pwd=local\" />\r\n</connectionStrings>";
        zip_with(&zip, &[("Web.config", web_config), ("app_config/CS.config", cs), ("bin/x.dll", "dll")]);
        let temp = dir.path().join("temp");
        let prepared =
            ZipWebConfig { temp_root: temp.clone() }.prepare(&zip, "Server=live;Password=a&b", true).unwrap();
        assert!(prepared.debug_off && prepared.rules_enabled.is_empty());
        let entries = zip_entries(&zip);
        assert_eq!(
            entries.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
            ["Web.config", "app_config/CS.config", "bin/x.dll"]
        );
        assert!(entries[0].1.contains("<compilation debug=\"false\" />"));
        assert!(entries[1].1.contains("connectionString=\"Server=live;Password=a&amp;b\""), "{}", entries[1].1);
        assert_eq!(entries[2].1, "dll");
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0, "the temporary copy is gone");
        assert!(!dir.path().join("shop.zip.tmp").exists());
    }

    #[test]
    fn a_zip_without_web_config_is_said_so() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("shop.zip");
        zip_with(&zip, &[("default.aspx", "x")]);
        let result = ZipWebConfig { temp_root: dir.path().join("temp") }.prepare(&zip, "x", false);
        assert_eq!(result.unwrap_err(), "The site has no web.config.");
        let zip = dir.path().join("other.zip");
        zip_with(&zip, &[("web.config", "<configuration />")]);
        let result = ZipWebConfig { temp_root: dir.path().join("temp") }.prepare(&zip, "x", false);
        assert!(result.unwrap_err().starts_with(
            "Could not set the connection string: connectionStrings/add[@name='SiteSqlServer'] not found in "
        ));
    }

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn the_default_folder_is_dated_in_deployments() {
        let now = Stamp::from_folder_name("x_20260928_154210").unwrap();
        assert!(default_folder("shop", now).ends_with(r"DnnManager\deployments\shop_20260928_154210"));
    }
}
