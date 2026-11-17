//! Settings → Keyboard shortcuts' model (`Pages/SettingsPage.Keyboard.cs`, `Services/Shortcut.cs` and the shortcut
//! part of `Services/AppCommands.cs`): a shortcut written as VS Code writes it (`Ctrl+Shift+P`), its rules, the rows
//! with their conflicts and warnings, and the changes - kept as the `keyboard.shortcuts` rows (only what differs from
//! the defaults; "" takes a command's shortcuts away). A command may have several shortcuts: written one after the
//! other, ", " between them (`Ctrl+J, Ctrl+Shift+J` - "Ctrl+," stays one, the comma key).
//!
//! The commands themselves are the app's: it passes each one's id, title, area and default shortcut.

use std::collections::BTreeMap;
use std::fmt;

/// A command as Settings lists it - what the app passes in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutCommand {
    /// What `keyboard.shortcuts` names it by, e.g. `project.start`.
    pub id: String,
    pub title: String,
    /// Projects, Project, Pages, Panel, Terminal, Application.
    pub area: String,
    /// `Ctrl+F5`; None for none.
    pub default_shortcut: Option<String>,
}

impl ShortcutCommand {
    pub fn new(id: &str, title: &str, area: &str, default_shortcut: Option<&str>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            area: area.into(),
            default_shortcut: default_shortcut.map(Into::into),
        }
    }

    /// As the palette lists it: "Project: Start project".
    pub fn label(&self) -> String {
        format!("{}: {}", self.area, self.title)
    }
}

/// Modifiers and one key (`Shortcut`). The key is kept as it is written: `P`, `F5`, `,`, `PageDown`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    pub key: String,
}

/// Keys by the names `Shortcut.ToString` writes, with the other names the WPF app's `Key` enum and GPUI give them.
const KEY_NAMES: &[(&str, &[&str])] = &[
    (",", &["OemComma", "comma"]),
    (".", &["OemPeriod", "period"]),
    ("`", &["Oem3", "OemTilde", "backquote"]),
    ("-", &["OemMinus", "minus"]),
    ("=", &["OemPlus", "equal"]),
    ("/", &["Oem2", "OemQuestion", "slash"]),
    ("\\", &["Oem5", "OemPipe", "backslash"]),
    ("[", &["OemOpenBrackets", "Oem4", "bracketleft"]),
    ("]", &["Oem6", "OemCloseBrackets", "bracketright"]),
    (";", &["Oem1", "OemSemicolon", "semicolon"]),
    ("'", &["OemQuotes", "Oem7", "quote"]),
    ("PageDown", &["Next"]),
    ("PageUp", &["Prior"]),
    ("Enter", &["Return"]),
    ("Backspace", &["Back"]),
    ("Esc", &["Escape"]),
    ("Delete", &[]),
    ("Insert", &[]),
    ("Home", &[]),
    ("End", &[]),
    ("Up", &[]),
    ("Down", &[]),
    ("Left", &[]),
    ("Right", &[]),
    ("Tab", &[]),
    ("Space", &[]),
    ("Pause", &[]),
];

/// The key `name` written as `Shortcut.ToString` writes it; None when it isn't a key (or is a modifier on its own).
pub fn canonical_key(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    for (canonical, aliases) in KEY_NAMES {
        if canonical.eq_ignore_ascii_case(name) || aliases.iter().any(|a| a.eq_ignore_ascii_case(name)) {
            return Some(canonical.to_string());
        }
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return (first.is_ascii_alphanumeric()).then(|| first.to_ascii_uppercase().to_string());
    }
    // D0 … D9: the digit keys.
    if name.len() == 2 && name.as_bytes()[0].eq_ignore_ascii_case(&b'd') && name.as_bytes()[1].is_ascii_digit() {
        return Some(name[1..].to_string());
    }
    if let Some(n) = name.strip_prefix(['f', 'F']).and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(format!("F{n}"));
    }
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "control"
            | "ctrl"
            | "shift"
            | "alt"
            | "platform"
            | "function"
            | "leftctrl"
            | "rightctrl"
            | "leftshift"
            | "rightshift"
            | "leftalt"
            | "rightalt"
            | "lwin"
            | "rwin"
            | "none"
    ) {
        return None;
    }
    // Another of the WPF app's key names (NumPad1, Add…): kept as written.
    name.chars().all(|c| c.is_ascii_alphanumeric()).then(|| name.to_string())
}

impl Shortcut {
    /// Reads `Ctrl+Shift+P` (`Shortcut.Parse`); None when it isn't a shortcut.
    pub fn parse(text: &str) -> Option<Shortcut> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // "Ctrl++" is Ctrl and the plus key: split off the last key first.
        let split = if trimmed.ends_with("++") { Some(trimmed.len() - 1) } else { trimmed.rfind('+') };
        let key_text = match split {
            Some(at) => &trimmed[at + 1..],
            None => trimmed,
        };
        let mut shortcut = Shortcut { ctrl: false, shift: false, alt: false, win: false, key: String::new() };
        if let Some(at) = split.filter(|at| *at > 0) {
            for part in trimmed[..at].split('+').map(str::trim).filter(|p| !p.is_empty()) {
                match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => shortcut.ctrl = true,
                    "shift" => shortcut.shift = true,
                    "alt" => shortcut.alt = true,
                    "win" | "windows" | "meta" => shortcut.win = true,
                    _ => return None,
                }
            }
        }
        shortcut.key = canonical_key(key_text)?;
        Some(shortcut)
    }

    /// The shortcut a GPUI key press is - None for a modifier on its own.
    pub fn from_keystroke(keystroke: &gpui::Keystroke) -> Option<Shortcut> {
        let m = keystroke.modifiers;
        Some(Shortcut {
            ctrl: m.control,
            shift: m.shift,
            alt: m.alt,
            win: m.platform,
            key: canonical_key(&keystroke.key)?,
        })
    }

    fn is_function_key(&self) -> bool {
        self.key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (1..=24).contains(&n))
    }

    /// Why this can't be a shortcut; None when it can.
    pub fn problem(&self) -> Option<&'static str> {
        (!(self.is_function_key() || self.ctrl || self.alt)).then_some(
            "Use Ctrl or Alt with it, or a function key (F1-F24) - plain keys are for typing and moving around.",
        )
    }

    /// A text box's editing key (Ctrl+A, C, V, X, Z, Y) - it would no longer copy or paste in one.
    pub fn is_editing_key(&self) -> bool {
        self.ctrl
            && !self.shift
            && !self.alt
            && !self.win
            && matches!(self.key.as_str(), "A" | "C" | "V" | "X" | "Z" | "Y")
    }

    /// As a GPUI key binding writes it: `ctrl-shift-p`, `f5`, `ctrl-,` - for the app's bindings.
    pub fn to_gpui(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (on, name) in [(self.ctrl, "ctrl"), (self.alt, "alt"), (self.shift, "shift"), (self.win, "win")] {
            if on {
                parts.push(name.into());
            }
        }
        let key = match self.key.as_str() {
            "Esc" => "escape".to_string(),
            other => other.to_ascii_lowercase(),
        };
        parts.push(key);
        parts.join("-")
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.shift, "Shift+"), (self.alt, "Alt+"), (self.win, "Win+")] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Reads a command's shortcuts as kept: `Ctrl+J, Ctrl+Shift+J` → both; "" → none. What isn't a shortcut is left out.
pub fn parse_list(text: &str) -> Vec<Shortcut> {
    let mut list: Vec<Shortcut> = Vec::new();
    for part in text.split(", ") {
        if let Some(shortcut) = Shortcut::parse(part)
            && !list.contains(&shortcut)
        {
            list.push(shortcut);
        }
    }
    list
}

/// Writes shortcuts as they are kept: `Ctrl+J, Ctrl+Shift+J`.
pub fn write_list(shortcuts: &[Shortcut]) -> String {
    shortcuts.iter().map(Shortcut::to_string).collect::<Vec<_>>().join(", ")
}

/// A command's row: its shortcuts now, its default, whether it was changed, and what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutRow {
    pub command: ShortcutCommand,
    /// Every shortcut that runs it, in the order chosen; empty for none.
    pub shortcuts: Vec<Shortcut>,
    pub default: Option<Shortcut>,
    pub is_custom: bool,
    pub warning: String,
}

fn none_or(shortcut: &Option<Shortcut>) -> String {
    shortcut.as_ref().map_or_else(|| "none".to_string(), Shortcut::to_string)
}

impl ShortcutRow {
    /// "Project · default F5" - where it belongs, and its default when it was changed.
    pub fn detail(&self) -> String {
        if self.is_custom {
            format!("{} · default {}", self.command.area, none_or(&self.default))
        } else {
            self.command.area.clone()
        }
    }

    /// Its shortcuts as one line: "Ctrl+J, Ctrl+Shift+J", or "-" for none.
    pub fn shortcuts_text(&self) -> String {
        if self.shortcuts.is_empty() { "-".to_string() } else { write_list(&self.shortcuts) }
    }

    pub fn shortcut_tip(recording: bool) -> &'static str {
        if recording { "Press the new shortcut - Esc cancels, Backspace removes it" } else { "Change this shortcut" }
    }

    pub fn add_tip(recording: bool) -> &'static str {
        if recording { "Press the new shortcut - Esc cancels" } else { "Add another shortcut" }
    }

    pub fn reset_tip(&self) -> String {
        if self.is_custom {
            format!("Back to the default: {}", none_or(&self.default))
        } else {
            "It has its default".into()
        }
    }

    /// Whether every word of `search` is in its title, area or shortcut.
    pub fn matches(&self, search: &str) -> bool {
        let text = format!(
            "{} {} {}",
            self.command.title,
            self.command.area,
            write_list(&self.shortcuts)
        )
        .to_lowercase();
        search.split_whitespace().all(|w| text.contains(&w.to_lowercase()))
    }
}

/// The command's shortcuts now: the ones chosen (`custom`, "" = none), or its default.
pub fn shortcuts_of(command: &ShortcutCommand, custom: &BTreeMap<String, String>) -> Vec<Shortcut> {
    match custom.get(&command.id) {
        Some(chosen) => parse_list(chosen),
        None => command.default_shortcut.as_deref().and_then(Shortcut::parse).into_iter().collect(),
    }
}

/// Every command's row, with who shares a shortcut and which editing keys are taken (`ShowShortcuts`).
pub fn rows(commands: &[ShortcutCommand], custom: &BTreeMap<String, String>) -> Vec<ShortcutRow> {
    let mut rows: Vec<ShortcutRow> = commands
        .iter()
        .map(|c| ShortcutRow {
            command: c.clone(),
            shortcuts: shortcuts_of(c, custom),
            default: c.default_shortcut.as_deref().and_then(Shortcut::parse),
            is_custom: custom.contains_key(&c.id),
            warning: String::new(),
        })
        .collect();
    let warnings: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            // One line for each of its shortcuts that another command shares, or that is a text box's editing key.
            let mut lines = Vec::new();
            for shortcut in &row.shortcuts {
                let sharing: Vec<usize> = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.shortcuts.contains(shortcut))
                    .map(|(j, _)| j)
                    .collect();
                if sharing.len() > 1 {
                    let others: Vec<String> =
                        sharing.iter().filter(|j| **j != i).map(|j| rows[*j].command.label()).collect();
                    // The first listed runs (AppCommands.Match).
                    let runs = sharing[0] == i;
                    lines.push(format!(
                        "{shortcut} is also the shortcut of {}{}",
                        others.join(", "),
                        if runs { " - this one runs." } else { " - that one runs, not this." }
                    ));
                } else if shortcut.is_editing_key() {
                    lines.push(format!(
                        "{shortcut} copies, pastes or undoes in text boxes - it won't do that there any more."
                    ));
                }
            }
            lines.join(" ")
        })
        .collect();
    for (row, warning) in rows.iter_mut().zip(warnings) {
        row.warning = warning;
    }
    rows
}

/// Which of a command's shortcuts a recording changes: one of them (by its place), or a new one after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    At(usize),
    New,
}

/// `custom` with `command`'s shortcut in `slot` set to `shortcut` - None takes that one away - the others kept;
/// Err with why it can't be (`AppCommands.Set`).
pub fn with_shortcut(
    custom: &BTreeMap<String, String>,
    command: &ShortcutCommand,
    slot: Slot,
    shortcut: Option<&Shortcut>,
) -> Result<BTreeMap<String, String>, &'static str> {
    if let Some(problem) = shortcut.and_then(Shortcut::problem) {
        return Err(problem);
    }
    let mut list = shortcuts_of(command, custom);
    match (slot, shortcut) {
        (Slot::At(i), Some(s)) if i < list.len() => list[i] = s.clone(),
        (Slot::At(i), None) if i < list.len() => {
            list.remove(i);
        }
        (_, Some(s)) => list.push(s.clone()),
        (Slot::New, None) | (Slot::At(_), None) => {}
    }
    // The same shortcut twice is once.
    let mut unique: Vec<Shortcut> = Vec::new();
    for s in list {
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    let mut next = custom.clone();
    let default: Vec<Shortcut> = command.default_shortcut.as_deref().and_then(Shortcut::parse).into_iter().collect();
    if unique == default {
        next.remove(&command.id);
    } else {
        next.insert(command.id.clone(), write_list(&unique));
    }
    Ok(next)
}

/// What a recorded key press does: cancel (Esc), remove the shortcut (Backspace or Delete alone), choose one, or
/// nothing yet (a modifier on its own - the key comes next).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Cancel,
    Remove,
    Chosen(Shortcut),
    Wait,
}

pub fn recorded(keystroke: &gpui::Keystroke) -> Recorded {
    let plain = !keystroke.modifiers.modified();
    match Shortcut::from_keystroke(keystroke) {
        Some(s) if plain && s.key == "Esc" => Recorded::Cancel,
        Some(s) if plain && (s.key == "Backspace" || s.key == "Delete") => Recorded::Remove,
        Some(s) => Recorded::Chosen(s),
        None => Recorded::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Vec<ShortcutCommand> {
        vec![
            ShortcutCommand::new("project.start", "Start project", "Project", Some("F5")),
            ShortcutCommand::new("project.stop", "Stop project", "Project", Some("Shift+F5")),
            ShortcutCommand::new("view.palette", "Show all commands", "Application", Some("Ctrl+Shift+P")),
            ShortcutCommand::new("view.none", "No default", "Pages", None),
        ]
    }

    #[test]
    fn parses_and_writes_as_vs_code_does() {
        assert_eq!(Shortcut::parse("Ctrl+Shift+P").unwrap().to_string(), "Ctrl+Shift+P");
        assert_eq!(Shortcut::parse("shift+ctrl+p").unwrap().to_string(), "Ctrl+Shift+P");
        assert_eq!(Shortcut::parse("Ctrl+`").unwrap().to_string(), "Ctrl+`");
        assert_eq!(Shortcut::parse("Ctrl+OemComma").unwrap().to_string(), "Ctrl+,");
        assert_eq!(Shortcut::parse("Ctrl+D1").unwrap().to_string(), "Ctrl+1");
        assert_eq!(Shortcut::parse("Ctrl+Next").unwrap().to_string(), "Ctrl+PageDown");
        assert_eq!(Shortcut::parse("f12").unwrap().to_string(), "F12");
        assert_eq!(Shortcut::parse("Ctrl++"), None, "as the WPF app: no key is named +");
        assert_eq!(Shortcut::parse(""), None);
        assert_eq!(Shortcut::parse("Hyper+P"), None);
        assert_eq!(Shortcut::parse("Ctrl+"), None);
        assert_eq!(Shortcut::parse("Ctrl+Shift+P").unwrap().to_gpui(), "ctrl-shift-p");
        assert_eq!(Shortcut::parse("Esc").unwrap().to_gpui(), "escape");
    }

    #[test]
    fn reads_gpui_key_presses() {
        let ks = |s: &str| gpui::Keystroke::parse(s).unwrap();
        assert_eq!(Shortcut::from_keystroke(&ks("ctrl-shift-p")).unwrap().to_string(), "Ctrl+Shift+P");
        assert_eq!(Shortcut::from_keystroke(&ks("f5")).unwrap().to_string(), "F5");
        assert_eq!(Shortcut::from_keystroke(&ks("alt-pageup")).unwrap().to_string(), "Alt+PageUp");
        assert_eq!(recorded(&ks("escape")), Recorded::Cancel);
        assert_eq!(recorded(&ks("backspace")), Recorded::Remove);
        assert_eq!(recorded(&ks("ctrl-backspace")), Recorded::Chosen(Shortcut::parse("Ctrl+Backspace").unwrap()));
    }

    #[test]
    fn needs_ctrl_alt_or_a_function_key() {
        assert!(Shortcut::parse("P").unwrap().problem().is_some());
        assert!(Shortcut::parse("Shift+P").unwrap().problem().is_some());
        assert!(Shortcut::parse("Shift+F5").unwrap().problem().is_none());
        assert!(Shortcut::parse("Alt+P").unwrap().problem().is_none());
        assert!(Shortcut::parse("Ctrl+C").unwrap().is_editing_key());
        assert!(!Shortcut::parse("Ctrl+Shift+C").unwrap().is_editing_key());
    }

    #[test]
    fn rows_say_who_shares_a_shortcut_and_which_one_runs() {
        let custom = BTreeMap::from([("project.stop".to_string(), "F5".to_string())]);
        let rows = rows(&commands(), &custom);
        assert_eq!(rows[0].warning, "F5 is also the shortcut of Project: Stop project - this one runs.");
        assert_eq!(rows[1].warning, "F5 is also the shortcut of Project: Start project - that one runs, not this.");
        assert_eq!(rows[1].detail(), "Project · default Shift+F5");
        assert_eq!(rows[1].reset_tip(), "Back to the default: Shift+F5");
        assert_eq!(rows[0].detail(), "Project");
        assert_eq!(rows[0].reset_tip(), "It has its default");
        assert_eq!(rows[3].shortcuts_text(), "-");
        assert!(rows[2].matches("application ctrl+shift"));
        assert!(!rows[2].matches("project"));
    }

    #[test]
    fn an_editing_key_is_warned_about() {
        let custom = BTreeMap::from([("view.none".to_string(), "Ctrl+V".to_string())]);
        assert_eq!(
            rows(&commands(), &custom)[3].warning,
            "Ctrl+V copies, pastes or undoes in text boxes - it won't do that there any more."
        );
    }

    #[test]
    fn changes_keep_only_what_differs_from_the_default() {
        let c = commands();
        let empty = BTreeMap::new();
        let next = with_shortcut(&empty, &c[0], Slot::At(0), Shortcut::parse("Ctrl+F5").as_ref()).unwrap();
        assert_eq!(next.get("project.start").map(String::as_str), Some("Ctrl+F5"));
        let back = with_shortcut(&next, &c[0], Slot::At(0), Shortcut::parse("F5").as_ref()).unwrap();
        assert!(back.is_empty(), "the default again is no change");
        let removed = with_shortcut(&empty, &c[0], Slot::At(0), None).unwrap();
        assert_eq!(removed.get("project.start").map(String::as_str), Some(""));
        assert!(shortcuts_of(&c[0], &removed).is_empty());
        assert_eq!(
            with_shortcut(&empty, &c[0], Slot::New, Shortcut::parse("P").as_ref()).unwrap_err(),
            "Use Ctrl or Alt with it, or a function key (F1-F24) - plain keys are for typing and moving around."
        );
    }

    #[test]
    fn a_command_can_have_several_shortcuts() {
        let c = commands();
        let empty = BTreeMap::new();
        // Another next to the default, then a third; the same one again isn't added twice.
        let two = with_shortcut(&empty, &c[0], Slot::New, Shortcut::parse("Ctrl+F5").as_ref()).unwrap();
        assert_eq!(two.get("project.start").map(String::as_str), Some("F5, Ctrl+F5"));
        let three = with_shortcut(&two, &c[0], Slot::New, Shortcut::parse("Ctrl+,").as_ref()).unwrap();
        assert_eq!(three.get("project.start").map(String::as_str), Some("F5, Ctrl+F5, Ctrl+,"));
        assert_eq!(with_shortcut(&three, &c[0], Slot::New, Shortcut::parse("F5").as_ref()).unwrap(), three);
        assert_eq!(shortcuts_of(&c[0], &three).len(), 3, "Ctrl+, is one shortcut, the comma key");
        // Changing one keeps the others; taking all but the default away is the default again.
        let changed = with_shortcut(&three, &c[0], Slot::At(1), Shortcut::parse("Alt+S").as_ref()).unwrap();
        assert_eq!(changed.get("project.start").map(String::as_str), Some("F5, Alt+S, Ctrl+,"));
        let fewer = with_shortcut(&changed, &c[0], Slot::At(2), None).unwrap();
        let back = with_shortcut(&fewer, &c[0], Slot::At(1), None).unwrap();
        assert!(back.is_empty());
        // Each shared one is warned about.
        let shared = BTreeMap::from([("view.none".to_string(), "Shift+F5, Ctrl+V".to_string())]);
        let rows = rows(&c, &shared);
        assert_eq!(rows[3].shortcuts_text(), "Shift+F5, Ctrl+V");
        assert!(rows[3].warning.contains("Shift+F5 is also the shortcut of Project: Stop project"));
        assert!(rows[3].warning.contains("Ctrl+V copies, pastes or undoes"));
        assert!(rows[3].matches("ctrl+v"));
        assert_eq!(parse_list(""), Vec::<Shortcut>::new());
        assert_eq!(write_list(&parse_list("Ctrl+J,  Ctrl+J, F2")), "Ctrl+J, F2");
    }
}
