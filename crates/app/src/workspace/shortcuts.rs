//! Keyboard shortcuts as Settings → Keyboard shortcuts changes them (`keyboard.shortcuts{id}` rows): every binding is
//! made again - the defaults, a changed command's default taken away, and its own shortcut running the command.

use std::collections::{BTreeMap, HashSet};

use gpui::{App, KeyBinding, NoAction};

use dnnm_application::settings::Settings;
use dnnm_dialogs::setting_rules::custom_shortcuts;
use dnnm_dialogs::settings_keyboard::{Shortcut, parse_list};

/// Runs a command of the palette by its id - what a changed shortcut is bound to.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = workspace)]
pub struct RunCommand {
    pub id: String,
}

/// The context a command's default shortcut is bound in (the Projects table's for a row's keys).
fn default_context(id: &str) -> Option<&'static str> {
    matches!(id, "project.start" | "project.stop" | "project.restart").then_some("ProjectsTable")
}

/// The shortcuts a command has now, as one line ("Ctrl+J, Ctrl+Shift+J"): the user's, else its default; None without.
pub(crate) fn shortcut_text(id: &str, settings: &Settings) -> Option<String> {
    let custom = custom_shortcuts(settings);
    match custom.get(id) {
        Some(text) if text.is_empty() => None,
        Some(text) => Some(text.clone()),
        None => super::commands::default_shortcut(id).map(str::to_string),
    }
}

/// Binds every key again: `defaults` (the app's own bindings), then the changes in `settings`.
pub(crate) fn apply(settings: &Settings, cx: &mut App) {
    cx.clear_key_bindings();
    crate::bind_default_keys(cx);
    bind_changes(&custom_shortcuts(settings), cx);
}

fn bind_changes(custom: &BTreeMap<String, String>, cx: &mut App) {
    let mut bindings = Vec::new();
    let mut taken = HashSet::new();
    for (id, text) in custom {
        // The default goes - in the context it was bound in, so it doesn't win there.
        if let Some(default) = super::commands::default_shortcut(id).and_then(Shortcut::parse) {
            bindings.push(KeyBinding::new(&default.to_gpui(), NoAction, default_context(id)));
        }
        // Each of its shortcuts runs it - the first command to take a key keeps it.
        for shortcut in parse_list(text) {
            if taken.insert(shortcut.to_gpui()) {
                bindings.push(KeyBinding::new(&shortcut.to_gpui(), RunCommand { id: id.clone() }, None));
            }
        }
    }
    cx.bind_keys(bindings);
}
