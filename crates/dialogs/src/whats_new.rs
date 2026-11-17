//! What's new - `Controls/WhatsNewDialog.xaml(.cs)` and `Services/ReleaseNotes.cs`: the release notes of one or more
//! versions, newest first, from `.docs/release-notes/v*.md` (embedded at build time by `build.rs`, as the WPF project
//! embeds them), drawn with the [`markdown`](crate::markdown) renderer. Shown by the first start of a newer version
//! ([`between`] the version before the update and this one) and by Help → What's New in This Version ([`latest`]).

use dnnm_ui::px;
use std::cmp::Ordering;
use std::fmt;
use std::rc::Rc;

use gpui::{App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, KeyBinding, SharedString, Window, actions, div, prelude::*};

use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::{ButtonKind, Tip, button, link};

use crate::help_menu::{RELEASES_URL, REPOSITORY};
use crate::markdown::{self, Block, OnLink};

actions!(whats_new, [CloseWhatsNew]);

/// Esc and Enter close it (the XAML's Close is `IsDefault` and `IsCancel`). Call once at the start.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseWhatsNew, Some("WhatsNewDialog")),
        KeyBinding::new("enter", CloseWhatsNew, Some("WhatsNewDialog")),
    ]);
}

/// A version as .NET's `System.Version` holds it: two to four numbers, the missing ones -1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: i64,
    pub minor: i64,
    pub build: i64,
    pub revision: i64,
}

impl Version {
    pub const fn new(major: i64, minor: i64, build: i64) -> Self {
        Self { major, minor, build, revision: -1 }
    }

    /// "v1.7.0" or "1.7.0" - a pre-release suffix ("-rc.1") is left out (`AppReleaseFeed.TryParseVersion`).
    pub fn parse(tag: &str) -> Option<Self> {
        let core = tag.trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
        let parts: Vec<i64> = core
            .split('.')
            .map(|p| p.trim().parse::<i64>().ok().filter(|n| (0..=i32::MAX as i64).contains(n)))
            .collect::<Option<_>>()?;
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let at = |i: usize| parts.get(i).copied().unwrap_or(-1);
        Some(Self { major: at(0), minor: at(1), build: at(2), revision: at(3) })
    }

    /// Whether this is newer than `current` - by major, minor and build only (`AppReleaseFeed.IsNewer`).
    pub fn is_newer_than(&self, current: &Version) -> bool {
        let n = |v: &Version| (v.major, v.minor, v.build.max(0));
        n(self) > n(current)
    }

    fn key(&self) -> (i64, i64, i64, i64) {
        (self.major, self.minor, self.build, self.revision)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.build >= 0 {
            write!(f, ".{}", self.build)?;
            if self.revision >= 0 {
                write!(f, ".{}", self.revision)?;
            }
        }
        Ok(())
    }
}

/// An embedded notes file: its tag (`v1.8.7`) and its Markdown.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedNote {
    pub tag: &'static str,
    pub markdown: &'static str,
}

const EMBEDDED: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/release_notes.rs"));

/// Every notes file built in, in file name order.
pub fn all_notes() -> impl Iterator<Item = EmbeddedNote> {
    EMBEDDED.iter().map(|&(tag, markdown)| EmbeddedNote { tag, markdown })
}

/// A release's notes: its version and its Markdown, as `.docs/release-notes/vX.Y.Z.md` has it (`ReleaseNote`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNote {
    pub version: Version,
    pub markdown: &'static str,
}

impl ReleaseNote {
    /// Where a link in the notes goes: a relative one (`../../CHANGELOG.md#v173`) to the file as it is at the release's
    /// tag on GitHub - as the release workflow turns them for the GitHub release (release-notes.ps1).
    pub fn link_target(&self, link: &str) -> String {
        if has_scheme(link) {
            return link.to_string();
        }
        if link.starts_with('#') {
            return format!("{REPOSITORY}/releases/tag/v{}", self.version);
        }
        resolve(&format!("{REPOSITORY}/blob/v{}/{}/", self.version, self.notes_folder()), link)
    }

    // Where the notes are at the release's tag: docs/ up to v1.7.6, .docs/ since the folder was renamed.
    fn notes_folder(&self) -> &'static str {
        if self.version <= Version::new(1, 7, 6) { "docs/release-notes" } else { ".docs/release-notes" }
    }
}

/// An absolute URI (`https:`, `mailto:`…) - kept as it is.
fn has_scheme(link: &str) -> bool {
    match link.find(':') {
        Some(at) if at > 1 => {
            let scheme = &link[..at];
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        _ => false,
    }
}

/// `new Uri(new Uri(base), link)` for a relative path: `..` and `.` taken off the base's folder, the query and the
/// fragment kept.
fn resolve(base: &str, link: &str) -> String {
    let (origin, base_path) = match base.find("://").and_then(|s| base[s + 3..].find('/').map(|p| s + 3 + p)) {
        Some(at) => (&base[..at], &base[at..]),
        None => (base, "/"),
    };
    let split = link.find(['?', '#']).unwrap_or(link.len());
    let (path, rest) = link.split_at(split);
    let mut segments: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        let folder = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        folder.split('/').filter(|s| !s.is_empty()).collect()
    };
    for part in path.split('/').filter(|s| !s.is_empty()) {
        match part {
            ".." => {
                segments.pop();
            }
            "." => {}
            p => segments.push(p),
        }
    }
    let trailing = path.ends_with('/') && !segments.is_empty();
    format!("{origin}/{}{}{rest}", segments.join("/"), if trailing { "/" } else { "" })
}

/// The notes of the versions after `from` up to `to` - newest first; every one up to `to` when `from` is `None`
/// (`ReleaseNotes.Between`). A pre-release's notes (v1.7.0-rc.1) aren't what an installed release changed.
pub fn between(from: Option<Version>, to: Version) -> Vec<ReleaseNote> {
    let mut notes: Vec<ReleaseNote> = all_notes()
        .filter(|n| !n.tag.contains('-'))
        .filter_map(|n| Version::parse(n.tag).map(|version| ReleaseNote { version, markdown: n.markdown }))
        .filter(|n| from.is_none_or(|from| n.version.is_newer_than(&from)) && !n.version.is_newer_than(&to))
        .collect();
    notes.sort_by_key(|n| std::cmp::Reverse(n.version));
    notes
}

/// Help → What's New in This Version: the notes of this version (the newest up to it).
pub fn latest(current: Version) -> Vec<ReleaseNote> {
    between(None, current).into_iter().take(1).collect()
}

/// A link in the notes was clicked - open it (the target is already resolved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenLink(pub String);

/// The dialog. Emits [`OpenLink`] for a link and "All releases on GitHub", and `DialogEvent::Done(())` on Close / Esc.
pub struct WhatsNewDialog {
    title: SharedString,
    notes: Vec<(ReleaseNote, Vec<Block>)>,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent<()>> for WhatsNewDialog {}
impl EventEmitter<OpenLink> for WhatsNewDialog {}

impl WhatsNewDialog {
    /// Shows `notes` (from [`between`] or [`latest`]); the app shows nothing when they are empty (a build without them),
    /// as `WhatsNewDialog.Show` does. Takes the keyboard.
    pub fn new(notes: Vec<ReleaseNote>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = match notes.as_slice() {
            [one] => format!("What's new in DNN Manager {}", one.version),
            _ => "What's new in DNN Manager".to_string(),
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        let notes = notes.into_iter().map(|n| (n.clone(), markdown::parse(n.markdown))).collect();
        Self { title: title.into(), notes, focus }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Done(()));
    }
}

impl Focusable for WhatsNewDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WhatsNewDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = dnnm_ui::theme::Theme::get(cx).clone();
        let k = theme.text.body / px(13.0);
        let this = cx.entity().downgrade();
        let on_link: OnLink = Rc::new(move |target: &str, _: &mut Window, cx: &mut App| {
            let target = target.to_string();
            let _ = this.update(cx, |_, cx| cx.emit(OpenLink(target)));
        });
        // One document: a note's last block and the next note's heading are spaced as any two blocks.
        let mut pieces = Vec::new();
        for (i, (note, blocks)) in self.notes.iter().enumerate() {
            pieces.extend(markdown::render(
                &format!("note-{i}"),
                blocks,
                i == 0,
                &|l| note.link_target(l),
                &on_link,
                cx,
            ));
        }
        let height = (window.viewport_size().height - px(80.0)).min(px(640.0));
        let buttons = div()
            .flex_1()
            .flex()
            .items_center()
            .justify_between()
            .child(
                link("whats-new-releases", "All releases on GitHub", cx)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(OpenLink(RELEASES_URL.to_string())))),
            )
            .child(
                button("whats-new-close", "Close", ButtonKind::Primary, true, cx)
                    .min_w(px(80.0))
                    .tip("Close (Esc)")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close(cx))),
            );
        div()
            .id("whats-new")
            .key_context("WhatsNewDialog")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseWhatsNew, _, cx| this.close(cx)))
            .child(
                dialog::frame(
                    self.title.clone(),
                    720.0,
                    // The document's own padding (28, 22) less the frame's (20, 12), about.
                    div().p(px(8.0)).text_size(px(13.5 * k)).line_height(px(21.0 * k)).child(markdown::stack(pieces)),
                    buttons,
                    cx,
                )
                .max_h(height),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_as_dotnet_does() {
        assert_eq!(Version::parse("v1.8.7"), Some(Version::new(1, 8, 7)));
        assert_eq!(Version::parse("1.7.0-rc.1"), Some(Version::new(1, 7, 0)));
        assert_eq!(Version::parse("v1.8").map(|v| v.to_string()).as_deref(), Some("1.8"));
        assert_eq!(Version::parse("v1"), None);
        assert_eq!(Version::parse("vx.y"), None);
        assert!(Version::new(1, 8, 0).is_newer_than(&Version::parse("1.7").unwrap()));
        assert!(!Version::parse("1.8").unwrap().is_newer_than(&Version::new(1, 8, 0)));
    }

    #[test]
    fn every_built_in_note_is_a_release() {
        // Whatever .docs/release-notes holds (none at all is allowed): each is a version's tag with text.
        for note in all_notes() {
            assert!(Version::parse(note.tag).is_some(), "{}", note.tag);
            assert!(!note.markdown.is_empty(), "{}", note.tag);
        }
    }

    #[test]
    fn between_is_newest_first_after_from_up_to_to() {
        let (from, to) = (Version::new(0, 0, 1), Version::new(999, 0, 0));
        let notes = between(Some(from), to);
        assert!(notes.windows(2).all(|w| !w[1].version.is_newer_than(&w[0].version)));
        assert!(notes.iter().all(|n| n.version.is_newer_than(&from) && !n.version.is_newer_than(&to)));
        // At most one note: the newest up to the version.
        assert!(latest(to).len() <= 1);
        assert!(between(None, Version::new(0, 0, 0)).is_empty());
    }

    #[test]
    fn links_go_to_the_file_at_the_release_tag() {
        let note = ReleaseNote { version: Version::new(1, 8, 7), markdown: "" };
        assert_eq!(
            note.link_target("../../CHANGELOG.md#v187"),
            "https://github.com/Albadit/DnnManager.NET/blob/v1.8.7/CHANGELOG.md#v187"
        );
        assert_eq!(note.link_target("#top"), "https://github.com/Albadit/DnnManager.NET/releases/tag/v1.8.7");
        assert_eq!(note.link_target("https://example.com/a"), "https://example.com/a");
        assert_eq!(
            note.link_target("v1.8.6.md"),
            "https://github.com/Albadit/DnnManager.NET/blob/v1.8.7/.docs/release-notes/v1.8.6.md"
        );
        let old = ReleaseNote { version: Version::new(1, 7, 6), markdown: "" };
        assert_eq!(
            old.link_target("../user-guide.md"),
            "https://github.com/Albadit/DnnManager.NET/blob/v1.7.6/docs/user-guide.md"
        );
    }
}
