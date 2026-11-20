//! The layout icons and the spinner - path data served to GPUI as SVG files: the app's asset
//! source answers `asset(path)` for every path under `shape/`, and `gpui::svg().path(..)` draws one in the element's
//! text colour (GPUI uses an SVG as a mask, so the colours here don't matter).

use gpui::SharedString;

/// The layout's frame: a window, its corners rounded by 1.
const FRAME: &str = "M2.5,2.5 H13.5 A1,1 0 0 1 14.5,3.5 V12.5 A1,1 0 0 1 13.5,13.5 H2.5 A1,1 0 0 1 1.5,12.5 V3.5 A1,1 0 0 1 2.5,2.5 Z";

/// A layout icon (16 x 16, stroked 1 px): its lines, and the part filled in while it is shown or chosen.
struct Layout {
    name: &'static str,
    stroke: &'static str,
    fill: &'static str,
}

const PANEL_FILL: &str = "M1.5,9.5 H14.5 V12.5 A1,1 0 0 1 13.5,13.5 H2.5 A1,1 0 0 1 1.5,12.5 Z";

const LAYOUTS: &[Layout] = &[
    // The menu bar (VS Code's `layout-menubar`): a strip along the top, its entries drawn in while it shows.
    Layout { name: "menu-bar", stroke: " M1.5,5.5 H14.5", fill: "M3.5,3.5 H5.5 V4.5 H3.5 Z M6.5,3.5 H8.5 V4.5 H6.5 Z M9.5,3.5 H11.5 V4.5 H9.5 Z" },
    // The activity bar (VS Code's `layout-activitybar-left` / `-right`): a narrow column along a side.
    Layout { name: "activity-bar-left", stroke: " M4.5,2.5 V13.5", fill: "M4.5,2.5 V13.5 H2.5 A1,1 0 0 1 1.5,12.5 V3.5 A1,1 0 0 1 2.5,2.5 Z" },
    Layout { name: "activity-bar-right", stroke: " M11.5,2.5 V13.5", fill: "M11.5,2.5 H13.5 A1,1 0 0 1 14.5,3.5 V12.5 A1,1 0 0 1 13.5,13.5 H11.5 Z" },
    Layout { name: "sidebar-left", stroke: " M6,2.5 V13.5", fill: "M6,2.5 V13.5 H2.5 A1,1 0 0 1 1.5,12.5 V3.5 A1,1 0 0 1 2.5,2.5 Z" },
    Layout { name: "sidebar-right", stroke: " M10,2.5 V13.5", fill: "M10,2.5 H13.5 A1,1 0 0 1 14.5,3.5 V12.5 A1,1 0 0 1 13.5,13.5 H10 Z" },
    Layout { name: "panel", stroke: " M1.5,9.5 H14.5", fill: PANEL_FILL },
    Layout { name: "status-bar", stroke: " M1.5,11.5 H14.5", fill: "M1.5,11.5 H14.5 V12.5 A1,1 0 0 1 13.5,13.5 H2.5 A1,1 0 0 1 1.5,12.5 Z" },
    Layout { name: "panel-left", stroke: " M1.5,9.5 H10 M10,2.5 V13.5", fill: "M1.5,9.5 H10 V13.5 H2.5 A1,1 0 0 1 1.5,12.5 Z" },
    Layout { name: "panel-right", stroke: " M6,2.5 V13.5 M6,9.5 H14.5", fill: "M6,9.5 H14.5 V12.5 A1,1 0 0 1 13.5,13.5 H6 Z" },
    Layout { name: "panel-center", stroke: " M5,2.5 V13.5 M11,2.5 V13.5 M5,9.5 H11", fill: "M5,9.5 H11 V13.5 H5 Z" },
    Layout { name: "panel-justify", stroke: " M1.5,9.5 H14.5 M5,2.5 V9.5 M11,2.5 V9.5", fill: PANEL_FILL },
    // Where the quick input opens: the bar is always filled in - it is the picture.
    Layout { name: "quick-input-top", stroke: "", fill: "M4.5,4 H11.5 A0.5,0.5 0 0 1 12,4.5 V5.5 A0.5,0.5 0 0 1 11.5,6 H4.5 A0.5,0.5 0 0 1 4,5.5 V4.5 A0.5,0.5 0 0 1 4.5,4 Z" },
    Layout { name: "quick-input-center", stroke: "", fill: "M4.5,7 H11.5 A0.5,0.5 0 0 1 12,7.5 V8.5 A0.5,0.5 0 0 1 11.5,9 H4.5 A0.5,0.5 0 0 1 4,8.5 V7.5 A0.5,0.5 0 0 1 4.5,7 Z" },
    Layout { name: "compact", stroke: " M6,2.5 V13.5 M6,9.5 H14.5", fill: "" },
];

/// The default density: the sidebar, the editor and the panel apart - three rounded boxes, no frame.
const LAYOUT_DEFAULT: &str = "M2.5,2.5 H4.5 A1,1 0 0 1 5.5,3.5 V12.5 A1,1 0 0 1 4.5,13.5 H2.5 A1,1 0 0 1 1.5,12.5 V3.5 A1,1 0 0 1 2.5,2.5 Z M8.5,2.5 H13.5 A1,1 0 0 1 14.5,3.5 V7.5 A1,1 0 0 1 13.5,8.5 H8.5 A1,1 0 0 1 7.5,7.5 V3.5 A1,1 0 0 1 8.5,2.5 Z M8.5,10.5 H13.5 A1,1 0 0 1 14.5,11.5 V12.5 A1,1 0 0 1 13.5,13.5 H8.5 A1,1 0 0 1 7.5,12.5 V11.5 A1,1 0 0 1 8.5,10.5 Z";

/// The Output's spinner (14 x 14, stroked 1 px as every icon): three quarters of a ring, turned while something runs.
const SPINNER: &str = "M7,2 A5,5 0 1 1 2,7";

/// A layout icon's path: `name` one of the C# app's (`menu-bar`, `sidebar-left`, `panel-center`, `default`…), filled
/// in while what it shows is on or chosen.
pub fn layout(name: &str, filled: bool) -> SharedString {
    format!("shape/layout-{name}{}.svg", if filled { "-filled" } else { "" }).into()
}

pub const SPINNER_PATH: &str = "shape/spinner.svg";

/// An SVG of `view` x `view` grid units drawn `size` px: the fill, then the stroke `stroke_width` units wide, round
/// caps and joins. The one builder of every drawn icon ([`crate::icons`] too).
pub fn svg_document(view: f32, size: f32, stroke_width: f32, stroke: &str, fill: &str) -> String {
    let mut body = String::new();
    if !fill.is_empty() {
        body.push_str(&format!(r#"<path fill="black" d="{fill}"/>"#));
    }
    if !stroke.is_empty() {
        body.push_str(&format!(
            r#"<path fill="none" stroke="black" stroke-width="{stroke_width}" stroke-linecap="round" stroke-linejoin="round" d="{stroke}"/>"#
        ));
    }
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" width="{size}" height="{size}">{body}</svg>"#)
}

fn svg(view: f32, stroke: &str, stroke_width: f32, fill: &str) -> String {
    svg_document(view, view, stroke_width, stroke, fill)
}

/// The SVG for a path under `shape/` - None for any other path.
pub fn asset(path: &str) -> Option<String> {
    let name = path.strip_prefix("shape/")?.strip_suffix(".svg")?;
    if name == "spinner" {
        return Some(svg(14.0, SPINNER, 1.0, ""));
    }
    let layout = name.strip_prefix("layout-")?;
    let (layout, filled) = match layout.strip_suffix("-filled") {
        Some(base) => (base, true),
        None => (layout, false),
    };
    if layout == "default" {
        return Some(svg(16.0, LAYOUT_DEFAULT, 1.0, ""));
    }
    let shape = LAYOUTS.iter().find(|l| l.name == layout)?;
    let stroke = format!("{FRAME}{}", shape.stroke);
    let always_filled = shape.stroke.is_empty();
    let fill = if filled || always_filled { shape.fill } else { "" };
    Some(svg(16.0, &stroke, 1.0, fill))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layout_icon_and_shape_has_its_svg() {
        for l in LAYOUTS {
            for filled in [false, true] {
                let path = layout(l.name, filled);
                let svg = asset(&path).unwrap_or_else(|| panic!("{path}"));
                assert!(svg.starts_with("<svg") && svg.contains(FRAME), "{path}");
            }
        }
        assert!(asset(&layout("default", false)).is_some());
        assert!(asset(&layout("panel", true)).unwrap().contains(PANEL_FILL));
        assert!(!asset(&layout("panel", false)).unwrap().contains(PANEL_FILL));
        assert!(asset(SPINNER_PATH).is_some());
        assert_eq!(asset("flame/outline.svg"), None);
        assert_eq!(asset(&layout("nothing", false)), None);
    }
}
