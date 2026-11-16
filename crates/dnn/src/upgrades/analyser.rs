//! The pre-upgrade analyser's rules (`src/DnnManager.Application/Upgrades/DnnUpgradeAnalyser.cs`): what was read about a
//! site against DNN's upgrade path - every step's requirements, what it removes, and what the site holds that it may
//! break. Each finding is Compatible, Warning, Blocking or Unknown; a blocking one keeps the upgrade from starting.
//! Pure: the facts are read elsewhere.

use std::fmt;

use dnnm_infrastructure::web_config::DatabaseKind;

use super::facts::{DnnAssembly, DnnSiteFacts, net_framework_name, sql_server_name};
use super::path::{DnnRequirementKind, DnnUpgradeMethod, DnnUpgradeStep, UpgradeStop, chain};
use crate::format::mb;
use crate::version::{Version, number};

/// How a finding weighs on the upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpgradeFindingSeverity {
    /// Checked and fine.
    Compatible,
    /// Doesn't stop the upgrade, but needs a look - before or after it.
    Warning,
    /// The upgrade doesn't start while this is so.
    Blocking,
    /// Couldn't be checked - said, so nobody counts on it.
    Unknown,
}

impl fmt::Display for UpgradeFindingSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Something the analyser found: what about (`area`), what, why it matters, and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeFinding {
    pub severity: UpgradeFindingSeverity,
    pub area: String,
    pub title: String,
    pub detail: Option<String>,
    pub fix: Option<String>,
}

impl UpgradeFinding {
    pub fn new(severity: UpgradeFindingSeverity, area: &str, title: impl Into<String>) -> Self {
        Self { severity, area: area.into(), title: title.into(), detail: None, fix: None }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    fn with_optional_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

impl fmt::Display for UpgradeFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} - {}", self.severity, self.area, self.title)?;
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

/// A step of the plan with what was checked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    pub step: DnnUpgradeStep,
    pub checks: Vec<UpgradeFinding>,
}

/// The upgrade plan: the site's version, the target, the steps between them each with its checks, and what was found
/// about the site as a whole. It starts only when nothing is blocking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnUpgradePlan {
    pub current: Version,
    pub target: Version,
    pub steps: Vec<PlannedStep>,
    pub site: Vec<UpgradeFinding>,
}

impl DnnUpgradePlan {
    pub fn all(&self) -> impl Iterator<Item = &UpgradeFinding> {
        self.site.iter().chain(self.steps.iter().flat_map(|s| &s.checks))
    }

    pub fn can_start(&self) -> bool {
        !self.steps.is_empty() && self.all().all(|f| f.severity != UpgradeFindingSeverity::Blocking)
    }

    pub fn blocking(&self) -> Vec<&UpgradeFinding> {
        self.all().filter(|f| f.severity == UpgradeFindingSeverity::Blocking).collect()
    }

    /// The plan as text - for the Output tab and each stage's backup folder.
    pub fn describe(&self) -> String {
        fn line(f: &UpgradeFinding) -> String {
            let mut text = format!("[{}] {}: {}", f.severity, f.area, f.title);
            if let Some(detail) = &f.detail {
                text.push_str(&format!(" - {detail}"));
            }
            if let Some(fix) = &f.fix {
                text.push_str(&format!(" → {fix}"));
            }
            text
        }
        let mut lines = vec![
            format!("Current: DNN {}", DnnUpgradeStep::name(&self.current)),
            format!("Target:  DNN {}", DnnUpgradeStep::name(&self.target)),
            String::new(),
            "Required path:".to_string(),
        ];
        lines.extend(self.steps.iter().enumerate().map(|(i, s)| format!("  {}. {}", i + 1, s.step)));
        lines.push(String::new());
        lines.push("The site:".into());
        lines.extend(self.site.iter().map(|f| format!("  {}", line(f))));
        for (i, step) in self.steps.iter().enumerate() {
            lines.push(String::new());
            lines.push(format!("Step {} - {}", i + 1, step.step));
            lines.extend(step.checks.iter().map(|f| format!("  {}", line(f))));
        }
        lines.join("\r\n")
    }
}

/// DNN 9.2 removed the APIs deprecated in DNN 7 and earlier.
const REMOVED_APIS: Version = Version::new(9, 2, 0);
const TEN: Version = Version::new(10, 0, 0);

/// The upgrade plan of a site to `target`. `added`: the path's edits made in Settings.
pub fn plan(facts: &DnnSiteFacts, target: Version, added: &[UpgradeStop]) -> DnnUpgradePlan {
    use UpgradeFindingSeverity::*;
    let mut site = Vec::new();
    let Some(current) = facts.files_version.as_deref().and_then(number) else {
        site.push(
            UpgradeFinding::new(Blocking, "DNN", "No DNN version found in the site's files")
                .with_detail(r"bin\DotNetNuke.dll is missing or can't be read."),
        );
        return DnnUpgradePlan { current: Version::new(0, 0, 0), target, steps: Vec::new(), site };
    };
    let steps = chain(current, target, added);
    if steps.is_empty() {
        site.push(UpgradeFinding::new(
            Blocking,
            "DNN",
            format!(
                "DNN {} isn't newer than the site's DNN {}",
                DnnUpgradeStep::name(&target),
                DnnUpgradeStep::name(&current)
            ),
        ));
    }
    site_checks(facts, current, steps.len(), &mut site);
    let steps = steps.into_iter().map(|step| PlannedStep { checks: step_checks(facts, &step), step }).collect();
    DnnUpgradePlan { current, target: target.normalize(), steps, site }
}

fn site_checks(f: &DnnSiteFacts, current: Version, steps: usize, found: &mut Vec<UpgradeFinding>) {
    use UpgradeFindingSeverity::*;
    // DNN's version in its files and in its database must agree: otherwise an earlier upgrade didn't finish.
    match f.database_version.as_deref() {
        None => found.push(
            UpgradeFinding::new(Unknown, "DNN", "The database's DNN version couldn't be read")
                .with_optional_detail(f.database_problem.clone()),
        ),
        Some(db) => match number(db) {
            None => found.push(
                UpgradeFinding::new(Unknown, "DNN", format!("The database's DNN version ({db}) isn't a version number"))
                    .with_detail("Whether files and database agree can't be told."),
            ),
            Some(number) if number.normalize() != current => found.push(
                UpgradeFinding::new(
                    Blocking,
                    "DNN",
                    format!(
                        "The files are at DNN {}, the database at {db}",
                        f.files_version.as_deref().unwrap_or_default()
                    ),
                )
                .with_detail("An earlier upgrade didn't run or didn't finish.")
                .with_fix("Undo it first: Restore backup (the project's right-click menu) puts back the backup made before it (\"Before upgrading DNN …\"), then upgrade again."),
            ),
            Some(_) => found.push(UpgradeFinding::new(
                Compatible,
                "DNN",
                format!("DNN {} - files and database agree", f.files_version.as_deref().unwrap_or_default()),
            )),
        },
    }
    if f.install_blocked {
        found.push(
            UpgradeFinding::new(Warning, "DNN", "installBlocker.lock is there")
                .with_detail("An installation or upgrade is running, or one was cut off. DNN Manager removes it before upgrading.")
                .with_fix("Make sure nobody runs DNN's installer or upgrade wizard meanwhile."),
        );
    }

    // The database.
    match f.database_kind {
        None => {
            found.push(UpgradeFinding::new(Blocking, "Database", "The site's web.config names no database of its own"))
        }
        Some(DatabaseKind::LocalDbFile) => found.push(
            UpgradeFinding::new(Blocking, "Database", "The database is a LocalDB file")
                .with_detail("It can't be backed up as .bacpac, so there would be no way back.")
                .with_fix("Move it to SQL Server first (Details → Database → Change connection…)."),
        ),
        Some(_) if f.database_problem.is_some() => found.push(
            UpgradeFinding::new(Blocking, "Database", "The database can't be read")
                .with_optional_detail(f.database_problem.clone())
                .with_fix("Check that the SQL Server is running and the site's login may sign in."),
        ),
        Some(_) => {
            let name = f.database_name.as_deref().unwrap_or_default();
            found.push(match f.database_state.as_deref() {
                None | Some("ONLINE") => UpgradeFinding::new(
                    Compatible,
                    "Database",
                    format!(
                        "[{name}] on {} {} ({})",
                        f.sql_server_major().map_or_else(|| "SQL Server".to_string(), sql_server_name),
                        f.sql_server_version.as_deref().unwrap_or_default(),
                        f.sql_server_edition.as_deref().unwrap_or_default()
                    ),
                ),
                Some(state) => UpgradeFinding::new(Blocking, "Database", format!("[{name}] is {state}"))
                    .with_detail("Only an ONLINE database can be upgraded."),
            });
            if let Some(counts) = &f.counts {
                found.push(
                    UpgradeFinding::new(Compatible, "Content", counts.to_string()).with_optional_detail(
                        (counts.portals > 1)
                            .then(|| "Several portals: each one's home page is checked after every step.".to_string()),
                    ),
                );
            }
        }
    }

    // IIS and this PC.
    if !f.has_host_binding {
        found.push(
            UpgradeFinding::new(Blocking, "IIS", "The site has no http binding with a host name")
                .with_detail("DNN's upgrade is run through one.")
                .with_fix("Add one (Details → IIS → Edit host names…)."),
        );
    }
    match f.app_pool_clr.as_deref() {
        Some(clr) if !clr.eq_ignore_ascii_case("v4.0") => found.push(
            UpgradeFinding::new(
                Blocking,
                "IIS",
                format!("The app pool runs .NET CLR {}", if clr.is_empty() { "none" } else { clr }),
            )
            .with_detail("DNN 7 and newer run on CLR v4.0.")
            .with_fix("Set the app pool's .NET CLR version to v4.0 (Details → IIS → Edit app pool…)."),
        ),
        Some(clr) => found.push(UpgradeFinding::new(
            Compatible,
            "IIS",
            format!("App pool: .NET CLR {clr}, {} pipeline", f.app_pool_pipeline.as_deref().unwrap_or_default()),
        )),
        None => {}
    }
    if f.app_pool_pipeline.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("Classic")) {
        found.push(
            UpgradeFinding::new(Warning, "IIS", "The app pool uses the Classic pipeline")
                .with_detail("DNN is made for the Integrated pipeline."),
        );
    }
    if f.net_framework_release.is_none() {
        found.push(UpgradeFinding::new(Unknown, "This PC", ".NET Framework 4.x wasn't found"));
    }
    // Every step keeps a backup of its own - the site and its database - and needs room to work besides.
    let step_count = steps.max(1) as i64;
    let needed = step_count * (f.site_bytes + f.database_bytes.unwrap_or(0)) + 500 * 1024 * 1024;
    if let Some(free) = f.backup_free_bytes.filter(|free| *free < needed) {
        found.push(
            UpgradeFinding::new(
                Blocking,
                "Backups",
                format!("Too little free space for the backups: {} MB, about {} MB needed", mb(free), mb(needed)),
            )
            .with_detail(format!(
                "Each of the {step_count} step(s) backs up the site ({} MB) and its database{} first, and keeps it.",
                mb(f.site_bytes),
                f.database_bytes.map(|d| format!(" ({} MB)", mb(d))).unwrap_or_default()
            ))
            .with_fix("Free some space on the drive of Documents\\DnnManager."),
        );
    }

    // web.config.
    if f.has_machine_key == Some(false) {
        found.push(UpgradeFinding::new(Warning, "web.config", "web.config has no machineKey").with_detail(
            "DNN writes a new one during the upgrade - what was encrypted with the old keys can't be read any more.",
        ));
    }
    if !f.custom_app_settings.is_empty() {
        let names: Vec<&str> = f.custom_app_settings.iter().take(8).map(String::as_str).collect();
        found.push(
            UpgradeFinding::new(
                Warning,
                "web.config",
                format!("{} appSetting(s) DNN doesn't ship: {}", f.custom_app_settings.len(), names.join(", ")),
            )
            .with_detail("web.config isn't replaced - they stay - but check them after the upgrade."),
        );
    }
    if f.connection_strings.len() > 1 {
        found.push(
            UpgradeFinding::new(
                Warning,
                "web.config",
                format!("{} connection strings: {}", f.connection_strings.len(), f.connection_strings.join(", ")),
            )
            .with_detail("Only SiteSqlServer's database is backed up and upgraded."),
        );
    }

    // Extensions.
    let theirs: Vec<_> = f.extensions.iter().filter(|e| !e.is_dnns).collect();
    if !theirs.is_empty() {
        let listed: Vec<String> = theirs
            .iter()
            .take(12)
            .map(|e| {
                let owner = e.owner.as_deref().filter(|o| !o.is_empty()).map(|o| format!(", {o}")).unwrap_or_default();
                format!("{} {} ({}{owner})", e.friendly_name, e.version, e.kind)
            })
            .collect();
        found.push(
            UpgradeFinding::new(Warning, "Extensions", format!("{} third-party extension(s)", theirs.len()))
                .with_detail(listed.join(", ") + if theirs.len() > 12 { ", …" } else { "" })
                .with_fix("Check that each has a version for the target DNN."),
        );
    } else if !f.extensions.is_empty() {
        found.push(UpgradeFinding::new(
            Compatible,
            "Extensions",
            format!("{} extensions, all DNN's own", f.extensions.len()),
        ));
    }
    let unregistered: Vec<_> = f.assemblies.iter().filter(|a| !a.registered).collect();
    if !unregistered.is_empty() {
        let files: Vec<&str> = unregistered.iter().take(10).map(|a| a.file.as_str()).collect();
        found.push(
            UpgradeFinding::new(
                Warning,
                "Files",
                format!("{} assembly(ies) in bin no extension installed: {}", unregistered.len(), files.join(", ")),
            )
            .with_detail("Copied in by hand - the upgrade leaves them, but nothing checks they fit the new DNN."),
        );
    }
}

fn step_checks(f: &DnnSiteFacts, step: &DnnUpgradeStep) -> Vec<UpgradeFinding> {
    use UpgradeFindingSeverity::*;
    let mut checks = Vec::new();
    let to = DnnUpgradeStep::name(&step.to);
    if step.method == DnnUpgradeMethod::Manual {
        checks.push(
            UpgradeFinding::new(Blocking, "Method", "DNN Manager can't carry out this step")
                .with_detail(step.warnings.join(" "))
                .with_fix("Upgrade the site this far by hand, following DNN's upgrade guide, then use DNN Manager for the rest."),
        );
    }

    for requirement in &step.requirements {
        let text = &requirement.text;
        checks.push(match requirement.kind {
            DnnRequirementKind::NetFramework => match f.net_framework_release {
                None => UpgradeFinding::new(
                    Unknown,
                    "Requirement",
                    format!("{text} - this PC's .NET Framework couldn't be read"),
                ),
                Some(release) if release >= requirement.minimum => UpgradeFinding::new(
                    Compatible,
                    "Requirement",
                    format!("{text}: {} is installed", net_framework_name(release)),
                ),
                Some(release) => UpgradeFinding::new(Blocking, "Requirement", format!("DNN {to} needs {text}"))
                    .with_detail(format!("This PC has {}.", net_framework_name(release)))
                    .with_fix(format!("Install {text} (or newer) and restart Windows.")),
            },
            DnnRequirementKind::SqlServer => match f.sql_server_major() {
                None => UpgradeFinding::new(
                    Unknown,
                    "Requirement",
                    format!("{text} - the SQL Server's version couldn't be read"),
                ),
                Some(major) if major >= requirement.minimum => UpgradeFinding::new(
                    Compatible,
                    "Requirement",
                    format!("{text}: the database is on {}", sql_server_name(major)),
                ),
                Some(major) => UpgradeFinding::new(Blocking, "Requirement", format!("DNN {to} needs {text}"))
                    .with_detail(format!(
                        "The database is on {} ({}).",
                        sql_server_name(major),
                        f.sql_server_version.as_deref().unwrap_or_default()
                    ))
                    .with_fix(format!("Move the database to {text} or newer, then upgrade.")),
            },
        });
    }

    // Extensions built on what this step removes.
    let theirs: Vec<&DnnAssembly> = f.assemblies.iter().filter(|a| a.dnn_version.is_some()).collect();
    let built_before = |below: Version| -> Vec<&DnnAssembly> {
        theirs.iter().copied().filter(|a| a.dnn_version.is_some_and(|v| v < below)).collect()
    };
    if step.from < REMOVED_APIS && step.to >= REMOVED_APIS {
        let old = built_before(Version::parse("8.0").expect("valid"));
        if !old.is_empty() {
            checks.push(
                UpgradeFinding::new(
                    Warning,
                    "Extensions",
                    format!("{} assembly(ies) built against DNN 7 or older: {}", old.len(), names(&old)),
                )
                .with_detail(
                    "DNN 9.2 removed the APIs deprecated in DNN 7 and earlier - these may use them and stop working.",
                )
                .with_fix("Get versions for DNN 9.2 or newer from their makers, or remove them, before upgrading."),
            );
        }
    }
    if step.from < TEN && step.to >= TEN {
        let telerik: Vec<&DnnAssembly> = f.assemblies.iter().filter(|a| a.uses_telerik).collect();
        if !telerik.is_empty() {
            checks.push(
                UpgradeFinding::new(
                    Blocking,
                    "Telerik",
                    format!("{} assembly(ies) use Telerik: {}", telerik.len(), names(&telerik)),
                )
                .with_detail("DNN 10 removes the Telerik assemblies DNN shipped - these extensions would stop working, and pages using them fail.")
                .with_fix("Update or remove these extensions first (DNN 9.8+: Settings → Servers → Telerik removal shows what still uses Telerik)."),
            );
        } else {
            checks.push(UpgradeFinding::new(
                Compatible,
                "Telerik",
                if f.telerik_in_bin {
                    "Telerik.Web.UI is in bin, but no extension outside DNN uses it - DNN 10 removes it"
                } else {
                    "No Telerik dependency found"
                },
            ));
        }
        let before_nine = built_before(Version::parse("9.0").expect("valid"));
        if !before_nine.is_empty() {
            checks.push(
                UpgradeFinding::new(
                    Warning,
                    "Extensions",
                    format!(
                        "{} assembly(ies) built against DNN 8 or older: {}",
                        before_nine.len(),
                        names(&before_nine)
                    ),
                )
                .with_detail("DNN 10 has breaking changes - check each has a version for DNN 10."),
            );
        }
    }
    let nine_eight = Version::new(9, 8, 0);
    if step.from < nine_eight && step.to >= nine_eight && step.to < TEN && f.assemblies.iter().any(|a| a.uses_telerik) {
        checks.push(UpgradeFinding::new(Warning, "Telerik", "Extensions still use Telerik").with_detail(
            "From DNN 9.8 on, removing Telerik is recommended - DNN 10 removes it, and these extensions with it.",
        ));
    }

    if step.method != DnnUpgradeMethod::Manual {
        checks.extend(step.warnings.iter().map(|w| UpgradeFinding::new(Warning, "Note", w.as_str())));
    }
    checks.extend(step.breaking_changes.iter().map(|b| UpgradeFinding::new(Warning, "Breaking change", b.as_str())));
    checks.extend(step.known_issues.iter().map(|i| UpgradeFinding::new(Warning, "Known issue", i.as_str())));
    checks.extend(step.tested.iter().map(|t| UpgradeFinding::new(Compatible, "Tested", t.as_str())));
    checks
}

fn names(assemblies: &[&DnnAssembly]) -> String {
    let listed: Vec<String> = assemblies
        .iter()
        .take(8)
        .map(|a| format!("{} (DNN {})", a.file, a.dnn_version.map_or_else(|| "-".to_string(), |v| v.to_string_n(3))))
        .collect();
    listed.join(", ") + if assemblies.len() > 8 { ", …" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upgrades::facts::{DnnContentCounts, DnnExtension};
    use UpgradeFindingSeverity::*;

    /// A healthy DNN 9.13.9 site on SQL Server 2022 and .NET 4.8.1 - each test changes what it is about.
    fn site(version: &str) -> DnnSiteFacts {
        DnnSiteFacts {
            site_name: "shop".into(),
            directory: r"C:\DNN\shop".into(),
            files_version: Some(version.into()),
            database_version: Some(version.into()),
            net_framework_release: Some(533320),
            app_pool_clr: Some("v4.0".into()),
            app_pool_pipeline: Some("Integrated".into()),
            has_host_binding: true,
            site_bytes: 300 << 20,
            backup_free_bytes: Some(50 << 30),
            database_kind: Some(DatabaseKind::SqlServer),
            database_name: Some("shop".into()),
            sql_server_version: Some("16.0.4135.4".into()),
            sql_server_edition: Some("Developer Edition".into()),
            database_state: Some("ONLINE".into()),
            counts: Some(DnnContentCounts {
                portals: 1,
                users: 3,
                roles: 4,
                user_roles: 5,
                pages: 30,
                modules: 40,
                permissions: 200,
                scheduled_jobs: 20,
                extensions: 90,
            }),
            has_machine_key: Some(true),
            extensions: vec![DnnExtension {
                name: "DotNetNuke.HTML".into(),
                friendly_name: "HTML".into(),
                kind: "Module".into(),
                version: "9.13.9".into(),
                owner: Some("DNN".into()),
                is_dnns: true,
            }],
            ..Default::default()
        }
    }

    fn plan_to(facts: &DnnSiteFacts, target: &str) -> DnnUpgradePlan {
        plan(facts, Version::parse(target).unwrap(), &[])
    }

    fn titles(findings: &[&UpgradeFinding]) -> Vec<String> {
        findings.iter().map(|f| f.title.clone()).collect()
    }

    fn assembly(file: &str, dnn: Option<Version>, telerik: bool, registered: bool) -> DnnAssembly {
        DnnAssembly {
            file: file.into(),
            name: file.trim_end_matches(".dll").into(),
            dnn_version: dnn,
            uses_telerik: telerik,
            registered,
        }
    }

    #[test]
    fn a_healthy_site_can_start_and_the_plan_has_every_step() {
        let plan = plan_to(&site("9.13.9"), "10.3.3");
        assert!(plan.can_start(), "{:?}", titles(&plan.blocking()));
        let steps: Vec<String> = plan.steps.iter().map(|s| s.step.to_string()).collect();
        assert_eq!(steps.join(" | "), "09.13.09 → 10.02.05 | 10.02.05 → 10.03.03");
        let ten = &plan.steps[0].checks;
        assert!(ten.iter().any(|c| c.severity == Compatible && c.title.starts_with("SQL Server 2017")));
        assert!(ten.iter().any(|c| c.severity == Compatible && c.title.starts_with(".NET Framework 4.8")));
        assert!(plan.describe().contains("Required path:"));
    }

    #[test]
    fn an_old_sql_server_blocks_the_step_to_dnn_10() {
        let plan =
            plan_to(&DnnSiteFacts { sql_server_version: Some("13.0.5026.0".into()), ..site("9.13.9") }, "10.3.3");
        assert!(!plan.can_start());
        // Both DNN 10 steps need it - the first is where it stops.
        assert_eq!(
            titles(&plan.blocking()),
            vec!["DNN 10.02.05 needs SQL Server 2017", "DNN 10.03.03 needs SQL Server 2017"]
        );
        assert!(plan.blocking()[0].detail.as_deref().unwrap().contains("SQL Server 2016"));
    }

    #[test]
    fn a_minimum_added_in_settings_blocks_like_a_known_one() {
        let added = vec![UpgradeStop {
            version: "11.0.0".into(),
            sql_server_major: 17,
            note: "Needs the new SQL Server.".into(),
            ..Default::default()
        }];
        // The site's SQL Server 2022 is too old for it.
        let p = plan(&site("10.3.3"), Version::new(11, 0, 0), &added);
        assert!(!p.can_start());
        assert_eq!(titles(&p.blocking()), vec!["DNN 11.00.00 needs SQL Server 2025"]);
        assert!(
            p.steps[0].checks.iter().any(|c| c.severity == Warning && c.title.contains("Needs the new SQL Server."))
        );
        // Not when the upgrade stops before it.
        assert!(plan(&site("10.3.3"), Version::new(10, 4, 0), &added).can_start());
    }

    #[test]
    fn an_old_net_framework_blocks_the_steps_that_need_a_newer_one() {
        // 4.7.2 is enough for 9.13.9 but not for DNN 10.
        let plan = plan_to(&DnnSiteFacts { net_framework_release: Some(461808), ..site("9.3.2") }, "10.3.3");
        let blocking = plan.blocking();
        assert_eq!(blocking[0].title, "DNN 10.02.05 needs .NET Framework 4.8");
        assert!(blocking.iter().all(|b| b.title.ends_with(".NET Framework 4.8")));
        assert!(
            plan.steps[0]
                .checks
                .iter()
                .any(|c| c.severity == Compatible && c.title.starts_with(".NET Framework 4.7.2"))
        );
    }

    #[test]
    fn telerik_dependent_extensions_block_the_step_to_dnn_10_and_only_that_one() {
        let facts = DnnSiteFacts {
            telerik_in_bin: true,
            assemblies: vec![assembly("Acme.Gallery.dll", Some(Version::new(9, 1, 0)), true, true)],
            ..site("9.3.2")
        };
        let plan = plan_to(&facts, "10.3.3");
        let blocking = plan.blocking();
        assert_eq!(blocking.len(), 1);
        assert_eq!(blocking[0].area, "Telerik");
        assert!(blocking[0].title.contains("Acme.Gallery.dll"));
        assert_eq!(plan.steps.iter().filter(|s| s.checks.iter().any(|c| c.severity == Blocking)).count(), 1);
        // On the way, 9.8 recommends removing it.
        assert!(plan.steps[0].checks.iter().any(|c| c.area == "Telerik" && c.severity == Warning));
    }

    #[test]
    fn telerik_only_in_dnns_own_files_is_fine() {
        let plan = plan_to(&DnnSiteFacts { telerik_in_bin: true, ..site("9.13.9") }, "10.3.3");
        assert!(plan.can_start());
        assert!(plan.steps[0].checks.iter().any(|c| c.area == "Telerik" && c.severity == Compatible));
    }

    #[test]
    fn extensions_built_for_dnn_7_warn_on_the_step_past_92() {
        let facts = DnnSiteFacts {
            assemblies: vec![assembly("Old.Module.dll", Some(Version::new(7, 0, 0)), false, true)],
            ..site("9.1.1")
        };
        let plan = plan_to(&facts, "10.3.3");
        assert!(plan.can_start());
        let step = plan.steps.iter().find(|s| s.step.to == Version::new(9, 3, 2)).unwrap();
        let warnings: Vec<_> = step.checks.iter().filter(|c| c.area == "Extensions").collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].severity, Warning);
        assert!(warnings[0].detail.as_deref().unwrap().contains("DNN 9.2 removed"));
    }

    #[test]
    fn files_and_database_out_of_step_block() {
        let plan = plan_to(&DnnSiteFacts { database_version: Some("9.13.4".into()), ..site("9.13.9") }, "10.3.3");
        let blocking = plan.blocking();
        assert_eq!(blocking.len(), 1);
        assert!(blocking[0].title.contains("the database at 9.13.4"));
    }

    #[test]
    fn a_localdb_file_or_an_unreadable_database_blocks_and_unknowns_are_said() {
        let local =
            plan_to(&DnnSiteFacts { database_kind: Some(DatabaseKind::LocalDbFile), ..site("9.13.9") }, "10.3.3");
        assert_eq!(titles(&local.blocking()), vec!["The database is a LocalDB file"]);
        let unreadable = plan_to(
            &DnnSiteFacts {
                database_problem: Some("Login failed".into()),
                sql_server_version: None,
                database_version: None,
                ..site("9.13.9")
            },
            "10.3.3",
        );
        assert_eq!(unreadable.blocking()[0].title, "The database can't be read");
        let unknown = plan_to(&DnnSiteFacts { sql_server_version: None, ..site("9.13.9") }, "10.3.3");
        assert!(unknown.can_start(), "What couldn't be checked doesn't block…");
        assert!(
            unknown.all().any(|f| f.severity == Unknown && f.title.starts_with("SQL Server 2017")),
            "…but it is said."
        );
    }

    #[test]
    fn manual_steps_too_little_space_and_a_wrong_app_pool_block() {
        assert_eq!(titles(&plan_to(&site("6.1.0"), "7.4.2").blocking()), vec!["DNN Manager can't carry out this step"]);
        let little = plan_to(&DnnSiteFacts { backup_free_bytes: Some(100 << 20), ..site("9.13.9") }, "10.3.3");
        assert!(little.blocking().iter().any(|f| f.area == "Backups"));
        let clr = plan_to(&DnnSiteFacts { app_pool_clr: Some("v2.0".into()), ..site("9.13.9") }, "10.3.3");
        assert!(clr.blocking().iter().any(|f| f.area == "IIS"));
        let binding = plan_to(&DnnSiteFacts { has_host_binding: false, ..site("9.13.9") }, "10.3.3");
        assert!(binding.blocking().iter().any(|f| f.area == "IIS"));
    }

    #[test]
    fn third_party_extensions_and_custom_settings_warn() {
        let plan = plan_to(
            &DnnSiteFacts {
                extensions: vec![DnnExtension {
                    name: "Acme.Blog".into(),
                    friendly_name: "Acme Blog".into(),
                    kind: "Module".into(),
                    version: "3.1.0".into(),
                    owner: Some("Acme".into()),
                    is_dnns: false,
                }],
                custom_app_settings: vec!["Acme.ApiKey".into()],
                assemblies: vec![assembly("Hand.Copied.dll", None, false, false)],
                has_machine_key: Some(false),
                ..site("9.13.9")
            },
            "10.3.3",
        );
        assert!(plan.can_start());
        for area in ["Extensions", "web.config", "Files"] {
            assert!(plan.site.iter().any(|f| f.area == area && f.severity == Warning), "{area}");
        }
    }
}
