//! A file's icon by its kind, as VS Code's file icon theme draws one: a short coloured tag for code (C#, JS, {}, <>,
//! #, @), and a line icon in the kind's colour for the rest (pictures, archives, databases, settings, text). A file of
//! no known kind gets the plain page in the muted text colour.
//!
//! The colours are the file icon theme's (Seti's, as VS Code ships it), a little darker in the light theme so they
//! keep their contrast on white.

use gpui::{App, Div, FontWeight, Hsla, div, prelude::*, rgb};

use crate::icons::{Icon, IconSize};
use crate::px;
use crate::theme::Theme;

/// What stands for the file: a tag of a letter or two, or a line icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Tag(&'static str),
    Icon(Icon),
}

/// The colour of a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Blue,
    Green,
    Yellow,
    Orange,
    Red,
    Purple,
    Pink,
    /// The muted text colour: a kind with no colour of its own.
    Muted,
}

impl Tint {
    fn color(self, dark: bool, cx: &App) -> Hsla {
        let (on_dark, on_light) = match self {
            Tint::Blue => (0x519aba, 0x2f7ba8),
            Tint::Green => (0x8dc149, 0x5a9216),
            Tint::Yellow => (0xcbcb41, 0x9a8f00),
            Tint::Orange => (0xe37933, 0xc25e16),
            Tint::Red => (0xcc3e44, 0xc4373d),
            Tint::Purple => (0xa074c4, 0x8a55b5),
            Tint::Pink => (0xf55385, 0xd1347a),
            Tint::Muted => return Theme::get(cx).colors.text_muted,
        };
        rgb(if dark { on_dark } else { on_light }).into()
    }
}

/// A file's glyph and colour by its name (its extension, or the whole name for a few).
pub fn kind(name: &str) -> (Glyph, Tint) {
    use Glyph::{Icon as I, Tag as T};
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "cs" => (T("C#"), Tint::Purple),
        "vb" => (T("VB"), Tint::Blue),
        "cshtml" | "vbhtml" | "razor" => (T("@"), Tint::Blue),
        "js" | "mjs" | "cjs" | "jsx" => (T("JS"), Tint::Yellow),
        "ts" | "tsx" | "mts" => (T("TS"), Tint::Blue),
        "json" | "jsonc" => (T("{}"), Tint::Yellow),
        "css" => (T("#"), Tint::Blue),
        "scss" | "sass" | "less" => (T("#"), Tint::Pink),
        "html" | "htm" => (T("<>"), Tint::Orange),
        "aspx" | "ascx" | "master" | "asmx" | "ashx" | "asax" => (T("<>"), Tint::Purple),
        "xml" | "xsl" | "xslt" | "dnn" | "xsd" | "csproj" | "vbproj" | "sln" | "targets" | "props" => (T("<>"), Tint::Green),
        "resx" | "resources" => (I(Icon::Globe), Tint::Blue),
        "config" | "ini" | "yml" | "yaml" | "toml" | "env" | "gitignore" | "gitattributes" | "editorconfig" => {
            (I(Icon::Settings), Tint::Muted)
        }
        "sql" | "sqldataprovider" | "mdf" | "ldf" | "db" | "sqlite" | "bak" => (I(Icon::Database), Tint::Pink),
        "svg" => (I(Icon::Image), Tint::Orange),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "avif" | "tif" | "tiff" => (I(Icon::Image), Tint::Purple),
        "zip" | "rar" | "7z" | "gz" | "tar" | "tgz" | "nupkg" => (I(Icon::Archive), Tint::Yellow),
        "dll" | "exe" | "pdb" => (I(Icon::Container), Tint::Blue),
        "woff" | "woff2" | "ttf" | "otf" | "eot" => (T("Aa"), Tint::Red),
        "ps1" | "psm1" | "sh" | "bat" | "cmd" => (I(Icon::Terminal), Tint::Blue),
        "md" | "markdown" => (I(Icon::FileText), Tint::Blue),
        "pdf" => (I(Icon::FileText), Tint::Red),
        "csv" => (I(Icon::FileText), Tint::Green),
        "txt" | "log" | "lic" | "license" => (I(Icon::FileText), Tint::Muted),
        _ => (I(Icon::File), Tint::Muted),
    }
}

/// The file's icon, in the box of a [`crate::icon`] so it lines up with the folders' and the text.
pub fn file_icon(name: &str, cx: &App) -> Div {
    let theme = Theme::get(cx);
    let (glyph, tint) = kind(name);
    let color = tint.color(theme.dark, cx);
    match glyph {
        Glyph::Icon(icon) => crate::icon(icon, color, cx),
        // Two letters fit the 15 px box at 9 px, bold so they stay legible that small.
        Glyph::Tag(tag) => div()
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .size(px(IconSize::Medium.pixels() + 2.0))
            .text_size(px(9.0))
            .font_weight(FontWeight::BOLD)
            .text_color(color)
            .child(tag),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_gets_its_kind_by_extension_in_any_case() {
        assert_eq!(kind("View.ascx.cs"), (Glyph::Tag("C#"), Tint::Purple));
        assert_eq!(kind("View.ASCX"), (Glyph::Tag("<>"), Tint::Purple));
        assert_eq!(kind("skin.css"), (Glyph::Tag("#"), Tint::Blue));
        assert_eq!(kind("web.config"), (Glyph::Icon(Icon::Settings), Tint::Muted));
        assert_eq!(kind("logo.png"), (Glyph::Icon(Icon::Image), Tint::Purple));
        assert_eq!(kind("SharedResources.resx"), (Glyph::Icon(Icon::Globe), Tint::Blue));
    }

    #[test]
    fn an_unknown_or_bare_name_is_a_plain_page() {
        assert_eq!(kind("LICENSE_NOT"), (Glyph::Icon(Icon::File), Tint::Muted));
        assert_eq!(kind("file.unknownext"), (Glyph::Icon(Icon::File), Tint::Muted));
        assert_eq!(kind(""), (Glyph::Icon(Icon::File), Tint::Muted));
    }
}
