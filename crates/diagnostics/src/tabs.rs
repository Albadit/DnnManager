//! The Details view's tabs from a [`ProjectSnapshot`] - and what is wrong in it (`ProjectDiagnostics.cs`). Every
//! label, value and note is the WPF app's, word for word.

use std::time::{Duration, SystemTime};

use dnnm_application::iis::{IisBindingDetails, IisSiteDetails, states};
use dnnm_application::project::view;
use dnnm_infrastructure::web_config::{DatabaseConnection, DatabaseKind};

use crate::assemblies::AssemblyProblem;
use crate::database::DnnPortalDetails;
use crate::format::{
    LocalDateTime, bool_text, byte_size, count, general, group_thousands, hh_mm, on_off, short_date, span,
};
use crate::model::{Health, Section, Tab, TabKind, Table, TableRow};
use crate::snapshot::ProjectSnapshot;
use crate::web_config::{
    DEFAULT_EXECUTION_TIMEOUT_SECONDS, DEFAULT_MAX_ALLOWED_CONTENT_LENGTH, DEFAULT_MAX_REQUEST_LENGTH_KB,
};

/// Something wrong or unusual about a site - always from a value that was detected, with that value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedIssue {
    pub health: Health,
    /// "IIS", "Portals", "web.config", "Limits", "Database", "Filesystem".
    pub area: String,
    pub text: String,
}

/// Every tab, in order, with the issues found once for all of them.
pub fn tabs(s: &ProjectSnapshot) -> Vec<Tab> {
    let found = issues(s);
    TabKind::ALL.iter().map(|&kind| Tab { kind, sections: tab(s, kind, &found) }).collect()
}

/// One tab - the WPF app builds only the one shown (`ProjectView.ShowVisiblePanel`).
pub fn tab(s: &ProjectSnapshot, kind: TabKind, issues: &[DetectedIssue]) -> Vec<Section> {
    match kind {
        TabKind::General => general_tab(s, issues),
        TabKind::Iis => iis_tab(s),
        TabKind::Dnn => dnn_tab(s),
        TabKind::Database => database_tab(s, issues),
        TabKind::Advanced => advanced_tab(s, issues),
    }
}

fn some(text: impl Into<String>) -> Option<String> {
    Some(text.into())
}

fn dnn_version(s: &ProjectSnapshot) -> &str {
    s.project.dnn_version.as_deref().unwrap_or_default()
}

fn ok_if(good: bool, otherwise: Health) -> Health {
    if good { Health::Ok } else { otherwise }
}

// ─── General ─────────────────────────────────────────────────────────────

pub fn general_tab(s: &ProjectSnapshot, issues: &[DetectedIssue]) -> Vec<Section> {
    let p = &s.project;
    let mut project = Section::new("Project", Some("the IIS site, its folder and bin"));
    project.text("Name", &p.name);
    let exists = p.directory.is_dir();
    let detail = if !exists {
        some("The folder IIS serves isn't there.")
    } else if p.in_projects_folder {
        None
    } else {
        some("Outside the projects folder.")
    };
    project.add(
        "Physical path",
        some(p.directory.display().to_string()),
        if exists { Health::None } else { Health::Bad },
        detail,
    );
    project.text("Folder size", view::size(p));
    project.text("Type", project_type(s));
    if s.is_dnn() {
        let product = s.dnn_product.as_ref().map(|p| format!(" ({p})")).unwrap_or_default();
        project.add(
            "DNN version",
            some(format!("{}{product}", dnn_version(s))),
            Health::None,
            some(r"From bin\DotNetNuke.dll."),
        );
    }
    if let Some(installed) = s.database.as_ref().and_then(|d| d.dnn.as_ref()).and_then(|d| d.installed) {
        project.add(
            "Installed",
            some(general(&installed)),
            Health::None,
            some("The first entry of DNN's Version table."),
        );
    } else if let Some(date) = &s.config.installation_date {
        project.add("Installed", some(date.clone()), Health::None, some("InstallationDate in web.config."));
    }
    if let Some(newest) = &s.assemblies.newest {
        project.add(
            "Last deployed (bin)",
            some(general(&LocalDateTime::from_system_time(newest.modified))),
            Health::None,
            some(format!("The newest assembly: {}.", newest.file)),
        );
    }
    if let Some(changed) = &s.config.modified {
        project.text("web.config changed", general(changed));
    }
    if let Some(branch) = &s.folder.git_branch {
        project.text("Git branch", branch);
    }
    if !s.folder.solutions.is_empty() {
        project.text("Solution", s.folder.solutions.join(", "));
    }
    let backups = &s.folder.backups;
    let text = match backups.first() {
        None => "none".to_string(),
        Some(newest) => format!("{} - newest {}", backups.len(), general(newest)),
    };
    project.add(
        "Backups",
        Some(text),
        Health::None,
        s.folder.backup_directory.as_ref().map(|d| d.display().to_string()),
    );
    vec![project, health_section(s, issues)]
}

/// Each part a DNN site needs - found, and as it should be, or what was found instead.
fn health_section(s: &ProjectSnapshot, issues: &[DetectedIssue]) -> Section {
    let p = &s.project;
    let mut section = Section::new("Project health", None);
    if s.is_dnn() {
        let (text, health) = if s.config.has_dnn_section {
            (format!(r"DNN {} - bin\DotNetNuke.dll and web.config's dotnetnuke section", dnn_version(s)), Health::Ok)
        } else {
            (r"bin\DotNetNuke.dll, but web.config has no dotnetnuke section".to_string(), Health::Bad)
        };
        section.add("DNN installation", Some(text), health, None);
    } else {
        section.text("DNN installation", r"none - no bin\DotNetNuke.dll");
    }

    let problem = s.config.problem.clone();
    section.add(
        "web.config",
        Some(problem.clone().unwrap_or_else(|| "found and read".into())),
        ok_if(problem.is_none(), Health::Bad),
        some(s.config.path.display().to_string()),
    );

    if let Some(c) = &s.connection {
        section.add(
            "Database configuration",
            some(format!("[{}] on {} - {}", c.database, c.server, authentication(c))),
            Health::Ok,
            some("SiteSqlServer in web.config."),
        );
    } else if let Some(why) = &p.database_problem {
        section.add("Database configuration", Some(why.clone()), Health::Warning, None);
    }

    if s.connection.is_some() && !p.database_is_file {
        let sql = view::sql(p);
        let text = if sql == "Live" {
            format!("[{}] answers", p.database_name.as_deref().unwrap_or_default())
        } else {
            view::sql_tip(p).unwrap_or_else(|| sql.to_string())
        };
        let health = match sql {
            "Live" => Health::Ok,
            "Offline" | "(none)" => Health::Bad,
            _ => Health::None,
        };
        section.add("Database", Some(text), health, None);
    }

    match &s.iis {
        None => {
            section.add("IIS site", some(view::state_text(&p.site)), Health::Warning, None);
        }
        Some(site) => {
            section.add(
                "IIS site",
                some(format!("{} - ID {}", site.state, site.id)),
                ok_if(states::is_started(&site.state), Health::Warning),
                None,
            );
            let (text, health) = match &site.pool {
                Some(pool) => (
                    format!("{} - {}", pool.name, pool.state.as_deref().unwrap_or("state unknown")),
                    ok_if(pool.state.as_deref().is_some_and(states::is_started), Health::Warning),
                ),
                None => ("not in IIS".to_string(), Health::Bad),
            };
            section.add("Application pool", Some(text), health, None);
        }
    }

    if let Some(dnn) = s.database.as_ref().and_then(|d| d.dnn.as_ref()) {
        let alias_issues = issues.iter().filter(|i| i.area == "Portals").count();
        let aliases: usize =
            dnn.portals.iter().map(|x| x.other_aliases.len() + usize::from(x.primary_alias.is_some())).sum();
        let mut text =
            format!("{}, {}", count(dnn.portals.len(), "portal", None), count(aliases, "alias", Some("aliases")));
        if alias_issues == 0 {
            text.push_str(" - every alias has an IIS binding");
        }
        section.add(
            "Portal configuration",
            Some(text),
            ok_if(alias_issues == 0, Health::Warning),
            (alias_issues > 0).then(|| "See Detected issues on Advanced.".to_string()),
        );
    }

    let (issues_text, issues_health) = tally(issues.iter().map(|i| i.health));
    section.add(
        "Detected issues",
        Some(issues_text),
        issues_health,
        (!issues.is_empty()).then(|| "Listed at the bottom of Advanced.".to_string()),
    );
    // Assembly problems have a list of their own - in Assemblies on Advanced, not in Detected issues.
    if !s.assemblies.problems.is_empty() {
        let (text, health) = tally(s.assemblies.problems.iter().map(assembly_health));
        section.add("Assemblies", Some(text), health, some("Listed in Assemblies on Advanced."));
    }
    if !s.database_read && s.connection.is_some() {
        section.note = some("Reading the database…");
    }
    section
}

// ─── IIS ─────────────────────────────────────────────────────────────────

pub fn iis_tab(s: &ProjectSnapshot) -> Vec<Section> {
    let Some(site) = &s.iis else {
        return vec![
            Section::new("Site", Some("IIS"))
                .with_note("IIS's configuration couldn't be read - DNN Manager reads it with administrator rights."),
        ];
    };
    let mut sections = Vec::new();

    let mut site_section = Section::new("Site", Some("IIS - applicationHost.config"));
    site_section.text("Name", &site.name);
    site_section.add(
        "Site ID",
        some(site.id.to_string()),
        Health::None,
        some(format!("Its IIS logs are in W3SVC{}.", site.id)),
    );
    site_section.add(
        "Physical path",
        some(site.physical_path.display().to_string()),
        if site.physical_path.is_dir() { Health::None } else { Health::Bad },
        None,
    );
    site_section.add("State", some(&site.state), ok_if(states::is_started(&site.state), Health::Warning), None);
    site_section.add(
        "Application pool",
        some(site.pool.as_ref().map_or("not in IIS", |p| p.name.as_str())),
        if site.pool.is_none() { Health::Bad } else { Health::None },
        None,
    );
    site_section.text("Configuration", &site.config_path);
    site_section
        .text("Auto-start", if site.server_auto_start { "on - starts with IIS" } else { "off - started by hand" });
    site_section.text(
        "Preload",
        match site.preload_enabled {
            Some(true) => "enabled - IIS starts the application without waiting for a request",
            Some(false) => "disabled",
            None => "not available in this IIS",
        },
    );
    sections.push(site_section);

    let rows = site
        .bindings
        .iter()
        .map(|b| {
            let (health, detail) = certificate_health(b);
            let host = if b.host.is_empty() { "(any host)".to_string() } else { b.host.clone() };
            let url = match b.port {
                Some(port) if b.protocol == "http" || b.protocol == "https" => {
                    let default = if b.protocol == "https" { 443 } else { 80 };
                    let name = if b.host.is_empty() { "localhost" } else { &b.host };
                    let port = if port == default { String::new() } else { format!(":{port}") };
                    format!("{}://{name}{port}", b.protocol)
                }
                _ => host,
            };
            let certificate = if b.protocol != "https" {
                "-".to_string()
            } else if let Some(cert) = &b.certificate {
                cert.friendly_name.clone().unwrap_or_else(|| cert.subject.clone())
            } else if b.certificate_hash.is_none() {
                "none".into()
            } else {
                "not found".into()
            };
            let cells = vec![
                b.protocol.clone(),
                url,
                b.port.map_or("-".into(), |p| p.to_string()),
                if b.address == "*" || b.address.is_empty() { "all".into() } else { b.address.clone() },
                certificate,
                b.certificate.as_ref().map_or("-".into(), |c| date(c.not_after)),
                if b.protocol != "https" {
                    "-".into()
                } else if b.sni {
                    "yes".into()
                } else {
                    "no".into()
                },
            ];
            TableRow::new(cells, health, detail)
        })
        .collect();
    let mut bindings = Section::new("Bindings", Some("IIS"));
    bindings.table =
        Some(Table::new(&["Protocol", "Host", "Port", "IP address", "Certificate", "Expires", "SNI"], rows));
    if site.bindings.is_empty() {
        bindings.note = some("The site has no bindings.");
    }
    sections.push(bindings);

    if let Some(pool) = &site.pool {
        let mut section = Section::new("Application pool", Some("IIS"));
        section.text("Name", &pool.name);
        section.add(
            "State",
            some(pool.state.as_deref().unwrap_or("unknown")),
            ok_if(pool.state.as_deref().is_some_and(states::is_started), Health::Warning),
            None,
        );
        section.text(".NET CLR", &pool.runtime);
        let classic = pool.pipeline == "Classic";
        section.add(
            "Pipeline mode",
            some(&pool.pipeline),
            if classic { Health::Warning } else { Health::None },
            classic.then(|| "DNN runs in Integrated mode.".to_string()),
        );
        section.text(
            "Identity",
            if pool.identity_type == "SpecificUser" {
                pool.account.clone()
            } else {
                format!("{} - {}", pool.identity_type, pool.account)
            },
        );
        section.text("Start mode", &pool.start_mode);
        section.text(
            "Idle time-out",
            if pool.idle_timeout.is_zero() {
                "none".to_string()
            } else {
                let then = pool.idle_timeout_action.as_ref().map(|a| format!(" - then {a}")).unwrap_or_default();
                format!("{}{then}", span(pool.idle_timeout))
            },
        );
        let mut recycling = if pool.recycle_interval.is_zero() {
            "off".to_string()
        } else {
            format!("every {}", span(pool.recycle_interval))
        };
        if !pool.recycle_at.is_empty() {
            recycling.push_str(&format!(
                "; at {}",
                pool.recycle_at.iter().map(|t| hh_mm(*t)).collect::<Vec<_>>().join(", ")
            ));
        }
        section.text("Regular recycling", recycling);
        section.text(
            "Private memory limit",
            if pool.private_memory_limit_kb == 0 {
                "none".to_string()
            } else {
                byte_size(pool.private_memory_limit_kb as f64 * 1024.0)
            },
        );
        if pool.virtual_memory_limit_kb > 0 {
            section.text("Virtual memory limit", byte_size(pool.virtual_memory_limit_kb as f64 * 1024.0));
        }
        section.text("Queue length", group_thousands(pool.queue_length as f64, 0));
        section.add(
            "32-bit applications",
            some(if pool.enable_32bit { "enabled - 32-bit worker process" } else { "disabled" }),
            if pool.enable_32bit { Health::Warning } else { Health::None },
            None,
        );
        section.text(
            "Rapid-fail protection",
            if pool.rapid_fail_protection {
                format!(
                    "on - stopped after {} crashes in {}",
                    pool.rapid_fail_max_crashes,
                    span(pool.rapid_fail_interval)
                )
            } else {
                "off".to_string()
            },
        );
        section.text("Load user profile", if pool.load_user_profile { "yes" } else { "no" });
        let garden = pool.max_processes > 1;
        section.add(
            "Max worker processes",
            some(pool.max_processes.to_string()),
            if garden { Health::Warning } else { Health::None },
            garden.then(|| "A web garden - DNN's cache and scheduler run in each process.".to_string()),
        );
        sections.push(section);
    }

    let mut runtime = Section::new("Worker processes", Some("Windows"));
    let pids = match site.pool.as_ref().map(|p| &p.worker_process_ids) {
        Some(ids) if !ids.is_empty() => ids.clone(),
        _ => s.project.site.worker_process_ids.clone(),
    };
    if pids.is_empty() {
        runtime.note = some("No worker process - IIS starts one on the site's first request.");
    } else {
        runtime.text("Count", pids.len().to_string());
        runtime.text("PID", pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", "));
        if let Some(ago) = s.project.stats.as_ref().and_then(|st| st.started_ago) {
            let started = LocalDateTime::from_system_time(SystemTime::now() - ago);
            runtime.text("Up since", format!("{} ({})", general(&started), span(ago)));
        }
        runtime.text("Memory", view::memory(&s.project));
        runtime.add("CPU", some(view::cpu(&s.project)), Health::None, some("Of the whole PC, at the last sample."));
    }
    sections.push(runtime);
    sections
}

fn date(at: SystemTime) -> String {
    short_date(&LocalDateTime::from_system_time(at))
}

fn soon() -> SystemTime {
    SystemTime::now() + Duration::from_secs(30 * 86_400)
}

fn certificate_health(b: &IisBindingDetails) -> (Health, Option<String>) {
    if b.protocol != "https" {
        return (Health::None, None);
    }
    let Some(hash) = &b.certificate_hash else { return (Health::Bad, some("https without a certificate")) };
    let Some(cert) = &b.certificate else {
        let store = b.certificate_store.as_deref().unwrap_or_default();
        return (Health::Bad, some(format!("Certificate {hash} isn't in the {store} store")));
    };
    if cert.not_after < SystemTime::now() {
        return (Health::Bad, some(format!("Expired on {}", date(cert.not_after))));
    }
    if cert.not_after < soon() {
        return (Health::Warning, some(format!("Expires on {}", date(cert.not_after))));
    }
    (Health::Ok, some(format!("{}, issued by {}", cert.subject, cert.issuer)))
}

// ─── DNN ─────────────────────────────────────────────────────────────────

pub fn dnn_tab(s: &ProjectSnapshot) -> Vec<Section> {
    let dnn = s.database.as_ref().and_then(|d| d.dnn.as_ref());
    let mut install = Section::new("Installation", Some("bin, web.config and DNN's database"));
    if !s.is_dnn() {
        return vec![install.with_note(r"No DNN here - the site's folder has no bin\DotNetNuke.dll.")];
    }
    let mut sections = Vec::new();
    let product = s.dnn_product.as_ref().map(|p| format!(" - {p}")).unwrap_or_default();
    install.text("DNN version (files)", format!("{}{product}", dnn_version(s)));
    if let Some(db_version) = dnn.and_then(|d| d.version.as_ref()) {
        let same = same_version(s.project.dnn_version.as_deref(), Some(db_version));
        install.add(
            "DNN version (database)",
            Some(db_version.clone()),
            if same { Health::None } else { Health::Bad },
            (!same).then(|| {
                "The files and the database are at different versions - an upgrade didn't run or didn't finish."
                    .to_string()
            }),
        );
    }
    install.add("InstallVersion", s.config.install_version.clone(), Health::None, some("In web.config's appSettings."));
    if let Some(installed) = dnn.and_then(|d| d.installed) {
        install.text("Installed", general(&installed));
    }
    if let (Some(upgraded), Some(first)) = (dnn.and_then(|d| d.upgraded), dnn.and_then(|d| d.installed))
        && upgraded.seconds() > first.seconds() + 5 * 60
    {
        install.text("Last upgrade", general(&upgraded));
    }
    if let Some(record) = &s.record {
        install.text(
            "Set up by",
            if record.automatic {
                format!("DNN Manager, automatic setup - {}", general(&record.created))
            } else {
                format!("DNN Manager, DNN's installation wizard - {}", general(&record.created))
            },
        );
    }
    match dnn {
        Some(dnn) => {
            install.text("Portals", dnn.portals.len().to_string());
            let mode = dnn.host_setting("SchedulerMode");
            install.add(
                "Scheduler",
                mode.map(|m| match m {
                    "0" => "disabled".to_string(),
                    "1" => "timer method".to_string(),
                    "2" => "request method".to_string(),
                    other => other.to_string(),
                }),
                if mode == Some("0") { Health::Warning } else { Health::None },
                dnn.schedule_items.map(|jobs| format!("{jobs} jobs, {} enabled.", dnn.schedule_enabled.unwrap_or(0))),
            );
            let provider = s.config.friendly_url_provider.as_ref();
            install.add(
                "Friendly URLs",
                match dnn.host_setting("UseFriendlyUrls") {
                    Some(friendly) => {
                        let state = if friendly == "Y" { "on" } else { "off" };
                        let with = provider
                            .map(|p| {
                                let format = s
                                    .config
                                    .friendly_url_format
                                    .as_ref()
                                    .map(|f| format!(" ({f})"))
                                    .unwrap_or_default();
                                format!(" - {p}{format}")
                            })
                            .unwrap_or_default();
                        Some(format!("{state}{with}"))
                    }
                    None => provider.cloned(),
                },
                Health::None,
                None,
            );
            let caching = match dnn.host_setting("PerformanceSetting") {
                Some("0") => " - no caching",
                Some("1") => " - light caching",
                Some("2") => " - moderate caching",
                Some("3") => " - heavy caching",
                _ => "",
            };
            install.text("Caching", format!("{}{caching}", s.config.caching_provider.as_deref().unwrap_or("-")));
            install.add(
                "Authentication",
                (!dnn.authentication_providers.is_empty()).then(|| {
                    dnn.authentication_providers
                        .iter()
                        .map(|(kind, enabled)| if *enabled { kind.clone() } else { format!("{kind} (disabled)") })
                        .collect::<Vec<_>>()
                        .join(", ")
                }),
                Health::None,
                None,
            );
            let records = dnn
                .event_log_records
                .map(|r| format!(" - {} records", group_thousands(r as f64, 0)))
                .unwrap_or_default();
            install.text("Event log", format!("{}{records}", s.config.logging_provider.as_deref().unwrap_or("-")));
            install.add(
                "Debug mode",
                match dnn.host_setting("DebugMode") {
                    Some("True") => some("on"),
                    Some("False") => some("off"),
                    _ => None,
                },
                Health::None,
                some("Host setting DebugMode."),
            );
            install.add(
                "Auto-add portal alias",
                match dnn.host_setting("AutoAddPortalAlias") {
                    Some("Y") => some("on"),
                    Some("N") => some("off"),
                    _ => None,
                },
                Health::None,
                None,
            );
        }
        None => install.note = Some(database_note(s)),
    }
    sections.push(install);

    if let Some(dnn) = dnn {
        let site = s.iis.as_ref();
        let rows = dnn
            .portals
            .iter()
            .map(|portal| {
                let missing = aliases_without_binding(portal, site);
                let cells = vec![
                    portal.id.to_string(),
                    format!("{}{}", portal.name, if portal.expired { " (expired)" } else { "" }),
                    portal.primary_alias.as_ref().map_or("none".into(), |a| alias_url(a, site)),
                    if portal.other_aliases.is_empty() {
                        "-".into()
                    } else {
                        portal
                            .other_aliases
                            .iter()
                            .map(|(alias, culture)| match culture.as_deref().filter(|c| !c.is_empty()) {
                                Some(c) => format!("{alias} ({c})"),
                                None => alias.clone(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                    portal.default_language.clone().unwrap_or_else(|| "-".into()),
                    portal.home_directory.clone().unwrap_or_else(|| "-".into()),
                ];
                let health = if !missing.is_empty() || portal.expired { Health::Warning } else { Health::None };
                let detail = (!missing.is_empty()).then(|| format!("No IIS binding for {}", missing.join(", ")));
                TableRow::new(cells, health, detail)
            })
            .collect();
        let mut portals = Section::new("Portals", Some("DNN's database"));
        portals.table =
            Some(Table::new(&["ID", "Name", "Primary alias", "Other aliases", "Language", "Home directory"], rows));
        if dnn.portals.is_empty() {
            portals.note = some("The database has no portals.");
        }
        sections.push(portals);

        let mut extensions = Section::new("Extensions", Some("DNN's database"));
        extensions.add("Modules", dnn.desktop_modules.map(|m| m.to_string()), Health::None, None);
        for (kind, label) in [
            ("Skin", "Themes (skins)"),
            ("Container", "Containers"),
            ("Auth_System", "Authentication systems"),
            ("Provider", "Providers"),
            ("Library", "Libraries"),
            ("JavaScript_Library", "JavaScript libraries"),
            ("CoreLanguagePack", "Language packs (core)"),
            ("ExtensionLanguagePack", "Language packs (extensions)"),
        ] {
            if let Some((_, n)) = dnn.packages.iter().find(|(k, _)| k.eq_ignore_ascii_case(kind)) {
                extensions.text(label, n.to_string());
            }
        }
        extensions.add(
            "Scheduled jobs",
            dnn.schedule_items.map(|total| format!("{total} - {} enabled", dnn.schedule_enabled.unwrap_or(0))),
            Health::None,
            None,
        );
        extensions.add(
            "Packages in all",
            (!dnn.packages.is_empty()).then(|| dnn.packages.values().sum::<i64>().to_string()),
            Health::None,
            None,
        );
        sections.push(extensions);
    }

    let w = &s.config;
    let mut config = Section::new("DNN configuration", Some("web.config"));
    config.add(
        "Compilation debug",
        some(match w.debug {
            Some(true) => "true",
            Some(false) => "false",
            None => "false (not set)",
        }),
        if w.debug == Some(true) { Health::Warning } else { Health::None },
        None,
    );
    config.text("Custom errors", w.custom_errors.clone().unwrap_or_else(|| "RemoteOnly (not set)".into()));
    config.add(
        "Machine key",
        Some(w.machine_key.clone().unwrap_or_else(|| "none - ASP.NET generates keys per machine".into())),
        if w.machine_key.is_none() && s.is_dnn() { Health::Warning } else { Health::None },
        None,
    );
    config.add(
        "Friendly URL provider",
        w.friendly_url_provider.as_ref().map(|fp| {
            let format = w.friendly_url_format.as_ref().map(|f| format!(" - urlFormat {f}")).unwrap_or_default();
            format!("{fp}{format}")
        }),
        Health::None,
        None,
    );
    config.add(
        "Data provider",
        w.data_provider.as_ref().map(|dp| {
            let qualifier = match w.object_qualifier.as_deref().filter(|q| !q.is_empty()) {
                Some(q) => format!("'{q}'"),
                None => "none".into(),
            };
            format!(
                "{dp} - objectQualifier {qualifier}, databaseOwner '{}'",
                w.database_owner.as_deref().unwrap_or_default()
            )
        }),
        Health::None,
        None,
    );
    config.add("Session state", w.session_state.clone(), Health::None, None);
    config.add("Authentication mode", w.authentication_mode.clone(), Health::None, None);
    config.add("Auto upgrade", w.auto_upgrade.clone(), Health::None, some("AutoUpgrade in appSettings."));
    sections.push(config);
    sections
}

// ─── Database ────────────────────────────────────────────────────────────

pub fn database_tab(s: &ProjectSnapshot, issues: &[DetectedIssue]) -> Vec<Section> {
    let p = &s.project;
    let mut connection = Section::new("Connection", Some("SiteSqlServer in web.config"));
    let Some(c) = &s.connection else {
        connection.note = Some(match &p.database_problem {
            Some(why) => format!("The database isn't known: {why}."),
            None => "The site's web.config names no SiteSqlServer database.".into(),
        });
        return vec![connection];
    };
    let w = &s.config;
    let mut sections = Vec::new();
    connection.text("SQL Server", &c.server);
    connection.text(
        "Database",
        if c.kind == DatabaseKind::LocalDbFile {
            format!(r"App_Data\{} (LocalDB file)", c.database)
        } else {
            c.database.clone()
        },
    );
    connection.text("Authentication", authentication(c));
    connection.text("Encrypt", w.sql_encrypt.clone().unwrap_or_else(|| "False (not set)".into()));
    connection.text(
        "Trust server certificate",
        w.sql_trust_server_certificate.map_or("False (not set)".to_string(), |v| bool_text(v).to_string()),
    );
    connection
        .text("Connection timeout", w.sql_connect_timeout.map_or("15 s (not set)".to_string(), |t| format!("{t} s")));
    connection.text(
        "Multiple active result sets",
        w.sql_multiple_active_result_sets.map_or("False (not set)".to_string(), |v| bool_text(v).to_string()),
    );
    if let Some(app) = &w.sql_application_name {
        connection.text("Application name", app);
    }
    let problem = s.database.as_ref().and_then(|d| d.problem.clone());
    if !p.database_is_file {
        let sql = view::sql(p);
        let health = if problem.is_some() || sql == "Offline" || sql == "(none)" {
            Health::Bad
        } else if sql == "Live" {
            Health::Ok
        } else {
            Health::None
        };
        let text = problem.clone().unwrap_or_else(|| view::sql_tip(p).unwrap_or_else(|| sql.to_string()));
        connection.add("Status", Some(text), health, None);
    }
    connection.note = some("Read with this connection - its password is never shown.");
    sections.push(connection);

    if p.database_is_file {
        sections.push(Section::new("Database", None).with_note(
            "A LocalDB file runs in the site's own LocalDB instance - DNN Manager doesn't open it while the site runs.",
        ));
        return sections;
    }
    if !s.database_read {
        sections.push(Section::new("Database", None).with_note("Reading the database…"));
        return sections;
    }
    let Some(inspection) = s.database.as_ref().filter(|d| d.problem.is_none()) else { return sections };

    if let Some(server) = &inspection.server {
        let mut sql = Section::new("SQL Server", Some("SERVERPROPERTY"));
        sql.add("Instance", server.instance.clone(), Health::None, None);
        let level = server.level.as_ref().map(|l| format!(" {l}")).unwrap_or_default();
        let year = sql_year(server.major_version).map(|y| format!(" - SQL Server {y}")).unwrap_or_default();
        sql.text("Version", format!("{}{level}{year}", server.version.as_deref().unwrap_or_default()));
        sql.add("Edition", server.edition.clone(), Health::None, None);
        sql.add("Server collation", server.collation.clone(), Health::None, None);
        sections.push(sql);
    }

    if let Some(db) = &inspection.database {
        let server_collation = inspection.server.as_ref().and_then(|s| s.collation.as_ref());
        let collation_differs =
            matches!((server_collation, &db.collation), (Some(sc), Some(dc)) if !sc.eq_ignore_ascii_case(dc));
        let mut section = Section::new("Database", Some("sys.databases and sys.database_files"));
        section.text("Name", &db.name);
        let online = db.state.as_deref() == Some("ONLINE");
        section.add("State", db.state.clone(), if online { Health::None } else { Health::Bad }, None);
        let read_only = db.read_only == Some(true);
        section.add(
            "Access",
            some(if read_only { "read-only" } else { "read-write" }),
            if read_only { Health::Bad } else { Health::None },
            None,
        );
        let sum = |kind: Option<&str>| {
            db.files.iter().filter(|f| kind.is_none_or(|k| f.kind == k)).map(|f| f.size_mb).sum::<f64>()
        };
        section.add(
            "Size",
            some(format!("{} MB", group_thousands(sum(None), 1))),
            Health::None,
            some(format!(
                "{} MB data, {} MB log",
                group_thousands(sum(Some("ROWS")), 1),
                group_thousands(sum(Some("LOG")), 1)
            )),
        );
        for file in &db.files {
            let label = match file.kind.as_str() {
                "LOG" => "Log file",
                "ROWS" => "Data file",
                other => other,
            };
            let used = file
                .used_mb
                .map(|used| {
                    let free = if file.size_mb > 0.0 { 100.0 * (file.size_mb - used) / file.size_mb } else { 0.0 };
                    format!(", {} MB used ({}% free)", group_thousands(used, 1), group_thousands(free, 0))
                })
                .unwrap_or_default();
            section.add(
                label,
                some(format!("{} MB{used}", group_thousands(file.size_mb, 1))),
                Health::None,
                some(format!(
                    "{} - grows by {}, max {} - {}",
                    file.name, file.growth, file.max_size, file.physical_name
                )),
            );
        }
        section.add("Recovery model", db.recovery_model.clone(), Health::None, None);
        section.add(
            "Compatibility level",
            db.compatibility_level.map(|level| {
                let year = compatibility_year(level).map(|y| format!(" (SQL Server {y})")).unwrap_or_default();
                format!("{level}{year}")
            }),
            if db.compatibility_level.is_some_and(|l| l < 110) { Health::Warning } else { Health::None },
            None,
        );
        section.add(
            "Collation",
            db.collation.clone(),
            if collation_differs { Health::Warning } else { Health::None },
            collation_differs.then(|| {
                format!(
                    "The server's is {} - temporary tables use the server's.",
                    server_collation.cloned().unwrap_or_default()
                )
            }),
        );
        section.add(
            "Auto shrink",
            some(on_off(db.auto_shrink)),
            if db.auto_shrink == Some(true) { Health::Warning } else { Health::None },
            None,
        );
        section.add(
            "Auto close",
            some(on_off(db.auto_close)),
            if db.auto_close == Some(true) { Health::Warning } else { Health::None },
            None,
        );
        section.add(
            "Snapshot isolation",
            db.snapshot_isolation.as_ref().map(|si| {
                let rcsi = if db.read_committed_snapshot == Some(true) { ", read committed snapshot on" } else { "" };
                format!("{si}{rcsi}")
            }),
            Health::None,
            None,
        );
        section.add(
            "Page verification",
            db.page_verify.clone(),
            if db.page_verify.as_deref() == Some("NONE") { Health::Warning } else { Health::None },
            None,
        );
        section.add("Created", db.created.as_ref().map(general), Health::None, None);
        section.add(
            "Signed in as",
            db.signed_in_as.clone(),
            Health::None,
            some("The login web.config's connection signs in with."),
        );
        sections.push(section);
    }

    match &inspection.dnn {
        Some(dnn) => {
            let mut section = Section::new("DNN database", Some("DNN's tables"));
            let same = same_version(p.dnn_version.as_deref(), dnn.version.as_deref());
            section.add("DNN version", dnn.version.clone(), if same { Health::None } else { Health::Bad }, None);
            section.text("Object qualifier", dnn.qualifier.as_ref().map_or("none".to_string(), |q| format!("'{q}'")));
            section.text("Portals", dnn.portals.len().to_string());
            section.add("Users", dnn.users.map(|n| group_thousands(n as f64, 0)), Health::None, None);
            section.add("Roles", dnn.roles.map(|n| group_thousands(n as f64, 0)), Health::None, None);
            section.add(
                "Scheduled jobs",
                dnn.schedule_items.map(|jobs| format!("{jobs} - {} enabled", dnn.schedule_enabled.unwrap_or(0))),
                Health::None,
                None,
            );
            section.add(
                "Event log",
                dnn.event_log_records.map(|r| format!("{} records", group_thousands(r as f64, 0))),
                Health::None,
                dnn.event_log_oldest.as_ref().map(|oldest| {
                    format!("{} - {}", general(oldest), dnn.event_log_newest.as_ref().map(general).unwrap_or_default())
                }),
            );
            sections.push(section);
        }
        None if inspection.notes.is_empty() => {
            sections.push(
                Section::new("DNN database", None).with_note("The database has no DNN tables (no Version table)."),
            );
        }
        None => {}
    }

    let mut health = Section::new("Database health", None);
    let db_issues: Vec<&DetectedIssue> = issues.iter().filter(|i| i.area == "Database").collect();
    if db_issues.is_empty() {
        health.add("Issues", some("none detected"), Health::Ok, None);
    } else {
        health.table = Some(Table::new(
            &["Issue"],
            db_issues.iter().map(|i| TableRow::new(vec![i.text.clone()], i.health, None)).collect(),
        ));
    }
    if !inspection.notes.is_empty() {
        health.note = Some(inspection.notes.join(" "));
    }
    sections.push(health);
    sections
}

// ─── Advanced ────────────────────────────────────────────────────────────

/// The upload limits: ASP.NET's, IIS's, DNN's host setting (None when not set), the smallest of them, and whether
/// they differ.
fn upload_limits(s: &ProjectSnapshot) -> (u64, u64, Option<u64>, u64, bool) {
    let w = &s.config;
    let asp_net = w.max_request_length_kb.unwrap_or(DEFAULT_MAX_REQUEST_LENGTH_KB) * 1024;
    let iis = w.max_allowed_content_length.unwrap_or(DEFAULT_MAX_ALLOWED_CONTENT_LENGTH);
    let dnn = s
        .database
        .as_ref()
        .and_then(|d| d.dnn.as_ref())
        .and_then(|d| d.host_setting("MaxUploadSize"))
        .and_then(|v| v.trim().parse::<u64>().ok());
    let effective = asp_net.min(iis).min(dnn.unwrap_or(u64::MAX));
    let mut all = vec![asp_net, iis];
    all.extend(dnn);
    all.sort_unstable();
    all.dedup();
    (asp_net, iis, dnn, effective, all.len() > 1)
}

pub fn advanced_tab(s: &ProjectSnapshot, issues: &[DetectedIssue]) -> Vec<Section> {
    let w = &s.config;
    let mut sections = Vec::new();
    let path = w.path.display().to_string();
    let mut config = Section::new("Configuration", Some(&path));
    if let Some(problem) = &w.problem {
        config.add("web.config", Some(problem.clone()), Health::Bad, None);
    } else {
        config.add(
            "Target framework",
            (w.runtime_target_framework.is_some() || w.compilation_target_framework.is_some()).then(|| {
                format!(
                    "httpRuntime {}, compilation {}",
                    w.runtime_target_framework.as_deref().unwrap_or("-"),
                    w.compilation_target_framework.as_deref().unwrap_or("-")
                )
            }),
            Health::None,
            None,
        );
        config.add("Request validation mode", w.request_validation_mode.clone(), Health::None, None);
        config.text(
            "Request filtering",
            if w.max_url.is_none() && w.max_query_string.is_none() {
                "IIS defaults (URL 4096, query string 2048)".to_string()
            } else {
                format!(
                    "max URL {}, max query string {}",
                    w.max_url.map_or("4096".into(), |v| v.to_string()),
                    w.max_query_string.map_or("2048".into(), |v| v.to_string())
                )
            },
        );
        config.text(
            "Compression",
            if w.static_compression.is_none() && w.dynamic_compression.is_none() {
                "IIS defaults (static on, dynamic off)".to_string()
            } else {
                format!(
                    "static {}, dynamic {}",
                    on_off(Some(w.static_compression.unwrap_or(true))),
                    on_off(Some(w.dynamic_compression.unwrap_or(false)))
                )
            },
        );
        config.text("Static content cache", w.client_cache.clone().unwrap_or_else(|| "not set".into()));
        config.text("HTTP errors", w.http_errors.clone().unwrap_or_else(|| "DetailedLocalOnly (not set)".into()));
        config.add(
            "All managed modules for all requests",
            w.run_all_managed_modules.map(|v| bool_text(v).to_string()),
            Health::None,
            None,
        );
        config.text("URL rewrite rules", if w.rewrite_rules > 0 { w.rewrite_rules.to_string() } else { "none".into() });
        if !w.disabled_https_rules.is_empty() {
            config.add(
                "HTTPS redirects",
                some(format!("switched off for local use: {}", w.disabled_https_rules.join(", "))),
                Health::Warning,
                some("Switch them back on before deploying."),
            );
        }
        config.text("Binding redirects", w.binding_redirects.len().to_string());
        if !w.code_bases.is_empty() {
            config.add(
                "Code bases",
                some(w.code_bases.len().to_string()),
                Health::None,
                some(r"Assemblies loaded from a file of their own (bin\Imageflow, say)."),
            );
        }
        config.add("Probing path", w.probing_path.clone(), Health::None, None);
        if !w.external_sections.is_empty() {
            config.add(
                "In files of their own",
                some(w.external_sections.join(", ")),
                Health::None,
                some("configSource - not read here."),
            );
        }
    }
    sections.push(config);

    let mut limits = Section::new("Limits", Some("web.config and DNN's host settings"));
    let (asp_net, iis, dnn_limit, effective, conflict) = upload_limits(s);
    if let Some(limit) = dnn_limit {
        limits.add(
            "DNN upload limit",
            some(byte_size(limit as f64)),
            Health::None,
            some("Host setting MaxUploadSize."),
        );
    }
    let default = |set: bool| if set { "" } else { " (default)" };
    limits.add(
        "ASP.NET maxRequestLength",
        some(format!("{}{}", byte_size(asp_net as f64), default(w.max_request_length_kb.is_some()))),
        if conflict && asp_net == effective { Health::Warning } else { Health::None },
        None,
    );
    limits.add(
        "IIS maxAllowedContentLength",
        some(format!("{}{}", byte_size(iis as f64), default(w.max_allowed_content_length.is_some()))),
        if conflict && iis == effective { Health::Warning } else { Health::None },
        None,
    );
    limits.add(
        "Effective upload limit",
        some(byte_size(effective as f64)),
        if conflict { Health::Warning } else { Health::Ok },
        conflict.then(|| "The limits differ - the smallest one wins.".to_string()),
    );
    limits.add(
        "Execution timeout",
        some(format!(
            "{} s{}",
            w.execution_timeout_seconds.unwrap_or(DEFAULT_EXECUTION_TIMEOUT_SECONDS),
            default(w.execution_timeout_seconds.is_some())
        )),
        Health::None,
        (w.debug == Some(true)).then(|| "Not applied while compilation debug is true.".to_string()),
    );
    sections.push(limits);

    let account = s.iis.as_ref().and_then(|i| i.pool.as_ref()).map(|p| p.account.clone());
    let source = account.as_ref().map_or("folder permissions".to_string(), |a| format!("permissions for {a}"));
    let mut filesystem = Section::new("Filesystem", Some(&source));
    let rows = s
        .folders
        .iter()
        .map(|f| {
            let writable = if !f.exists {
                "-".to_string()
            } else {
                match f.writable {
                    Some(true) => format!("yes - {}", f.via.as_deref().unwrap_or_default()),
                    Some(false) => format!("no - {}", f.via.as_deref().unwrap_or_default()),
                    None => f.via.clone().unwrap_or_else(|| "unknown".into()),
                }
            };
            let cells = vec![
                f.folder.clone(),
                if f.exists { "yes".into() } else { "no".into() },
                writable,
                if !f.exists {
                    "-".into()
                } else if f.read_only {
                    "yes".into()
                } else {
                    "no".into()
                },
            ];
            let health = if (!f.exists && f.required) || (f.exists && f.needs_write && f.writable == Some(false)) {
                Health::Bad
            } else if f.read_only {
                Health::Warning
            } else {
                Health::None
            };
            TableRow::new(cells, health, None)
        })
        .collect();
    let writable = if account.is_none() { "Writable" } else { "Writable by the app pool" };
    filesystem.table = Some(Table::new(&["Folder", "Exists", writable, "Read-only"], rows));
    if account.is_none() {
        filesystem.note =
            some("The app pool's account isn't known (IIS couldn't be read) - only whether the folders are there.");
    }
    sections.push(filesystem);

    sections.push(assemblies_section(s));

    let mut all = Section::new("Detected issues", Some("everything above but the assemblies"));
    if issues.is_empty() {
        all.add("Issues", some("none detected"), Health::Ok, None);
    } else {
        all.table = Some(Table::new(
            &["Area", "Issue"],
            issues.iter().map(|i| TableRow::new(vec![i.area.clone(), i.text.clone()], i.health, None)).collect(),
        ));
    }
    if !s.database_read && s.connection.is_some() {
        all.note = some("The database is still being read - its issues follow.");
    }
    sections.push(all);
    sections
}

fn assemblies_section(s: &ProjectSnapshot) -> Section {
    let a = &s.assemblies;
    let mut section = Section::new("Assemblies", Some("bin and its folders, the probing path and codeBase"));
    // bin first, then its folders: "bin 186, bin\2sxc 1, bin\Imageflow 12".
    let mut folders: Vec<(String, usize, usize)> = Vec::new(); // folder, count, not loaded
    for x in &a.assemblies {
        let folder = x.folder().to_string();
        match folders.iter_mut().find(|(f, _, _)| f.eq_ignore_ascii_case(&folder)) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 += usize::from(!x.loaded);
            }
            None => folders.push((folder, 1, usize::from(!x.loaded))),
        }
    }
    folders.sort_by_key(|(f, _, _)| (!f.eq_ignore_ascii_case("bin"), f.to_lowercase()));
    let native = if a.native_files > 0 { format!(" (+ {} native DLLs)", a.native_files) } else { String::new() };
    section.add(
        "Assemblies",
        some(format!("{}{native}", a.assemblies.len())),
        Health::None,
        (folders.len() > 1).then(|| folders.iter().map(|(f, n, _)| format!("{f} {n}")).collect::<Vec<_>>().join(", ")),
    );
    let own: Vec<&(String, usize, usize)> = folders.iter().filter(|(_, _, n)| *n > 0).collect();
    if !own.is_empty() {
        let total: usize = own.iter().map(|(_, _, n)| n).sum();
        section.add(
            "Not loaded by ASP.NET",
            some(format!("{total} - {}", own.iter().map(|(f, _, n)| format!("{f} {n}")).collect::<Vec<_>>().join(", "))),
            Health::None,
            some(r"In a folder .NET doesn't look in, and no codeBase in web.config points at them - only what uses that folder loads them (a module, or the compiler in bin\roslyn)."),
        );
    }
    // DNN's own (DotNetNuke.*) - modules and libraries (Dnn.Modules.*) have versions of their own.
    let dnn: Vec<_> =
        a.assemblies.iter().filter(|x| x.loaded && x.name.to_lowercase().starts_with("dotnetnuke")).collect();
    if let Some(core) = a.assemblies.iter().find(|x| x.loaded && x.name.eq_ignore_ascii_case("DotNetNuke")) {
        let others: Vec<String> = dnn
            .iter()
            .filter(|x| x.version.0 != core.version.0 || x.version.1 != core.version.1)
            .map(|o| format!("{} {}", o.name, o.version))
            .collect();
        section.add(
            "DNN assemblies",
            some(format!("{} DotNetNuke.* - DotNetNuke.dll {}", dnn.len(), core.version)),
            Health::None,
            some(if others.is_empty() {
                "All at the same version.".to_string()
            } else {
                format!("At another version: {}", others.join(", "))
            }),
        );
    }
    if a.problems.is_empty() {
        section.add("Conflicts", some("none detected"), Health::Ok, None);
    } else {
        section.table = Some(Table::new(
            &["Kind", "Detected"],
            a.problems
                .iter()
                .map(|x| TableRow::new(vec![kind_text(&x.kind).to_string(), x.text.clone()], assembly_health(x), None))
                .collect(),
        ));
    }
    section
}

// ─── Detected issues ─────────────────────────────────────────────────────

/// What is wrong or unusual - each from a detected value, which it names; problems first.
pub fn issues(s: &ProjectSnapshot) -> Vec<DetectedIssue> {
    let mut list = Vec::new();
    let mut add =
        |health: Health, area: &str, text: String| list.push(DetectedIssue { health, area: area.into(), text });
    let p = &s.project;
    let w = &s.config;

    if let Some(site) = &s.iis {
        if !site.physical_path.is_dir() {
            add(Health::Bad, "IIS", format!("The physical path {} doesn't exist.", site.physical_path.display()));
        }
        match &site.pool {
            None => add(Health::Bad, "IIS", "The site's application pool isn't in IIS.".into()),
            Some(pool) => {
                if let Some(state) =
                    pool.state.as_ref().filter(|st| states::is_started(&site.state) && !states::is_started(st))
                {
                    add(
                        Health::Bad,
                        "IIS",
                        format!("The site is started, but its application pool '{}' is {state}.", pool.name),
                    );
                }
                if pool.enable_32bit {
                    add(
                        Health::Warning,
                        "IIS",
                        format!(
                            "Application pool '{}' runs 32-bit worker processes (enable32BitAppOnWin64).",
                            pool.name
                        ),
                    );
                }
                if pool.pipeline == "Classic" {
                    add(
                        Health::Warning,
                        "IIS",
                        format!(
                            "Application pool '{}' uses the Classic pipeline - DNN runs in Integrated mode.",
                            pool.name
                        ),
                    );
                }
                if s.is_dnn() && pool.runtime != "v4.0" {
                    add(
                        Health::Bad,
                        "IIS",
                        format!("Application pool '{}' runs .NET CLR {} - DNN needs v4.0.", pool.name, pool.runtime),
                    );
                }
                if pool.max_processes > 1 {
                    add(
                        Health::Warning,
                        "IIS",
                        format!(
                            "Application pool '{}' is a web garden ({} worker processes).",
                            pool.name, pool.max_processes
                        ),
                    );
                }
            }
        }
        for b in site.bindings.iter().filter(|b| b.protocol == "https") {
            let host = if b.host.is_empty() { "(any host)" } else { &b.host };
            let port = b.port.map(|p| p.to_string()).unwrap_or_default();
            match (&b.certificate_hash, &b.certificate) {
                (None, _) => add(Health::Bad, "IIS", format!("https binding {host}:{port} has no certificate.")),
                (Some(hash), None) => add(
                    Health::Bad,
                    "IIS",
                    format!(
                        "https binding {host}:{port} uses certificate {hash}, which isn't in the {} store.",
                        b.certificate_store.as_deref().unwrap_or_default()
                    ),
                ),
                (Some(_), Some(cert)) if cert.not_after < SystemTime::now() => add(
                    Health::Bad,
                    "IIS",
                    format!(
                        "https binding {host}:{port} uses an expired certificate ({}, expired {}).",
                        cert.subject,
                        date(cert.not_after)
                    ),
                ),
                (Some(_), Some(cert)) if cert.not_after < soon() => add(
                    Health::Warning,
                    "IIS",
                    format!("The certificate of https binding {host}:{port} expires on {}.", date(cert.not_after)),
                ),
                _ => {}
            }
        }
    }

    // Portals against the bindings
    if let (Some(dnn), Some(site)) = (s.database.as_ref().and_then(|d| d.dnn.as_ref()), &s.iis) {
        for portal in &dnn.portals {
            for alias in aliases_without_binding(portal, Some(site)) {
                add(
                    Health::Warning,
                    "Portals",
                    format!(
                        "Portal alias {alias} (portal {}) has no IIS binding - its requests don't reach this site.",
                        portal.id
                    ),
                );
            }
        }
        let auto_add = dnn.host_setting("AutoAddPortalAlias") == Some("Y");
        let alias_hosts: Vec<String> = dnn
            .portals
            .iter()
            .flat_map(|x| {
                x.other_aliases
                    .iter()
                    .map(|(a, _)| a.as_str())
                    .chain(std::iter::once(x.primary_alias.as_deref().unwrap_or_default()))
            })
            .map(|a| alias_host(a).to_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        if !auto_add {
            for b in site.bindings.iter().filter(|b| {
                (b.protocol == "http" || b.protocol == "https")
                    && !b.host.is_empty()
                    && !alias_hosts.contains(&b.host.to_lowercase())
            }) {
                add(
                    Health::Warning,
                    "Portals",
                    format!(
                        "IIS binding {}://{} has no portal alias - DNN redirects its requests to a portal's alias (AutoAddPortalAlias is off).",
                        b.protocol, b.host
                    ),
                );
            }
        }
    }

    // web.config
    if let Some(problem) = &w.problem {
        add(Health::Bad, "web.config", format!("web.config {problem}."));
    } else {
        if s.is_dnn() && !w.has_dnn_section {
            add(
                Health::Bad,
                "web.config",
                r"bin\DotNetNuke.dll is there, but web.config has no dotnetnuke section.".into(),
            );
        }
        if w.debug == Some(true) {
            add(
                Health::Warning,
                "web.config",
                "compilation debug=\"true\" - pages are compiled for debugging, and executionTimeout doesn't apply."
                    .into(),
            );
        }
        if s.is_dnn() && w.machine_key.is_none() {
            add(
                Health::Warning,
                "web.config",
                "No machineKey - ASP.NET generates keys per machine; sign-ins and DNN's encrypted values break when they change.".into(),
            );
        }
        let asp_net = w.max_request_length_kb.unwrap_or(DEFAULT_MAX_REQUEST_LENGTH_KB) * 1024;
        let iis = w.max_allowed_content_length.unwrap_or(DEFAULT_MAX_ALLOWED_CONTENT_LENGTH);
        if asp_net.abs_diff(iis) > asp_net.max(iis) / 100 {
            add(
                Health::Warning,
                "Limits",
                format!(
                    "Upload limits differ: ASP.NET maxRequestLength {}, IIS maxAllowedContentLength {} - the effective limit is {}.",
                    byte_size(asp_net as f64),
                    byte_size(iis as f64),
                    byte_size(asp_net.min(iis) as f64)
                ),
            );
        }
    }

    // Database
    if s.connection.is_some() && !p.database_is_file {
        if let Some(why) = s.database.as_ref().and_then(|d| d.problem.as_ref()) {
            add(Health::Bad, "Database", format!("The SQL Server doesn't answer web.config's connection: {why}"));
        } else if p.sql_reachable == Some(true) && !p.database_exists {
            add(
                Health::Bad,
                "Database",
                format!(
                    "Database [{}] from web.config doesn't exist on {}.",
                    p.database_name.as_deref().unwrap_or_default(),
                    p.database_server.as_deref().unwrap_or_default()
                ),
            );
        }
    }
    if let Some(inspection) = s.database.as_ref().filter(|d| d.problem.is_none()) {
        if let Some(db_version) = inspection.dnn.as_ref().and_then(|d| d.version.as_deref())
            && s.is_dnn()
            && !same_version(p.dnn_version.as_deref(), Some(db_version))
        {
            add(
                Health::Bad,
                "Database",
                format!(
                    "DNN's files are {}, its database {db_version} - an upgrade didn't run or didn't finish.",
                    dnn_version(s)
                ),
            );
        }
        if let Some(db) = &inspection.database {
            if let Some(state) = db.state.as_ref().filter(|st| *st != "ONLINE") {
                add(Health::Bad, "Database", format!("The database is {state}."));
            }
            if db.read_only == Some(true) {
                add(Health::Bad, "Database", "The database is read-only.".into());
            }
            if let Some(level) = db.compatibility_level.filter(|l| *l < 110) {
                add(
                    Health::Warning,
                    "Database",
                    format!(
                        "Compatibility level {level} is unusually old (SQL Server {}).",
                        compatibility_year(level).unwrap_or("2008")
                    ),
                );
            }
            if let (Some(sc), Some(dc)) = (inspection.server.as_ref().and_then(|s| s.collation.as_ref()), &db.collation)
                && !sc.eq_ignore_ascii_case(dc)
            {
                add(
                    Health::Warning,
                    "Database",
                    format!(
                        "The database's collation {dc} differs from the server's {sc} - queries joining temporary tables can fail (\"Cannot resolve the collation conflict\")."
                    ),
                );
            }
            if db.auto_close == Some(true) {
                add(
                    Health::Warning,
                    "Database",
                    "AUTO_CLOSE is on - the database closes after each use and opens slowly again.".into(),
                );
            }
            if db.auto_shrink == Some(true) {
                add(Health::Warning, "Database", "AUTO_SHRINK is on - it fragments the database.".into());
            }
            let data: f64 = db.files.iter().filter(|f| f.kind == "ROWS").map(|f| f.size_mb).sum();
            for log in db.files.iter().filter(|f| f.kind == "LOG" && f.size_mb > 1024.0 && f.size_mb > 2.0 * data) {
                let full = if db.recovery_model.as_deref() == Some("FULL") {
                    ", in FULL recovery without log backups?"
                } else {
                    "."
                };
                add(
                    Health::Warning,
                    "Database",
                    format!(
                        "The transaction log {} is {} MB - over twice the data ({} MB){full}",
                        log.name,
                        group_thousands(log.size_mb, 0),
                        group_thousands(data, 0)
                    ),
                );
            }
            for file in &db.files {
                if let Some(used) = file.used_mb
                    && file.size_mb > 0.0
                    && used / file.size_mb > 0.9
                    && file.growth == "none"
                {
                    add(
                        Health::Bad,
                        "Database",
                        format!(
                            "File {} is {}% full and can't grow.",
                            file.name,
                            group_thousands(100.0 * used / file.size_mb, 0)
                        ),
                    );
                }
            }
        }
    }

    // Folders
    let account = s.iis.as_ref().and_then(|i| i.pool.as_ref()).map_or("The app pool", |p| p.account.as_str());
    for f in &s.folders {
        if !f.exists && f.required {
            add(Health::Bad, "Filesystem", format!("{} is missing.", f.folder));
        } else if f.exists && f.needs_write && f.writable == Some(false) {
            add(
                Health::Bad,
                "Filesystem",
                format!("{account} can't write to {} ({}).", f.folder, f.via.as_deref().unwrap_or_default()),
            );
        } else if f.exists && f.read_only {
            add(Health::Warning, "Filesystem", format!("{} is read-only.", f.folder));
        }
    }

    // Not the assemblies' problems: Assemblies on Advanced lists them, and Project health counts them.
    list.sort_by_key(|i| i.health != Health::Bad);
    list
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn aliases_without_binding(portal: &DnnPortalDetails, site: Option<&IisSiteDetails>) -> Vec<String> {
    let Some(site) = site else { return Vec::new() };
    portal
        .primary_alias
        .iter()
        .chain(portal.other_aliases.iter().map(|(a, _)| a))
        .filter(|alias| {
            let host = alias_host(alias);
            let before_path = alias.split('/').next().unwrap_or_default();
            let parts: Vec<&str> = before_path.split(':').collect();
            let port: Option<u16> = if parts.len() == 2 { parts[1].parse().ok() } else { None };
            !site.bindings.iter().any(|b| {
                (b.protocol == "http" || b.protocol == "https")
                    && (b.host.is_empty() || b.host.eq_ignore_ascii_case(host))
                    && match port {
                        None => matches!(b.port, Some(80 | 443)),
                        Some(port) => b.port == Some(port),
                    }
            })
        })
        .cloned()
        .collect()
}

/// "dzp.dnndev.me:8080/child" → "dzp.dnndev.me".
fn alias_host(alias: &str) -> &str {
    alias.split('/').next().unwrap_or_default().split(':').next().unwrap_or_default()
}

/// The alias as an address: https when the site has an https binding for its host.
fn alias_url(alias: &str, site: Option<&IisSiteDetails>) -> String {
    let host = alias_host(alias);
    let https = site.is_some_and(|s| {
        s.bindings.iter().any(|b| b.protocol == "https" && (b.host.is_empty() || b.host.eq_ignore_ascii_case(host)))
    });
    format!("{}://{alias}", if https { "https" } else { "http" })
}

fn project_type(s: &ProjectSnapshot) -> &'static str {
    if s.is_dnn() {
        return if s.config.has_dnn_section { "DNN site" } else { "DNN files without DNN's web.config" };
    }
    if s.config.path.is_file() { "ASP.NET site (no DNN)" } else { "static site (no web.config)" }
}

fn database_note(s: &ProjectSnapshot) -> String {
    if s.connection.is_none() {
        format!(
            "The database isn't known: {}.",
            s.project.database_problem.as_deref().unwrap_or("web.config names none")
        )
    } else if s.project.database_is_file {
        "The database is a LocalDB file the site runs itself - not read while the site runs.".into()
    } else if !s.database_read {
        "Reading the database…".into()
    } else if let Some(problem) = s.database.as_ref().and_then(|d| d.problem.as_ref()) {
        format!("The database couldn't be read: {problem}")
    } else {
        "The database has no DNN tables.".into()
    }
}

fn authentication(c: &DatabaseConnection) -> String {
    if c.windows_authentication {
        "Windows - the site signs in as its app pool's account".into()
    } else {
        format!("SQL Server login '{}'", c.user)
    }
}

/// .NET's Version.TryParse: two to four numbers; the build is -1 when left out.
fn parse_version(text: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<i64> = text.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    if !(2..=4).contains(&parts.len()) || parts.iter().any(|p| *p < 0) {
        return None;
    }
    Some((parts[0], parts[1], parts.get(2).copied().unwrap_or(-1)))
}

/// The files' and the database's DNN versions are the same release (major, minor, build); unknown counts as same.
pub fn same_version(files: Option<&str>, database: Option<&str>) -> bool {
    let (Some(files), Some(database)) = (files, database) else { return true };
    match (parse_version(files), parse_version(database)) {
        (Some(a), Some(b)) => a.0 == b.0 && a.1 == b.1 && a.2.max(0) == b.2.max(0),
        _ => files == database,
    }
}

fn sql_year(major: Option<i32>) -> Option<&'static str> {
    Some(match major? {
        17 => "2025",
        16 => "2022",
        15 => "2019",
        14 => "2017",
        13 => "2016",
        12 => "2014",
        11 => "2012",
        10 => "2008",
        _ => return None,
    })
}

fn compatibility_year(level: i32) -> Option<&'static str> {
    Some(match level {
        170 => "2025",
        160 => "2022",
        150 => "2019",
        140 => "2017",
        130 => "2016",
        120 => "2014",
        110 => "2012",
        100 => "2008",
        _ => return None,
    })
}

/// A binding redirect or codeBase to a file that isn't there: that assembly can't load. The rest may not matter.
fn assembly_health(problem: &AssemblyProblem) -> Health {
    if problem.kind == "redirect" || problem.kind == "codebase" { Health::Bad } else { Health::Warning }
}

/// "1 problem, 3 warnings" - and the worst of them; "none" and Ok when there are none.
fn tally(healths: impl Iterator<Item = Health>) -> (String, Health) {
    let list: Vec<Health> = healths.collect();
    let bad = list.iter().filter(|h| **h == Health::Bad).count();
    let warn = list.iter().filter(|h| **h == Health::Warning).count();
    if bad == 0 && warn == 0 {
        return ("none".into(), Health::Ok);
    }
    let parts: Vec<String> = [(bad, "problem"), (warn, "warning")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, one)| count(*n, one, None))
        .collect();
    (parts.join(", "), if bad > 0 { Health::Bad } else { Health::Warning })
}

fn kind_text(kind: &str) -> &str {
    match kind {
        "duplicate" => "Duplicate",
        "redirect" => "Binding redirect",
        "codebase" => "Code base",
        "version" => "Version conflict",
        "missing" => "Missing",
        other => other,
    }
}
