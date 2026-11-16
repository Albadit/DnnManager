//! Edit DNN upgrade path (`Controls/DnnUpgradePathDialog.xaml(.cs)`, Settings → DNN releases → Edit upgrade path…):
//! the whole path, a row per version - the version, what the step to it needs, a note for the plan. A version DNN
//! Manager knows can be changed or taken off, one DNN lists later added; the path can be exported to a file and
//! imported from one (`UpgradePathFile`, ported here as [`path_file`]). Saved as its edits of the path DNN Manager
//! knows (`edits_for`), so a later DNN Manager's new versions still come in.
//!
//! Emits `DialogEvent::Done(Vec<UpgradeStop>)` - the edits to save as `projects.dnnUpgradeStops` ([`set_upgrade_stops`],
//! then [`saved_toast`], or [`not_saved`] when the save fails) - and [`PathDialogEvent`] for what the app shows: an
//! error dialog and a toast. Import… and Export… ask for the file through GPUI's own Windows file dialogs (the app's
//! OS dialog): they have no file-type filter, so the title says it is a `.json` and the export suggests its name.

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    PathPromptOptions, SharedString, Subscription, Window, div, prelude::*, px,
};

use dnnm_application::settings::Settings;
use dnnm_dnn::upgrades::{
    DnnPathVersion, DnnRequirementKind, DnnUpgradeStep, NET_FRAMEWORK_CHOICES, SOURCE, SQL_SERVER_CHOICES, STEPS,
    UpgradeStop, VERSIONS, edits_for, is_manual, net_framework_name, path_with, sql_server_name, upgrade_stop_version,
    upgrade_stops_problem,
};
use dnnm_dnn::version::Version;
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Pad, Tooltip, icons};

/// "No new minimum".
const NO_NEW_MINIMUM: i32 = 0;
const MAX_NOTE: usize = dnnm_dnn::upgrades::MAX_UPGRADE_STOP_NOTE;

/// The columns of a row (`VersionWidth`, `MinimumWidth`, `RemoveWidth`); the note takes the rest.
const VERSION_WIDTH: f32 = 90.0;
const MINIMUM_WIDTH: f32 = 178.0;
const REMOVE_WIDTH: f32 = 36.0;
/// Between the path's columns.
const COLUMN_GAP: f32 = 8.0;

/// What the app shows for the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathDialogEvent {
    /// An error dialog (`Dialogs.Error`).
    Error(String),
    /// A success toast.
    Toast(String),
}

/// The text at the top.
pub const INTRO: &str = "Upgrade DNN stops at every version of this path on the way. When DNN lists a new version on its suggested upgrade path, add it here; change what a version needs, or take one off. A version DNN released without listing it needs nothing: pick it in Upgrade DNN as it is.";

/// What DNN Manager knows (`Known`).
pub fn known_text() -> String {
    let newest = VERSIONS.last().map(DnnUpgradeStep::name).unwrap_or_default();
    format!(
        "DNN Manager knows DNN's path up to DNN {newest}. What a version needs is checked before the step to it - and, past the newest version, before the steps after it."
    )
}

/// The file dialogs' titles and the export's suggested name (`UpgradePathFile.Filter`: `DNN upgrade path (*.json)`).
pub const IMPORT_TITLE: &str = "Import an upgrade path";
pub const EXPORT_TITLE: &str = "Export the upgrade path";
pub const EXPORT_NAME: &str = "dnn-upgrade-path.json";

/// A row as the form holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowValues {
    pub version: String,
    pub net_framework_release: i32,
    pub sql_server_major: i32,
    pub note: String,
}

impl RowValues {
    pub fn is_empty(&self) -> bool {
        self.version.trim().is_empty()
    }

    /// As the settings rules check it.
    pub fn stop(&self) -> UpgradeStop {
        UpgradeStop {
            // As a plain number: DNN writes its versions as 10.04.02.
            version: upgrade_stop_version(&self.version)
                .map(|v| v.to_string())
                .unwrap_or_else(|| self.version.trim().to_string()),
            net_framework_release: self.net_framework_release,
            sql_server_major: self.sql_server_major,
            note: self.note.trim().to_string(),
            ..Default::default()
        }
    }
}

/// What the rows come to (`Check`): what is wrong, the versions taken off, and the path (None while a row is wrong).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub problem: Option<String>,
    pub removed: Option<String>,
    pub path: Option<Vec<DnnPathVersion>>,
}

/// Checks the rows as the settings rules do.
pub fn check(rows: &[RowValues]) -> Checked {
    let stops: Vec<UpgradeStop> = rows.iter().map(RowValues::stop).collect();
    let problem = if rows.is_empty() {
        Some("The upgrade path needs at least one version - add one, or reset it.".to_string())
    } else if rows.iter().any(RowValues::is_empty) {
        Some("Fill in the version, or remove the row.".to_string())
    } else {
        upgrade_stops_problem(&stops)
    };
    let on_path: Vec<Version> = stops.iter().filter_map(|s| upgrade_stop_version(&s.version)).collect();
    let off: Vec<String> = VERSIONS.iter().filter(|v| !on_path.contains(v)).map(DnnUpgradeStep::name).collect();
    let removed = (!off.is_empty()).then(|| {
        let them = if off.len() == 1 { "it" } else { "them" };
        format!("Taken off the path: DNN {}. Upgrades skip {them} - DNN's path goes through {them}.", off.join(", "))
    });
    let path = problem.is_none().then(|| {
        let mut path: Vec<DnnPathVersion> = stops
            .iter()
            .filter_map(|s| {
                Some(DnnPathVersion {
                    version: upgrade_stop_version(&s.version)?,
                    net_framework_release: s.net_framework_release,
                    sql_server_major: s.sql_server_major,
                    note: s.note.clone(),
                })
            })
            .collect();
        path.sort_by_key(|v| v.version);
        path
    });
    Checked { problem, removed, path }
}

/// What DNN Manager knows the step to one of DNN's own versions needs: its minimum and DNN's words for it ("SQL Server
/// 2008 R2" - a major version alone can't say R2). None for a version that isn't DNN's own.
pub fn known_minimum(version: Option<Version>, kind: DnnRequirementKind) -> Option<(i32, Option<String>)> {
    let version = version.filter(|v| VERSIONS.contains(v))?;
    Some(
        STEPS
            .iter()
            .find(|s| s.to == version)
            .and_then(|s| s.requirements.iter().find(|r| r.kind == kind))
            .map_or((NO_NEW_MINIMUM, None), |r| (r.minimum, Some(r.text.clone()))),
    )
}

/// A minimum's list (`Minimums`): "No new minimum" and `choices` - and a saved value that isn't one of them, so it
/// isn't lost. What DNN Manager knows for the version is said in DNN's words, and in the tooltip.
pub fn minimum_choices(
    choices: &[i32],
    saved: i32,
    name: fn(i32) -> String,
    known: Option<&(i32, Option<String>)>,
) -> (Vec<(i32, String)>, Option<String>) {
    let label = |value: i32| match known {
        Some((minimum, Some(text))) if value == *minimum => text.clone(),
        _ if value == NO_NEW_MINIMUM => "No new minimum".to_string(),
        _ => name(value),
    };
    let tip = known.map(|(minimum, text)| {
        format!("DNN's path: {}", if text.is_none() { "no new minimum".to_string() } else { label(*minimum) })
    });
    let mut values: Vec<i32> = choices
        .iter()
        .copied()
        .chain([saved, known.map_or(NO_NEW_MINIMUM, |k| k.0)])
        .filter(|c| *c != NO_NEW_MINIMUM)
        .collect();
    values.sort_unstable();
    values.dedup();
    let items = std::iter::once(NO_NEW_MINIMUM).chain(values).map(|v| (v, label(v))).collect();
    (items, tip)
}

/// The settings' rows for `stops` (`projects.dnnUpgradeStops`), in place of the ones before.
pub fn set_upgrade_stops(settings: &mut Settings, stops: &[UpgradeStop]) {
    const KEY: &str = "projects.dnnUpgradeStops";
    // set_list clears the list's rows (`KEY[…`) and writes its count; the items' fields go after it.
    settings.set_list(KEY, &[] as &[String]);
    settings.set(KEY, stops.len().to_string());
    for (i, stop) in stops.iter().enumerate() {
        let field = |name: &str| format!("{KEY}[{i}].{name}");
        settings.set(&field("version"), stop.version.clone());
        settings.set(&field("netFrameworkRelease"), stop.net_framework_release.to_string());
        settings.set(&field("sqlServerMajor"), stop.sql_server_major.to_string());
        settings.set(&field("note"), stop.note.clone());
        settings.set(&field("replacesListed"), if stop.replaces_listed { "true" } else { "false" });
        settings.set(&field("removed"), if stop.removed { "true" } else { "false" });
    }
}

/// The toast once the path is saved (`EditUpgradePath_Click`).
pub fn saved_toast(stops: &[UpgradeStop]) -> &'static str {
    if stops.is_empty() { "Upgrade path saved: DNN's own, as DNN Manager knows it." } else { "Upgrade path saved." }
}

/// The error when it couldn't be saved.
pub fn not_saved(error: &str) -> String {
    format!("The upgrade path could not be saved: {error}")
}

/// DNN's upgrade path as a file (`UpgradePathFile`): the whole path, each version with what the step to it needs and
/// its note - to keep, or to use on another PC. A file is data only: every value goes through the settings rules, and
/// an import only fills the dialog - Save keeps it.
pub mod path_file {
    use std::path::Path;

    use serde_json::Value;

    use super::*;

    pub const KIND: &str = "dnnmanager.upgradePath";
    /// A path is a few kilobytes; more isn't one.
    pub const MAX_BYTES: u64 = 1024 * 1024;
    pub const MAX_VERSIONS: usize = 1000;

    /// `path` as the file's text, oldest first - indented as System.Text.Json writes it.
    pub fn to_json(path: &[DnnPathVersion]) -> String {
        let mut sorted: Vec<&DnnPathVersion> = path.iter().collect();
        sorted.sort_by_key(|v| v.version);
        let quote = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
        let known = VERSIONS.last().map(|v| v.to_string()).unwrap_or_default();
        let entries: Vec<String> = sorted
            .iter()
            .map(|v| {
                format!(
                    "    {{\n      \"version\": {},\n      \"netFrameworkRelease\": {},\n      \"sqlServerMajor\": {},\n      \"note\": {}\n    }}",
                    quote(&v.version.to_string()),
                    v.net_framework_release,
                    v.sql_server_major,
                    quote(&v.note)
                )
            })
            .collect();
        format!(
            "{{\n  \"kind\": {},\n  \"knownUpTo\": {},\n  \"path\": [\n{}\n  ]\n}}",
            quote(KIND),
            quote(&known),
            entries.join(",\n")
        )
    }

    pub fn write(file: &Path, path: &[DnnPathVersion]) -> Result<(), String> {
        std::fs::write(file, to_json(path)).map_err(|e| e.to_string())
    }

    pub fn read(file: &Path) -> Result<Vec<DnnPathVersion>, String> {
        let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
        if size > MAX_BYTES {
            return Err("it is too big to be an upgrade path.".into());
        }
        let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        parse(text.trim_start_matches('\u{feff}'))
    }

    /// The path in `json`, oldest first - with the versions this DNN Manager lists that the one which wrote it didn't
    /// know yet - or why it can't be used. Comments and trailing commas are allowed: it can be edited by hand; names
    /// are read whatever their case.
    pub fn parse(json: &str) -> Result<Vec<DnnPathVersion>, String> {
        let not_ours = "it isn't an upgrade path exported by DNN Manager.";
        let model: Value = serde_json::from_str(&relaxed(json))
            .map_err(|e| format!("it isn't an upgrade path exported by DNN Manager ({e})."))?;
        let Some(model) = model.as_object() else { return Err(not_ours.into()) };
        let field = |object: &serde_json::Map<String, Value>, name: &str| {
            object.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
        };
        let text = |value: Option<Value>, name: &str| -> Result<Option<String>, String> {
            match value {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s)),
                Some(other) => Err(format!(
                    "it isn't an upgrade path exported by DNN Manager (the value of '{name}' isn't text: {other})."
                )),
            }
        };
        let number = |value: Option<Value>, name: &str| -> Result<i32, String> {
            match value {
                None => Ok(0),
                Some(Value::Number(n)) if n.as_i64().is_some_and(|n| i32::try_from(n).is_ok()) => {
                    Ok(n.as_i64().unwrap_or(0) as i32)
                }
                Some(other) => Err(format!(
                    "it isn't an upgrade path exported by DNN Manager (the value of '{name}' isn't a whole number: {other})."
                )),
            }
        };
        if text(field(model, "kind"), "kind")?.as_deref() != Some(KIND) {
            return Err(not_ours.into());
        }
        let entries = match field(model, "path") {
            Some(Value::Array(entries)) => entries,
            None | Some(Value::Null) => return Err(not_ours.into()),
            Some(other) => {
                return Err(format!(
                    "it isn't an upgrade path exported by DNN Manager (the path isn't a list: {other})."
                ));
            }
        };
        if entries.is_empty() {
            return Err("it has no versions.".into());
        }
        if entries.len() > MAX_VERSIONS {
            return Err(format!("it has more than {MAX_VERSIONS} versions."));
        }
        let mut stops = Vec::new();
        for entry in &entries {
            let empty = serde_json::Map::new();
            let entry = match entry {
                Value::Object(o) => o,
                Value::Null => &empty,
                other => {
                    return Err(format!(
                        "it isn't an upgrade path exported by DNN Manager (a version isn't an object: {other})."
                    ));
                }
            };
            stops.push(UpgradeStop {
                version: text(field(entry, "version"), "version")?.unwrap_or_default(),
                net_framework_release: number(field(entry, "netFrameworkRelease"), "netFrameworkRelease")?,
                sql_server_major: number(field(entry, "sqlServerMajor"), "sqlServerMajor")?,
                note: text(field(entry, "note"), "note")?.unwrap_or_default(),
                ..Default::default()
            });
        }
        if let Some(problem) = upgrade_stops_problem(&stops) {
            return Err(problem);
        }
        let mut path: Vec<DnnPathVersion> = stops
            .iter()
            .filter_map(|s| {
                Some(DnnPathVersion {
                    version: upgrade_stop_version(&s.version)?,
                    net_framework_release: s.net_framework_release,
                    sql_server_major: s.sql_server_major,
                    note: s.note.trim().to_string(),
                })
            })
            .collect();
        let known_up_to = text(field(model, "knownUpTo"), "knownUpTo")?;
        if let Some(known_up_to) = known_up_to.as_deref().and_then(upgrade_stop_version) {
            let later: Vec<DnnPathVersion> = VERSIONS
                .iter()
                .filter(|v| **v > known_up_to && path.iter().all(|p| p.version != **v))
                .map(|v| dnnm_dnn::upgrades::listed(*v))
                .collect();
            path.extend(later);
        }
        path.sort_by_key(|v| v.version);
        Ok(path)
    }

    /// JSON without comments (`//`, `/* */`) and trailing commas - outside strings.
    fn relaxed(json: &str) -> String {
        let chars: Vec<char> = json.chars().collect();
        let mut out = String::with_capacity(json.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '"' {
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        out.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if chars[i - 1] == '"' {
                        break;
                    }
                }
                continue;
            }
            if c == '/' && chars.get(i + 1) == Some(&'/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && chars.get(i + 1) == Some(&'*') {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                continue;
            }
            if c == ',' {
                // A comma before the end of an object or a list (comments between allowed) is left out.
                let mut j = i + 1;
                loop {
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'/') {
                        while j < chars.len() && chars[j] != '\n' {
                            j += 1;
                        }
                    } else if chars.get(j) == Some(&'/') && chars.get(j + 1) == Some(&'*') {
                        j += 2;
                        while j < chars.len() && !(chars[j] == '*' && chars.get(j + 1) == Some(&'/')) {
                            j += 1;
                        }
                        j += 2;
                    } else {
                        break;
                    }
                }
                if matches!(chars.get(j), Some('}') | Some(']')) {
                    i += 1;
                    continue;
                }
            }
            out.push(c);
            i += 1;
        }
        out
    }
}

/// Which minimum of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Minimum {
    NetFramework,
    SqlServer,
}

struct Row {
    /// One of DNN's own: changed by what it needs, taken off by removing it - its number stays.
    listed: Option<Version>,
    version: Entity<TextInput>,
    net_framework: i32,
    sql_server: i32,
    net_choices: (Vec<(i32, String)>, Option<String>),
    sql_choices: (Vec<(i32, String)>, Option<String>),
    note: Entity<TextInput>,
    _subscriptions: [Subscription; 2],
}

pub struct DnnUpgradePathDialog {
    rows: Vec<Row>,
    /// Gives each row's elements ids of their own across Add and Remove.
    ids: Vec<usize>,
    next_id: usize,
    original: Vec<UpgradeStop>,
    original_path: Vec<DnnPathVersion>,
    open: Option<(usize, Minimum)>,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent<Vec<UpgradeStop>>> for DnnUpgradePathDialog {}
impl EventEmitter<PathDialogEvent> for DnnUpgradePathDialog {}

impl DnnUpgradePathDialog {
    /// `stops`: the edits saved now (`projects.dnnUpgradeStops`).
    pub fn new(stops: Vec<UpgradeStop>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let original_path = path_with(&stops);
        let mut dialog = Self {
            rows: Vec::new(),
            ids: Vec::new(),
            next_id: 0,
            original: stops,
            original_path: original_path.clone(),
            open: None,
            focus,
        };
        dialog.fill(&original_path, cx);
        dialog
    }

    /// The rows for `path`, newest first.
    fn fill(&mut self, path: &[DnnPathVersion], cx: &mut Context<Self>) {
        self.rows.clear();
        self.ids.clear();
        self.open = None;
        let mut sorted: Vec<&DnnPathVersion> = path.iter().collect();
        sorted.sort_by(|a, b| b.version.cmp(&a.version));
        for version in sorted {
            let row = self.row(Some(version), cx);
            self.push(row, false);
        }
        cx.notify();
    }

    fn push(&mut self, row: Row, at_top: bool) {
        let id = self.next_id;
        self.next_id += 1;
        if at_top {
            self.rows.insert(0, row);
            self.ids.insert(0, id);
        } else {
            self.rows.push(row);
            self.ids.push(id);
        }
    }

    fn row(&mut self, version: Option<&DnnPathVersion>, cx: &mut Context<Self>) -> Row {
        let listed = version.map(|v| v.version).filter(|v| VERSIONS.contains(v));
        let text = match (version, listed) {
            (None, _) => String::new(),
            (Some(_), Some(v)) => DnnUpgradeStep::name(&v),
            (Some(v), None) => v.version.to_string(),
        };
        let version_input = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(text, cx);
            input
        });
        let note = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_text(version.map(|v| v.note.clone()).unwrap_or_default(), cx);
            input
        });
        let net = version.map_or(NO_NEW_MINIMUM, |v| v.net_framework_release);
        let sql = version.map_or(NO_NEW_MINIMUM, |v| v.sql_server_major);
        let known_net = known_minimum(listed, DnnRequirementKind::NetFramework);
        let known_sql = known_minimum(listed, DnnRequirementKind::SqlServer);
        let changed = |_: &mut Self, _: Entity<TextInput>, _: &TextChanged, cx: &mut Context<Self>| cx.notify();
        // The note is one line of at most 500 characters (`MaxLength`).
        let note_changed = |_: &mut Self, input: Entity<TextInput>, e: &TextChanged, cx: &mut Context<Self>| {
            if e.0.chars().count() > MAX_NOTE {
                let kept: String = e.0.chars().take(MAX_NOTE).collect();
                input.update(cx, |i, cx| i.set_text(kept, cx));
            }
            cx.notify();
        };
        let subscriptions = [cx.subscribe(&version_input, changed), cx.subscribe(&note, note_changed)];
        Row {
            listed,
            version: version_input,
            net_framework: net,
            sql_server: sql,
            net_choices: minimum_choices(&NET_FRAMEWORK_CHOICES, net, net_framework_name, known_net.as_ref()),
            sql_choices: minimum_choices(&SQL_SERVER_CHOICES, sql, sql_server_name, known_sql.as_ref()),
            note,
            _subscriptions: subscriptions,
        }
    }

    fn values(&self, cx: &App) -> Vec<RowValues> {
        self.rows
            .iter()
            .map(|r| RowValues {
                version: r.version.read(cx).text().to_string(),
                net_framework_release: r.net_framework,
                sql_server_major: r.sql_server,
                note: r.note.read(cx).text().to_string(),
            })
            .collect()
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.row(None, cx);
        row.version.read(cx).focus_handle(cx).focus(window);
        self.push(row, true);
        cx.notify();
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.rows.len() {
            self.rows.remove(index);
            self.ids.remove(index);
            self.open = None;
            cx.notify();
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let checked = check(&self.values(cx));
        if let Some(path) = checked.path.filter(|p| *p != self.original_path) {
            cx.emit(DialogEvent::Done(edits_for(&path, &self.original)));
        }
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("{IMPORT_TITLE} (.json)").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.import_from(&file, cx));
            }
        })
        .detach();
    }

    /// Fills the rows from an exported file; Save keeps it.
    pub fn import_from(&mut self, file: &std::path::Path, cx: &mut Context<Self>) {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match path_file::read(file) {
            Err(e) => {
                cx.emit(PathDialogEvent::Error(format!("{name} can't be imported: {e} The path stays as it was.")))
            }
            Ok(path) => {
                self.fill(&path, cx);
                cx.emit(PathDialogEvent::Toast(format!("Upgrade path imported from {name} - Save keeps it.")));
            }
        }
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        if check(&self.values(cx)).path.is_none() {
            return;
        }
        let documents = dnnm_infrastructure::paths::app_data_directory();
        let start = documents.parent().map(std::path::Path::to_path_buf).unwrap_or(documents);
        let chosen = cx.prompt_for_new_path(&start, Some(EXPORT_NAME));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(file))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.export_to(&file, cx));
            }
        })
        .detach();
    }

    /// Writes the path as the rows have it.
    pub fn export_to(&mut self, file: &std::path::Path, cx: &mut Context<Self>) {
        let Some(path) = check(&self.values(cx)).path else { return };
        match path_file::write(file, &path) {
            Err(e) => cx.emit(PathDialogEvent::Error(format!("The upgrade path could not be exported: {e}"))),
            Ok(()) => cx.emit(PathDialogEvent::Toast(format!("Upgrade path exported to {}.", file.display()))),
        }
    }

    fn minimum(&self, index: usize, which: Minimum, cx: &mut Context<Self>) -> AnyElement {
        let id = self.ids[index];
        let row = &self.rows[index];
        let (choices, tip, value) = match which {
            Minimum::NetFramework => (&row.net_choices.0, &row.net_choices.1, row.net_framework),
            Minimum::SqlServer => (&row.sql_choices.0, &row.sql_choices.1, row.sql_server),
        };
        let open = self.open == Some((index, which));
        let key = if which == Minimum::NetFramework { "net" } else { "sql" };
        let shown = choices.iter().find(|c| c.0 == value).map(|c| c.1.clone()).unwrap_or_default();
        let options = choices
            .iter()
            .enumerate()
            .map(|(i, (choice, label))| {
                let choice = *choice;
                dialog::option(SharedString::from(format!("{key}-{id}-{i}")), label.clone(), choice == value, cx)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(at) = this.ids.iter().position(|x| *x == id) {
                            match which {
                                Minimum::NetFramework => this.rows[at].net_framework = choice,
                                Minimum::SqlServer => this.rows[at].sql_server = choice,
                            }
                        }
                        this.open = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .id(SharedString::from(format!("{key}-{id}")))
            .w(px(170.0))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let Some(at) = this.ids.iter().position(|x| *x == id) else { return };
                this.open = if this.open == Some((at, which)) { None } else { Some((at, which)) };
                cx.notify();
            }))
            .when_some(tip.clone(), |d, tip| d.tooltip(Tooltip::text(tip)))
            .child(dialog::select(SharedString::from(format!("{key}-{id}-box")), shown, open, options, cx))
            .into_any_element()
    }

    fn render_row(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let id = self.ids[index];
        let row = &self.rows[index];
        let (control_height, foreground) = {
            let theme = Theme::get(cx);
            (theme.metrics.control_height, theme.colors.text_primary)
        };
        let version: AnyElement = match row.listed {
            // Read as text, not as a box to type in.
            Some(listed) => div()
                .id(SharedString::from(format!("version-{id}")))
                .h(control_height)
                .flex()
                .items_center()
                .pad(px(0.0), px(6.0))
                .font_family("Cascadia Mono")
                .child(DnnUpgradeStep::name(&listed))
                .tooltip(Tooltip::text(if is_manual(listed) {
                    "On DNN's path. Done by hand: GitHub has no packages of it."
                } else {
                    "On DNN's path, as DNN Manager knows it"
                }))
                .into_any_element(),
            None => div()
                .id(SharedString::from(format!("version-{id}")))
                .font_family("Cascadia Mono")
                .tooltip(Tooltip::text("A version you added - three numbers, e.g. 10.4.2"))
                .child(dialog::text_field(&row.version, window, cx))
                .into_any_element(),
        };
        let remove_tip = if row.listed.is_some() { "Take this version off the path" } else { "Remove this version" };
        let net = self.minimum(index, Minimum::NetFramework, cx);
        let sql = self.minimum(index, Minimum::SqlServer, cx);
        // The columns 8 apart (a minimum's column is its 170 list); the remove button 4 after the note.
        div()
            .flex()
            .items_center()
            .gap(px(COLUMN_GAP))
            .child(div().w(px(VERSION_WIDTH)).flex_none().child(version))
            .child(div().w(px(MINIMUM_WIDTH - COLUMN_GAP)).flex_none().child(net))
            .child(div().w(px(MINIMUM_WIDTH - COLUMN_GAP)).flex_none().child(sql))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(div().flex_1().min_w_0().child(dialog::text_field(&row.note, window, cx)))
                    .child(
                        div().w(px(REMOVE_WIDTH - 4.0)).flex_none().child(
                            ui::icon_button(
                                SharedString::from(format!("remove-{id}")),
                                icons::DELETE,
                                foreground,
                                true,
                                cx,
                            )
                            .tooltip(Tooltip::text(remove_tip))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(at) = this.ids.iter().position(|x| *x == id) {
                                    this.remove(at, cx);
                                }
                            })),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for DnnUpgradePathDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DnnUpgradePathDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let checked = check(&self.values(cx));
        let can_export = checked.problem.is_none();
        let can_save = checked.path.as_ref().is_some_and(|p| *p != self.original_path);
        let rows: Vec<AnyElement> = (0..self.rows.len()).map(|i| self.render_row(i, window, cx)).collect();
        let header = |text: &'static str, width: Option<f32>, cx: &App| {
            let label = dialog::label(text, cx);
            match width {
                Some(w) => label.w(px(w)).flex_none(),
                None => label.flex_1(),
            }
        };

        let stack = |gap: f32| div().w_full().flex().flex_col().gap(px(gap));
        let source = stack(4.0).child(div().w_full().child(INTRO)).child(div().child(
            ui::link("upgrade-path-source", "Open DNN's suggested upgrade path", cx).on_click(cx.listener(
                |_, _: &ClickEvent, _, cx| {
                    if let Err(e) = dnnm_infrastructure::shell::open_url(SOURCE) {
                        cx.emit(PathDialogEvent::Error(e));
                    }
                },
            )),
        ));
        let heading = stack(4.0)
            .child(dialog::label("The path", cx).font_weight(gpui::FontWeight::SEMIBOLD))
            .child(dialog::hint(known_text(), cx));
        // The labels 4 above the rows, the rows 6 apart.
        let table = stack(4.0)
            .child(
                div()
                    .flex()
                    .gap(px(COLUMN_GAP))
                    .child(header("Version", Some(VERSION_WIDTH), cx))
                    .child(header("Needs .NET Framework", Some(MINIMUM_WIDTH - COLUMN_GAP), cx))
                    .child(header("Needs SQL Server", Some(MINIMUM_WIDTH - COLUMN_GAP), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap(px(4.0))
                            .child(header("Note in the upgrade plan", None, cx))
                            .child(div().w(px(REMOVE_WIDTH - 4.0)).flex_none()),
                    ),
            )
            .when(!rows.is_empty(), |d| d.child(stack(6.0).children(rows)));
        let links =
            stack(4.0).when_some(checked.removed.clone(), |d, removed| d.child(dialog::hint(removed, cx))).child(
                div()
                    .flex()
                    .gap(px(16.0))
                    .child(
                        ui::link("add-version", "Add version", cx)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.add(window, cx))),
                    )
                    .child(
                        ui::link("reset-path", "Reset to DNN Manager's path", cx)
                            .tooltip(Tooltip::text(
                                "The path as DNN Manager came with it - every change made here undone",
                            ))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.fill(&path_with(&[]), cx))),
                    ),
            );

        let body = div().flex().flex_col().gap(px(12.0)).child(source).child(
            stack(14.0).child(heading).child(
                stack(10.0)
                    .child(table)
                    .child(links)
                    .children(dialog::field_error(checked.problem.clone().map(SharedString::from), cx)),
            ),
        );

        // The path as a file on the left (to keep, or to use on another PC); Save and Cancel on the right.
        let buttons = div()
            .flex()
            .flex_1()
            .justify_between()
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        ui::button("path-import", "Import…", ui::ButtonKind::Secondary, true, cx)
                            .min_w(px(80.0))
                            .tooltip(Tooltip::text("Fill the path from a file exported before - Save keeps it"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.import(cx))),
                    )
                    .child(
                        ui::button("path-export", "Export…", ui::ButtonKind::Secondary, can_export, cx)
                            .min_w(px(80.0))
                            .tooltip(Tooltip::text("Save the path as it is here to a file"))
                            .when(can_export, |b| {
                                b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.export(cx)))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        ui::button("dialog-ok", "Save", ui::ButtonKind::Primary, can_save, cx)
                            .min_w(px(80.0))
                            .tooltip(Tooltip::text("Save (Enter)"))
                            .when(can_save, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx)))),
                    )
                    .child(
                        ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                            .min_w(px(80.0))
                            .tooltip(Tooltip::text("Cancel (Esc)"))
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
                    ),
            );

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.save(cx);
                }
            }))
            .child(dialog::frame("Edit DNN upgrade path", 860.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(path: &[DnnPathVersion]) -> Vec<RowValues> {
        path.iter()
            .map(|v| RowValues {
                version: if VERSIONS.contains(&v.version) {
                    DnnUpgradeStep::name(&v.version)
                } else {
                    v.version.to_string()
                },
                net_framework_release: v.net_framework_release,
                sql_server_major: v.sql_server_major,
                note: v.note.clone(),
            })
            .collect()
    }

    #[test]
    fn dnns_own_path_checks_out_and_saves_as_no_edits() {
        let own = path_with(&[]);
        let checked = check(&rows_of(&own));
        assert_eq!(checked.problem, None);
        assert_eq!(checked.removed, None);
        assert_eq!(checked.path.as_ref(), Some(&own));
        assert!(edits_for(&own, &[]).is_empty());
    }

    #[test]
    fn rows_are_checked_word_for_word() {
        assert_eq!(
            check(&[]).problem.as_deref(),
            Some("The upgrade path needs at least one version - add one, or reset it.")
        );
        let mut rows = rows_of(&path_with(&[]));
        rows.insert(0, RowValues::default());
        assert_eq!(check(&rows).problem.as_deref(), Some("Fill in the version, or remove the row."));
        rows[0].version = "10.4".into();
        assert_eq!(check(&rows).problem.as_deref(), Some("'10.4' isn't a DNN version - three numbers, e.g. 10.4.2."));
        assert_eq!(check(&rows).path, None);
        rows[0].version = "10.04.02".into();
        assert_eq!(check(&rows).problem, None);
        assert_eq!(rows[0].stop().version, "10.4.2", "kept as a plain number");
        rows[0].note = "x".repeat(MAX_NOTE + 1);
        assert_eq!(
            check(&rows).problem,
            Some(format!("The note of DNN 10.4.2 can have at most {MAX_NOTE} characters."))
        );
    }

    #[test]
    fn versions_taken_off_are_said_and_saved_as_removed() {
        let own = path_with(&[]);
        let rows: Vec<RowValues> = rows_of(&own).into_iter().filter(|r| r.version != "10.02.05").collect();
        let checked = check(&rows);
        assert_eq!(
            checked.removed.as_deref(),
            Some("Taken off the path: DNN 10.02.05. Upgrades skip it - DNN's path goes through it.")
        );
        let edits = edits_for(&checked.path.unwrap(), &[]);
        assert_eq!(edits.len(), 1);
        assert!(edits[0].removed && edits[0].version == "10.2.5");
    }

    #[test]
    fn a_minimums_list_names_dnns_words_and_keeps_a_saved_value() {
        let ten = Version::new(10, 2, 5);
        let known = known_minimum(Some(ten), DnnRequirementKind::SqlServer).unwrap();
        assert_eq!(known.0, 14);
        let (items, tip) = minimum_choices(&SQL_SERVER_CHOICES, 99, sql_server_name, Some(&known));
        assert_eq!(items[0], (0, "No new minimum".to_string()));
        assert!(items.iter().any(|i| i.0 == 99), "a saved value that isn't a choice isn't lost");
        assert_eq!(tip, Some(format!("DNN's path: {}", known.1.clone().unwrap())));
        assert_eq!(known_minimum(Some(Version::new(10, 4, 2)), DnnRequirementKind::SqlServer), None);
        let (_, none_tip) = minimum_choices(&SQL_SERVER_CHOICES, 0, sql_server_name, Some(&(0, None)));
        assert_eq!(none_tip.as_deref(), Some("DNN's path: no new minimum"));
    }

    #[test]
    fn the_path_file_round_trips_and_refuses_what_isnt_one() {
        let mut path = path_with(&[]);
        path.push(DnnPathVersion {
            version: Version::new(10, 4, 2),
            net_framework_release: 528040,
            sql_server_major: 14,
            note: "Mine \"quoted\"".into(),
        });
        let json = path_file::to_json(&path);
        assert!(json.starts_with("{\n  \"kind\": \"dnnmanager.upgradePath\",\n  \"knownUpTo\": "));
        assert_eq!(path_file::parse(&json).unwrap(), path);

        // By hand: comments, trailing commas, names in another case.
        let by_hand = r#"{ // mine
            "Kind": "dnnmanager.upgradePath", /* old */ "knownUpTo": "99.0.0",
            "PATH": [ { "version": "10.4.2", "note": "x", }, ],
        }"#;
        let read = path_file::parse(by_hand).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].version, Version::new(10, 4, 2));

        // A file written before DNN Manager listed its newest versions gets them.
        let old = r#"{"kind":"dnnmanager.upgradePath","knownUpTo":"9.13.9","path":[{"version":"7.4.2"}]}"#;
        let read = path_file::parse(old).unwrap();
        assert_eq!(read.first().map(|v| v.version), Some(Version::new(7, 4, 2)));
        assert!(read.iter().any(|v| v.version == *VERSIONS.last().unwrap()));
        assert!(!read.iter().any(|v| v.version == Version::new(9, 13, 9)));

        let not_ours = "it isn't an upgrade path exported by DNN Manager.";
        assert_eq!(path_file::parse(r#"{"kind":"other","path":[]}"#).unwrap_err(), not_ours);
        assert_eq!(path_file::parse(r#"{"kind":"dnnmanager.upgradePath"}"#).unwrap_err(), not_ours);
        assert_eq!(
            path_file::parse(r#"{"kind":"dnnmanager.upgradePath","path":[]}"#).unwrap_err(),
            "it has no versions."
        );
        assert!(
            path_file::parse("not json").unwrap_err().starts_with("it isn't an upgrade path exported by DNN Manager (")
        );
        assert_eq!(
            path_file::parse(r#"{"kind":"dnnmanager.upgradePath","path":[{"version":"10.4"}]}"#).unwrap_err(),
            "'10.4' isn't a DNN version - three numbers, e.g. 10.4.2."
        );
    }

    #[test]
    fn a_file_too_big_isnt_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.json");
        std::fs::write(&file, vec![b' '; (path_file::MAX_BYTES + 1) as usize]).unwrap();
        assert_eq!(path_file::read(&file).unwrap_err(), "it is too big to be an upgrade path.");
        let file = dir.path().join("path.json");
        path_file::write(&file, &path_with(&[])).unwrap();
        assert_eq!(path_file::read(&file).unwrap(), path_with(&[]));
    }

    #[test]
    fn the_edits_are_saved_as_settings_rows() {
        let mut settings = Settings::default();
        settings.set("projects.dnnUpgradeStops[3].version", "old");
        let stops = vec![UpgradeStop { version: "10.4.2".into(), net_framework_release: 528040, ..Default::default() }];
        set_upgrade_stops(&mut settings, &stops);
        let rows = settings.rows();
        assert_eq!(rows.get("projects.dnnUpgradeStops").map(String::as_str), Some("1"));
        assert_eq!(rows.get("projects.dnnUpgradeStops[0].version").map(String::as_str), Some("10.4.2"));
        assert_eq!(rows.get("projects.dnnUpgradeStops[0].netFrameworkRelease").map(String::as_str), Some("528040"));
        assert_eq!(rows.get("projects.dnnUpgradeStops[0].removed").map(String::as_str), Some("false"));
        assert!(!rows.contains_key("projects.dnnUpgradeStops[3].version"));
        assert_eq!(saved_toast(&[]), "Upgrade path saved: DNN's own, as DNN Manager knows it.");
        assert_eq!(saved_toast(&stops), "Upgrade path saved.");
        assert!(known_text().starts_with("DNN Manager knows DNN's path up to DNN "));
    }
}
