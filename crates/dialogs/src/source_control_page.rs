//! The Source Control page: a project's files against a server, as VS Code's Source Control lays it out - in the
//! primary side bar the title with its actions (Refresh, Upload, Download, More), the project and server, the Commit
//! button (uploads the staged changes - or, with none staged, every change shown), the files in sections (Conflicts,
//! Staged Changes, Changes, Server changes) with their folder and change letter and their own actions under the mouse
//! (Stage on a change, Unstage on a staged one); in the main view the files opened, in tabs as VS Code's editors -
//! a file opened with one click is a preview (its name in italics) that the next one takes the place of, a double
//! click (on the file or its tab) keeps it - each the side-by-side diff of the server's copy (left) and the project's
//! (right), drawn by [`crate::source_control_diff`]; or the history.
//!
//! Reading and comparing, and sending, are operations the app runs (`SourceControlEvent`) - so they show in Output
//! with their progress, can be cancelled, and ask their questions there; the page gets their result back
//! ([`SourceControlPage::set_scan`]). A file's two copies for the diff are read off the UI thread by the page itself.
//!
//! Keyboard: Up / Down move through the list, Shift with them selects, Left / Right fold and unfold a folder, Enter
//! compares the file (or folds a folder), Space selects, Ctrl+A selects every file shown; F7 / Shift+F7 go to the next /
//! previous change in the diff. The section the top row is in stays at the top of the list as it scrolls.

use dnnm_ui::px;
use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, ClickEvent, Context, DragMoveEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, ScrollStrategy, ScrollWheelEvent, SharedString, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::*, uniform_list,
};

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::diff::text_of;
use dnnm_application::source_control::model::{Change, Entry, Environment, HistoryLine};
use dnnm_application::source_control::sync::{Action, BothCopies, TransferItem};
use dnnm_ui::menu::{ContextMenu, MenuItem};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Icon, Pad, Tip};

use crate::source_control_diff::{self, HScrollDrag, Layout, OneFile, SplitDrag, TextDiff};
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
    /// The project and server chosen by hand - to be chosen again next time ([`SourceControlPage::restore`]).
    Chosen { project: Option<String>, env: Option<String> },
}

/// Where the page is with its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotCompared,
    Working(String),
    Compared { at: String },
    Failed(String),
}

enum DiffState {
    Loading,
    Text(Box<TextDiff>),
    /// Only one side has the file: that copy alone, as VS Code opens a new file.
    File(Box<OneFile>),
    /// Not text: what is known of each side.
    Binary,
    Failed(String),
}

/// A tab of the main view: a file's comparison.
struct DiffView {
    entry: Entry,
    state: DiffState,
    /// The change shown last (F7).
    at: Option<usize>,
    /// Opened with one click and not kept yet: the next file opened so takes its tab (VS Code's preview, in italics).
    preview: bool,
    /// Where its diff is scrolled to - each tab keeps its own, down and sideways (both halves together).
    scroll: UniformListScrollHandle,
    scroll_x: Pixels,
    /// Reading its two copies.
    cancel: CancelToken,
    task: Option<Task<()>>,
}

pub struct SourceControlPage {
    backend: Arc<dyn SourceControlBackend>,
    focus: FocusHandle,
    list_focus: FocusHandle,
    projects: Vec<(String, PathBuf)>,
    project: Option<String>,
    /// The project chosen last time: chosen when the projects arrive, if it is still one of them.
    wanted_project: Option<String>,
    project_open: bool,
    environments: Vec<Environment>,
    env: Option<String>,
    env_open: bool,
    /// The files as a folder tree instead of VS Code's flat list (More: View as Tree / List).
    tree: bool,
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
    /// The row under the mouse: its actions are shown.
    hovered: Option<usize>,
    /// The files open in the main view, and the one shown.
    tabs: Vec<DiffView>,
    active: usize,
    /// The tab under the mouse: its close button shows.
    hovered_tab: Option<usize>,
    history: Option<Vec<HistoryLine>>,
    menu: Option<ContextMenu>,
    list_scroll: UniformListScrollHandle,
    /// The left half's share of the diff's width - the line between the halves dragged (kept from file to file).
    split: f32,
    /// The diff's area as last drawn: a drag or a sideways wheel is measured against it.
    diff_area: Rc<Cell<Bounds<Pixels>>>,
    /// A character's width in the diff's font, measured when drawn.
    char_width: Pixels,
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
            wanted_project: None,
            project_open: false,
            environments: Vec::new(),
            env: None,
            env_open: false,
            tree: false,
            loading_cached: None,
            entries: Vec::new(),
            status: Status::NotCompared,
            search,
            filter: Filter::default(),
            collapsed: HashSet::new(),
            selected: BTreeSet::new(),
            cursor: None,
            anchor: None,
            hovered: None,
            tabs: Vec::new(),
            active: 0,
            hovered_tab: None,
            history: None,
            menu: None,
            list_scroll: UniformListScrollHandle::new(),
            split: 0.5,
            diff_area: Rc::new(Cell::new(Bounds::default())),
            char_width: Pixels::ZERO,
            rechecking: None,
            _subscription: subscription,
        }
    }

    // ─── From the app ────────────────────────────────────────────────────────

    /// The project and server chosen last time (as [`SourceControlEvent::Chosen`] gave them) - before the projects
    /// are given: each is chosen again when it is still there.
    pub fn restore(&mut self, project: Option<String>, env: Option<String>) {
        self.wanted_project = project.filter(|p| !p.is_empty());
        self.env = env.filter(|e| !e.is_empty());
    }

    /// The projects to choose from: name and folder. The one chosen last time, else the first, is chosen when none
    /// is.
    pub fn set_projects(&mut self, projects: Vec<(String, PathBuf)>, cx: &mut Context<Self>) {
        if self.projects == projects {
            return;
        }
        self.projects = projects;
        let has = |p: &String| self.projects.iter().find(|(n, _)| n.eq_ignore_ascii_case(p)).map(|(n, _)| n.clone());
        let still = self.project.as_ref().and_then(has).is_some();
        if !still {
            let wanted = self.wanted_project.as_ref().and_then(has);
            if wanted.is_some() {
                self.wanted_project = None;
            }
            let choice = wanted.or_else(|| self.projects.first().map(|(n, _)| n.clone()));
            self.choose_project(choice, cx);
        }
        cx.notify();
    }

    /// Tells the app the project and server chosen, to choose them again next time.
    fn remember_choice(&self, cx: &mut Context<Self>) {
        cx.emit(SourceControlEvent::Chosen { project: self.project.clone(), env: self.env.clone() });
    }

    /// The environments read again (after one was saved or deleted); `choose` is chosen when given.
    pub fn reload_environments(&mut self, choose: Option<String>, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else { return };
        self.environments = self.backend.environments(&project).unwrap_or_default();
        let chosen = choose.is_some();
        let keep = choose.or(self.env.clone()).filter(|id| self.environments.iter().any(|e| &e.id == id));
        if keep != self.env {
            self.clear_comparison();
        }
        self.env = keep.or_else(|| self.environments.first().map(|e| e.id.clone()));
        if chosen {
            self.remember_choice(cx);
        }
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
        self.keep_staged_changes();
        self.status = Status::Compared { at };
        // Every tab read again: the server's copy may have changed too.
        let open: Vec<Entry> =
            self.tabs.iter().filter_map(|t| self.entries.iter().find(|e| e.path == t.entry.path).cloned()).collect();
        for entry in open {
            self.load_tab(entry, cx);
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
        self.filter.staged.clear();
        self.cursor = None;
        self.close_all_tabs();
        self.history = None;
        self.status = Status::NotCompared;
    }

    /// Another project - with the same server chosen, as servers aren't tied to a project; its comparison is that
    /// project's.
    fn choose_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        self.project = project;
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
                    this.keep_staged_changes();
                    // The tabs follow their files.
                    let changed: Vec<Entry> = this
                        .tabs
                        .iter()
                        .filter_map(|t| this.entries.iter().find(|e| e.path == t.entry.path && **e != t.entry).cloned())
                        .collect();
                    for entry in changed {
                        this.load_tab(entry, cx);
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
        let rows = list::rows(std::slice::from_ref(entry), &self.filter, &HashSet::new(), false);
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

    /// Staged paths stay staged only while they are a change in the project (an upload, a revert, a file saved back
    /// to the server's text puts them out of the Changes).
    fn keep_staged_changes(&mut self) {
        let changes: HashSet<&str> =
            self.entries.iter().filter(|e| list::stageable(e.change)).map(|e| e.path.as_str()).collect();
        self.filter.staged.retain(|p| changes.contains(p.as_str()));
    }

    /// Stages (or unstages) these files - only changes in the project are staged.
    fn stage(&mut self, entries: Vec<Entry>, staged: bool, cx: &mut Context<Self>) {
        for e in entries.into_iter().filter(|e| list::stageable(e.change)) {
            if staged {
                self.filter.staged.insert(e.path);
            } else {
                self.filter.staged.remove(&e.path);
            }
        }
        // The rows moved between sections: the row numbers kept no longer point at them.
        self.cursor = None;
        self.anchor = None;
        self.hovered = None;
        cx.notify();
    }

    /// What Commit sends: the staged changes, and only those - with none staged, Commit is off.
    fn commit_entries(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| list::stageable(e.change) && self.filter.staged.contains(&e.path)).collect()
    }

    /// Commit: the changes to the server in one operation - uploaded, and a file deleted here deleted there (Output asks
    /// first, as for Delete on server).
    fn commit(&mut self, cx: &mut Context<Self>) {
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        let entries = self.commit_entries();
        if entries.is_empty() {
            return;
        }
        let items: Vec<TransferItem> = entries
            .iter()
            .map(|e| {
                let action = if e.change == Change::Deleted { Action::DeleteRemote } else { Action::Upload };
                TransferItem::from_entry(e, action, false)
            })
            .collect();
        let title = format!("Commit {} file(s)", items.len());
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
        self.history = Some(self.backend.history(&env).unwrap_or_default());
        cx.notify();
    }

    // ─── The diff ────────────────────────────────────────────────────────────

    fn tab(&self) -> Option<&DiffView> {
        self.tabs.get(self.active)
    }

    /// Opens a file's comparison in a tab - its own when it has one. `keep`: as a kept tab; else as the preview, in
    /// the preview's place (VS Code: one click previews, a double click keeps).
    fn open_diff(&mut self, entry: Entry, keep: bool, cx: &mut Context<Self>) {
        self.history = None;
        if let Some(i) = self.tabs.iter().position(|t| t.entry.path == entry.path) {
            self.active = i;
            if keep {
                self.tabs[i].preview = false;
            }
            if self.tabs[i].entry != entry || matches!(self.tabs[i].state, DiffState::Failed(_)) {
                self.load_tab(entry, cx);
            }
            return cx.notify();
        }
        let tab = DiffView {
            entry: entry.clone(),
            state: DiffState::Loading,
            at: None,
            preview: !keep,
            scroll: UniformListScrollHandle::new(),
            scroll_x: Pixels::ZERO,
            cancel: CancelToken::new(),
            task: None,
        };
        if let Some(i) = self.tabs.iter().position(|t| t.preview) {
            self.tabs[i].cancel.cancel();
            self.tabs[i] = tab;
            self.active = i;
        } else {
            self.active = if self.tabs.is_empty() { 0 } else { self.active + 1 };
            self.tabs.insert(self.active, tab);
        }
        self.load_tab(entry, cx);
        cx.notify();
    }

    /// The tab shown kept: no longer the preview.
    fn keep_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.preview = false;
            cx.notify();
        }
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index).cancel.cancel();
        // The one to its right takes its place, else the one to its left - as VS Code does.
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.hovered_tab = None;
        cx.notify();
    }

    fn close_all_tabs(&mut self) {
        for tab in self.tabs.drain(..) {
            tab.cancel.cancel();
        }
        self.active = 0;
        self.hovered_tab = None;
    }

    /// Reads the file's two copies again into its tab, off the UI thread.
    fn load_tab(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) else { return };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.entry.path == entry.path) else { return };
        tab.cancel.cancel();
        let cancel = CancelToken::new();
        tab.cancel = cancel.clone();
        tab.entry = entry.clone();
        tab.state = DiffState::Loading;
        tab.at = None;
        let backend = self.backend.clone();
        let path = entry.path.clone();
        tab.task = Some(cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { backend.read_both(&project_dir, &env, &path, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(tab) = this.tabs.iter_mut().find(|t| t.entry.path == entry.path) else { return };
                tab.state = diff_state(&entry.path, read);
                // Straight to the first change.
                if let DiffState::Text(text) = &tab.state {
                    tab.at = text.changes().first().copied();
                    if let Some(row) = tab.at {
                        tab.scroll.scroll_to_item(row, ScrollStrategy::Center);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(diff) = self.tabs.get_mut(self.active) else { return };
        let DiffState::Text(text) = &diff.state else { return };
        let changes = text.changes();
        diff.at = if forward {
            changes.iter().find(|&&c| diff.at.is_none_or(|r| c > r)).or(changes.first()).copied()
        } else {
            changes.iter().rev().find(|&&c| diff.at.is_none_or(|r| c < r)).or(changes.last()).copied()
        };
        if let Some(row) = diff.at {
            diff.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// How far the shown diff can scroll sideways: the half whose widest line is the furthest past its width.
    fn max_scroll_x(&self) -> Pixels {
        let width = self.diff_area.get().size.width;
        match self.tab().map(|t| &t.state) {
            Some(DiffState::Text(text)) => {
                let (left, right) = source_control_diff::halves(width, self.split);
                source_control_diff::overflow(text.widest.0, self.char_width, left)
                    .max(source_control_diff::overflow(text.widest.1, self.char_width, right))
            }
            Some(DiffState::File(file)) => source_control_diff::overflow(file.widest, self.char_width, width),
            _ => Pixels::ZERO,
        }
    }

    fn set_scroll_x(&mut self, to: Pixels, cx: &mut Context<Self>) {
        let max = self.max_scroll_x();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let to = to.clamp(Pixels::ZERO, max);
            if to != tab.scroll_x {
                tab.scroll_x = to;
                cx.notify();
            }
        }
    }

    /// A sideways wheel (Shift+wheel, a touchpad) over the diff: both halves move together.
    fn wheel_sideways(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        if delta.x == Pixels::ZERO {
            return;
        }
        let now = self.tab().map_or(Pixels::ZERO, |t| t.scroll_x);
        self.set_scroll_x(now - delta.x, cx);
    }

    /// The line between the halves dragged: the left's share of the width follows the mouse.
    fn drag_split(&mut self, event: &DragMoveEvent<SplitDrag>, cx: &mut Context<Self>) {
        let bounds = event.bounds;
        if bounds.size.width <= Pixels::ZERO {
            return;
        }
        let split = source_control_diff::clamp_split((event.event.position.x - bounds.left()) / bounds.size.width);
        if split != self.split {
            self.split = split;
            let now = self.tab().map_or(Pixels::ZERO, |t| t.scroll_x);
            self.set_scroll_x(now, cx);
            cx.notify();
        }
    }

    /// A half's sideways scroll bar dragged: its thumb's middle follows the mouse.
    fn drag_sideways(&mut self, event: &DragMoveEvent<HScrollDrag>, cx: &mut Context<Self>) {
        let left = event.drag(cx).left;
        let bounds = event.bounds;
        let (widest, x, half) = match self.tab().map(|t| &t.state) {
            Some(DiffState::Text(text)) => {
                let (lw, rw) = source_control_diff::halves(bounds.size.width, self.split);
                if left { (text.widest.0, Pixels::ZERO, lw) } else { (text.widest.1, lw, rw) }
            }
            Some(DiffState::File(file)) => (file.widest, Pixels::ZERO, bounds.size.width),
            _ => return,
        };
        let max = source_control_diff::overflow(widest, self.char_width, half);
        let Some((_, thumb)) = source_control_diff::thumb(widest, self.char_width, half, Pixels::ZERO) else { return };
        let track = half - px(source_control_diff::NUMBER_WIDTH);
        let track_left = bounds.left() + x + px(source_control_diff::NUMBER_WIDTH);
        let room = track - thumb;
        if room <= Pixels::ZERO {
            return;
        }
        let at = ((event.event.position.x - track_left - thumb / 2.0) / room).clamp(0.0, 1.0);
        self.set_scroll_x(max * at, cx);
    }

    fn merge(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if let (Some(env), Some(project_dir)) = (self.environment().cloned(), self.project_dir()) {
            cx.emit(SourceControlEvent::Merge { project_dir, env, path: entry.path.clone() });
        }
    }

    // ─── The list ────────────────────────────────────────────────────────────

    fn rows(&self) -> Vec<Row> {
        list::rows(&self.entries, &self.filter, &self.collapsed, self.tree)
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
            self.open_diff(entry, false, cx);
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
        let staged_paths = self.filter.staged.clone();
        let stageable = chosen.iter().filter(|e| list::stageable(e.change) && !staged_paths.contains(&e.path)).count();
        let unstageable = chosen.iter().filter(|e| list::stageable(e.change) && staged_paths.contains(&e.path)).count();
        let mut items = vec![
            item("Compare".into(), single.is_some(), |page, chosen, cx| {
                if let Some(e) = chosen.into_iter().next() {
                    page.open_diff(e, true, cx);
                }
            })
            .bold(),
            MenuItem::Separator,
            item(format!("Stage Changes ({stageable})"), stageable > 0, |page, chosen, cx| page.stage(chosen, true, cx)),
            item(format!("Unstage Changes ({unstageable})"), unstageable > 0, |page, chosen, cx| page.stage(chosen, false, cx)),
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
        let changes = self.entries.iter().filter(|e| list::stageable(e.change)).count();
        let staged = self.filter.staged.len();
        let items = vec![
            item(&format!("Stage all changes ({changes})"), changes > staged, |page, cx| {
                let all = page.entries.iter().filter(|e| list::stageable(e.change)).cloned().collect();
                page.stage(all, true, cx)
            }),
            item(&format!("Unstage all changes ({staged})"), staged > 0, |page, cx| {
                let all = page.entries.clone();
                page.stage(all, false, cx)
            }),
            MenuItem::Separator,
            item(&format!("Upload all changes ({uploads})"), uploads > 0, |page, cx| page.upload_all(cx)),
            item(&format!("Download all server changes ({downloads})"), downloads > 0, |page, cx| page.download_all(cx)),
            item(&format!("Delete on server the files deleted here… ({deletions})"), deletions > 0, |page, cx| {
                let entries = page.entries.iter().filter(|e| e.change == Change::Deleted).cloned().collect();
                page.delete_on_server(entries, cx)
            }),
            MenuItem::Separator,
            item(if self.tree { "View as List" } else { "View as Tree" }, true, |page, cx| {
                page.tree = !page.tree;
                page.cursor = None;
                cx.notify();
            }),
            item(if self.filter.unchanged { "Hide unchanged files" } else { "Show unchanged files" }, true, |page, cx| {
                page.filter.unchanged = !page.filter.unchanged;
                page.cursor = None;
                cx.notify();
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
            item("Add server…", self.project.is_some(), |page, cx| page.add_environment(cx)),
            item("Edit server…", has_env, |page, cx| {
                if let Some(env) = page.environment().cloned() {
                    cx.emit(SourceControlEvent::EditEnvironment { env, is_new: false });
                }
            }),
            item("Delete server…", has_env, |page, cx| {
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
        let env = crate::source_control_environment::new_environment(&project);
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
        // Ctrl+A (Cmd+A): every file the list shows - and those in its folded folders, as selecting a folder takes its
        // files. A folded section's files stay out: they aren't shown.
        if key == "a" && m.secondary() && !m.shift && !m.alt {
            self.selected = rows
                .iter()
                .filter(|r| matches!(r, Row::File { .. } | Row::Folder { collapsed: true, .. }))
                .flat_map(|r| list::files_in(r, &self.entries, &self.filter))
                .map(|i| self.entries[i].path.clone())
                .collect();
            cx.stop_propagation();
            return cx.notify();
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

/// The page's files in the workspace's primary side bar: a view of its own over the same page, drawn again whenever
/// the page changes.
pub struct SourceControlSide {
    page: Entity<SourceControlPage>,
    _observe: Subscription,
}

impl SourceControlSide {
    pub fn new(page: Entity<SourceControlPage>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&page, |_, _, cx| cx.notify());
        Self { page, _observe: observe }
    }
}

impl Render for SourceControlSide {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.page.update(cx, |page, cx| page.render_side(window, cx))
    }
}

impl Focusable for SourceControlPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A row of the list: VS Code's 22 px.
fn row_height() -> gpui::Pixels {
    px(22.0)
}

/// VS Code's sticky section headers: the copy of the row of the section the top of the list is in, held at the top
/// while its own row has scrolled away, and pushed up by the next section's row as that arrives. Drawn over the rows
/// (a uniform list draws its decorations after them) and blocking the mouse, so it is the row clicked and hovered.
struct StickySection {
    page: gpui::WeakEntity<SourceControlPage>,
    rows: Rc<Vec<Row>>,
    /// Where the sections' rows are, in order.
    sections: Rc<Vec<usize>>,
}

impl gpui::UniformListDecoration for StickySection {
    fn compute(
        &self,
        visible: std::ops::Range<usize>,
        _bounds: gpui::Bounds<gpui::Pixels>,
        scroll_offset: gpui::Point<gpui::Pixels>,
        item_height: gpui::Pixels,
        _item_count: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> gpui::AnyElement {
        // The decoration is placed at the list's content origin (moved by the scroll): `top` is the view's top in it.
        let top = -scroll_offset.y;
        let at = self.sections.partition_point(|&s| s <= visible.start);
        let Some(&section) = at.checked_sub(1).and_then(|k| self.sections.get(k)) else {
            return div().into_any_element();
        };
        // Its own row still starts in view: nothing to hold.
        if top <= item_height * section as f32 {
            return div().into_any_element();
        }
        let y = match self.sections.get(at) {
            Some(&next) => top.min(item_height * next as f32 - item_height),
            None => top,
        };
        let Ok(row) = self.page.update(cx, |page, cx| page.render_row(section, &self.rows[section], true, cx)) else {
            return div().into_any_element();
        };
        let c = Theme::get(cx).colors;
        // A root is laid out at the origin it is given whatever its own insets: the header goes in one at the content
        // origin, placed by its `top`.
        div()
            .size_full()
            .child(
                div()
                    .id("sc-sticky")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(y)
                    .h(item_height)
                    .occlude()
                    .bg(c.window_bg)
                    .child(row)
                    // Its edge over the rows going under it.
                    .child(div().absolute().left_0().right_0().bottom_0().h(px(1.0)).bg(c.card_border)),
            )
            .into_any_element()
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
        // The comparison (or the history) fills the main view; the files are in the primary side bar.
        let right = self.render_right(env.as_ref(), busy, window, cx);
        let page = div()
            .id("source-control-page")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .pad(px(10.0), px(10.0))
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .font_family(theme.font.clone())
            .child(right);
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
    /// The primary side bar's part of the page (VS Code's Source Control view): the title and its actions, the project,
    /// environment and part, the status, the search and filters, and the files. The comparison is the main view's.
    pub fn render_side(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
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
                this.remember_choice(cx);
            }),
            cx,
        );
        let env_items: Vec<(SharedString, bool)> = self
            .environments
            .iter()
            .map(|e| (SharedString::from(e.label()), self.env.as_deref() == Some(e.id.as_str())))
            .collect();
        let env_value = env.as_ref().map_or_else(|| "No server yet".to_string(), |e| e.label());
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
                    this.remember_choice(cx);
                }
            }),
            cx,
        );
        // ─── The title and its actions ────────────────────────────────────────
        let has_env = env.is_some() && !busy;
        let uploads = self.targets().iter().filter(|e| e.change.uploads()).count();
        let downloads = self.targets().iter().filter(|e| e.change.downloads()).count();
        let scope = if self.selected.is_empty() { "shown" } else { "selected" };
        let title = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .h(px(35.0))
            .pad(px(0.0), px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pad(px(0.0), px(4.0))
                    .text_size(theme.text.caption)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.text_muted)
                    .child(ui::one_line("SOURCE CONTROL")),
            )
            .child(if busy {
                // Where Refresh is, while a comparison or a transfer runs (its progress is in Output).
                div()
                    .id("sc-busy")
                    .size(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .tip(match &self.status {
                        Status::Working(what) => what.clone(),
                        _ => "Working…".to_string(),
                    })
                    .child(ui::spinner("sc-spinner", 14.0, c.text_muted, cx))
                    .into_any_element()
            } else {
                ui::icon_button("sc-refresh", Icon::Refresh, c.text_primary, has_env, cx)
                    .tip("Refresh: compare the project with the server again")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx)))
                    .into_any_element()
            })
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
            );

        // ─── The main button: VS Code's Commit, and only that ─────────────────
        let to_commit = self.commit_entries().len();
        let tip = match () {
            _ if env.is_none() => "Add a server first: … → Add server…".to_string(),
            _ if to_commit == 0 => "Nothing staged: stage changes (+) to commit them".to_string(),
            _ => format!("Commit: upload the {to_commit} staged change(s) to the server"),
        };
        let main_button = ui::button("sc-main", "Commit", ui::ButtonKind::Primary, env.is_some() && !busy && to_commit > 0, cx)
            .w_full()
            .tip(tip)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.commit(cx)));

        // ─── The list ─────────────────────────────────────────────────────────
        let rows = Rc::new(self.rows());
        let sections: Rc<Vec<usize>> =
            Rc::new(rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Section { .. })).map(|(i, _)| i).collect());
        let list = uniform_list(
            "sc-files",
            rows.len(),
            cx.processor({
                let rows = rows.clone();
                move |this: &mut Self, range: std::ops::Range<usize>, _window, cx| {
                    range.map(|i| this.render_row(i, &rows[i], false, cx)).collect::<Vec<_>>()
                }
            }),
        )
        // VS Code's sticky headers: the section the top row is in stays at the top, pushed up by the next one.
        .with_decoration(StickySection { page: cx.entity().downgrade(), rows, sections })
        .track_scroll(self.list_scroll.clone())
        .size_full();
        let empty_text = match (&env, &self.status) {
            (None, _) => Some("No server yet. Add one with … → Add server…: its address, how to sign in, and its remote directory."),
            (Some(_), Status::NotCompared) => Some("Refresh to compare the project with the server."),
            (Some(_), Status::Failed(_)) if self.entries.is_empty() => Some("The comparison failed - Output has why. Refresh to try again."),
            (Some(_), Status::Compared { .. }) if self.rows().is_empty() => Some("Nothing differs - or nothing matches the filter."),
            _ => None,
        };
        let list_panel = div()
            .id("sc-list")
            .track_focus(&self.list_focus)
            .key_context("SourceControlList")
            // The rows move under the mouse: their actions wait for the next move to find the row now under it.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                if this.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .relative()
            .flex_1()
            .min_h_0()
            .child(list)
            // The uniform list keeps its size and offset in its inner scroll handle, as a scrolled div does: the app's
            // scroll bar reads and drags that one.
            .child(ui::scrollbar(&self.list_scroll.0.borrow().base_handle, cx))
            .when_some(empty_text, |d, text| {
                d.child(
                    // Wrapped in the side bar's width, centred - a text node of its own wouldn't wrap in a flex row.
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_start()
                        .justify_center()
                        .p(px(20.0))
                        .text_color(c.text_muted)
                        .child(div().w_full().min_w_0().text_center().child(text)),
                )
            });

        // VS Code's view: the title with its actions; the project and server where its message box is, the main button
        // under them; then the files to the bottom.
        let fields = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pad(px(0.0), px(12.0))
            .child(div().w_full().child(project_select))
            .child(div().w_full().child(env_select))
            .child(main_button)
            .child(div().w_full().child(search_field(&self.search, window, cx)));
        div()
            .id("source-control-side")
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .text_size(theme.text.body)
            .text_color(c.text_primary)
            .font_family(theme.font.clone())
            .child(title)
            .child(fields)
            .child(div().flex().flex_col().flex_1().min_h_0().child(list_panel))
            .into_any_element()
    }

    /// One row of the list. A row's own actions are there only while the mouse is on it - so the name and folder
    /// have the row's whole width the rest of the time, and give way to the actions under the mouse. `sticky` is the
    /// copy of a section's row held at the top of the list ([`StickySection`]).
    fn render_row(&mut self, i: usize, row: &Row, sticky: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let is_selected = match row {
            Row::File { entry, .. } => self.selected.contains(&self.entries[*entry].path),
            _ => false,
        };
        let show_actions = self.hovered == Some(i);
        // A folder's depth as a blank before the row's content (the codebase pads evenly, never one side).
        let indent = |depth: usize, extra: f32| div().flex_none().w(px(8.0 * depth as f32 + extra));
        let base = div()
            .id(SharedString::from(format!("sc-row-{i}")))
            // The side bar's width and no more: the name and folder give way, the letter stays.
            .w_full()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(row_height())
            .pad(px(0.0), px(8.0))
            .cursor_pointer()
            .when(is_selected, |d| d.bg(c.selection_bg))
            .when(self.cursor == Some(i) && !sticky, |d| d.border_1().border_color(c.focus_border))
            .hover(move |s| s.bg(c.hover_bg))
            // The row under the mouse, for its actions. A move (not only entering) sets it, so after the list scrolls
            // under a still mouse the first move puts the actions on the row now there.
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.hovered != Some(i) {
                    this.hovered = Some(i);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered && this.hovered == Some(i) {
                    this.hovered = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| this.open_menu(i, e.position, cx)),
            );
        let chevron = |collapsed: bool| {
            ui::icon_sized(if collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, ui::IconSize::Small, c.text_muted, cx)
        };
        let actions = || div().flex().flex_none().items_center();
        let action = |id: String, glyph: Icon, tip: String| {
            ui::icon_button(SharedString::from(id), glyph, c.text_primary, true, cx).size(px(20.0)).tip(tip)
        };
        match row {
            Row::Section { section, count, collapsed } => {
                let section_actions = show_actions.then(|| {
                    let files = list::files_in(row, &self.entries, &self.filter);
                    let ups: Vec<Entry> = files.iter().map(|&f| &self.entries[f]).filter(|e| e.change.uploads()).cloned().collect();
                    let downs: Vec<Entry> =
                        files.iter().map(|&f| &self.entries[f]).filter(|e| e.change.downloads()).cloned().collect();
                    let (n_up, n_down) = (ups.len(), downs.len());
                    let all: Vec<Entry> = files.iter().map(|&f| self.entries[f].clone()).collect();
                    actions()
                        .when(*section == list::Section::Changes, |d| {
                            d.child(action(format!("sc-stage-all-{i}"), Icon::Add, "Stage All Changes".into()).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.stage(all.clone(), true, cx);
                                }),
                            ))
                        })
                        .when(*section == list::Section::Staged, |d| {
                            let all = files.iter().map(|&f| self.entries[f].clone()).collect::<Vec<_>>();
                            d.child(action(format!("sc-unstage-all-{i}"), Icon::Dash, "Unstage All Changes".into()).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.stage(all.clone(), false, cx);
                                }),
                            ))
                        })
                        // The staged ones go up with Commit; a conflict's are kept one by one.
                        .when(n_up > 0 && !matches!(section, list::Section::Changes | list::Section::Staged), |d| {
                            d.child(action(format!("sc-up-all-{i}"), Icon::Upload, format!("Upload all {n_up}")).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    let title = format!("Upload {} file(s)", ups.len());
                                    this.send(ups.clone(), Action::Upload, false, title, cx);
                                }),
                            ))
                        })
                        .when(n_down > 0, |d| {
                            d.child(action(format!("sc-down-all-{i}"), Icon::Download, format!("Download all {n_down}")).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    let title = format!("Download {} file(s)", downs.len());
                                    this.send(downs.clone(), Action::Download, false, title, cx);
                                }),
                            ))
                        })
                });
                base.tip(section.tip())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(row) = this.rows().get(i).cloned() {
                            this.cursor = Some(i);
                            this.toggle_collapsed(&row, cx);
                            // Folded from the sticky copy: back to the section, or the list would show what follows
                            // it far below.
                            if sticky {
                                this.list_scroll.scroll_to_item(i, ScrollStrategy::Top);
                            }
                        }
                    }))
                    .child(chevron(*collapsed))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(theme.text.caption)
                            .child(ui::one_line(section.title())),
                    )
                    .children(section_actions)
                    // VS Code's count badge.
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(18.0))
                            .h(px(16.0))
                            .pad(px(0.0), px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .bg(c.card_border)
                            .text_size(theme.text.caption)
                            .child(count.to_string()),
                    )
            }
            Row::Folder { name, depth, count, collapsed, .. } => base
                .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    let m = e.modifiers();
                    this.press_row(i, m.secondary(), m.shift, cx);
                }))
                .child(indent(*depth, 8.0))
                .child(chevron(*collapsed))
                .child(ui::icon(if *collapsed { Icon::Folder } else { Icon::FolderOpen }, c.text_muted, cx))
                .child(div().flex_1().min_w_0().child(ui::one_line(name.clone())))
                .child(div().text_size(theme.text.caption).text_color(c.text_muted).child(count.to_string())),
            Row::File { entry, name, folder, depth, .. } => {
                let e = &self.entries[*entry];
                let color = change_color(e.change, cx);
                let gone = matches!(e.change, Change::Deleted | Change::DeletedOnServer);
                let file_actions = show_actions.then(|| {
                    let revertible = e.remote.is_some() && matches!(e.change, Change::Modified | Change::Conflict | Change::Deleted);
                    actions()
                        .when(revertible, |d| {
                            let e = e.clone();
                            d.child(action(format!("sc-revert-{i}"), Icon::Reset, "Revert to the server's copy".into()).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.revert_local(vec![e.clone()], cx);
                                }),
                            ))
                        })
                        .when(e.change == Change::Conflict, |d| {
                            let (mine, theirs) = (e.clone(), e.clone());
                            d.child(action(format!("sc-keep-mine-{i}"), Icon::Upload, "Keep mine - upload".into()).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.keep_local(vec![mine.clone()], cx);
                                }),
                            ))
                            .child(
                                action(format!("sc-keep-theirs-{i}"), Icon::Download, "Keep the server's - download".into())
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.keep_server(vec![theirs.clone()], cx);
                                    })),
                            )
                        })
                        // VS Code's Stage Changes (+) and Unstage Changes (-): Commit sends what is staged.
                        .when(list::stageable(e.change), |d| {
                            let (one, staged) = (e.clone(), self.filter.staged.contains(&e.path));
                            let (id, glyph, tip) = if staged {
                                (format!("sc-unstage-{i}"), Icon::Dash, "Unstage Changes")
                            } else {
                                (format!("sc-stage-{i}"), Icon::Add, "Stage Changes")
                            };
                            d.child(action(id, glyph, tip.into()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.stage(vec![one.clone()], !staged, cx);
                            })))
                        })
                        .when(e.change.downloads(), |d| {
                            let e = e.clone();
                            d.child(action(format!("sc-down-{i}"), Icon::Download, "Download from the server".into()).on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    let title = format!("Download {}", e.path);
                                    this.send(vec![e.clone()], Action::Download, false, title, cx);
                                }),
                            ))
                        })
                });
                // In the list view a file starts at the row's edge, as its section's chevron; in the tree, under its
                // folder.
                base.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    let m = e.modifiers();
                    this.press_row(i, m.secondary(), m.shift, cx);
                    if e.click_count() == 2 && !m.secondary() && !m.shift {
                        this.keep_tab(cx);
                    }
                }))
                .tip(format!("{} - {}", e.path, e.change.description()))
                .when(self.tree, |d| d.child(indent(*depth, 18.0)))
                .child(ui::file_icons::file_icon(name, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(div().min_w_0().text_color(color).when(gone, |d| d.line_through()).child(ui::one_line(name.clone())))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(theme.text.caption)
                                .text_color(c.text_muted)
                                .child(ui::one_line(folder.clone())),
                        ),
                )
                .children(file_actions)
                .child(
                    div()
                        .w(px(14.0))
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .text_center()
                        .child(e.change.letter()),
                )
            }
        }
    }

    /// VS Code's tab bar: a tab a file - its icon, its name in its change's colour (in italics while it is the
    /// preview), and its close button on the one shown and the one under the mouse. A click shows it, a double click
    /// keeps it, a middle click closes it.
    /// The shown file's buttons, on the right of the tab bar as VS Code has its editor's: the previous and next
    /// change, then what can be done with this file (merge or settle a conflict, download, delete on the server) -
    /// sending changes is Stage and Commit in the side bar.
    fn diff_actions(&self, busy: bool, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let diff = self.tab()?;
        let c = Theme::get(cx).colors;
        let entry = diff.entry.clone();
        let e = entry.clone();
        let mut row = div().flex().flex_none().items_center().gap(px(4.0)).pad(px(0.0), px(6.0));
        let text_diff = matches!(diff.state, DiffState::Text(_));
        row = row
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
        Some(row)
    }

    fn render_tabs(&mut self, actions: Option<gpui::Div>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let tabs = self.tabs.iter().enumerate().map(|(i, tab)| {
            let shown = i == self.active && self.history.is_none();
            let name = tab.entry.path.rsplit('/').next().unwrap_or_default().to_string();
            let close_shown = shown || self.hovered_tab == Some(i);
            div()
                .id(SharedString::from(format!("sc-tab-{i}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .h_full()
                .pad(px(0.0), px(8.0))
                .border_r_1()
                .border_color(c.card_border)
                .cursor_pointer()
                .bg(if shown { c.card_bg } else { c.window_bg })
                .relative()
                // The tab shown: a line of the accent along its top, as VS Code's - a line of its own, as a border
                // has one colour on every side.
                .when(shown, |d| d.child(div().absolute().top_0().left_0().right_0().h(px(1.0)).bg(c.accent)))
                .tip(format!("{} - {}", tab.entry.path, tab.entry.change.description()))
                .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                    if this.hovered_tab != Some(i) {
                        this.hovered_tab = Some(i);
                        cx.notify();
                    }
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if !*hovered && this.hovered_tab == Some(i) {
                        this.hovered_tab = None;
                        cx.notify();
                    }
                }))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.close_tab(i, cx)))
                .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                    this.history = None;
                    this.active = i;
                    if e.click_count() == 2 {
                        this.keep_tab(cx);
                    }
                    cx.notify();
                }))
                .child(ui::file_icons::file_icon(&name, cx))
                .child(
                    div()
                        .text_color(if shown { change_color(tab.entry.change, cx) } else { change_color(tab.entry.change, cx).opacity(0.8) })
                        .when(tab.preview, |d| d.italic())
                        .child(name),
                )
                .child(
                    div()
                        .text_size(theme.text.caption)
                        .text_color(c.text_muted)
                        .child(match (&tab.entry.remote, &tab.entry.local) {
                            (Some(_), Some(_)) => "Server ↔ Project",
                            (Some(_), None) => "Server",
                            _ => "Project",
                        }),
                )
                // Its place kept while hidden, so the tabs don't move under the mouse.
                .child(div().size(px(20.0)).flex_none().when(close_shown, |d| {
                    d.child(
                        ui::icon_button(SharedString::from(format!("sc-tab-close-{i}")), Icon::Close, c.text_primary, true, cx)
                            .size(px(20.0))
                            .tip("Close (middle click)")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, cx);
                            })),
                    )
                }))
        });
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(35.0))
            .bg(c.window_bg)
            .border_b_1()
            .border_color(c.card_border)
            .child(div().id("sc-tabs").flex().flex_1().min_w_0().h_full().overflow_x_scroll().children(tabs))
            .children(actions)
            .into_any_element()
    }

    fn render_right(&mut self, env: Option<&Environment>, busy: bool, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::get(cx).clone();
        // The diff's font is monospaced: one character's width gives how wide its widest line is.
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&gpui::font(theme.mono_font.clone()));
        self.char_width = text_system.advance(font_id, theme.text.caption, 'm').map_or(theme.text.caption * 0.6, |s| s.width);
        let c = theme.colors;
        let actions = if self.history.is_none() { self.diff_actions(busy, cx) } else { None };
        let mut tab_bar = (!self.tabs.is_empty()).then(|| self.render_tabs(actions, cx));
        let mut frame = || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .rounded(theme.metrics.card_radius)
                .border_1()
                .border_color(c.card_border)
                .bg(c.card_bg)
                .overflow_hidden()
                .children(tab_bar.take())
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
        let Some(diff) = self.tab() else {
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
        let summary = match &diff.state {
            DiffState::Text(text) => {
                let (added, removed) = text.sides.added_removed();
                // Both copies are still shown: only the times differ, or the line endings.
                if text.same_text() { "Same text".to_string() } else { format!("+{added} -{removed}") }
            }
            DiffState::File(file) => if file.server { "Only on the server" } else { "Only in the project" }.to_string(),
            _ => String::new(),
        };
        // VS Code's breadcrumbs: the folders, then the file with its icon.
        let parts: Vec<String> = entry.path.split('/').map(str::to_string).collect();
        let last = parts.len().saturating_sub(1);
        let crumbs = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .overflow_hidden()
            .children(parts.into_iter().enumerate().map(|(k, part)| {
                if k == last {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .min_w_0()
                        .child(ui::file_icons::file_icon(&part, cx))
                        .child(ui::one_line(part))
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.0))
                        .text_color(c.text_muted)
                        .child(part)
                        .child(ui::icon_sized(Icon::ChevronRight, ui::IconSize::Small, c.text_muted, cx))
                        .into_any_element()
                }
            }));
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pad(px(6.0), px(12.0))
            .border_b_1()
            .border_color(c.card_border)
            .child(crumbs)
            .child(div().text_size(theme.text.caption).text_color(c.text_muted).child(summary));
        // The labels over the halves; the area, with the sideways wheel and the drags of the line between the halves
        // and of the sideways scroll bars.
        let labels = match &diff.state {
            DiffState::File(file) if file.server => (format!("Server - {}", meta_text(entry.remote.as_ref())), String::new()),
            DiffState::File(_) => (format!("Project - {}", meta_text(entry.local.as_ref())), String::new()),
            _ => (
                format!("Server - {}", meta_text(entry.remote.as_ref())),
                format!("Project - {}", meta_text(entry.local.as_ref())),
            ),
        };
        let layout = Layout {
            split: self.split,
            scroll_x: diff.scroll_x,
            char_width: self.char_width,
            labels,
            bounds: self.diff_area.clone(),
        };
        let area = |cx: &mut Context<Self>| {
            div()
                .id("sc-diff-area")
                .on_scroll_wheel(cx.listener(Self::wheel_sideways))
                .on_drag_move(cx.listener(|this, e: &DragMoveEvent<SplitDrag>, _, cx| this.drag_split(e, cx)))
                .on_drag_move(cx.listener(|this, e: &DragMoveEvent<HScrollDrag>, _, cx| this.drag_sideways(e, cx)))
        };
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
            DiffState::Text(text) => source_control_diff::side_by_side_view(
                text,
                diff.at,
                &diff.scroll,
                theme.mono_font.clone(),
                &layout,
                area(cx),
                cx,
            ),
            DiffState::File(file) => {
                source_control_diff::one_file_view(file, &diff.scroll, theme.mono_font.clone(), &layout, area(cx), cx)
            }
        };
        frame()
            .child(header)
            .child(div().flex_1().min_h_0().text_size(theme.text.caption).child(body))
            .into_any_element()
    }
}

/// What a file's read copies are shown as: both as text - the diff; one alone - that file; else a binary file.
fn diff_state(path: &str, read: Result<BothCopies, String>) -> DiffState {
    let (remote, local) = match read {
        Ok(copies) => copies,
        Err(e) => return DiffState::Failed(e),
    };
    match (remote.as_deref().map(text_of), local.as_deref().map(text_of)) {
        (None, None) => DiffState::Failed("Neither side has the file any more - Refresh.".into()),
        (Some(None), _) | (_, Some(None)) => DiffState::Binary,
        (Some(Some(server)), None) => DiffState::File(Box::new(OneFile::new(path, &server, true))),
        (None, Some(Some(project))) => DiffState::File(Box::new(OneFile::new(path, &project, false))),
        (Some(Some(server)), Some(Some(project))) => DiffState::Text(Box::new(TextDiff::new(path, &server, &project))),
    }
}

/// "12.3 KB, 23-11-2026 14:02" - or "not there".
fn meta_text(meta: Option<&dnnm_application::source_control::model::FileMeta>) -> String {
    match meta {
        None => "not there".into(),
        Some(m) => format!("{}, {}", size_text(m.size), time_text(m.mtime)),
    }
}
