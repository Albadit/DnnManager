//! The Source Control page: a project's files against one of its environments' server, as VS Code's Source Control
//! lays it out - a toolbar (Refresh, Upload, Download, More), the files in sections (Conflicts, Changes, Server
//! changes) as folder trees with their change letters, search and change filters, and on the right the side-by-side
//! diff of the file chosen (the server's copy left, the project's right) or the history.
//!
//! Reading and comparing, and sending, are operations the app runs (`SourceControlEvent`) - so they show in Output
//! with their progress, can be cancelled, and ask their questions there; the page gets their result back
//! ([`SourceControlPage::set_scan`]). A file's two copies for the diff are read off the UI thread by the page itself.
//!
//! Keyboard: Up / Down move through the list, Shift with them selects, Left / Right fold and unfold a folder, Enter
//! compares the file (or folds a folder), Space selects; F7 / Shift+F7 go to the next / previous change in the diff.

use dnnm_ui::px;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::*, uniform_list,
};

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::diff::{Inline, RowKind, SideBySide, side_by_side, text_of};
use dnnm_application::source_control::highlight::{Token, highlight, language_of};
use dnnm_application::source_control::model::{Change, Entry, Environment, HistoryLine};
use dnnm_application::source_control::sync::{Action, BothCopies, TransferItem};
use dnnm_ui::menu::{ContextMenu, MenuItem};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Icon, Pad, Tip};

use crate::source_control_list::{self as list, Filter, Row};

/// What the page reads and saves itself; the app gives it the real one (`dnnm_wiring::source_control`).
pub trait SourceControlBackend: Send + Sync {
    fn environments(&self, project: &str) -> Result<Vec<Environment>, String>;
    fn history(&self, env: &Environment) -> Result<Vec<HistoryLine>, String>;
    fn read_both(&self, project_dir: &Path, env: &Environment, path: &str, cancel: &CancelToken)
    -> Result<BothCopies, String>;
    /// The comparison again from the project and the last comparison's server side - no connection.
    fn reclassify_local(&self, project_dir: &Path, env: &Environment, last: &[Entry]) -> Result<Vec<Entry>, String>;
    /// The last comparison again without connecting: when the server was listed, and the entries; None before one.
    fn cached(&self, project_dir: &Path, env: &Environment) -> Result<Option<(i64, Vec<Entry>)>, String>;
}

/// What the page asks the app to do.
#[derive(Debug, Clone)]
pub enum SourceControlEvent {
    /// Compare the project with the environment's server (an operation).
    Scan { project_dir: PathBuf, env: Environment },
    /// Send these files (an operation), then compare again.
    Transfer { project_dir: PathBuf, env: Environment, items: Vec<TransferItem>, title: String },
    /// Open the environment dialog for `env` - a new one when `is_new`.
    EditEnvironment { env: Environment, is_new: bool },
    DeleteEnvironment(Environment),
    /// Open a folder (the server backups).
    OpenFolder(PathBuf),
    /// Merge a conflict by hand: both copies with markers in the project's file, opened in an editor.
    Merge { project_dir: PathBuf, env: Environment, path: String },
    /// Watch this project's folder (None: nothing) - a change there re-checks the list ([`SourceControlPage::local_changed`]).
    Watch(Option<PathBuf>),
}

/// Where the page is with its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotCompared,
    Working(String),
    Compared { at: String },
    Failed(String),
}

/// A text diff, both ways it is shown, with each side's lines coloured.
struct TextDiff {
    sides: SideBySide,
    inline: Inline,
    /// The coloured pieces of each line of the server's copy (index: line number - 1), and of the project's.
    left: Vec<Vec<(std::ops::Range<usize>, Token)>>,
    right: Vec<Vec<(std::ops::Range<usize>, Token)>>,
}

impl TextDiff {
    fn new(path: &str, left: &str, right: &str) -> Self {
        let sides = side_by_side(left, right);
        let inline = sides.inline();
        let language = language_of(path);
        let (l, r): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
        Self { left: highlight(language, &l), right: highlight(language, &r), sides, inline }
    }

    fn changes(&self, inline: bool) -> &[usize] {
        if inline { &self.inline.changes } else { &self.sides.changes }
    }
}

enum DiffState {
    Loading,
    Text(Box<TextDiff>),
    /// Not text: what is known of each side.
    Binary,
    Failed(String),
}

struct DiffView {
    entry: Entry,
    state: DiffState,
    /// The change shown last (F7).
    at: Option<usize>,
}

pub struct SourceControlPage {
    backend: Arc<dyn SourceControlBackend>,
    focus: FocusHandle,
    list_focus: FocusHandle,
    projects: Vec<(String, PathBuf)>,
    project: Option<String>,
    project_open: bool,
    environments: Vec<Environment>,
    env: Option<String>,
    env_open: bool,
    /// The DNN part picker's list is open.
    scope_open: bool,
    /// Reading the saved comparison.
    loading_cached: Option<Task<()>>,
    entries: Vec<Entry>,
    status: Status,
    search: Entity<TextInput>,
    filter: Filter,
    collapsed: HashSet<String>,
    selected: BTreeSet<String>,
    /// The row the keyboard is on, and where a Shift selection started.
    cursor: Option<usize>,
    anchor: Option<usize>,
    diff: Option<DiffView>,
    diff_task: Option<Task<()>>,
    diff_cancel: CancelToken,
    history: Option<Vec<HistoryLine>>,
    menu: Option<ContextMenu>,
    list_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    /// The diff as one column instead of side by side (kept from file to file).
    inline: bool,
    /// A re-check of the project's side is running (the watcher saw a change).
    rechecking: Option<Task<()>>,
    _subscription: Subscription,
}

impl EventEmitter<SourceControlEvent> for SourceControlPage {}

impl SourceControlPage {
    pub fn new(backend: Arc<dyn SourceControlBackend>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::search("Filter files", cx));
        let subscription = cx.subscribe(&search, |this, _, e: &TextChanged, cx| {
            this.filter.text = e.0.to_string();
            this.cursor = None;
            cx.notify();
        });
        Self {
            backend,
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle().tab_stop(true),
            projects: Vec::new(),
            project: None,
            project_open: false,
            environments: Vec::new(),
            env: None,
            env_open: false,
            scope_open: false,
            loading_cached: None,
            entries: Vec::new(),
            status: Status::NotCompared,
            search,
            filter: Filter::default(),
            collapsed: HashSet::new(),
            selected: BTreeSet::new(),
            cursor: None,
            anchor: None,
            diff: None,
            diff_task: None,
            diff_cancel: CancelToken::new(),
            history: None,
            menu: None,
            list_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            inline: false,
            rechecking: None,
            _subscription: subscription,
        }
    }

    // ─── From the app ────────────────────────────────────────────────────────

    /// The projects to choose from: name and folder. The first is chosen when none is.
    pub fn set_projects(&mut self, projects: Vec<(String, PathBuf)>, cx: &mut Context<Self>) {
        if self.projects == projects {
            return;
        }
        self.projects = projects;
        let still = self.project.as_ref().is_some_and(|p| self.projects.iter().any(|(n, _)| n.eq_ignore_ascii_case(p)));
        if !still {
            let first = self.projects.first().map(|(n, _)| n.clone());
            self.choose_project(first, cx);
        }
        cx.notify();
    }

    /// The environments read again (after one was saved or deleted); `choose` is chosen when given.
    pub fn reload_environments(&mut self, choose: Option<String>, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else { return };
        self.environments = self.backend.environments(&project).unwrap_or_default();
        let keep = choose.or(self.env.clone()).filter(|id| self.environments.iter().any(|e| &e.id == id));
        if keep != self.env {
            self.clear_comparison();
        }
        self.env = keep.or_else(|| self.environments.first().map(|e| e.id.clone()));
        self.load_cached(cx);
        cx.notify();
    }

    /// The saved comparison of the environment chosen, shown at once - the server as it was last listed, the project
    /// as it is now - until a Refresh reads the server again.
    fn load_cached(&mut self, cx: &mut Context<Self>) {
        if self.status != Status::NotCompared {
            return;
        }
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        let backend = self.backend.clone();
        let env_id = env.id.clone();
        self.loading_cached = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { backend.cached(&project_dir, &env) }).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_cached = None;
                if this.env.as_deref() != Some(env_id.as_str()) || this.status != Status::NotCompared {
                    return;
                }
                if let Ok(Some((at, entries))) = found {
                    this.entries = entries;
                    this.status = Status::Compared { at: format!("{} (saved - Refresh to read the server)", time_text(at)) };
                    cx.notify();
                }
            });
        }));
    }

    /// What a comparison found, for the environment it was of.
    pub fn set_scan(&mut self, env_id: &str, entries: Vec<Entry>, at: String, cx: &mut Context<Self>) {
        if self.env.as_deref() != Some(env_id) {
            return;
        }
        // The environment's trusted key may be new: read it again.
        if let Some(project) = self.project.clone() {
            self.environments = self.backend.environments(&project).unwrap_or_default();
        }
        self.entries = entries;
        let known: HashSet<String> = self.entries.iter().map(|e| e.path.clone()).collect();
        self.selected.retain(|p| known.contains(p));
        self.status = Status::Compared { at };
        if let Some(diff) = &self.diff
            && let Some(entry) = self.entries.iter().find(|e| e.path == diff.entry.path).cloned()
        {
            self.open_diff(entry, cx);
        }
        if self.history.is_some() {
            self.show_history(cx);
        }
        cx.notify();
    }

    pub fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }

    /// The environment chosen.
    pub fn environment(&self) -> Option<&Environment> {
        self.env.as_ref().and_then(|id| self.environments.iter().find(|e| &e.id == id))
    }

    fn project_dir(&self) -> Option<PathBuf> {
        let project = self.project.as_ref()?;
        self.projects.iter().find(|(n, _)| n.eq_ignore_ascii_case(project)).map(|(_, d)| d.clone())
    }

    fn clear_comparison(&mut self) {
        self.entries.clear();
        self.selected.clear();
        self.cursor = None;
        self.diff = None;
        self.diff_cancel.cancel();
        self.history = None;
        self.status = Status::NotCompared;
    }

    fn choose_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        self.project = project;
        self.env = None;
        self.clear_comparison();
        self.reload_environments(None, cx);
        cx.emit(SourceControlEvent::Watch(self.project_dir()));
    }

    /// Files changed in the project's folder (the app's watcher, after a quiet moment): the list is checked again
    /// against the server as the last comparison read it - without connecting.
    pub fn local_changed(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Compared { .. }) || self.rechecking.is_some() {
            return;
        }
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        let (backend, last) = (self.backend.clone(), self.entries.clone());
        let env_id = env.id.clone();
        self.rechecking = Some(cx.spawn(async move |this, cx| {
            let entries = cx
                .background_executor()
                .spawn(async move { backend.reclassify_local(&project_dir, &env, &last) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rechecking = None;
                if this.env.as_deref() != Some(env_id.as_str()) || !matches!(this.status, Status::Compared { .. }) {
                    return;
                }
                if let Ok(entries) = entries {
                    let known: HashSet<String> = entries.iter().map(|e| e.path.clone()).collect();
                    this.selected.retain(|p| known.contains(p));
                    this.entries = entries;
                    // The diff shown follows its file.
                    if let Some(diff) = &this.diff
                        && let Some(entry) = this.entries.iter().find(|e| e.path == diff.entry.path).cloned()
                        && entry != diff.entry
                    {
                        this.open_diff(entry, cx);
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn choose_env(&mut self, id: String, cx: &mut Context<Self>) {
        if self.env.as_deref() != Some(id.as_str()) {
            self.env = Some(id);
            self.clear_comparison();
            self.load_cached(cx);
            cx.notify();
        }
    }

    // ─── Actions ─────────────────────────────────────────────────────────────

    /// Compare again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) {
            cx.emit(SourceControlEvent::Scan { project_dir, env });
        }
    }

    fn selected_entries(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| self.selected.contains(&e.path)).collect()
    }

    /// The entries an action applies to: the selected ones, else every one shown.
    fn targets(&self) -> Vec<&Entry> {
        let selected = self.selected_entries();
        if !selected.is_empty() {
            return selected;
        }
        self.entries.iter().filter(|e| e.change != Change::Unchanged && self.shown(e)).collect()
    }

    fn shown(&self, entry: &Entry) -> bool {
        let rows = list::rows(std::slice::from_ref(entry), &self.filter, &HashSet::new());
        rows.iter().any(|r| matches!(r, Row::File { .. }))
    }

    fn send(&mut self, entries: Vec<Entry>, action: Action, resolved: bool, title: String, cx: &mut Context<Self>) {
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        if entries.is_empty() {
            return;
        }
        let items = entries.iter().map(|e| TransferItem::from_entry(e, action, resolved)).collect();
        cx.emit(SourceControlEvent::Transfer { project_dir, env, items, title });
    }

    fn upload(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = self.targets().into_iter().filter(|e| e.change.uploads()).cloned().collect();
        let title = format!("Upload {} file(s)", entries.len());
        self.send(entries, Action::Upload, false, title, cx);
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = self.targets().into_iter().filter(|e| e.change.downloads()).cloned().collect();
        let title = format!("Download {} file(s)", entries.len());
        self.send(entries, Action::Download, false, title, cx);
    }

    fn upload_all(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.upload(cx);
    }

    fn download_all(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.download(cx);
    }

    /// The server's copy in place of the project's (a local change thrown away).
    fn revert_local(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = entries.into_iter().filter(|e| e.remote.is_some() && e.change != Change::Unchanged).collect();
        let title = format!("Revert {} file(s) to the server's copy", entries.len());
        self.send(entries, Action::Download, true, title, cx);
    }

    fn keep_local(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = entries.into_iter().filter(|e| e.change == Change::Conflict && e.local.is_some()).collect();
        let title = format!("Keep the project's copy of {} file(s)", entries.len());
        self.send(entries, Action::Upload, true, title, cx);
    }

    fn keep_server(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = entries.into_iter().filter(|e| e.change == Change::Conflict && e.remote.is_some()).collect();
        let title = format!("Keep the server's copy of {} file(s)", entries.len());
        self.send(entries, Action::Download, true, title, cx);
    }

    fn delete_on_server(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        let entries: Vec<Entry> = entries.into_iter().filter(|e| e.change == Change::Deleted).collect();
        let title = format!("Delete {} file(s) on the server", entries.len());
        self.send(entries, Action::DeleteRemote, false, title, cx);
    }

    fn show_history(&mut self, cx: &mut Context<Self>) {
        let Some(env) = self.environment().cloned() else { return };
        self.diff = None;
        self.history = Some(self.backend.history(&env).unwrap_or_default());
        cx.notify();
    }

    // ─── The diff ────────────────────────────────────────────────────────────

    fn open_diff(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        self.history = None;
        self.diff_cancel.cancel();
        let cancel = CancelToken::new();
        self.diff_cancel = cancel.clone();
        self.diff = Some(DiffView { entry: entry.clone(), state: DiffState::Loading, at: None });
        let backend = self.backend.clone();
        let path = entry.path.clone();
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { backend.read_both(&project_dir, &env, &path, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(diff) = this.diff.as_mut().filter(|d| d.entry.path == entry.path) else { return };
                diff.state = match read {
                    Err(e) => DiffState::Failed(e),
                    Ok((remote, local)) => {
                        let text = |b: &Option<Vec<u8>>| match b {
                            None => Some(String::new()),
                            Some(bytes) => text_of(bytes),
                        };
                        match (text(&remote), text(&local)) {
                            (Some(left), Some(right)) => DiffState::Text(Box::new(TextDiff::new(&entry.path, &left, &right))),
                            _ => DiffState::Binary,
                        }
                    }
                };
                // Straight to the first change.
                if let DiffState::Text(text) = &diff.state {
                    diff.at = text.changes(this.inline).first().copied();
                    if let Some(row) = diff.at {
                        this.diff_scroll.scroll_to_item(row, ScrollStrategy::Center);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let inline = self.inline;
        let Some(diff) = self.diff.as_mut() else { return };
        let DiffState::Text(text) = &diff.state else { return };
        let changes = text.changes(inline);
        diff.at = if forward {
            changes.iter().find(|&&c| diff.at.is_none_or(|r| c > r)).or(changes.first()).copied()
        } else {
            changes.iter().rev().find(|&&c| diff.at.is_none_or(|r| c < r)).or(changes.last()).copied()
        };
        if let Some(row) = diff.at {
            self.diff_scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Side by side ↔ one column; the change shown stays the one shown.
    fn toggle_inline(&mut self, cx: &mut Context<Self>) {
        self.inline = !self.inline;
        if let Some(diff) = self.diff.as_mut()
            && let DiffState::Text(text) = &diff.state
        {
            let (from, to) = (text.changes(!self.inline), text.changes(self.inline));
            diff.at = diff.at.and_then(|row| from.iter().position(|&c| c == row)).and_then(|i| to.get(i).copied());
            if let Some(row) = diff.at {
                self.diff_scroll.scroll_to_item(row, ScrollStrategy::Center);
            }
        }
        cx.notify();
    }

    fn merge(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) {
            cx.emit(SourceControlEvent::Merge { project_dir, env, path: entry.path.clone() });
        }
    }

    // ─── The list ────────────────────────────────────────────────────────────

    fn rows(&self) -> Vec<Row> {
        list::rows(&self.entries, &self.filter, &self.collapsed)
    }

    fn toggle_collapsed(&mut self, row: &Row, cx: &mut Context<Self>) {
        if let Some(key) = row.key()
            && !self.collapsed.remove(&key)
        {
            self.collapsed.insert(key);
        }
        cx.notify();
    }

    /// A row clicked (or reached by the keyboard): `add` (Ctrl / ⌘) adds it to the selection or takes it out,
    /// `extend` (Shift) selects from the anchor to it; a file is compared.
    fn press_row(&mut self, index: usize, add: bool, extend: bool, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(row) = rows.get(index).cloned() else { return };
        let paths_of = |row: &Row| -> Vec<String> {
            list::files_in(row, &self.entries, &self.filter).into_iter().map(|i| self.entries[i].path.clone()).collect()
        };
        if extend && let Some(anchor) = self.anchor {
            let (a, b) = (anchor.min(index), anchor.max(index));
            self.selected = rows[a..=b].iter().flat_map(paths_of).collect();
        } else if add {
            let paths = paths_of(&row);
            if paths.iter().all(|p| self.selected.contains(p)) {
                paths.iter().for_each(|p| {
                    self.selected.remove(p);
                });
            } else {
                self.selected.extend(paths);
            }
            self.anchor = Some(index);
        } else {
            self.selected = paths_of(&row).into_iter().collect();
            self.anchor = Some(index);
        }
        self.cursor = Some(index);
        if let Row::File { entry, .. } = row
            && !add
            && !extend
        {
            let entry = self.entries[entry].clone();
            self.open_diff(entry, cx);
        }
        cx.notify();
    }

    fn open_menu(&mut self, index: usize, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(row) = rows.get(index) else { return };
        // A row outside the selection becomes it.
        let paths: Vec<String> =
            list::files_in(row, &self.entries, &self.filter).into_iter().map(|i| self.entries[i].path.clone()).collect();
        if !paths.iter().all(|p| self.selected.contains(p)) {
            self.selected = paths.into_iter().collect();
            self.anchor = Some(index);
        }
        self.cursor = Some(index);
        let chosen: Vec<Entry> = self.selected_entries().into_iter().cloned().collect();
        let weak = cx.entity().downgrade();
        let item = |label: String, enabled: bool, run: fn(&mut SourceControlPage, Vec<Entry>, &mut Context<SourceControlPage>)| {
            let (weak, chosen) = (weak.clone(), chosen.clone());
            MenuItem::new(label, move |_, cx| {
                let chosen = chosen.clone();
                let _ = weak.update(cx, |page, cx| run(page, chosen, cx));
            })
            .enabled(enabled)
        };
        let count = |f: fn(&Entry) -> bool| chosen.iter().filter(|e| f(e)).count();
        let single = (chosen.len() == 1).then(|| chosen[0].clone());
        let uploads = count(|e| e.change.uploads());
        let downloads = count(|e| e.change.downloads());
        let conflicts = count(|e| e.change == Change::Conflict);
        let revertible = count(|e| e.remote.is_some() && matches!(e.change, Change::Modified | Change::Conflict | Change::Deleted));
        let deletions = count(|e| e.change == Change::Deleted);
        let mut items = vec![
            item("Compare".into(), single.is_some(), |page, chosen, cx| {
                if let Some(e) = chosen.into_iter().next() {
                    page.open_diff(e, cx);
                }
            })
            .bold(),
            MenuItem::Separator,
            item(format!("Upload ({uploads})"), uploads > 0, |page, chosen, cx| {
                let entries = chosen.into_iter().filter(|e| e.change.uploads()).collect::<Vec<_>>();
                let title = format!("Upload {} file(s)", entries.len());
                page.send(entries, Action::Upload, false, title, cx);
            }),
            item(format!("Download ({downloads})"), downloads > 0, |page, chosen, cx| {
                let entries = chosen.into_iter().filter(|e| e.change.downloads()).collect::<Vec<_>>();
                let title = format!("Download {} file(s)", entries.len());
                page.send(entries, Action::Download, false, title, cx);
            }),
            item(format!("Revert local to the server's copy ({revertible})"), revertible > 0, |page, chosen, cx| {
                page.revert_local(chosen, cx)
            }),
        ];
        if conflicts > 0 {
            items.push(MenuItem::Separator);
            if let Some(one) = single.clone().filter(|e| e.change == Change::Conflict) {
                let weak = weak.clone();
                items.push(MenuItem::new("Merge… (both copies with markers, in your editor)", move |_, cx| {
                    let _ = weak.update(cx, |page, cx| page.merge(&one, cx));
                }));
            }
            items.push(item(format!("Keep mine - upload ({conflicts})"), true, |page, chosen, cx| page.keep_local(chosen, cx)));
            items.push(item(format!("Keep the server's - download ({conflicts})"), true, |page, chosen, cx| {
                page.keep_server(chosen, cx)
            }));
        }
        items.push(MenuItem::Separator);
        items.push(item(format!("Delete on server… ({deletions})"), deletions > 0, |page, chosen, cx| {
            page.delete_on_server(chosen, cx)
        }));
        self.menu = Some(ContextMenu::new(position, items));
        cx.notify();
    }

    fn more_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let env = self.environment().cloned();
        let has_env = env.is_some();
        let item = |label: &str, enabled: bool, run: fn(&mut SourceControlPage, &mut Context<SourceControlPage>)| {
            let weak = weak.clone();
            MenuItem::new(label.to_string(), move |_, cx| {
                let _ = weak.update(cx, run);
            })
            .enabled(enabled)
        };
        let uploads = self.entries.iter().filter(|e| e.change.uploads()).count();
        let downloads = self.entries.iter().filter(|e| e.change.downloads()).count();
        let deletions = self.entries.iter().filter(|e| e.change == Change::Deleted).count();
        let items = vec![
            item(&format!("Upload all changes ({uploads})"), uploads > 0, |page, cx| page.upload_all(cx)),
            item(&format!("Download all server changes ({downloads})"), downloads > 0, |page, cx| page.download_all(cx)),
            item(&format!("Delete on server the files deleted here… ({deletions})"), deletions > 0, |page, cx| {
                let entries = page.entries.iter().filter(|e| e.change == Change::Deleted).cloned().collect();
                page.delete_on_server(entries, cx)
            }),
            MenuItem::Separator,
            item("Show history", has_env, |page, cx| page.show_history(cx)),
            item("Open the server backups folder", has_env, |page, cx| {
                if let Some(env) = page.environment() {
                    let folder = PathBuf::from(&env.project);
                    cx.emit(SourceControlEvent::OpenFolder(folder));
                }
            }),
            MenuItem::Separator,
            item("Add environment…", self.project.is_some(), |page, cx| page.add_environment(cx)),
            item("Edit environment…", has_env, |page, cx| {
                if let Some(env) = page.environment().cloned() {
                    cx.emit(SourceControlEvent::EditEnvironment { env, is_new: false });
                }
            }),
            item("Delete environment…", has_env, |page, cx| {
                if let Some(env) = page.environment().cloned() {
                    cx.emit(SourceControlEvent::DeleteEnvironment(env));
                }
            }),
        ];
        let mut menu = ContextMenu::new(position, items);
        menu.anchor = gpui::Corner::TopRight;
        self.menu = Some(menu);
        cx.notify();
    }

    fn add_environment(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else { return };
        let env = Environment {
            id: crate::source_control_environment::new_id(&project),
            project: project.clone(),
            name: "Production".into(),
            kind: dnnm_application::source_control::model::EnvironmentKind::Production,
            protocol: dnnm_application::source_control::model::Protocol::Sftp,
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: dnnm_application::source_control::model::Auth::Password,
            remote_root: format!("/var/www/{project}"),
            mappings: Vec::new(),
            ignore: Vec::new(),
            protected: Vec::new(),
            backup_remote: true,
            host_key: None,
        };
        cx.emit(SourceControlEvent::EditEnvironment { env, is_new: true });
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let m = event.keystroke.modifiers;
        if key == "f7" {
            self.step_change(!m.shift, cx);
            return cx.stop_propagation();
        }
        if !self.list_focus.is_focused(window) {
            return;
        }
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let last = rows.len() - 1;
        let at = self.cursor.unwrap_or(0).min(last);
        match key {
            "down" | "up" => {
                let next = if key == "down" { (at + usize::from(self.cursor.is_some())).min(last) } else { at.saturating_sub(1) };
                if m.shift {
                    if self.anchor.is_none() {
                        self.anchor = Some(at);
                    }
                    self.press_row(next, false, true, cx);
                } else {
                    self.cursor = Some(next);
                    self.anchor = Some(next);
                }
                self.list_scroll.scroll_to_item(next, ScrollStrategy::Center);
            }
            "left" | "right" => {
                let collapsed = matches!(rows[at], Row::Folder { collapsed: true, .. } | Row::Section { collapsed: true, .. });
                if rows[at].key().is_some() && (key == "right") == collapsed {
                    self.toggle_collapsed(&rows[at], cx);
                }
            }
            "enter" => {
                if rows[at].key().is_some() {
                    self.toggle_collapsed(&rows[at], cx);
                } else {
                    self.press_row(at, false, false, cx);
                }
            }
            "space" => self.press_row(at, m.secondary(), false, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for SourceControlPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A change's letter colour.
fn change_color(change: Change, cx: &App) -> gpui::Hsla {
    let c = Theme::get(cx).colors;
    match change {
        Change::Conflict | Change::Deleted | Change::DeletedOnServer => c.error_text,
        Change::Modified => c.log_warn,
        Change::Added => c.success_text,
        Change::Incoming | Change::RemoteOnly => c.link_fg,
        Change::Unchanged => c.text_muted,
    }
}

/// A Unix time as the user's date and time.
fn time_text(seconds: i64) -> String {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds.max(0) as u64);
    dnnm_infrastructure::local_time::short_date_time(at)
}

/// The search box over the list, as Settings draws its own.
fn search_field(input: &Entity<TextInput>, window: &Window, cx: &App) -> gpui::Div {
    let theme = Theme::get(cx);
    let c = theme.colors;
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(theme.metrics.search_height)
        .pad(px(0.0), px(6.0))
        .rounded(theme.metrics.control_radius)
        .border_1()
        .border_color(if focused { c.focus_border } else { c.field_border })
        .bg(c.input_bg)
        .child(ui::icon(Icon::Search, c.text_muted, cx))
        .child(div().flex_1().child(input.clone()))
}

/// "12.3 KB".
fn size_text(bytes: u64) -> String {
    dnnm_application::project::view::byte_size(bytes as f64)
}

impl Render for SourceControlPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let env = self.environment().cloned();
        let busy = matches!(self.status, Status::Working(_));

        // ─── Header: project, environment, status ─────────────────────────────
        let project_items: Vec<(SharedString, bool)> = self
            .projects
            .iter()
            .map(|(n, _)| (SharedString::from(n.clone()), self.project.as_deref() == Some(n.as_str())))
            .collect();
        let project_select = dnnm_ui::dialog::select(
            "sc-project",
            self.project.clone().unwrap_or_else(|| "No projects".into()),
            self.project_open,
            project_items,
            cx.listener(|this, open: &bool, _, cx| {
                this.project_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| {
                this.project_open = false;
                let name = this.projects.get(*i).map(|(n, _)| n.clone());
                this.choose_project(name, cx);
            }),
            cx,
        );
        let env_items: Vec<(SharedString, bool)> = self
            .environments
            .iter()
            .map(|e| (SharedString::from(e.label()), self.env.as_deref() == Some(e.id.as_str())))
            .collect();
        let env_value = env.as_ref().map_or_else(|| "No environment yet".to_string(), |e| e.label());
        let env_select = dnnm_ui::dialog::select(
            "sc-environment",
            env_value,
            self.env_open,
            env_items,
            cx.listener(|this, open: &bool, _, cx| {
                this.env_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| {
                this.env_open = false;
                if let Some(id) = this.environments.get(*i).map(|e| e.id.clone()) {
                    this.choose_env(id, cx);
                }
            }),
            cx,
        );
        let (status_text, status_color) = match &self.status {
            Status::NotCompared if env.is_some() => ("Not compared yet - Refresh".to_string(), c.text_muted),
            Status::NotCompared => ("Add an environment to compare with its server".to_string(), c.text_muted),
            Status::Working(what) => (what.clone(), c.log_warn),
            Status::Compared { at } => {
                let changes = self.entries.iter().filter(|e| e.change != Change::Unchanged).count();
                (format!("Compared {at} - {changes} change(s)"), c.success_text)
            }
            Status::Failed(why) => (format!("Failed: {why}"), c.error_text),
        };
        // The DNN parts the comparison found: deploy one module, skin or container on its own.
        let scopes = list::dnn_scopes(&self.entries);
        let scope_value = self
            .filter
            .scope
            .as_ref()
            .and_then(|f| scopes.iter().find(|s| &s.folder == f).map(|s| s.label.clone()))
            .unwrap_or_else(|| "Whole project".into());
        let mut scope_items: Vec<(SharedString, bool)> = vec![("Whole project".into(), self.filter.scope.is_none())];
        scope_items.extend(scopes.iter().map(|s| (SharedString::from(s.label.clone()), self.filter.scope.as_ref() == Some(&s.folder))));
        let scope_folders: Vec<String> = scopes.iter().map(|s| s.folder.clone()).collect();
        let scope_select = dnnm_ui::dialog::select(
            "sc-scope",
            scope_value,
            self.scope_open,
            scope_items,
            cx.listener(|this, open: &bool, _, cx| {
                this.scope_open = *open;
                cx.notify();
            }),
            cx.listener(move |this, i: &usize, _, cx| {
                this.scope_open = false;
                this.filter.scope = if *i == 0 { None } else { scope_folders.get(*i - 1).cloned() };
                this.selected.clear();
                this.cursor = None;
                cx.notify();
            }),
            cx,
        );
        let header = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.0))
            .child(div().text_size(theme.text.heading).font_weight(FontWeight::SEMIBOLD).child("Source Control"))
            .child(div().w(px(200.0)).child(project_select))
            .child(div().w(px(320.0)).child(env_select))
            .child(
                div()
                    .id("sc-scope-box")
                    .w(px(240.0))
                    .tip("Deploy only one part of the site: a module, a skin or a container")
                    .child(scope_select),
            )
            .child(
                ui::icon_button("sc-add-env", Icon::Add, c.text_primary, self.project.is_some(), cx)
                    .tip("Add an environment: a server to deploy this project to")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_environment(cx))),
            )
            .child(
                div()
                    .id("sc-status")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w_0()
                    .text_color(status_color)
                    .when(busy, |d| d.child(ui::spinner("sc-spinner", 12.0, status_color, cx)))
                    .child(ui::one_line(status_text)),
            );

        // ─── Toolbar ──────────────────────────────────────────────────────────
        let has_env = env.is_some() && !busy;
        let uploads = self.targets().iter().filter(|e| e.change.uploads()).count();
        let downloads = self.targets().iter().filter(|e| e.change.downloads()).count();
        let scope = if self.selected.is_empty() { "shown" } else { "selected" };
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(
                ui::icon_button("sc-refresh", Icon::Refresh, c.text_primary, has_env, cx)
                    .tip("Refresh: compare the project with the server again")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx))),
            )
            .child(
                ui::icon_button("sc-upload", Icon::Upload, c.text_primary, has_env && uploads > 0, cx)
                    .tip(format!("Upload the {scope} changes ({uploads}) to the server"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.upload(cx))),
            )
            .child(
                ui::icon_button("sc-download", Icon::Download, c.text_primary, has_env && downloads > 0, cx)
                    .tip(format!("Download the {scope} server changes ({downloads}) to the project"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.download(cx))),
            )
            .child(
                ui::icon_button("sc-more", Icon::More, c.text_primary, self.project.is_some() && !busy, cx)
                    .tip("More actions")
                    .on_click(cx.listener(|this, e: &ClickEvent, _, cx| this.more_menu(e.position(), cx))),
            )
            .child(div().flex_1())
            .child(div().w(px(240.0)).child(search_field(&self.search, window, cx)));

        // ─── Change filters ───────────────────────────────────────────────────
        let counts = dnnm_application::source_control::classify::counts(&self.entries);
        let mut chips = div().flex().flex_wrap().items_center().gap(px(4.0));
        for (change, n) in &counts {
            let on = self.filter.changes.contains(change);
            let change = *change;
            chips = chips.child(
                ui::focusable_bare(div().id(SharedString::from(format!("sc-chip-{}", change.letter()))), cx)
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .pad(px(1.0), px(8.0))
                    .rounded(theme.metrics.small_radius)
                    .cursor_pointer()
                    .text_size(theme.text.caption)
                    .when(on, |d| d.bg(c.selection_bg))
                    .hover(move |s| s.bg(c.hover_bg))
                    .tip(format!("Show only: {} - {}", change.name(), change.description()))
                    .child(div().font_weight(FontWeight::BOLD).text_color(change_color(change, cx)).child(change.letter()))
                    .child(format!("{} {n}", change.name()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.filter.changes.remove(&change) {
                            this.filter.changes.insert(change);
                        }
                        cx.notify();
                    })),
            );
        }
        let unchanged = self.filter.unchanged;
        chips = chips.child(
            ui::check_box("sc-unchanged", unchanged, cx)
                .tip("Also list the files that are the same on both sides")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.filter.unchanged = !this.filter.unchanged;
                    cx.notify();
                })),
        );
        chips = chips.child(div().text_size(theme.text.caption).text_color(c.text_muted).child("Unchanged"));

        // ─── The list ─────────────────────────────────────────────────────────
        let rows = self.rows();
        let entries = self.entries.clone();
        let selected = self.selected.clone();
        let cursor = self.cursor;
        let row_height = theme.metrics.table_row_height;
        let list = uniform_list(
            "sc-files",
            rows.len(),
            cx.processor(move |_: &mut Self, range: std::ops::Range<usize>, _window, cx| {
                let theme = Theme::get(cx).clone();
                let c = theme.colors;
                range
                    .map(|i| {
                        let row = rows[i].clone();
                        let is_selected = match &row {
                            Row::File { entry, .. } => selected.contains(&entries[*entry].path),
                            _ => false,
                        };
                        // A folder's depth as a blank before the row's content (the codebase pads evenly, never one side).
                        let indent = |depth: usize, extra: f32| div().flex_none().w(px(14.0 * depth as f32 + extra));
                        let base = div()
                            .id(SharedString::from(format!("sc-row-{i}")))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .h(row_height)
                            .pad(px(0.0), px(8.0))
                            .cursor_pointer()
                            .when(is_selected, |d| d.bg(c.selection_bg))
                            .when(cursor == Some(i), |d| d.border_1().border_color(c.focus_border))
                            .hover(move |s| s.bg(c.hover_bg))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                let m = e.modifiers();
                                this.press_row(i, m.secondary(), m.shift, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, e: &MouseDownEvent, _, cx| this.open_menu(i, e.position, cx)),
                            );
                        let chevron = |collapsed: bool| {
                            ui::icon_sized(
                                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                                ui::IconSize::Small,
                                c.text_muted,
                                cx,
                            )
                        };
                        match &row {
                            Row::Section { section, count, collapsed } => base
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(theme.text.caption)
                                .tip(section.tip())
                                .child(chevron(*collapsed))
                                .child(section.title().to_uppercase())
                                .child(div().flex_1())
                                .child(div().text_color(c.text_muted).child(count.to_string())),
                            Row::Folder { name, depth, count, collapsed, .. } => base
                                .child(indent(*depth, 0.0))
                                .child(chevron(*collapsed))
                                .child(ui::icon(if *collapsed { Icon::Folder } else { Icon::FolderOpen }, c.text_muted, cx))
                                .child(div().flex_1().min_w_0().child(ui::one_line(name.clone())))
                                .child(div().text_size(theme.text.caption).text_color(c.text_muted).child(count.to_string())),
                            Row::File { entry, name, folder, depth, .. } => {
                                let e = &entries[*entry];
                                base.child(indent(*depth, 18.0))
                                    .tip(format!("{} - {}", e.path, e.change.description()))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_baseline()
                                            .gap(px(6.0))
                                            .child(div().flex_none().when(e.change == Change::Deleted || e.change == Change::DeletedOnServer, |d| d.line_through()).child(name.clone()))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_size(theme.text.caption)
                                                    .text_color(c.text_muted)
                                                    .child(ui::one_line(folder.clone())),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w(px(14.0))
                                            .flex_none()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(change_color(e.change, cx))
                                            .child(e.change.letter()),
                                    )
                            }
                        }
                    })
                    .collect()
            }),
        )
        .track_scroll(self.list_scroll.clone())
        .size_full();
        let empty_text = match (&env, &self.status) {
            (None, _) => Some("No environment for this project yet. Add one (+): a server, its folder, and how to sign in."),
            (Some(_), Status::NotCompared) => Some("Refresh to compare the project with the server."),
            (Some(_), Status::Compared { .. }) if self.rows().is_empty() => Some("Nothing differs - or nothing matches the filter."),
            _ => None,
        };
        let list_panel = div()
            .id("sc-list")
            .track_focus(&self.list_focus)
            .key_context("SourceControlList")
            .relative()
            .flex_1()
            .min_h_0()
            .rounded(theme.metrics.card_radius)
            .border_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .pad(px(4.0), px(0.0))
            .child(list)
            .when_some(empty_text, |d, text| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child(text),
                )
            });

        // ─── The diff (or the history) ────────────────────────────────────────
        let right = self.render_right(env.as_ref(), busy, cx);

        let page = div()
            .id("source-control-page")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .pad(px(18.0), px(22.0))
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .font_family(theme.font.clone())
            .child(header)
            .child(toolbar)
            .when(!counts.is_empty() || !self.entries.is_empty(), |d| d.child(chips))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(10.0))
                    .child(div().flex().flex_col().w(px(380.0)).flex_none().min_h_0().child(list_panel))
                    .child(div().flex().flex_col().flex_1().min_w_0().min_h_0().child(right)),
            );
        // Last: the menu borrows `cx` for as long as it is drawn.
        let menu = self.menu.as_ref().map(|menu| {
            let weak = cx.entity().downgrade();
            dnnm_ui::menu::render(
                menu,
                move |_, cx| {
                    let _ = weak.update(cx, |page, cx| {
                        page.menu = None;
                        cx.notify();
                    });
                },
                |_, _, _| {},
                window,
                cx,
            )
        });

        page.children(menu)
    }
}

impl SourceControlPage {
    fn render_right(&mut self, env: Option<&Environment>, busy: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let frame = || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .rounded(theme.metrics.card_radius)
                .border_1()
                .border_color(c.card_border)
                .bg(c.card_bg)
                .overflow_hidden()
        };
        if let Some(history) = &self.history {
            let lines = history.clone();
            let rows = uniform_list("sc-history", lines.len(), move |range, _, cx| {
                let c = Theme::get(cx).colors;
                range
                    .map(|i| {
                        let h = &lines[i];
                        let ok = h.outcome == "Done";
                        div()
                            .flex()
                            .gap(px(10.0))
                            .pad(px(2.0), px(12.0))
                            .child(div().w(px(130.0)).flex_none().text_color(c.text_muted).child(time_text(h.at)))
                            .child(div().w(px(110.0)).flex_none().child(h.action.clone()))
                            .child(div().flex_1().min_w_0().child(ui::one_line(h.path.clone())))
                            .child(
                                div()
                                    .w(px(220.0))
                                    .flex_none()
                                    .text_color(if ok { c.success_text } else { c.error_text })
                                    .child(ui::one_line(h.outcome.clone())),
                            )
                    })
                    .collect()
            })
            .size_full();
            let empty = history.is_empty();
            return frame()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .pad(px(8.0), px(12.0))
                        .border_b_1()
                        .border_color(c.card_border)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("History")
                        .child(div().flex_1())
                        .child(
                            ui::icon_button("sc-close-history", Icon::Close, c.text_muted, true, cx)
                                .tip("Close the history")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.history = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(div().flex_1().min_h_0().text_size(theme.text.caption).child(rows))
                .when(empty, |d| d.child(div().p(px(20.0)).text_color(c.text_muted).child("Nothing sent to or from this server yet.")))
                .into_any_element();
        }
        let Some(diff) = &self.diff else {
            let text = if env.is_some() {
                "Choose a file to compare the server's copy (left) with the project's (right)."
            } else {
                "Source Control compares a project's files with a server's over SFTP, FTPS or FTP, and sends them either way - never overwriting or deleting a server's file without asking."
            };
            return frame()
                .items_center()
                .justify_center()
                .p(px(24.0))
                .text_color(c.text_muted)
                .child(text)
                .into_any_element();
        };
        let entry = diff.entry.clone();
        let header_buttons = {
            let e = entry.clone();
            let mut row = div().flex().items_center().gap(px(2.0));
            let text_diff = matches!(diff.state, DiffState::Text(_));
            row = row
                .child(
                    ui::button("sc-diff-mode", if self.inline { "Side by side" } else { "Inline" }, ui::ButtonKind::Secondary, text_diff, cx)
                        .tip(if self.inline { "Show the two copies side by side" } else { "Show the changes in one column" })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_inline(cx))),
                )
                .child(
                    ui::icon_button("sc-prev-change", Icon::ChevronUp, c.text_primary, text_diff, cx)
                        .tip("Previous change (Shift+F7)")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_change(false, cx))),
                )
                .child(
                    ui::icon_button("sc-next-change", Icon::ChevronDown, c.text_primary, text_diff, cx)
                        .tip("Next change (F7)")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step_change(true, cx))),
                );
            let action = |id: &'static str, label: &'static str, enabled: bool| {
                ui::button(id, label, ui::ButtonKind::Secondary, enabled && !busy, cx)
            };
            match e.change {
                Change::Conflict => {
                    let (a, b, m) = (e.clone(), e.clone(), e.clone());
                    row = row
                        .child(action("sc-merge", "Merge…", text_diff).tip("Both copies in the project's file between markers, opened in your editor - keep what belongs, save, Refresh, then Keep mine").on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| this.merge(&m, cx),
                        )))
                        .child(action("sc-keep-local", "Keep mine", e.local.is_some()).on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| this.keep_local(vec![a.clone()], cx),
                        )))
                        .child(action("sc-keep-server", "Keep the server's", e.remote.is_some()).on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| this.keep_server(vec![b.clone()], cx),
                        )));
                }
                change if change.uploads() => {
                    let a = e.clone();
                    row = row.child(action("sc-diff-upload", "Upload", true).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            let title = format!("Upload {}", a.path);
                            this.send(vec![a.clone()], Action::Upload, false, title, cx)
                        },
                    )));
                }
                change if change.downloads() => {
                    let a = e.clone();
                    row = row.child(action("sc-diff-download", "Download", true).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            let title = format!("Download {}", a.path);
                            this.send(vec![a.clone()], Action::Download, false, title, cx)
                        },
                    )));
                }
                Change::Deleted => {
                    let a = e.clone();
                    row = row.child(action("sc-diff-delete", "Delete on server…", true).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| this.delete_on_server(vec![a.clone()], cx),
                    )));
                }
                _ => {}
            }
            row
        };
        let summary = match &diff.state {
            DiffState::Text(text) => {
                let (added, removed) = text.sides.added_removed();
                format!("+{added} -{removed}")
            }
            _ => String::new(),
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pad(px(6.0), px(12.0))
            .border_b_1()
            .border_color(c.card_border)
            .child(div().font_weight(FontWeight::BOLD).text_color(change_color(entry.change, cx)).child(entry.change.letter()))
            .child(div().flex_1().min_w_0().child(ui::one_line(entry.path.clone())))
            .child(div().text_size(theme.text.caption).text_color(c.text_muted).child(summary))
            .child(header_buttons);
        let sides = div()
            .flex()
            .text_size(theme.text.caption)
            .text_color(c.text_muted)
            .border_b_1()
            .border_color(c.card_border)
            .child(div().flex_1().pad(px(3.0), px(12.0)).child(format!("Server - {}", meta_text(entry.remote.as_ref()))))
            .child(div().flex_1().pad(px(3.0), px(12.0)).child(format!("Project - {}", meta_text(entry.local.as_ref()))));
        let body: gpui::AnyElement = match &diff.state {
            DiffState::Loading => div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(ui::spinner("sc-diff-spinner", 14.0, c.text_muted, cx))
                .child("Reading both copies…")
                .into_any_element(),
            DiffState::Failed(e) => div().p(px(20.0)).text_color(c.error_text).child(e.clone()).into_any_element(),
            DiffState::Binary => div()
                .p(px(20.0))
                .text_color(c.text_muted)
                .child("A binary file: no line-by-line comparison. The sizes and times are above.")
                .into_any_element(),
            DiffState::Text(text) if text.sides.changes.is_empty() => div()
                .p(px(20.0))
                .text_color(c.text_muted)
                .child("The text is the same on both sides (only the times differ, or the line endings).")
                .into_any_element(),
            DiffState::Text(text) => {
                let at = diff.at;
                let mono = theme.mono_font.clone();
                let left = Arc::new(text.left.clone());
                let right = Arc::new(text.right.clone());
                if self.inline {
                    let rows = text.inline.rows.clone();
                    uniform_list("sc-diff-inline", rows.len(), move |range, _, cx| {
                        let c = Theme::get(cx).colors;
                        range
                            .map(|i| {
                                let row = &rows[i];
                                let (bg, sign) = match row.kind {
                                    RowKind::Removed => (Some(c.diff_removed_bg), "-"),
                                    RowKind::Added => (Some(c.diff_added_bg), "+"),
                                    _ => (None, " "),
                                };
                                let spans = match row.kind {
                                    RowKind::Removed => row.left.and_then(|n| left.get(n - 1)),
                                    _ => row.right.and_then(|n| right.get(n - 1)),
                                };
                                let number = |n: Option<usize>| {
                                    div()
                                        .w(px(40.0))
                                        .flex_none()
                                        .pad(px(0.0), px(4.0))
                                        .flex()
                                        .justify_end()
                                        .text_color(c.text_muted)
                                        .child(n.map(|n| n.to_string()).unwrap_or_default())
                                };
                                div()
                                    .flex()
                                    .when_some(bg, |d, bg| d.bg(bg))
                                    .when(at == Some(i), |d| d.border_t_1().border_color(c.focus_border))
                                    .child(number(row.left))
                                    .child(number(row.right))
                                    .child(div().w(px(14.0)).flex_none().text_color(c.text_muted).child(sign))
                                    .child(div().whitespace_nowrap().child(code_line(&row.text, spans.map(Vec::as_slice), cx)))
                            })
                            .collect()
                    })
                    .track_scroll(self.diff_scroll.clone())
                    .font_family(mono)
                    .size_full()
                    .into_any_element()
                } else {
                    let rows = text.sides.rows.clone();
                    uniform_list("sc-diff", rows.len(), move |range, _, cx| {
                        let c = Theme::get(cx).colors;
                        range
                            .map(|i| {
                                let row = &rows[i];
                                let (left_bg, right_bg) = match row.kind {
                                    RowKind::Same => (None, None),
                                    RowKind::Removed => (Some(c.diff_removed_bg), None),
                                    RowKind::Added => (None, Some(c.diff_added_bg)),
                                    RowKind::Changed => (Some(c.diff_removed_bg), Some(c.diff_added_bg)),
                                };
                                let half = |line: Option<&dnnm_application::source_control::diff::Line>,
                                            bg: Option<gpui::Hsla>,
                                            spans: &Vec<Vec<(std::ops::Range<usize>, Token)>>| {
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .when_some(bg, |d, bg| d.bg(bg))
                                        .child(
                                            div()
                                                .w(px(44.0))
                                                .flex_none()
                                                .pad(px(0.0), px(4.0))
                                                .flex()
                                                .justify_end()
                                                .text_color(c.text_muted)
                                                .child(line.map(|l| l.number.to_string()).unwrap_or_default()),
                                        )
                                        .child(div().whitespace_nowrap().children(line.map(|l| {
                                            code_line(&l.text, spans.get(l.number - 1).map(Vec::as_slice), cx)
                                        })))
                                };
                                div()
                                    .flex()
                                    .when(at == Some(i), |d| d.border_t_1().border_color(c.focus_border))
                                    .child(half(row.left.as_ref(), left_bg, &left))
                                    .child(div().w(px(1.0)).flex_none().bg(c.card_border))
                                    .child(half(row.right.as_ref(), right_bg, &right))
                            })
                            .collect()
                    })
                    .track_scroll(self.diff_scroll.clone())
                    .font_family(mono)
                    .size_full()
                    .into_any_element()
                }
            }
        };
        frame()
            .child(header)
            .child(sides)
            .child(div().flex_1().min_h_0().text_size(theme.text.caption).child(body))
            .into_any_element()
    }
}

/// A line of code with its syntax colours (pieces from [`highlight`]; none: plain).
fn code_line(text: &str, spans: Option<&[(std::ops::Range<usize>, Token)]>, cx: &App) -> gpui::StyledText {
    let c = Theme::get(cx).colors;
    let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = spans
        .unwrap_or_default()
        .iter()
        .filter(|(r, _)| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        .map(|(r, token)| {
            let color = match token {
                Token::Keyword => c.syntax_keyword,
                Token::String => c.syntax_string,
                Token::Comment => c.syntax_comment,
                Token::Number => c.syntax_number,
                Token::Tag => c.syntax_tag,
                Token::Attribute => c.syntax_attribute,
            };
            (r.clone(), gpui::HighlightStyle { color: Some(color), ..Default::default() })
        })
        .collect();
    gpui::StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights)
}

/// "12.3 KB, 23-11-2026 14:02" - or "not there".
fn meta_text(meta: Option<&dnnm_application::source_control::model::FileMeta>) -> String {
    match meta {
        None => "not there".into(),
        Some(m) => format!("{}, {}", size_text(m.size), time_text(m.mtime)),
    }
}
