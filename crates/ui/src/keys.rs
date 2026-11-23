//! The keyboard conventions of the operating system the app runs on: macOS takes Cmd (⌘) where Windows and Linux take
//! Ctrl, and writes shortcuts with its own symbols (⇧⌘P). Everything that depends on it takes an [`Os`], so each OS's
//! keys are tested on any machine; the app passes [`Os::CURRENT`].

/// An operating system, as far as its keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    /// The one this build runs on.
    pub const CURRENT: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };

    pub const ALL: [Os; 3] = [Os::Windows, Os::Mac, Os::Linux];

    /// The modifier of the copy / paste keys and the app's shortcuts, as a GPUI key binding writes it: `cmd` on macOS,
    /// `ctrl` elsewhere - GPUI's own `secondary-` for the OS given.
    pub fn primary(self) -> &'static str {
        if self == Os::Mac { "cmd" } else { "ctrl" }
    }
}

/// How a shortcut is written on `os`, from its modifiers and its key as Settings → Keyboard shortcuts names it (`P`,
/// `F5`, `,`, `PageDown`, `Backspace`): `Ctrl+Shift+P` on Windows, `Ctrl+Shift+Super+P` on Linux (Super is the
/// Windows key), `⌃⌥⇧⌘P` on macOS - its modifiers in its own order, and its own signs for the keys that have one.
pub fn label(os: Os, ctrl: bool, alt: bool, shift: bool, platform: bool, key: &str) -> String {
    let mut text = String::new();
    if os == Os::Mac {
        for (on, sign) in [(ctrl, '⌃'), (alt, '⌥'), (shift, '⇧'), (platform, '⌘')] {
            if on {
                text.push(sign);
            }
        }
        text.push_str(mac_key(key));
    } else {
        let platform_name = if os == Os::Windows { "Win+" } else { "Super+" };
        for (on, name) in [(ctrl, "Ctrl+"), (shift, "Shift+"), (alt, "Alt+"), (platform, platform_name)] {
            if on {
                text.push_str(name);
            }
        }
        text.push_str(key);
    }
    text
}

/// The signs macOS writes keys with in its menus; a key without one is written by its name.
pub const MAC_KEY_SIGNS: &[(&str, &str)] = &[
    ("Backspace", "⌫"),
    ("Delete", "⌦"),
    ("Enter", "↩"),
    ("Esc", "⎋"),
    ("Tab", "⇥"),
    ("Left", "←"),
    ("Right", "→"),
    ("Up", "↑"),
    ("Down", "↓"),
    ("PageUp", "⇞"),
    ("PageDown", "⇟"),
    ("Home", "↖"),
    ("End", "↘"),
];

fn mac_key(key: &str) -> &str {
    MAC_KEY_SIGNS.iter().find(|(name, _)| *name == key).map_or(key, |(_, sign)| sign)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_writes_a_shortcut_its_own_way() {
        assert_eq!(label(Os::Windows, true, false, true, false, "P"), "Ctrl+Shift+P");
        assert_eq!(label(Os::Linux, true, true, false, true, "B"), "Ctrl+Alt+Super+B");
        assert_eq!(label(Os::Mac, false, false, true, true, "P"), "⇧⌘P");
        assert_eq!(label(Os::Mac, true, true, true, true, "Z"), "⌃⌥⇧⌘Z");
        assert_eq!(label(Os::Mac, false, false, false, true, "Backspace"), "⌘⌫");
        assert_eq!(label(Os::Mac, true, false, false, false, "`"), "⌃`");
        assert_eq!(label(Os::Mac, false, false, true, false, "F5"), "⇧F5");
        assert_eq!(label(Os::Windows, false, false, false, false, "F5"), "F5");
    }

    #[test]
    fn the_primary_modifier_is_cmd_on_macos_only() {
        assert_eq!(Os::Mac.primary(), "cmd");
        assert_eq!(Os::Windows.primary(), "ctrl");
        assert_eq!(Os::Linux.primary(), "ctrl");
        let expected = if cfg!(target_os = "macos") { Os::Mac } else if cfg!(windows) { Os::Windows } else { Os::Linux };
        assert_eq!(Os::CURRENT, expected);
    }
}
