//! The design tokens of `Themes/LightTheme.xaml`, `Themes/DarkTheme.xaml` and `Themes/Tokens.xaml`, one for one -
//! VS Code's Light Modern and Dark Modern. Every view takes its colours and sizes from here, never inline.

use crate::px;
use gpui::{App, Global, Hsla, Pixels, SharedString, WindowAppearance, rgb, rgba};

use dnnm_application::settings::{Density, ThemeChoice};

/// A colour written as XAML writes it: `#RRGGBB`.
fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// `#AARRGGBB`, as XAML writes a colour with alpha.
fn ca(argb: u32) -> Hsla {
    let alpha = argb >> 24;
    rgba(((argb & 0x00FF_FFFF) << 8) | alpha).into()
}

/// The colours, named after their XAML keys.
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub window_bg: Hsla,
    pub card_bg: Hsla,
    pub card_border: Hsla,
    pub input_bg: Hsla,
    pub input_border: Hsla,
    pub field_border: Hsla,
    pub hover_bg: Hsla,
    pub selection_bg: Hsla,
    pub header_bg: Hsla,
    pub grid_line: Hsla,
    pub scroll_thumb: Hsla,
    pub scroll_thumb_hover: Hsla,
    pub text_primary: Hsla,
    pub text_muted: Hsla,
    pub error_text: Hsla,
    pub success_text: Hsla,
    pub keep_warm_fg: Hsla,
    pub accent: Hsla,
    pub focus_border: Hsla,
    pub link_fg: Hsla,
    pub on_accent: Hsla,
    pub accent_hover: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub info_banner_bg: Hsla,
    pub info_banner_border: Hsla,
    pub warn_banner_bg: Hsla,
    pub warn_banner_border: Hsla,
    pub sidebar_bg: Hsla,
    pub sidebar_border: Hsla,
    pub sidebar_fg: Hsla,
    pub sidebar_active_fg: Hsla,
    pub sidebar_hover: Hsla,
    pub sidebar_selected: Hsla,
    pub sidebar_divider: Hsla,
    pub status_bar_bg: Hsla,
    pub modal_backdrop: Hsla,
    pub picker_group_border: Hsla,
    pub command_center_fg: Hsla,
    pub command_center_bg: Hsla,
    pub command_center_hover_bg: Hsla,
    pub command_center_border: Hsla,
    pub command_center_hover_border: Hsla,
    pub log_bg: Hsla,
    pub log_header_fg: Hsla,
    pub log_fg: Hsla,
    pub log_muted: Hsla,
    pub log_fail: Hsla,
    pub log_warn: Hsla,
    pub out_heading: Hsla,
    pub out_error: Hsla,
    pub out_pill_running_bg: Hsla,
    pub out_pill_running_fg: Hsla,
    pub out_pill_ok_bg: Hsla,
    pub out_pill_ok_fg: Hsla,
    pub out_pill_fail_bg: Hsla,
    pub out_pill_fail_fg: Hsla,
    pub out_pill_neutral_bg: Hsla,
    pub out_pill_neutral_fg: Hsla,
    /// Source Control's diff: the background of a line added here, and of one removed (VS Code's
    /// diffEditor.insertedLineBackground / removedLineBackground, the same in both themes).
    pub diff_added_bg: Hsla,
    pub diff_removed_bg: Hsla,
    /// The words that differ inside a changed line, over its line's colour (insertedTextBackground /
    /// removedTextBackground); the blank where a side has no line (diagonalFill); a change's mark on the overview
    /// ruler (diffEditorOverview).
    pub diff_added_text_bg: Hsla,
    pub diff_removed_text_bg: Hsla,
    pub diff_filler: Hsla,
    pub diff_ruler_added: Hsla,
    pub diff_ruler_removed: Hsla,
    /// Source Control's syntax colours (VS Code's Light / Dark Modern): keywords, strings, comments, numbers, markup
    /// tags, attributes and properties.
    pub syntax_keyword: Hsla,
    pub syntax_string: Hsla,
    pub syntax_comment: Hsla,
    pub syntax_number: Hsla,
    pub syntax_tag: Hsla,
    pub syntax_attribute: Hsla,
    /// The close button's hover in the title bar - Windows' own red, the same in both themes.
    pub close_hover: Hsla,
}

impl Colors {
    /// `LightTheme.xaml` - VS Code's Light Modern.
    pub fn light() -> Self {
        Self {
            window_bg: c(0xFFFFFF),
            card_bg: c(0xF8F8F8),
            card_border: c(0xE5E5E5),
            input_bg: c(0xFFFFFF),
            input_border: c(0x8A8A8A),
            field_border: c(0xCECECE),
            hover_bg: c(0xF2F2F2),
            selection_bg: c(0xE8E8E8),
            header_bg: c(0xF8F8F8),
            grid_line: c(0xE5E5E5),
            scroll_thumb: ca(0x66646464),
            scroll_thumb_hover: ca(0xB3646464),
            text_primary: c(0x3B3B3B),
            text_muted: c(0x616161),
            error_text: c(0xB01011),
            success_text: c(0x236B26),
            keep_warm_fg: c(0xD9480F),
            accent: c(0x005FB8),
            focus_border: c(0x005FB8),
            link_fg: c(0x005FB8),
            on_accent: c(0xFFFFFF),
            accent_hover: c(0x0258A8),
            danger: c(0xC72E0F),
            danger_hover: c(0xA8270D),
            info_banner_bg: c(0xD6ECF2),
            info_banner_border: c(0x007ACC),
            warn_banner_bg: c(0xF6F5D2),
            warn_banner_border: c(0xB89500),
            sidebar_bg: c(0xF8F8F8),
            sidebar_border: c(0xE5E5E5),
            sidebar_fg: c(0x3B3B3B),
            sidebar_active_fg: c(0x000000),
            sidebar_hover: c(0xF2F2F2),
            sidebar_selected: c(0xE8E8E8),
            sidebar_divider: c(0xE5E5E5),
            status_bar_bg: c(0xF8F8F8),
            modal_backdrop: ca(0x33000000),
            picker_group_border: c(0xE5E5E5),
            command_center_fg: c(0x1E1E1E),
            command_center_bg: ca(0x0D000000),
            command_center_hover_bg: ca(0x14000000),
            command_center_border: ca(0x331E1E1E),
            command_center_hover_border: ca(0x4D1E1E1E),
            log_bg: c(0xF8F8F8),
            log_header_fg: c(0x000000),
            log_fg: c(0x3B3B3B),
            log_muted: c(0x616161),
            log_fail: c(0xCD3131),
            log_warn: c(0x855F00),
            out_heading: c(0x1F1F1F),
            out_error: c(0xCD3131),
            out_pill_running_bg: c(0xD6ECF2),
            out_pill_running_fg: c(0x0063D3),
            out_pill_ok_bg: ca(0x26369432),
            out_pill_ok_fg: c(0x236B26),
            out_pill_fail_bg: c(0xF2DEDE),
            out_pill_fail_fg: c(0xB01011),
            out_pill_neutral_bg: c(0xE5E5E5),
            out_pill_neutral_fg: c(0x3B3B3B),
            diff_added_bg: ca(0x339BB955),
            diff_removed_bg: ca(0x33FF0000),
            diff_added_text_bg: ca(0x409CCC2C),
            diff_removed_text_bg: ca(0x33FF0000),
            diff_filler: ca(0x1A222222),
            diff_ruler_added: ca(0xCC6FA21D),
            diff_ruler_removed: ca(0xCCE51400),
            syntax_keyword: c(0x0000FF),
            syntax_string: c(0xA31515),
            syntax_comment: c(0x008000),
            syntax_number: c(0x098658),
            syntax_tag: c(0x800000),
            syntax_attribute: c(0xE50000),
            close_hover: c(0xC42B1C),
        }
    }

    /// `DarkTheme.xaml` - VS Code's Dark Modern.
    pub fn dark() -> Self {
        Self {
            window_bg: c(0x1F1F1F),
            card_bg: c(0x181818),
            card_border: c(0x2B2B2B),
            input_bg: c(0x313131),
            input_border: c(0x6B6B6B),
            field_border: c(0x3C3C3C),
            hover_bg: c(0x2A2D2E),
            selection_bg: c(0x04395E),
            header_bg: c(0x181818),
            grid_line: c(0x2B2B2B),
            scroll_thumb: ca(0x66797979),
            scroll_thumb_hover: ca(0xB3646464),
            text_primary: c(0xCCCCCC),
            text_muted: c(0xA3A3A3),
            error_text: c(0xFF6B61),
            success_text: c(0x73C991),
            keep_warm_fg: c(0xFF8A3D),
            accent: c(0x0078D4),
            focus_border: c(0x3794FF),
            link_fg: c(0x4DAAFC),
            on_accent: c(0xFFFFFF),
            accent_hover: c(0x026EC1),
            danger: c(0xC72E0F),
            danger_hover: c(0xA8270D),
            info_banner_bg: c(0x063B49),
            info_banner_border: c(0x007ACC),
            warn_banner_bg: c(0x352A05),
            warn_banner_border: c(0xB89500),
            sidebar_bg: c(0x181818),
            sidebar_border: c(0x2B2B2B),
            sidebar_fg: c(0xCCCCCC),
            sidebar_active_fg: c(0xFFFFFF),
            sidebar_hover: c(0x2A2D2E),
            sidebar_selected: c(0x37373D),
            sidebar_divider: c(0x2B2B2B),
            status_bar_bg: c(0x181818),
            modal_backdrop: ca(0x66000000),
            picker_group_border: c(0x3C3C3C),
            command_center_fg: c(0xCCCCCC),
            command_center_bg: ca(0x0DFFFFFF),
            command_center_hover_bg: ca(0x14FFFFFF),
            command_center_border: ca(0x33CCCCCC),
            command_center_hover_border: ca(0x4DCCCCCC),
            log_bg: c(0x181818),
            log_header_fg: c(0xFFFFFF),
            log_fg: c(0xCCCCCC),
            log_muted: c(0x9D9D9D),
            log_fail: c(0xF14C4C),
            log_warn: c(0xCCA700),
            out_heading: c(0xFFFFFF),
            out_error: c(0xF14C4C),
            out_pill_running_bg: c(0x063B49),
            out_pill_running_fg: c(0x59A4F9),
            out_pill_ok_bg: ca(0x2673C991),
            out_pill_ok_fg: c(0x73C991),
            out_pill_fail_bg: c(0x5A1D1D),
            out_pill_fail_fg: c(0xF88070),
            out_pill_neutral_bg: c(0x313131),
            out_pill_neutral_fg: c(0xCCCCCC),
            diff_added_bg: ca(0x339BB955),
            diff_removed_bg: ca(0x33FF0000),
            diff_added_text_bg: ca(0x339CCC2C),
            diff_removed_text_bg: ca(0x33FF0000),
            diff_filler: ca(0x1ACCCCCC),
            diff_ruler_added: ca(0xCC9CCC2C),
            diff_ruler_removed: ca(0xCCF14C4C),
            syntax_keyword: c(0x569CD6),
            syntax_string: c(0xCE9178),
            syntax_comment: c(0x6A9955),
            syntax_number: c(0xB5CEA8),
            syntax_tag: c(0x569CD6),
            syntax_attribute: c(0x9CDCFE),
            close_hover: c(0xC42B1C),
        }
    }
}

/// Text sizes by role, for the default body size 13 (`Tokens.xaml`); scaled by Settings → General's font size,
/// keeping these proportions (`ThemeManager.ApplyLayout`).
#[derive(Debug, Clone, Copy)]
pub struct TextSizes {
    pub small: Pixels,
    pub hint: Pixels,
    pub caption: Pixels,
    pub body: Pixels,
    pub medium: Pixels,
    pub heading: Pixels,
    pub large: Pixels,
    pub title: Pixels,
    pub display: Pixels,
}

impl TextSizes {
    fn for_body(body: f32) -> Self {
        let k = body / 13.0;
        Self {
            small: px(11.0 * k),
            hint: px(11.5 * k),
            caption: px(12.0 * k),
            body: px(13.0 * k),
            medium: px(14.0 * k),
            heading: px(15.0 * k),
            large: px(16.0 * k),
            title: px(20.0 * k),
            display: px(22.0 * k),
        }
    }
}

/// The frame's sizes; the Compact density makes the sidebar entries and buttons smaller.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub control_radius: Pixels,
    pub small_radius: Pixels,
    pub card_radius: Pixels,
    pub control_height: Pixels,
    pub search_height: Pixels,
    pub title_bar_height: Pixels,
    pub caption_button_width: Pixels,
    pub layout_button_width: Pixels,
    pub layout_button_height: Pixels,
    pub status_bar_height: Pixels,
    pub sidebar_width: Pixels,
    pub sidebar_compact_width: Pixels,
    /// Gap around the cards in the Default density; none in Compact, where lines divide them.
    pub card_gap: Pixels,
    pub panel_height: Pixels,
    pub table_row_height: Pixels,
}

impl Metrics {
    fn for_density(density: Density) -> Self {
        let compact = density == Density::Compact;
        Self {
            control_radius: px(4.0),
            small_radius: px(3.0),
            card_radius: px(6.0),
            control_height: px(30.0),
            search_height: px(26.0),
            title_bar_height: px(36.0),
            caption_button_width: px(46.0),
            layout_button_width: px(28.0),
            layout_button_height: px(26.0),
            status_bar_height: px(28.0),
            sidebar_width: px(if compact { 190.0 } else { 230.0 }),
            // A 32 px entry with as much room beside it as above it: 8 px (Default, 1 px border each side), 4 px (Compact,
            // its one edge line).
            sidebar_compact_width: px(if compact { 41.0 } else { 50.0 }),
            card_gap: px(if compact { 0.0 } else { 5.0 }),
            panel_height: px(230.0),
            table_row_height: px(38.0),
        }
    }
}

/// The theme in use: colours, text sizes and metrics. A GPUI global, so every view reads the same one.
#[derive(Debug, Clone)]
pub struct Theme {
    pub dark: bool,
    pub colors: Colors,
    pub text: TextSizes,
    pub metrics: Metrics,
    pub density: Density,
    /// The UI font (WPF's default, Segoe UI).
    pub font: SharedString,
    pub mono_font: SharedString,
    /// Slides, fades and pulses play: the setting "Play animations", and Windows' animation effects on.
    pub animations: bool,
}

impl Global for Theme {}

impl Theme {
    pub fn new(choice: ThemeChoice, appearance: WindowAppearance, font_size: u32, density: Density) -> Self {
        let dark = match choice {
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
            ThemeChoice::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        Self {
            dark,
            colors: if dark { Colors::dark() } else { Colors::light() },
            text: TextSizes::for_body(font_size as f32),
            metrics: Metrics::for_density(density),
            density,
            font: "Segoe UI".into(),
            mono_font: "Cascadia Mono".into(),
            animations: true,
        }
    }

    /// The same theme, with animations on or off.
    pub fn with_animations(mut self, on: bool) -> Self {
        self.animations = on;
        self
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Theme>()
    }
}
