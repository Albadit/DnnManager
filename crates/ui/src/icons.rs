//! The icons of `Themes/Icons.xaml`: Segoe Fluent Icons glyphs by their code points, and the layout icons as the
//! same 16 × 16 path data, drawn with GPUI's path builder.

use crate::px;
use gpui::{Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, canvas, point};

// --- Sizes: one per role, so icons doing the same job look the same everywhere ---

/// Chevrons, check marks, sort arrows, the window's buttons, the Output pill.
pub const SIZE_SMALL: f32 = 10.0;
/// Every action and state icon - alone (icon buttons, 28 x 28, as the C# app's `IconButton`) or beside text.
pub const SIZE: f32 = 13.0;
/// A ✕ that closes something (the panel, a bar, a list): Segoe's close glyph is drawn larger than the others, so a
/// size smaller (the C# app's 11).
pub const SIZE_CLOSE: f32 = 11.0;
/// The page entries: the sidebar and Settings' categories.
pub const SIZE_NAV: f32 = 16.0;
/// A dialog's own large icon (the prompt).
pub const SIZE_LARGE: f32 = 26.0;
/// Between an icon and its label.
pub const LABEL_GAP: f32 = 8.0;

pub const MINIMIZE: &str = "\u{E921}";
pub const MAXIMIZE: &str = "\u{E922}";
pub const RESTORE: &str = "\u{E923}";
pub const CLOSE: &str = "\u{E8BB}";
pub const SETTINGS: &str = "\u{E713}";
pub const LIST: &str = "\u{E8FD}";
pub const FOLDER: &str = "\u{E8B7}";
pub const ADD: &str = "\u{E710}";
pub const PLAY: &str = "\u{E768}";
pub const STOP: &str = "\u{E71A}";
pub const REFRESH: &str = "\u{E72C}";
pub const DELETE: &str = "\u{E74D}";
pub const SEARCH: &str = "\u{E721}";
pub const CHEVRON_UP: &str = "\u{E70E}";
pub const CHEVRON_DOWN: &str = "\u{E70D}";
pub const CHEVRON_RIGHT: &str = "\u{E76C}";
pub const CHECK: &str = "\u{E73E}";
pub const INFO: &str = "\u{E946}";
pub const QUESTION: &str = "\u{E9CE}";
pub const WARNING: &str = "\u{E7BA}";
pub const ERROR: &str = "\u{EA39}";
pub const GLOBE: &str = "\u{E774}";
pub const BACK: &str = "\u{E72B}";

/// The layout icons: sidebar, panel - each a stroke, and a fill while the part it shows is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutIcon {
    /// Customize Layout (`LayoutDefault`): the frame with a sidebar and a panel.
    Customize,
    /// The sidebar on the left (`LayoutSidebarLeft`).
    SidebarLeft,
    /// The sidebar on the right (`LayoutSidebarRight`).
    SidebarRight,
    /// The bottom panel (`LayoutPanel`).
    Panel,
}

/// The rounded frame every layout icon starts with: `M2.5,2.5 H13.5 A1,1 … Z`.
fn frame(b: &mut PathBuilder, o: Point<Pixels>) {
    let p = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
    let r = point(px(1.0), px(1.0));
    b.move_to(p(2.5, 2.5));
    b.line_to(p(13.5, 2.5));
    b.arc_to(r, px(0.0), false, true, p(14.5, 3.5));
    b.line_to(p(14.5, 12.5));
    b.arc_to(r, px(0.0), false, true, p(13.5, 13.5));
    b.line_to(p(2.5, 13.5));
    b.arc_to(r, px(0.0), false, true, p(1.5, 12.5));
    b.line_to(p(1.5, 3.5));
    b.arc_to(r, px(0.0), false, true, p(2.5, 2.5));
    b.close();
}

/// The Columns button's icon (`Columns`): a rounded frame split in three, stroked 1.5 px.
pub fn columns_icon(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let o = bounds.origin;
            let p = |x: f32, y: f32| point(o.x + px(x), o.y + px(y + 1.0));
            let r = point(px(1.5), px(1.5));
            let mut b = PathBuilder::stroke(px(1.5));
            b.move_to(p(2.25, 0.75));
            b.line_to(p(13.75, 0.75));
            b.arc_to(r, px(0.0), false, true, p(15.25, 2.25));
            b.line_to(p(15.25, 11.75));
            b.arc_to(r, px(0.0), false, true, p(13.75, 13.25));
            b.line_to(p(2.25, 13.25));
            b.arc_to(r, px(0.0), false, true, p(0.75, 11.75));
            b.line_to(p(0.75, 2.25));
            b.arc_to(r, px(0.0), false, true, p(2.25, 0.75));
            b.close();
            b.move_to(p(5.5, 0.75));
            b.line_to(p(5.5, 13.25));
            b.move_to(p(10.5, 0.75));
            b.line_to(p(10.5, 13.25));
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(16.0))
}

/// Draws a 16 × 16 layout icon; `filled` fills the part it stands for, as VS Code does while it is shown.
pub fn layout_icon(kind: LayoutIcon, filled: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let o = bounds.origin;
            let p = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
            let mut stroke = PathBuilder::stroke(px(1.0));
            frame(&mut stroke, o);
            let mut fill = PathBuilder::fill();
            let mut has_fill = false;
            match kind {
                LayoutIcon::Customize => {
                    stroke.move_to(p(6.0, 2.5));
                    stroke.line_to(p(6.0, 13.5));
                    stroke.move_to(p(6.0, 9.5));
                    stroke.line_to(p(14.5, 9.5));
                }
                LayoutIcon::SidebarLeft => {
                    stroke.move_to(p(6.0, 2.5));
                    stroke.line_to(p(6.0, 13.5));
                    if filled {
                        has_fill = true;
                        fill.move_to(p(6.0, 2.5));
                        fill.line_to(p(6.0, 13.5));
                        fill.line_to(p(2.5, 13.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(1.5, 12.5));
                        fill.line_to(p(1.5, 3.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(2.5, 2.5));
                        fill.close();
                    }
                }
                LayoutIcon::SidebarRight => {
                    stroke.move_to(p(10.0, 2.5));
                    stroke.line_to(p(10.0, 13.5));
                    if filled {
                        has_fill = true;
                        fill.move_to(p(10.0, 2.5));
                        fill.line_to(p(13.5, 2.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(14.5, 3.5));
                        fill.line_to(p(14.5, 12.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(13.5, 13.5));
                        fill.line_to(p(10.0, 13.5));
                        fill.close();
                    }
                }
                LayoutIcon::Panel => {
                    stroke.move_to(p(1.5, 9.5));
                    stroke.line_to(p(14.5, 9.5));
                    if filled {
                        has_fill = true;
                        fill.move_to(p(1.5, 9.5));
                        fill.line_to(p(14.5, 9.5));
                        fill.line_to(p(14.5, 12.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(13.5, 13.5));
                        fill.line_to(p(2.5, 13.5));
                        fill.arc_to(point(px(1.0), px(1.0)), px(0.0), false, true, p(1.5, 12.5));
                        fill.close();
                    }
                }
            }
            if has_fill && let Ok(path) = fill.build() {
                window.paint_path(path, color);
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(16.0))
}
