//! What a site's web.config says that a developer looks for (`WebConfigInspector.cs`, and the HTTPS redirect rules
//! of `WebConfigService.ReadFacts`) - never a password or a key, only whether one is set. A value that is None wasn't
//! in the file: ASP.NET, IIS or DNN then use their default (the `DEFAULT_*` constants).

use std::path::{Path, PathBuf};

use dnnm_infrastructure::web_config::load_site_xml;
use roxmltree::Node;

use crate::format::LocalDateTime;

/// ASP.NET's and IIS's defaults, for a value web.config doesn't set.
pub const DEFAULT_MAX_REQUEST_LENGTH_KB: u64 = 4096;
pub const DEFAULT_EXECUTION_TIMEOUT_SECONDS: u64 = 110;
pub const DEFAULT_MAX_ALLOWED_CONTENT_LENGTH: u64 = 30_000_000;

const ASM: &str = "urn:schemas-microsoft-com:asm.v1";

/// A binding redirect in web.config's runtime section: `name` from `old_version` to `new_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRedirect {
    pub name: String,
    pub old_version: String,
    pub new_version: String,
}

/// A codeBase in web.config's runtime section: `name` `version` loads from `href` - a file outside the probing
/// path, often bin\<module>.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBase {
    pub name: String,
    pub version: String,
    pub href: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebConfigInspection {
    pub path: PathBuf,
    /// Why it couldn't be read; None when it was.
    pub problem: Option<String>,
    pub modified: Option<LocalDateTime>,

    // system.web
    pub debug: Option<bool>,
    pub compilation_target_framework: Option<String>,
    pub runtime_target_framework: Option<String>,
    pub custom_errors: Option<String>,
    pub max_request_length_kb: Option<u64>,
    pub execution_timeout_seconds: Option<u64>,
    pub request_validation_mode: Option<String>,
    pub session_state: Option<String>,
    pub authentication_mode: Option<String>,
    /// None: no machineKey element (ASP.NET generates keys per machine); else how its keys are set.
    pub machine_key: Option<String>,

    // system.webServer
    pub max_allowed_content_length: Option<u64>,
    pub max_url: Option<u64>,
    pub max_query_string: Option<u64>,
    pub static_compression: Option<bool>,
    pub dynamic_compression: Option<bool>,
    pub client_cache: Option<String>,
    pub http_errors: Option<String>,
    pub run_all_managed_modules: Option<bool>,
    pub rewrite_rules: usize,
    /// HTTPS redirect rules DNN Manager switched off for local development.
    pub disabled_https_rules: Vec<String>,

    // dotnetnuke
    pub has_dnn_section: bool,
    pub friendly_url_provider: Option<String>,
    pub friendly_url_format: Option<String>,
    pub caching_provider: Option<String>,
    pub logging_provider: Option<String>,
    pub data_provider: Option<String>,
    pub object_qualifier: Option<String>,
    pub database_owner: Option<String>,

    // appSettings
    pub install_version: Option<String>,
    pub installation_date: Option<String>,
    pub auto_upgrade: Option<String>,

    // runtime
    pub binding_redirects: Vec<BindingRedirect>,
    pub code_bases: Vec<CodeBase>,
    pub probing_path: Option<String>,

    // The SiteSqlServer connection's options - never its password.
    pub sql_server: Option<String>,
    pub sql_database: Option<String>,
    pub sql_integrated_security: Option<bool>,
    pub sql_user: Option<String>,
    /// "True", "False" or "Strict", as SqlClient names it.
    pub sql_encrypt: Option<String>,
    pub sql_trust_server_certificate: Option<bool>,
    pub sql_connect_timeout: Option<u64>,
    pub sql_multiple_active_result_sets: Option<bool>,
    pub sql_application_name: Option<String>,
    pub sql_attach_db_file: Option<String>,
    /// Sections kept in a file of their own (configSource) - not read here.
    pub external_sections: Vec<String>,
}

fn child<'a>(node: Option<Node<'a, 'a>>, name: &str) -> Option<Node<'a, 'a>> {
    node?.children().find(|c| c.is_element() && c.tag_name().name() == name && c.tag_name().namespace().is_none())
}

fn asm_children<'a>(node: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> + 'a {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name && c.tag_name().namespace() == Some(ASM))
}

fn attr(node: Option<Node<'_, '_>>, name: &str) -> Option<String> {
    node?.attribute(name).map(str::to_string)
}

fn bool_attr(node: Option<Node<'_, '_>>, name: &str) -> Option<bool> {
    match node?.attribute(name)?.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn number_attr(node: Option<Node<'_, '_>>, name: &str) -> Option<u64> {
    node?.attribute(name)?.trim().parse().ok()
}

/// A DNN provider section's default provider entry.
fn default_provider<'a>(section: Option<Node<'a, 'a>>) -> Option<Node<'a, 'a>> {
    let name = section?.attribute("defaultProvider")?;
    child(section, "providers")?.children().find(|a| a.has_tag_name("add") && a.attribute("name") == Some(name))
}

/// How machineKey's keys are set - "auto-generated keys", "fixed keys" - never the keys.
fn machine_key(key: Option<Node<'_, '_>>) -> Option<String> {
    let key = key?;
    let validation = key.attribute("validationKey").unwrap_or("AutoGenerate");
    let decryption = key.attribute("decryptionKey").unwrap_or("AutoGenerate");
    let auto = validation.to_ascii_lowercase().starts_with("autogenerate")
        && decryption.to_ascii_lowercase().starts_with("autogenerate");
    let algorithms = format!(
        "{} / {}",
        key.attribute("validation").unwrap_or("default"),
        key.attribute("decryption").unwrap_or("Auto")
    );
    Some(format!("{} ({algorithms})", if auto { "auto-generated keys" } else { "fixed keys" }))
}

fn app_setting(root: Node<'_, '_>, key: &str) -> Option<String> {
    child(Some(root), "appSettings")?
        .children()
        .find(|a| a.has_tag_name("add") && a.attribute("key") == Some(key))
        .and_then(|a| a.attribute("value").map(str::to_string))
}

/// A rewrite rule DNN Manager switched off: `enabled="false"`, and right before it its "Disabled by DNN Manager"
/// comment.
fn disabled_by_dnn_manager(rule: Node<'_, '_>) -> bool {
    if !rule.attribute("enabled").is_some_and(|e| e.eq_ignore_ascii_case("false")) {
        return false;
    }
    let mut node = rule.prev_sibling();
    while let Some(n) = node {
        if n.is_text() && n.text().is_some_and(|t| t.trim().is_empty()) {
            node = n.prev_sibling();
            continue;
        }
        return n.is_comment() && n.text().is_some_and(|t| t.contains("Disabled by DNN Manager"));
    }
    false
}

/// Reads `directory`\web.config. Never fails: what can't be read is in `problem`.
pub fn inspect(directory: &Path) -> WebConfigInspection {
    let path = directory.join("web.config");
    if !path.is_file() {
        return WebConfigInspection { path, problem: Some("not found".into()), ..Default::default() };
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            return WebConfigInspection { path, problem: Some(format!("can't be read: {e}")), ..Default::default() };
        }
    };
    // The site's web.config, which its app pool can change: read without a DTD.
    let doc = match load_site_xml(&text) {
        Ok(doc) => doc,
        Err(e) => {
            return WebConfigInspection { path, problem: Some(format!("can't be read: {e}")), ..Default::default() };
        }
    };
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok().map(LocalDateTime::from_system_time);
    inspect_document(&doc, path, modified)
}

fn inspect_document(
    doc: &roxmltree::Document<'_>,
    path: PathBuf,
    modified: Option<LocalDateTime>,
) -> WebConfigInspection {
    let root = doc.root_element();
    let web = child(Some(root), "system.web");
    let server = child(Some(root), "system.webServer");
    let dnn = child(Some(root), "dotnetnuke");
    let compilation = child(web, "compilation");
    let runtime = child(web, "httpRuntime");
    let limits = child(child(child(server, "security"), "requestFiltering"), "requestLimits");
    let compression = child(server, "urlCompression");
    let cache = child(child(server, "staticContent"), "clientCache");
    let external = root
        .descendants()
        .filter(|e| e.is_element() && e.attribute("configSource").is_some())
        .map(|e| e.tag_name().name().to_string())
        .collect();

    let friendly = default_provider(child(dnn, "friendlyUrl"));
    let data_section = child(dnn, "data");
    let data = default_provider(data_section);

    // runtime can have more than one assemblyBinding, and a dependentAssembly more than one bindingRedirect and
    // codeBase (one per version range).
    let mut redirects = Vec::new();
    let mut code_bases = Vec::new();
    let mut probing = Vec::new();
    if let Some(runtime_section) = child(Some(root), "runtime") {
        for binding in asm_children(runtime_section, "assemblyBinding") {
            if let Some(path) = asm_children(binding, "probing").next().and_then(|p| p.attribute("privatePath"))
                && !path.trim().is_empty()
            {
                probing.push(path.to_string());
            }
            for dependent in asm_children(binding, "dependentAssembly") {
                let name = asm_children(dependent, "assemblyIdentity")
                    .next()
                    .and_then(|i| i.attribute("name"))
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                for r in asm_children(dependent, "bindingRedirect") {
                    redirects.push(BindingRedirect {
                        name: name.into(),
                        old_version: r.attribute("oldVersion").unwrap_or_default().into(),
                        new_version: r.attribute("newVersion").unwrap_or_default().into(),
                    });
                }
                for c in asm_children(dependent, "codeBase") {
                    let href = c.attribute("href").unwrap_or_default();
                    if !href.is_empty() {
                        code_bases.push(CodeBase {
                            name: name.into(),
                            version: c.attribute("version").unwrap_or_default().into(),
                            href: href.into(),
                        });
                    }
                }
            }
        }
    }

    let rules: Vec<Node> = child(child(server, "rewrite"), "rules")
        .map(|r| r.children().filter(|c| c.has_tag_name("rule")).collect())
        .unwrap_or_default();
    let disabled_https_rules = root
        .descendants()
        .filter(|n| n.has_tag_name("system.webServer"))
        .filter_map(|s| child(child(Some(s), "rewrite"), "rules"))
        .flat_map(|r| r.children().filter(|c| c.has_tag_name("rule")))
        .filter(|r| disabled_by_dnn_manager(*r))
        .map(|r| r.attribute("name").unwrap_or("(unnamed)").to_string())
        .collect();

    let mut inspection = WebConfigInspection {
        path,
        problem: None,
        modified,
        debug: bool_attr(compilation, "debug"),
        compilation_target_framework: attr(compilation, "targetFramework"),
        runtime_target_framework: attr(runtime, "targetFramework"),
        custom_errors: attr(child(web, "customErrors"), "mode"),
        max_request_length_kb: number_attr(runtime, "maxRequestLength"),
        execution_timeout_seconds: number_attr(runtime, "executionTimeout"),
        request_validation_mode: attr(runtime, "requestValidationMode"),
        session_state: attr(child(web, "sessionState"), "mode"),
        authentication_mode: attr(child(web, "authentication"), "mode"),
        machine_key: machine_key(child(web, "machineKey")),
        max_allowed_content_length: number_attr(limits, "maxAllowedContentLength"),
        max_url: number_attr(limits, "maxUrl"),
        max_query_string: number_attr(limits, "maxQueryString"),
        static_compression: bool_attr(compression, "doStaticCompression"),
        dynamic_compression: bool_attr(compression, "doDynamicCompression"),
        client_cache: cache.map(|c| {
            let mode = c.attribute("cacheControlMode").unwrap_or("NoControl");
            match c.attribute("cacheControlMaxAge") {
                Some(age) => format!("{mode}, max-age {age}"),
                None => mode.to_string(),
            }
        }),
        http_errors: attr(child(server, "httpErrors"), "errorMode"),
        run_all_managed_modules: bool_attr(child(server, "modules"), "runAllManagedModulesForAllRequests"),
        rewrite_rules: rules.len(),
        disabled_https_rules,
        has_dnn_section: dnn.is_some(),
        friendly_url_provider: attr(child(dnn, "friendlyUrl"), "defaultProvider"),
        friendly_url_format: attr(friendly, "urlFormat"),
        caching_provider: attr(child(dnn, "caching"), "defaultProvider"),
        logging_provider: attr(child(dnn, "logging"), "defaultProvider"),
        data_provider: attr(data_section, "defaultProvider"),
        object_qualifier: attr(data, "objectQualifier"),
        database_owner: attr(data, "databaseOwner"),
        install_version: app_setting(root, "InstallVersion"),
        installation_date: app_setting(root, "InstallationDate"),
        auto_upgrade: app_setting(root, "AutoUpgrade"),
        binding_redirects: redirects,
        code_bases,
        probing_path: (!probing.is_empty()).then(|| probing.join(";")),
        external_sections: external,
        ..Default::default()
    };
    let connection = child(Some(root), "connectionStrings")
        .and_then(|c| c.children().find(|a| a.has_tag_name("add") && a.attribute("name") == Some("SiteSqlServer")))
        .and_then(|a| a.attribute("connectionString"))
        .filter(|c| !c.trim().is_empty());
    if let Some(connection) = connection {
        with_connection(&mut inspection, connection);
    }
    inspection
}

/// `key=value;key='va;lue'` → pairs; None for a string that doesn't parse (an unclosed quote).
fn split_pairs(text: &str) -> Option<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ';' || c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(pairs);
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if let Some(quote @ ('\'' | '"')) = chars.peek().copied() {
            chars.next();
            loop {
                match chars.next() {
                    Some(c) if c == quote && chars.peek() == Some(&quote) => {
                        chars.next();
                        value.push(c);
                    }
                    Some(c) if c == quote => break,
                    Some(c) => value.push(c),
                    None => return None,
                }
            }
            while chars.peek().is_some_and(|c| *c != ';') {
                chars.next();
            }
        } else {
            value = chars.by_ref().take_while(|c| *c != ';').collect::<String>().trim_end().to_string();
        }
        pairs.push((key.trim().to_ascii_lowercase(), value));
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

/// The SiteSqlServer connection's options - parsed here, so its password never leaves.
fn with_connection(inspection: &mut WebConfigInspection, text: &str) {
    let Some(pairs) = split_pairs(text) else { return };
    let mut integrated = false;
    let mut user = None;
    for (key, value) in pairs {
        match key.as_str() {
            "data source" | "server" | "address" | "addr" | "network address" => inspection.sql_server = Some(value),
            "initial catalog" | "database" => inspection.sql_database = Some(value),
            "integrated security" | "trusted_connection" => {
                integrated = matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "sspi")
            }
            "user id" | "uid" | "user" => user = Some(value),
            "encrypt" => {
                inspection.sql_encrypt = Some(
                    match value.trim().to_ascii_lowercase().as_str() {
                        "true" | "yes" | "mandatory" => "True",
                        "strict" => "Strict",
                        _ => "False",
                    }
                    .into(),
                )
            }
            "trustservercertificate" | "trust server certificate" => {
                inspection.sql_trust_server_certificate = parse_bool(&value)
            }
            "connect timeout" | "connection timeout" | "timeout" => {
                inspection.sql_connect_timeout = value.trim().parse().ok()
            }
            "multipleactiveresultsets" | "multiple active result sets" => {
                inspection.sql_multiple_active_result_sets = parse_bool(&value)
            }
            "application name" | "app" => inspection.sql_application_name = Some(value),
            "attachdbfilename" | "extended properties" | "initial file name" => {
                inspection.sql_attach_db_file = Some(value).filter(|v| !v.is_empty())
            }
            _ => {}
        }
    }
    inspection.sql_integrated_security = Some(integrated);
    inspection.sql_user = if integrated { None } else { user };
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <connectionStrings>
    <add name="SiteSqlServer" connectionString="Server=.\SQL;Database=shop;User ID=dnn;Password='p;w';Encrypt=Mandatory;TrustServerCertificate=true;Connect Timeout=30" providerName="System.Data.SqlClient" />
  </connectionStrings>
  <appSettings>
    <add key="InstallVersion" value="10.0.1" />
    <add key="AutoUpgrade" value="false" />
  </appSettings>
  <system.web>
    <compilation debug="true" targetFramework="4.8" />
    <httpRuntime targetFramework="4.8" maxRequestLength="102400" executionTimeout="1200" requestValidationMode="2.0" />
    <customErrors mode="RemoteOnly" />
    <machineKey validationKey="ABC" decryptionKey="DEF" decryption="3DES" validation="SHA1" />
    <sessionState mode="Off" />
  </system.web>
  <system.webServer>
    <security><requestFiltering><requestLimits maxAllowedContentLength="104857600" /></requestFiltering></security>
    <rewrite>
      <rules>
        <!-- Disabled by DNN Manager for local development (no HTTPS locally). -->
        <rule name="HTTP to HTTPS" enabled="false" />
        <rule name="Other" enabled="false" />
      </rules>
    </rewrite>
    <staticContent><clientCache cacheControlMode="UseMaxAge" cacheControlMaxAge="365.00:00:00" /></staticContent>
    <modules runAllManagedModulesForAllRequests="true" />
  </system.webServer>
  <dotnetnuke>
    <friendlyUrl defaultProvider="DNNFriendlyUrl">
      <providers><add name="DNNFriendlyUrl" urlFormat="advanced" /></providers>
    </friendlyUrl>
    <data defaultProvider="SqlDataProvider">
      <providers><add name="SqlDataProvider" objectQualifier="dnn_" databaseOwner="dbo" /></providers>
    </data>
    <caching defaultProvider="FileBasedCachingProvider" />
  </dotnetnuke>
  <runtime>
    <assemblyBinding xmlns="urn:schemas-microsoft-com:asm.v1">
      <probing privatePath="bin\HttpModules;bin\Providers" />
      <dependentAssembly>
        <assemblyIdentity name="Newtonsoft.Json" publicKeyToken="30ad4fe6b2a6aeed" />
        <bindingRedirect oldVersion="0.0.0.0-32767.32767.32767.32767" newVersion="13.0.0.0" />
      </dependentAssembly>
      <dependentAssembly>
        <assemblyIdentity name="Imageflow.Net" />
        <codeBase version="2.2.0.0" href="bin\Imageflow\Imageflow.Net.dll" />
      </dependentAssembly>
    </assemblyBinding>
  </runtime>
  <log4net configSource="DotNetNuke.log4net.config" />
</configuration>"#;

    #[test]
    fn reads_what_a_developer_looks_for() {
        let doc = load_site_xml(CONFIG).unwrap();
        let w = inspect_document(&doc, PathBuf::from("web.config"), None);
        assert_eq!(w.debug, Some(true));
        assert_eq!((w.max_request_length_kb, w.execution_timeout_seconds), (Some(102_400), Some(1200)));
        assert_eq!(w.max_allowed_content_length, Some(104_857_600));
        assert_eq!(w.machine_key.as_deref(), Some("fixed keys (SHA1 / 3DES)"));
        assert_eq!(w.client_cache.as_deref(), Some("UseMaxAge, max-age 365.00:00:00"));
        assert_eq!((w.rewrite_rules, w.disabled_https_rules.clone()), (2, vec!["HTTP to HTTPS".to_string()]));
        assert!(w.has_dnn_section);
        assert_eq!(w.friendly_url_format.as_deref(), Some("advanced"));
        assert_eq!((w.object_qualifier.as_deref(), w.database_owner.as_deref()), (Some("dnn_"), Some("dbo")));
        assert_eq!(w.caching_provider.as_deref(), Some("FileBasedCachingProvider"));
        assert_eq!(w.install_version.as_deref(), Some("10.0.1"));
        assert_eq!(w.binding_redirects[0].new_version, "13.0.0.0");
        assert_eq!(w.code_bases[0].href, r"bin\Imageflow\Imageflow.Net.dll");
        assert_eq!(w.probing_path.as_deref(), Some(r"bin\HttpModules;bin\Providers"));
        assert_eq!(w.external_sections, ["log4net"]);
        assert_eq!(w.sql_encrypt.as_deref(), Some("True"));
        assert_eq!((w.sql_trust_server_certificate, w.sql_connect_timeout), (Some(true), Some(30)));
        assert_eq!((w.sql_user.as_deref(), w.sql_database.as_deref()), (Some("dnn"), Some("shop")));
        assert!(!format!("{w:?}").contains("p;w"), "the password is never kept");
    }

    #[test]
    fn a_missing_or_broken_file_is_a_problem_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(inspect(dir.path()).problem.as_deref(), Some("not found"));
        std::fs::write(dir.path().join("web.config"), "<configuration>").unwrap();
        assert!(inspect(dir.path()).problem.unwrap().starts_with("can't be read"));
        std::fs::write(dir.path().join("web.config"), "<!DOCTYPE x><configuration/>").unwrap();
        assert!(inspect(dir.path()).problem.unwrap().contains("DTD"));
    }
}
