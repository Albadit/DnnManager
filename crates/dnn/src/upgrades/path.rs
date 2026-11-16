//! DNN's suggested upgrade path and the steps from a site's version to the one asked for
//! (`src/DnnManager.Application/Upgrades/DnnUpgradePath.cs`), with the path's edits made in Settings
//! (`DnnUpgradeStopSetting` and `SettingRules.UpgradeStopVersion` / `UpgradeStopsProblem`).

use std::fmt;
use std::sync::LazyLock;

use super::facts::{net_framework_name, sql_server_name};
use super::knowledge::STEPS;
use crate::version::Version;

/// DNN's suggested upgrade path (docs.dnncommunity.org → Getting started → Setup → Upgrades → Suggested upgrade path).
pub const SOURCE: &str =
    "https://docs.dnncommunity.org/content/getting-started/setup/upgrades/suggested-upgrade-path/index.html";

/// How a step of the upgrade path is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DnnUpgradeMethod {
    /// The release's `…_Upgrade.zip` over the site's files (never its web.config), then DNN's unattended upgrade.
    #[default]
    UpgradePackage,
    /// For a site already on DNN 10.2 or newer: the release's `…_Install.zip` put in as DNN's own local upgrade does
    /// (each assembly with its binding redirect), then DNN's unattended upgrade.
    LocalUpgrade,
    /// Not carried out by DNN Manager: GitHub has no packages of the version, or it is older than the path knows.
    Manual,
}

/// What a requirement is about - how the analyser checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DnnRequirementKind {
    /// The .NET Framework installed on this PC, as the `Release` number of the v4 Full registry key.
    NetFramework,
    /// The SQL Server the site's database is on, by its major version (14 = SQL Server 2017).
    SqlServer,
}

/// A minimum the target version of a step needs - checked before the upgrade starts; not met, it is blocking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnRequirement {
    pub kind: DnnRequirementKind,
    /// .NET Framework: the `Release` number (461808 = 4.7.2, 528040 = 4.8); SQL Server: its major version.
    pub minimum: i32,
    /// How people say it: ".NET Framework 4.8", "SQL Server 2017".
    pub text: String,
    pub source: String,
}

/// One step of DNN's suggested upgrade path and what is known about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnUpgradeStep {
    pub from: Version,
    pub to: Version,
    pub method: DnnUpgradeMethod,
    pub requirements: Vec<DnnRequirement>,
    /// What to look out for - shown in the plan as warnings.
    pub warnings: Vec<String>,
    /// What the target version removed or changed that may break a site or its extensions.
    pub breaking_changes: Vec<String>,
    /// Problems seen with this step - DNN's issues, the community's reports, these tests - and their fixes.
    pub known_issues: Vec<String>,
    /// Whether the site may still depend on Telerik once it runs the target version (DNN 10 removed it).
    pub removes_telerik: bool,
    /// How DNN Manager's tests carried this step out on real sites - empty when it wasn't tested.
    pub tested: Vec<String>,
}

impl Default for DnnUpgradeStep {
    fn default() -> Self {
        Self {
            from: Version::new(0, 0, 0),
            to: Version::new(0, 0, 0),
            method: DnnUpgradeMethod::UpgradePackage,
            requirements: Vec::new(),
            warnings: Vec::new(),
            breaking_changes: Vec::new(),
            known_issues: Vec::new(),
            removes_telerik: false,
            tested: Vec::new(),
        }
    }
}

impl DnnUpgradeStep {
    /// DNN's way of writing a version in the path: 09.13.09.
    pub fn name(version: &Version) -> String {
        format!("{:02}.{:02}.{:02}", version.major, version.minor, version.build_or_zero())
    }
}

impl fmt::Display for DnnUpgradeStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} → {}", Self::name(&self.from), Self::name(&self.to))
    }
}

/// A version of the upgrade path as Settings shows it: what the step to it needs (0: nothing) and its note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnPathVersion {
    pub version: Version,
    pub net_framework_release: i32,
    pub sql_server_major: i32,
    pub note: String,
}

/// A version of DNN's upgrade path added, changed or taken off in Settings (`DnnUpgradeStopSetting`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpgradeStop {
    /// Three numbers, e.g. 10.4.2.
    pub version: String,
    /// The .NET Framework this version and the ones after it need, as its `Release` number; 0: nothing new.
    pub net_framework_release: i32,
    /// The SQL Server this version and the ones after it need, by its major version; 0: nothing new.
    pub sql_server_major: i32,
    /// Shown in the upgrade plan, as a warning on the step to this version.
    pub note: String,
    /// A version of the path DNN Manager knows, changed in Settings: its minimums replace what is known about the step
    /// to it - lower ones too. False: they only raise it (a version added before DNN Manager listed it).
    pub replaces_listed: bool,
    /// Taken off the path: upgrades don't stop at this version.
    pub removed: bool,
}

/// The longest note a version of the path may have (`SettingRules.MaxUpgradeStopNote`).
pub const MAX_UPGRADE_STOP_NOTE: usize = 500;

/// The listed versions, oldest first.
pub static VERSIONS: LazyLock<Vec<Version>> =
    LazyLock::new(|| STEPS.iter().map(|s| s.from).chain(STEPS.last().map(|s| s.to)).collect());

/// The oldest version GitHub publishes packages of - every step to an older version is manual.
pub const OLDEST_ON_GITHUB: Version = Version::new(7, 4, 2);

/// What Settings offers as a version's .NET Framework minimum, as the v4 Full key's `Release` number (4.5 to 4.8.1).
pub const NET_FRAMEWORK_CHOICES: [i32; 11] =
    [378389, 378675, 379893, 393295, 394254, 394802, 460798, 461308, 461808, 528040, 533320];

/// What Settings offers as a version's SQL Server minimum, by major version (2008 on) - the last ones don't exist yet:
/// a later DNN may need one when there is no newer DNN Manager to offer it.
pub const SQL_SERVER_CHOICES: [i32; 11] = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

/// `SettingRules.UpgradeStopVersion`: three numbers, as 10.4.2 or DNN's own 10.04.02 - or None when it isn't one.
pub fn upgrade_stop_version(text: &str) -> Option<Version> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.len() != 3 || !parts.iter().all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    Some(Version::new(parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
}

/// `SettingRules.UpgradeStopsProblem`: what is wrong with the edits of the path, or None when they can be used. A
/// version DNN Manager knows itself isn't wrong: a later DNN Manager may list it, and the saved stop must stay allowed.
pub fn upgrade_stops_problem(stops: &[UpgradeStop]) -> Option<String> {
    let mut seen = Vec::new();
    let mut removed = Vec::new();
    let mut added = false;
    for stop in stops {
        let Some(version) = upgrade_stop_version(&stop.version) else {
            return Some(format!("'{}' isn't a DNN version - three numbers, e.g. 10.4.2.", stop.version));
        };
        if seen.contains(&version) {
            return Some(format!("DNN {version} is there twice."));
        }
        seen.push(version);
        if stop.removed {
            removed.push(version);
            continue;
        }
        added = true;
        // The path's own old versions can be changed: their steps are done by hand anyway.
        if version < OLDEST_ON_GITHUB && !VERSIONS.contains(&version) {
            return Some(format!(
                "DNN {version} can't be a step: GitHub has no DNN packages older than {OLDEST_ON_GITHUB}."
            ));
        }
        if stop.net_framework_release < 0 || stop.sql_server_major < 0 {
            return Some(format!("DNN {version} has a minimum that isn't a version."));
        }
        if stop.note.encode_utf16().count() > MAX_UPGRADE_STOP_NOTE {
            return Some(format!("The note of DNN {version} can have at most {MAX_UPGRADE_STOP_NOTE} characters."));
        }
        if stop.note.chars().any(char::is_control) {
            return Some(format!("The note of DNN {version} must be one line of text."));
        }
    }
    if !added && VERSIONS.iter().all(|v| removed.contains(v)) {
        return Some("The upgrade path needs at least one version.".into());
    }
    None
}

/// An edit made in Settings, by the version it is about.
#[derive(Debug, Clone)]
struct Edit {
    version: Version,
    setting: UpgradeStop,
}

impl Edit {
    /// A change of one of the path's own versions: what it says replaces what is known.
    fn replaces(&self) -> bool {
        self.setting.replaces_listed && !self.setting.removed && VERSIONS.contains(&self.version)
    }

    /// A version added: it only raises what is known, from its version on.
    fn adds(&self) -> bool {
        !self.setting.replaces_listed && !self.setting.removed
    }
}

fn edits_of(edits: &[UpgradeStop]) -> Vec<Edit> {
    edits
        .iter()
        .filter_map(|s| upgrade_stop_version(&s.version).map(|version| Edit { version, setting: s.clone() }))
        .collect()
}

/// The path's versions with the edits: the ones taken off left out, the ones added in, oldest first.
fn path_of(edits: &[Edit]) -> Vec<Version> {
    let removed: Vec<Version> = edits.iter().filter(|e| e.setting.removed).map(|e| e.version).collect();
    let mut path: Vec<Version> = VERSIONS
        .iter()
        .copied()
        .chain(edits.iter().filter(|e| !e.setting.removed).map(|e| e.version))
        .filter(|v| !removed.contains(v))
        .collect();
    path.sort();
    path.dedup();
    // The rules keep one version at least (upgrade_stops_problem); a path of none would be DNN's own.
    if path.is_empty() { VERSIONS.clone() } else { path }
}

/// The steps from `current` to `target`, in order: through every version of the path above the current one and below
/// the target, then to the target itself. A step that isn't one of the path's own carries what is known about the
/// path's steps it spans, and says it isn't listed. `edits`: the path's edits made in Settings. Empty when the target
/// isn't newer.
pub fn chain(current: Version, target: Version, edits: &[UpgradeStop]) -> Vec<DnnUpgradeStep> {
    let current = current.normalize();
    let target = target.normalize();
    let mut steps = Vec::new();
    if target <= current {
        return steps;
    }
    let parsed = edits_of(edits);
    let path = path_of(&parsed);
    let stops = path.iter().copied().filter(|v| *v > current && *v < target).chain(std::iter::once(target));
    let mut from = current;
    for to in stops {
        steps.push(with_edits(step_for(from, to, &path), &parsed, &path));
        from = to;
    }
    steps
}

/// What is known about going from `from` to `to` - the path's own step when it is one.
fn step_for(from: Version, to: Version, path: &[Version]) -> DnnUpgradeStep {
    if let Some(listed) = STEPS.iter().find(|s| s.from == from && s.to == to) {
        return listed.clone();
    }
    // The path's steps this one spans - one, unless a listed version was taken off - or, past the newest listed
    // version, the newest step.
    let spanned: Vec<&DnnUpgradeStep> = STEPS.iter().filter(|s| s.to > from && s.from < to).collect();
    let newest = *VERSIONS.last().expect("listed");
    let known = if !spanned.is_empty() {
        Some(merge(&spanned))
    } else if to > newest {
        STEPS.last().cloned()
    } else {
        None
    };
    let method = if to < OLDEST_ON_GITHUB
        || from < VERSIONS[0]
        || spanned.iter().any(|s| s.method == DnnUpgradeMethod::Manual)
    {
        DnnUpgradeMethod::Manual
    } else {
        method_for(from)
    };
    // Said of the version that is off the path - the site's own (a hotfix release such as 9.13.10) or the target.
    // None when both are on it: a step to or from a version added in Settings, which says so itself.
    let path_newest = *path.last().expect("a path has a version");
    let name = DnnUpgradeStep::name;
    let note = if to > path_newest {
        Some(format!(
            "DNN {} is newer than the path's newest listed version ({}) - check DNN's upgrade path for anything it needs, and add what it lists in Settings → DNN releases.",
            name(&to),
            name(&path_newest)
        ))
    } else if from < VERSIONS[0] {
        Some(format!(
            "DNN {} is older than the path's oldest listed version ({}) - DNN Manager doesn't know how to upgrade it.",
            name(&from),
            name(&VERSIONS[0])
        ))
    } else if !path.contains(&from) {
        Some(format!(
            "Not one of the path's own steps: the site's DNN {} lies between its listed versions.",
            name(&from)
        ))
    } else if !path.contains(&to) {
        Some(format!("Not one of the path's own steps: DNN {} lies between its listed versions.", name(&to)))
    } else {
        None
    };
    let skipped: Vec<String> =
        VERSIONS.iter().filter(|v| **v > from && **v < to && !path.contains(v)).map(name).collect();
    let skips = (!skipped.is_empty()).then(|| {
        format!(
            "DNN's path goes through DNN {} on the way - taken off the path in Settings → DNN releases, so this step skips {}. DNN Manager has no tests of this step.",
            skipped.join(", "),
            if skipped.len() == 1 { "it" } else { "them" }
        )
    });
    let base = known.unwrap_or_default();
    let warnings = skips.into_iter().chain(note).chain(base.warnings.iter().cloned()).collect();
    DnnUpgradeStep { from, to, method, warnings, tested: Vec::new(), ..base }
}

/// The path's steps as one: the highest minimum of each kind and everything said about any of them.
fn merge(steps: &[&DnnUpgradeStep]) -> DnnUpgradeStep {
    let last = steps[steps.len() - 1];
    if steps.len() == 1 {
        return last.clone();
    }
    let mut requirements: Vec<DnnRequirement> = Vec::new();
    for requirement in steps.iter().flat_map(|s| &s.requirements) {
        match requirements.iter_mut().find(|r| r.kind == requirement.kind) {
            Some(kept) if requirement.minimum > kept.minimum => *kept = requirement.clone(),
            Some(_) => {}
            None => requirements.push(requirement.clone()),
        }
    }
    let distinct = |list: &dyn Fn(&DnnUpgradeStep) -> &Vec<String>| {
        let mut all: Vec<String> = Vec::new();
        for item in steps.iter().flat_map(|s| list(s)) {
            if !all.contains(item) {
                all.push(item.clone());
            }
        }
        all
    };
    DnnUpgradeStep {
        requirements,
        warnings: distinct(&|s| &s.warnings),
        breaking_changes: distinct(&|s| &s.breaking_changes),
        known_issues: distinct(&|s| &s.known_issues),
        removes_telerik: steps.iter().any(|s| s.removes_telerik),
        ..last.clone()
    }
}

/// A step with what the edits made in Settings say about it. A changed version of the path - the step's target, or
/// the version of the path it falls in - replaces the minimums known for it, lower ones too. An added version's
/// minimum holds from its version on, and only ever raises what is known. A version's note goes with the step to it.
fn with_edits(step: DnnUpgradeStep, edits: &[Edit], path: &[Version]) -> DnnUpgradeStep {
    if edits.is_empty() {
        return step;
    }
    let mut requirements = step.requirements.clone();
    let row = path.iter().copied().find(|v| *v >= step.to).unwrap_or_else(|| *path.last().expect("a path"));
    if let Some(changed) = edits.iter().find(|e| e.replaces() && e.version == row) {
        replace(&mut requirements, row, DnnRequirementKind::NetFramework, changed.setting.net_framework_release);
        replace(&mut requirements, row, DnnRequirementKind::SqlServer, changed.setting.sql_server_major);
    }
    let added: Vec<&Edit> = edits.iter().filter(|e| e.adds() && e.version <= step.to).collect();
    if !added.is_empty() {
        let net = added.iter().map(|e| e.setting.net_framework_release).max().unwrap_or(0);
        let sql = added.iter().map(|e| e.setting.sql_server_major).max().unwrap_or(0);
        raise(&mut requirements, DnnRequirementKind::NetFramework, net);
        raise(&mut requirements, DnnRequirementKind::SqlServer, sql);
    }

    let mut warnings = Vec::new();
    if let Some(own) = edits.iter().find(|e| !e.setting.removed && e.version == step.to) {
        if !VERSIONS.contains(&own.version) {
            warnings.push(format!(
                "You added DNN {} to the path (Settings → DNN releases): DNN Manager has no tests of this step, so read DNN's release notes for it.",
                DnnUpgradeStep::name(&own.version)
            ));
        } else if own.replaces() {
            warnings.push(format!(
                "You changed what DNN {} needs in Settings → DNN releases: its checks are yours, not DNN's.",
                DnnUpgradeStep::name(&own.version)
            ));
        }
        let note = own.setting.note.trim();
        if !note.is_empty() {
            warnings.push(note.to_string());
        }
    }
    warnings.extend(step.warnings.iter().cloned());
    DnnUpgradeStep { requirements, warnings, ..step }
}

fn requirement_name(kind: DnnRequirementKind, minimum: i32) -> String {
    match kind {
        DnnRequirementKind::NetFramework => net_framework_name(minimum),
        DnnRequirementKind::SqlServer => sql_server_name(minimum),
    }
}

fn replace(requirements: &mut Vec<DnnRequirement>, row: Version, kind: DnnRequirementKind, minimum: i32) {
    // Left as DNN Manager knows it: its own wording and source stay.
    if minimum == listed_minimum(row, kind) {
        return;
    }
    requirements.retain(|r| r.kind != kind);
    if minimum > 0 {
        requirements.push(DnnRequirement {
            kind,
            minimum,
            text: requirement_name(kind, minimum),
            source: "Changed in Settings → DNN releases".into(),
        });
    }
}

fn raise(requirements: &mut Vec<DnnRequirement>, kind: DnnRequirementKind, minimum: i32) {
    if minimum <= 0 || requirements.iter().any(|r| r.kind == kind && r.minimum >= minimum) {
        return;
    }
    requirements.retain(|r| r.kind != kind);
    requirements.push(DnnRequirement {
        kind,
        minimum,
        text: requirement_name(kind, minimum),
        source: "Added in Settings → DNN releases".into(),
    });
}

/// What DNN Manager knows the step to `version`, one of the path's own, needs of `kind`; 0 for nothing.
pub fn listed_minimum(version: Version, kind: DnnRequirementKind) -> i32 {
    STEPS
        .iter()
        .find(|s| s.to == version)
        .and_then(|s| s.requirements.iter().find(|r| r.kind == kind))
        .map_or(0, |r| r.minimum)
}

/// One of the path's own versions as DNN Manager knows it.
pub fn listed(version: Version) -> DnnPathVersion {
    DnnPathVersion {
        version,
        net_framework_release: listed_minimum(version, DnnRequirementKind::NetFramework),
        sql_server_major: listed_minimum(version, DnnRequirementKind::SqlServer),
        note: String::new(),
    }
}

/// Whether the step to `version`, one of the path's own, is done by hand (or there is none: the oldest).
pub fn is_manual(version: Version) -> bool {
    version == VERSIONS[0]
        || STEPS.iter().find(|s| s.to == version).is_some_and(|s| s.method == DnnUpgradeMethod::Manual)
}

/// The whole path with `edits`, oldest first, as Settings shows it: each version with what the step to it needs and
/// its note.
pub fn path_with(edits: &[UpgradeStop]) -> Vec<DnnPathVersion> {
    let parsed = edits_of(edits);
    path_of(&parsed)
        .into_iter()
        .map(|v| {
            let Some(edit) = parsed.iter().find(|e| e.version == v && !e.setting.removed) else {
                return listed(v);
            };
            let own = DnnPathVersion {
                version: v,
                net_framework_release: edit.setting.net_framework_release,
                sql_server_major: edit.setting.sql_server_major,
                note: edit.setting.note.trim().to_string(),
            };
            if !VERSIONS.contains(&v) || edit.replaces() {
                return own;
            }
            // Added before DNN Manager listed it: it only raises what is known.
            let known = listed(v);
            DnnPathVersion {
                net_framework_release: own.net_framework_release.max(known.net_framework_release),
                sql_server_major: own.sql_server_major.max(known.sql_server_major),
                ..own
            }
        })
        .collect()
}

/// The edits that make `path` of the path DNN Manager knows: its versions left out taken off, the ones changed and
/// the ones added. One as it was in `before` keeps its edit as it was - an added version DNN Manager has learned since
/// still only raises what it knows.
pub fn edits_for(path: &[DnnPathVersion], before: &[UpgradeStop]) -> Vec<UpgradeStop> {
    let mut rows: Vec<DnnPathVersion> = path
        .iter()
        .map(|r| DnnPathVersion { version: r.version.normalize(), note: r.note.trim().to_string(), ..r.clone() })
        .collect();
    rows.sort_by_key(|r| r.version);
    let was = path_with(before);
    let mut previous: Vec<Edit> = Vec::new();
    for edit in edits_of(before).into_iter().filter(|e| !e.setting.removed) {
        if !previous.iter().any(|p| p.version == edit.version) {
            previous.push(edit);
        }
    }
    let mut edits = Vec::new();
    for row in &rows {
        let is_listed = VERSIONS.contains(&row.version);
        let unchanged = was.iter().any(|old| old == row);
        if let Some(kept) = previous.iter().find(|p| p.version == row.version).filter(|_| unchanged) {
            edits.push(kept.setting.clone());
        } else if !is_listed || *row != listed(row.version) {
            edits.push(UpgradeStop {
                version: row.version.to_string(),
                net_framework_release: row.net_framework_release,
                sql_server_major: row.sql_server_major,
                note: row.note.clone(),
                replaces_listed: is_listed,
                removed: false,
            });
        }
    }
    edits.extend(VERSIONS.iter().filter(|v| !rows.iter().any(|r| r.version == **v)).map(|v| UpgradeStop {
        version: v.to_string(),
        removed: true,
        ..Default::default()
    }));
    edits.sort_by_key(|e| upgrade_stop_version(&e.version));
    edits
}

/// DNN 10.2 brought its own local upgrade: from a 10.2+ site on, that is how its files go in.
pub fn method_for(from: Version) -> DnnUpgradeMethod {
    if from >= Version::new(10, 2, 0) { DnnUpgradeMethod::LocalUpgrade } else { DnnUpgradeMethod::UpgradePackage }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn joined(steps: &[DnnUpgradeStep]) -> String {
        steps.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" | ")
    }

    fn chain_of(from: &str, to: &str) -> String {
        joined(&chain(v(from), v(to), &[]))
    }

    fn stop(version: &str, net_framework: i32, sql_server: i32, note: &str) -> UpgradeStop {
        UpgradeStop {
            version: version.into(),
            net_framework_release: net_framework,
            sql_server_major: sql_server,
            note: note.into(),
            ..Default::default()
        }
    }

    fn off(version: &str) -> UpgradeStop {
        UpgradeStop { version: version.into(), removed: true, ..Default::default() }
    }

    fn chain_with(from: &str, to: &str, added: &[UpgradeStop]) -> Vec<DnnUpgradeStep> {
        chain(v(from), v(to), added)
    }

    fn requirement(step: &DnnUpgradeStep, kind: DnnRequirementKind) -> Vec<&DnnRequirement> {
        step.requirements.iter().filter(|r| r.kind == kind).collect()
    }

    fn texts(step: &DnnUpgradeStep) -> Vec<&str> {
        let mut texts: Vec<&str> = step.requirements.iter().map(|r| r.text.as_str()).collect();
        texts.sort();
        texts
    }

    #[test]
    fn the_path_is_dnns_own_in_order() {
        assert_eq!(
            VERSIONS.iter().map(DnnUpgradeStep::name).collect::<Vec<_>>().join(" "),
            "02.00.04 02.01.02 03.01.01 03.02.02 04.03.07 04.04.01 04.06.02 04.09.05 05.04.04 05.06.08 06.02.08 07.04.02 08.00.04 09.01.01 09.03.02 09.13.09 10.02.05 10.03.03"
        );
    }

    #[test]
    fn a_listed_version_goes_through_every_listed_one_after_it() {
        assert_eq!(
            chain_of("9.1.1", "10.3.3"),
            "09.01.01 → 09.03.02 | 09.03.02 → 09.13.09 | 09.13.09 → 10.02.05 | 10.02.05 → 10.03.03"
        );
        assert_eq!(chain_of("10.2.5", "10.3.3"), "10.02.05 → 10.03.03");
    }

    #[test]
    fn a_version_between_two_listed_ones_goes_to_the_next_listed_one_first() {
        assert_eq!(chain_of("9.5.0", "10.3.3"), "09.05.00 → 09.13.09 | 09.13.09 → 10.02.05 | 10.02.05 → 10.03.03");
        // Never straight to DNN 10: through 9.13.9 even from a later 9.13.
        assert_eq!(chain_of("9.13.4", "10.2.5"), "09.13.04 → 09.13.09 | 09.13.09 → 10.02.05");
        assert_eq!(chain_of("10.0.1", "10.3.3"), "10.00.01 → 10.02.05 | 10.02.05 → 10.03.03");
    }

    #[test]
    fn a_target_between_listed_versions_ends_the_chain_and_carries_the_steps_knowledge() {
        let steps = chain(Version::new(9, 3, 2), Version::new(10, 1, 0), &[]);
        assert_eq!(joined(&steps), "09.03.02 → 09.13.09 | 09.13.09 → 10.01.00");
        // 10.1.0 is a DNN 10: what DNN 10 needs applies.
        assert!(steps[1].requirements.iter().any(|r| r.kind == DnnRequirementKind::SqlServer && r.minimum == 14));
        assert!(steps[1].removes_telerik);
        assert!(steps[1].warnings[0].contains("between its listed versions"));
    }

    #[test]
    fn a_target_past_the_newest_listed_version_says_so() {
        let steps = chain(Version::new(10, 2, 5), Version::new(10, 4, 0), &[]);
        assert_eq!(joined(&steps), "10.02.05 → 10.03.03 | 10.03.03 → 10.04.00");
        assert!(steps[1].warnings[0].contains("newer than the path's newest listed version"));
        // From a 10.2+ site on, DNN's own local upgrade.
        assert_eq!(steps[1].method, DnnUpgradeMethod::LocalUpgrade);
        let to_ten = chain(Version::new(9, 13, 9), Version::new(10, 2, 5), &[]);
        assert_eq!(to_ten.len(), 1);
        assert_eq!(to_ten[0].method, DnnUpgradeMethod::UpgradePackage);
    }

    #[test]
    fn steps_to_versions_github_has_no_packages_of_are_manual() {
        let steps = chain(Version::new(5, 6, 8), Version::new(8, 0, 4), &[]);
        assert_eq!(joined(&steps), "05.06.08 → 06.02.08 | 06.02.08 → 07.04.02 | 07.04.02 → 08.00.04");
        assert_eq!(
            steps.iter().map(|s| s.method).collect::<Vec<_>>(),
            vec![DnnUpgradeMethod::Manual, DnnUpgradeMethod::UpgradePackage, DnnUpgradeMethod::UpgradePackage]
        );
        let oldest = chain(Version::new(1, 0, 0), Version::new(2, 0, 4), &[]);
        assert_eq!(oldest.len(), 1);
        assert_eq!(oldest[0].method, DnnUpgradeMethod::Manual);
    }

    #[test]
    fn nothing_to_do_when_the_target_isnt_newer() {
        assert!(chain(Version::new(10, 3, 3), Version::new(10, 3, 3), &[]).is_empty());
        assert!(chain(Version::new4(10, 3, 3, 0), Version::new(10, 2, 5), &[]).is_empty());
    }

    // ─── Versions added in Settings ───────────────────────────────────────

    #[test]
    fn an_added_version_is_a_stop_on_the_way_like_a_listed_one() {
        let steps = chain_with("10.2.5", "10.5.0", &[stop("10.4.2", 0, 0, "")]);
        assert_eq!(joined(&steps), "10.02.05 → 10.03.03 | 10.03.03 → 10.04.02 | 10.04.02 → 10.05.00");
        // The listed step is the path's own, with what was tested on it.
        assert!(!steps[0].tested.is_empty());
        // From a 10.2+ site on, DNN's local upgrade - and what is known about the newest step goes on.
        assert_eq!(steps[1].method, DnnUpgradeMethod::LocalUpgrade);
        assert!(steps[1].requirements.iter().any(|r| r.kind == DnnRequirementKind::SqlServer && r.minimum == 14));
        // Never presented as tested, and the user is told it's their own.
        assert!(steps[1].tested.is_empty());
        assert!(steps[1].warnings[0].contains("You added DNN 10.04.02"));
        // Past the newest stop the note names that one, not the built-in end of the path.
        assert!(steps[2].warnings[0].contains("newest listed version (10.04.02)"));
    }

    #[test]
    fn an_added_version_between_two_listed_ones_is_a_stop_too() {
        let steps = chain_with("10.2.5", "10.3.3", &[stop("10.2.8", 0, 0, "")]);
        assert_eq!(joined(&steps), "10.02.05 → 10.02.08 | 10.02.08 → 10.03.03");
        assert!(
            !steps[1].warnings.iter().any(|w| w.contains("between its listed versions")),
            "{:?}",
            steps[1].warnings
        );
    }

    #[test]
    fn added_versions_never_take_a_listed_stop_away() {
        assert_eq!(
            joined(&chain_with("9.3.2", "11.0.0", &[stop("10.4.2", 0, 0, "")])),
            "09.03.02 → 09.13.09 | 09.13.09 → 10.02.05 | 10.02.05 → 10.03.03 | 10.03.03 → 10.04.02 | 10.04.02 → 11.00.00"
        );
    }

    #[test]
    fn a_version_outside_the_upgrade_changes_nothing() {
        let steps = chain_with("9.13.9", "10.3.3", &[stop("10.4.2", 0, 16, "Later.")]);
        assert_eq!(joined(&steps), "09.13.09 → 10.02.05 | 10.02.05 → 10.03.03");
        assert!(
            steps
                .iter()
                .flat_map(|s| &s.requirements)
                .all(|r| r.kind != DnnRequirementKind::SqlServer || r.minimum == 14)
        );
        assert!(!steps.iter().flat_map(|s| &s.warnings).any(|w| w == "Later."));
    }

    #[test]
    fn a_minimum_holds_from_its_version_on_and_replaces_a_lower_one() {
        let steps = chain_with(
            "10.3.3",
            "11.2.0",
            &[stop("10.4.2", 0, 0, ""), stop("11.0.0", 533320, 15, ""), stop("11.1.0", 0, 0, "")],
        );
        assert_eq!(
            joined(&steps),
            "10.03.03 → 10.04.02 | 10.04.02 → 11.00.00 | 11.00.00 → 11.01.00 | 11.01.00 → 11.02.00"
        );
        assert!(steps[0].requirements.iter().any(|r| r.kind == DnnRequirementKind::SqlServer && r.minimum == 14));
        for step in &steps[1..] {
            // One requirement of a kind - not 2017 and 2019 both.
            let sql = requirement(step, DnnRequirementKind::SqlServer);
            assert_eq!(sql.len(), 1, "{step}");
            assert_eq!(sql[0].text, "SQL Server 2019", "{step}");
            let net = requirement(step, DnnRequirementKind::NetFramework);
            assert_eq!(net.len(), 1, "{step}");
            assert_eq!(net[0].text, ".NET Framework 4.8.1", "{step}");
        }
    }

    #[test]
    fn a_minimum_below_what_is_known_changes_nothing() {
        // SQL Server 2016, .NET 4.5 - DNN 10 needs more, and that stays.
        let steps = chain_with("10.3.3", "10.4.2", &[stop("10.4.2", 378389, 13, "")]);
        assert_eq!(steps.len(), 1);
        assert_eq!(texts(&steps[0]), vec![".NET Framework 4.8", "SQL Server 2017"]);
    }

    #[test]
    fn a_version_dnn_manager_lists_itself_keeps_what_is_known_and_gets_the_additions() {
        let steps = chain_with("10.2.5", "10.3.3", &[stop("10.3.3", 0, 15, "Seen on our sites.")]);
        let step = &steps[0];
        assert!(!step.tested.is_empty());
        assert!(step.warnings.iter().any(|w| w == "Seen on our sites."));
        assert!(!step.warnings.iter().any(|w| w.starts_with("You added")), "A listed step isn't the user's own.");
        assert_eq!(requirement(step, DnnRequirementKind::SqlServer)[0].text, "SQL Server 2019");
    }

    #[test]
    fn the_note_goes_with_the_step_to_its_version() {
        let steps = chain_with("10.3.3", "10.5.0", &[stop("10.4.2", 0, 0, "Read the release notes.")]);
        assert!(steps[0].warnings.iter().any(|w| w == "Read the release notes."));
        assert!(!steps[1].warnings.iter().any(|w| w == "Read the release notes."));
    }

    #[test]
    fn a_site_on_an_added_version_starts_from_it() {
        let steps = chain_with("10.4.2", "10.5.0", &[stop("10.4.2", 0, 15, "")]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].to_string(), "10.04.02 → 10.05.00");
        assert_eq!(requirement(&steps[0], DnnRequirementKind::SqlServer)[0].text, "SQL Server 2019");
    }

    // ─── The whole path edited in Settings ────────────────────────────────

    #[test]
    fn a_version_taken_off_the_path_is_skipped_and_the_step_says_so() {
        let steps = chain_with("9.13.9", "10.3.3", &[off("10.2.5")]);
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        assert_eq!(step.to_string(), "09.13.09 → 10.03.03");
        assert!(step.warnings[0].contains("10.02.05"));
        assert!(step.warnings[0].contains("taken off the path"));
        // What is known about both steps it spans goes with it - DNN 10 drops Telerik - and it was never tested.
        assert!(step.removes_telerik);
        assert!(step.tested.is_empty());
        assert_eq!(texts(step), vec![".NET Framework 4.8", "SQL Server 2017"]);
    }

    #[test]
    fn skipping_a_step_done_by_hand_keeps_it_done_by_hand() {
        let steps = chain_with("5.6.8", "7.4.2", &[off("6.2.8")]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].method, DnnUpgradeMethod::Manual);
    }

    #[test]
    fn a_changed_version_replaces_what_is_known_lower_too() {
        let mut lowered = UpgradeStop {
            version: "10.3.3".into(),
            net_framework_release: 528040,
            sql_server_major: 13,
            replaces_listed: true,
            note: "Runs on 2016 here.".into(),
            ..Default::default()
        };
        let step = chain_with("10.2.5", "10.3.3", std::slice::from_ref(&lowered)).remove(0);
        assert_eq!(requirement(&step, DnnRequirementKind::SqlServer)[0].text, "SQL Server 2016");
        // Left as it was, the requirement keeps DNN Manager's own wording and source.
        let known = STEPS.iter().find(|s| s.to == Version::new(10, 3, 3)).unwrap();
        let known_net = requirement(known, DnnRequirementKind::NetFramework)[0];
        assert_eq!(known_net, requirement(&step, DnnRequirementKind::NetFramework)[0]);
        assert!(step.warnings.iter().any(|w| w == "Runs on 2016 here."));
        assert!(step.warnings.iter().any(|w| w.starts_with("You changed what DNN 10.03.03 needs")));
        // Still the path's own step, with what was tested on it.
        assert!(!step.tested.is_empty());

        // No minimum at all: nothing of the kind is checked.
        lowered.sql_server_major = 0;
        let none = chain_with("10.2.5", "10.3.3", &[lowered]).remove(0);
        assert!(requirement(&none, DnnRequirementKind::SqlServer).is_empty());
    }

    #[test]
    fn a_change_of_the_newest_version_goes_with_a_target_past_it() {
        let lowered = UpgradeStop {
            version: "10.3.3".into(),
            net_framework_release: 528040,
            sql_server_major: 13,
            replaces_listed: true,
            ..Default::default()
        };
        let past = chain_with("10.3.3", "10.4.0", &[lowered]).remove(0);
        assert_eq!(requirement(&past, DnnRequirementKind::SqlServer)[0].text, "SQL Server 2016");
    }

    #[test]
    fn the_path_as_settings_show_it_and_back() {
        assert!(edits_for(&path_with(&[]), &[]).is_empty(), "DNN's own path needs no edits.");
        let mut path: Vec<DnnPathVersion> = path_with(&[])
            .into_iter()
            .filter(|p| p.version != Version::new(10, 2, 5))
            .map(|p| if p.version == Version::new(10, 3, 3) { DnnPathVersion { sql_server_major: 15, ..p } } else { p })
            .collect();
        path.push(DnnPathVersion {
            version: Version::new(10, 4, 2),
            net_framework_release: 0,
            sql_server_major: 16,
            note: "New.".into(),
        });
        let edits = edits_for(&path, &[]);
        assert_eq!(
            edits
                .iter()
                .map(|e| format!(
                    "{} {}",
                    e.version,
                    if e.removed {
                        "off"
                    } else if e.replaces_listed {
                        "replaces"
                    } else {
                        "adds"
                    }
                ))
                .collect::<Vec<_>>(),
            vec!["10.2.5 off", "10.3.3 replaces", "10.4.2 adds"]
        );
        path.sort_by_key(|p| p.version);
        assert_eq!(path_with(&edits), path);
        assert_eq!(upgrade_stops_problem(&edits), None);
    }

    #[test]
    fn a_version_added_before_dnn_manager_listed_it_keeps_only_raising() {
        // Saved by an older DNN Manager: 10.3.3 added with nothing new - it must not take DNN 10's minimums away.
        let before = vec![stop("10.3.3", 0, 0, "Ours.")];
        let edits = edits_for(&path_with(&before), &before);
        assert_eq!(edits, before);
        assert!(!edits[0].replaces_listed);
    }

    #[test]
    fn taking_off_the_path_has_rules() {
        // The path's own old versions can be taken off and changed - they are done by hand anyway.
        assert_eq!(
            upgrade_stops_problem(&[
                off("2.0.4"),
                UpgradeStop {
                    version: "6.2.8".into(),
                    replaces_listed: true,
                    note: "By hand.".into(),
                    ..Default::default()
                }
            ]),
            None
        );
        let all_off: Vec<UpgradeStop> = VERSIONS.iter().map(|v| off(&v.to_string())).collect();
        assert!(upgrade_stops_problem(&all_off).unwrap().contains("at least one version"));
        let mut with_one = all_off.clone();
        with_one.push(stop("11.0.0", 0, 0, ""));
        assert_eq!(upgrade_stops_problem(&with_one), None);
        assert!(upgrade_stops_problem(&[off("10.2.5"), stop("10.2.5", 0, 0, "")]).unwrap().contains("twice"));
    }

    #[test]
    fn a_version_can_be_written_as_dnn_writes_it() {
        assert_eq!(upgrade_stop_version("10.4.2"), Some(Version::new(10, 4, 2)));
        assert_eq!(upgrade_stop_version(" 10.04.02 "), Some(Version::new(10, 4, 2)));
        assert_eq!(upgrade_stop_version("9.13.9"), Some(Version::new(9, 13, 9)));
    }

    #[test]
    fn anything_else_isnt_a_version() {
        for text in ["", "10", "10.4", "10.4.2.1", "v10.4.2", "10.4.x", "10.4.-2", "10.4.2; calc", "9999.0.0"] {
            assert_eq!(upgrade_stop_version(text), None, "{text}");
            assert!(upgrade_stops_problem(&[stop(text, 0, 0, "")]).is_some(), "{text}");
        }
    }

    #[test]
    fn what_can_be_added_has_rules() {
        assert_eq!(upgrade_stops_problem(&[]), None);
        assert_eq!(upgrade_stops_problem(&[stop("10.4.2", 533320, 18, "A note."), stop("11.0.0", 0, 0, "")]), None);
        // A later DNN Manager may list it - the saved stop has to stay allowed then.
        assert_eq!(upgrade_stops_problem(&[stop("10.3.3", 0, 0, "")]), None);
        assert!(upgrade_stops_problem(&[stop("7.0.0", 0, 0, "")]).unwrap().contains("older than 7.4.2"));
        assert!(
            upgrade_stops_problem(&[stop("10.4.2", 0, 0, ""), stop("10.04.02", 0, 0, "")]).unwrap().contains("twice")
        );
        assert!(upgrade_stops_problem(&[stop("10.4.2", 0, -1, "")]).is_some());
        let long = "x".repeat(MAX_UPGRADE_STOP_NOTE + 1);
        assert!(upgrade_stops_problem(&[stop("10.4.2", 0, 0, &long)]).is_some());
        assert!(upgrade_stops_problem(&[stop("10.4.2", 0, 0, "two\nlines")]).is_some());
        assert!(
            is_manual(Version::new(2, 0, 4)) && is_manual(Version::new(6, 2, 8)) && !is_manual(Version::new(10, 3, 3))
        );
    }
}
