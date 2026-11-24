//! Every icon the window draws, in one place: [`Icon`] names one, its path data lives here on a 16 x 16 grid, and
//! [`crate::icon`] / [`crate::icon_button`] draw it. Change an icon's path here and it changes everywhere.
//!
//! All of them are line drawings stroked **1 px at the size they're drawn** - the SVG's stroke width is 16 / size in
//! grid units, so a 13 px icon and a 16 px one have lines just as thick. The app's asset source serves them as SVGs
//! under `icon/` ([`asset`]); GPUI draws an SVG as a mask in the element's text colour. The few solid parts (the dot
//! of ⓘ, the three of …) are fills, so they don't vanish at 1 px.

use std::f32::consts::PI;

use gpui::{IntoElement, SharedString, Styled};

use crate::{px, ui_scale};
use crate::shapes;

// --- Sizes: one per role, so icons doing the same job look the same everywhere ---

/// How big an icon is drawn, by what it is for - never a number at the call. [`crate::icon`] draws `Medium`;
/// [`crate::icon_sized`] takes another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IconSize {
    /// Chevrons, check marks, sort arrows, the window's buttons, the Output pill.
    Small,
    /// Every action and state icon - alone (icon buttons, 28 x 28) or beside text.
    #[default]
    Medium,
    /// The page entries: the sidebar and Settings' categories.
    Nav,
    /// A dialog's own large icon (the prompt).
    Large,
}

impl IconSize {
    /// Its size in pixels, before the UI scale.
    pub const fn pixels(self) -> f32 {
        match self {
            IconSize::Small => SIZE_SMALL,
            IconSize::Medium => SIZE,
            IconSize::Nav => SIZE_NAV,
            IconSize::Large => SIZE_LARGE,
        }
    }
}

/// [`IconSize::Small`] in pixels - for laying out around an icon, not for drawing one.
pub const SIZE_SMALL: f32 = 10.0;
/// [`IconSize::Medium`] in pixels.
pub const SIZE: f32 = 13.0;
/// [`IconSize::Nav`] in pixels.
pub const SIZE_NAV: f32 = 16.0;
/// [`IconSize::Large`] in pixels.
pub const SIZE_LARGE: f32 = 26.0;
/// Between an icon and its label.
pub const LABEL_GAP: f32 = 8.0;

/// An icon of the app. Draw one with [`crate::icon`]; its look is [`Icon::shape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    // The window's caption buttons.
    Minimize,
    Maximize,
    Restore,
    /// ✕: closes a panel, a bar, a dialog, a list.
    Close,
    // Actions.
    Add,
    Back,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    /// The projects list's Columns button: a frame split in three.
    Columns,
    Copy,
    Dash,
    Delete,
    Download,
    /// Upload: an arrow up to a line.
    Upload,
    /// Source Control's page: one arrow up, one down - the files going both ways.
    SourceControl,
    Hide,
    /// The panel's maximize: four corners pointing out.
    PanelMaximize,
    /// The panel's restore: four corners pointing in.
    PanelRestore,
    Play,
    Refresh,
    /// Back to the default (an arrow turning back).
    Reset,
    Search,
    Show,
    Stop,
    // Things and places.
    Container,
    Database,
    Folder,
    FolderOpen,
    Globe,
    Keyboard,
    List,
    More,
    Settings,
    Terminal,
    // States.
    Error,
    Info,
    Question,
    Warning,
}

/// An icon's drawing on the 16 x 16 grid: lines stroked 1 px, and solid parts filled.
pub struct Shape {
    pub stroke: String,
    pub fill: String,
}

/// A circle as path data (two half arcs), for strokes and fills alike.
fn circle(cx: f32, cy: f32, r: f32) -> String {
    format!("M{},{cy} A{r},{r} 0 1 0 {},{cy} A{r},{r} 0 1 0 {},{cy} Z", cx - r, cx + r, cx - r)
}

/// Path data as commands: each letter (absolute M L H V C A Z) with its numbers.
type Commands = Vec<(char, Vec<f32>)>;

/// How many numbers each command takes.
fn arity(command: char) -> usize {
    match command {
        'M' | 'L' => 2,
        'H' | 'V' => 1,
        'C' => 6,
        'A' => 7,
        _ => 0,
    }
}

/// `d` as commands; a command's numbers repeated without the letter (`L1,2 3,4`) become two commands.
fn parse(d: &str) -> Commands {
    let mut tokens: Vec<String> = Vec::new();
    let mut number = String::new();
    for ch in d.chars() {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && number.is_empty()) {
            number.push(ch);
            continue;
        }
        if !number.is_empty() {
            tokens.push(std::mem::take(&mut number));
        }
        if ch == '-' {
            number.push(ch);
        } else if ch.is_ascii_alphabetic() {
            tokens.push(ch.to_string());
        }
    }
    if !number.is_empty() {
        tokens.push(number);
    }
    let mut commands = Commands::new();
    let mut command = 'M';
    let mut i = 0;
    while i < tokens.len() {
        if let Some(c) = tokens[i].chars().next().filter(char::is_ascii_alphabetic) {
            command = c;
            i += 1;
            if c == 'Z' {
                commands.push(('Z', Vec::new()));
                continue;
            }
        }
        let n = arity(command).max(1);
        let numbers: Vec<f32> = tokens[i..(i + n).min(tokens.len())].iter().map(|t| t.parse().unwrap_or(0.0)).collect();
        i += n;
        if numbers.len() == arity(command) {
            commands.push((command, numbers));
        }
    }
    commands
}

/// Commands back to path data.
fn write(commands: &Commands) -> String {
    let fmt = |v: f32| {
        let t = format!("{v:.3}");
        let t = t.trim_end_matches('0').trim_end_matches('.');
        if t == "-0" { "0".to_string() } else { t.to_string() }
    };
    let mut out = String::new();
    for (c, n) in commands {
        out.push(*c);
        let numbers: Vec<String> = n.iter().map(|v| fmt(*v)).collect();
        out.push_str(&match c {
            'M' | 'L' => format!("{},{}", numbers[0], numbers[1]),
            'C' => format!("{},{} {},{} {},{}", numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5]),
            'A' => format!("{},{} {} {} {} {},{}", numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5], numbers[6]),
            _ => numbers.join(" "),
        });
    }
    out
}

/// The points an arc from `from` passes through (SVG's endpoint-to-centre conversion, unrotated arcs), so its bulge
/// counts in the bounds - half a circle reaches well past its two ends.
fn arc_points(from: (f32, f32), a: &[f32]) -> Vec<(f32, f32)> {
    let (x1, y1, x2, y2) = (from.0, from.1, a[5], a[6]);
    let (mut rx, mut ry) = (a[0].abs(), a[1].abs());
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        return vec![(x2, y2)];
    }
    let (large, sweep) = (a[3] != 0.0, a[4] != 0.0);
    let (x1p, y1p) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let numerator = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let denominator = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let sign = if large != sweep { 1.0 } else { -1.0 };
    let coef = sign * (numerator / denominator).max(0.0).sqrt();
    let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
    let (cx, cy) = (cxp + (x1 + x2) / 2.0, cyp + (y1 + y2) / 2.0);
    let start = ((y1p - cyp) / ry).atan2((x1p - cxp) / rx);
    let mut delta = ((-y1p - cyp) / ry).atan2((-x1p - cxp) / rx) - start;
    if !sweep && delta > 0.0 {
        delta -= 2.0 * PI;
    } else if sweep && delta < 0.0 {
        delta += 2.0 * PI;
    }
    (0..=32).map(|t| start + delta * t as f32 / 32.0).map(|angle| (cx + rx * angle.cos(), cy + ry * angle.sin())).collect()
}

/// The box the drawing covers: its points, its arcs' bulges, and Bézier control points (a curve stays inside them -
/// the eye's are a fraction past its curve, close enough).
fn bounds(paths: &[&Commands]) -> Option<(f32, f32, f32, f32)> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let mut add = |(x, y): (f32, f32)| {
        b = Some(match b {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    for commands in paths {
        let (mut x, mut y) = (0.0_f32, 0.0_f32);
        let mut start = (0.0, 0.0);
        for (c, n) in commands.iter() {
            match c {
                'M' => {
                    (x, y) = (n[0], n[1]);
                    start = (x, y);
                    add((x, y));
                }
                'L' => {
                    (x, y) = (n[0], n[1]);
                    add((x, y));
                }
                'H' => {
                    x = n[0];
                    add((x, y));
                }
                'V' => {
                    y = n[0];
                    add((x, y));
                }
                'C' => {
                    add((n[0], n[1]));
                    add((n[2], n[3]));
                    (x, y) = (n[4], n[5]);
                    add((x, y));
                }
                'A' => {
                    arc_points((x, y), n).into_iter().for_each(&mut add);
                    (x, y) = (n[5], n[6]);
                }
                'Z' => (x, y) = start,
                _ => {}
            }
        }
    }
    b
}

/// The commands moved and scaled: every point to `(p - centre) * k + 8`, radii by `k`.
fn rescale(commands: &Commands, centre: (f32, f32), k: f32) -> Commands {
    let x = |v: f32| (v - centre.0) * k + 8.0;
    let y = |v: f32| (v - centre.1) * k + 8.0;
    commands
        .iter()
        .map(|(c, n)| {
            let n = match c {
                'M' | 'L' => vec![x(n[0]), y(n[1])],
                'H' => vec![x(n[0])],
                'V' => vec![y(n[0])],
                'C' => vec![x(n[0]), y(n[1]), x(n[2]), y(n[3]), x(n[4]), y(n[5])],
                'A' => vec![n[0] * k, n[1] * k, n[2], n[3], n[4], x(n[5]), y(n[6])],
                _ => Vec::new(),
            };
            (*c, n)
        })
        .collect()
}

/// `commands`, on the 16 grid, redrawn for a `size` px box with every point on the pixel grid. A 1 px line between two
/// pixel rows is smeared over both, half as bright: so a stroke's points go to the middle of a pixel (n + 0.5), and a
/// line along them covers exactly one row or column. Bézier control points are only scaled; an arc's radii follow its
/// snapped end points, so circles and corners stay round. `fill` is left unsnapped (a dot of 1.6 px reads better soft
/// than squared off), as is a drawing that doesn't snap ([`Icon::snaps`]).
fn fit(commands: &Commands, size: f32, fill: bool) -> String {
    let s = size / 16.0;
    let snap = |v: f32| if fill { v * s } else { (v * s - 0.5).round() + 0.5 };
    let (mut x, mut y) = (0.0_f32, 0.0_f32); // the current point, on the 16 grid
    let mut start = (0.0, 0.0);
    let mut out = Commands::new();
    for (c, n) in commands {
        let n = match c {
            'M' | 'L' => {
                (x, y) = (n[0], n[1]);
                if *c == 'M' {
                    start = (x, y);
                }
                vec![snap(x), snap(y)]
            }
            'H' => {
                x = n[0];
                vec![snap(x)]
            }
            'V' => {
                y = n[0];
                vec![snap(y)]
            }
            'C' => {
                (x, y) = (n[4], n[5]);
                vec![n[0] * s, n[1] * s, n[2] * s, n[3] * s, snap(x), snap(y)]
            }
            'A' => {
                let (from_x, from_y) = (snap(x), snap(y));
                let chord = (n[5] - x).hypot(n[6] - y);
                (x, y) = (n[5], n[6]);
                let snapped_chord = (snap(x) - from_x).hypot(snap(y) - from_y);
                // The radii in proportion to how far the snapped ends are apart, so the arc still meets both.
                let k = if snapped_chord > 0.0 && chord > 0.0 { snapped_chord / chord } else { s };
                vec![n[0] * k, n[1] * k, n[2], n[3], n[4], snap(x), snap(y)]
            }
            _ => {
                (x, y) = start;
                Vec::new()
            }
        };
        out.push((*c, n));
    }
    write(&out)
}

/// A rectangle with rounded corners as path data.
fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> String {
    format!(
        "M{},{y0} H{} A{r},{r} 0 0 1 {x1},{} V{} A{r},{r} 0 0 1 {},{y1} H{} A{r},{r} 0 0 1 {x0},{} V{} A{r},{r} 0 0 1 {},{y0} Z",
        x0 + r,
        x1 - r,
        y0 + r,
        y1 - r,
        x1 - r,
        x0 + r,
        y1 - r,
        y0 + r,
        x0 + r,
    )
}

/// The settings cog: six wide teeth around a ring, a hole in the middle - eight read as a dotted ring at 16 px.
fn cog() -> String {
    let (inner, outer) = (4.75_f32, 6.75_f32);
    let at = |r: f32, degrees: f32| {
        let a = degrees * PI / 180.0;
        format!("{:.2},{:.2}", 8.0 + r * a.sin(), 8.0 - r * a.cos())
    };
    let mut d = String::new();
    for tooth in 0..6 {
        let mid = tooth as f32 * 60.0;
        let points = [at(inner, mid - 18.0), at(outer, mid - 11.0), at(outer, mid + 11.0), at(inner, mid + 18.0)];
        for (i, p) in points.iter().enumerate() {
            d.push_str(if tooth == 0 && i == 0 { "M" } else { "L" });
            d.push_str(p);
            d.push(' ');
        }
    }
    d.push('Z');
    d.push_str(&circle(8.0, 8.0, 2.0));
    d
}

/// The eye of Show and Hide.
const EYE: &str = "M1.5,8 C3,5 5.25,3.5 8,3.5 C10.75,3.5 13,5 14.5,8 C13,11 10.75,12.5 8,12.5 C5.25,12.5 3,11 1.5,8 Z";

impl Icon {
    /// Every icon - for the asset source and the tests.
    pub const ALL: &[Icon] = &[
        Icon::Minimize,
        Icon::Maximize,
        Icon::Restore,
        Icon::Close,
        Icon::Add,
        Icon::Back,
        Icon::Check,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::ChevronUp,
        Icon::Columns,
        Icon::Copy,
        Icon::Dash,
        Icon::Delete,
        Icon::Download,
        Icon::Upload,
        Icon::SourceControl,
        Icon::Hide,
        Icon::PanelMaximize,
        Icon::PanelRestore,
        Icon::Play,
        Icon::Refresh,
        Icon::Reset,
        Icon::Search,
        Icon::Show,
        Icon::Stop,
        Icon::Container,
        Icon::Database,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::Globe,
        Icon::Keyboard,
        Icon::List,
        Icon::More,
        Icon::Settings,
        Icon::Terminal,
        Icon::Error,
        Icon::Info,
        Icon::Question,
        Icon::Warning,
    ];

    /// Its name in the asset path.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Minimize => "minimize",
            Icon::Maximize => "maximize",
            Icon::Restore => "restore",
            Icon::Close => "close",
            Icon::Add => "add",
            Icon::Back => "back",
            Icon::Check => "check",
            Icon::ChevronDown => "chevron-down",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronUp => "chevron-up",
            Icon::Columns => "columns",
            Icon::Copy => "copy",
            Icon::Dash => "dash",
            Icon::Delete => "delete",
            Icon::Download => "download",
            Icon::Upload => "upload",
            Icon::SourceControl => "source-control",
            Icon::Hide => "hide",
            Icon::PanelMaximize => "panel-maximize",
            Icon::PanelRestore => "panel-restore",
            Icon::Play => "play",
            Icon::Refresh => "refresh",
            Icon::Reset => "reset",
            Icon::Search => "search",
            Icon::Show => "show",
            Icon::Stop => "stop",
            Icon::Container => "container",
            Icon::Database => "database",
            Icon::Folder => "folder",
            Icon::FolderOpen => "folder-open",
            Icon::Globe => "globe",
            Icon::Keyboard => "keyboard",
            Icon::List => "list",
            Icon::More => "more",
            Icon::Settings => "settings",
            Icon::Terminal => "terminal",
            Icon::Error => "error",
            Icon::Info => "info",
            Icon::Question => "question",
            Icon::Warning => "warning",
        }
    }

    /// Its drawing on the 16 x 16 grid. Lines sit on the half pixels where they can, so at 16 px each covers one row
    /// of pixels; most fill 2..14, a little inside the box, as the glyphs they replace did.
    pub fn shape(self) -> Shape {
        let (stroke, fill): (String, String) = match self {
            Icon::Minimize => ("M3,8.5 H13".into(), String::new()),
            // Square corners: at the caption buttons' 10 px a rounded one is a smudge.
            Icon::Maximize => ("M3.5,3.5 H12.5 V12.5 H3.5 Z".into(), String::new()),
            Icon::Restore => ("M2.5,5.5 H10.5 V13.5 H2.5 Z M5.5,5.5 V2.5 H13.5 V10.5 H10.5".into(), String::new()),
            Icon::Close => ("M3.5,3.5 L12.5,12.5 M12.5,3.5 L3.5,12.5".into(), String::new()),
            Icon::Add => ("M8,2.5 V13.5 M2.5,8 H13.5".into(), String::new()),
            Icon::Back => ("M13.5,8 H2.5 M7,3.5 L2.5,8 L7,12.5".into(), String::new()),
            Icon::Check => ("M2.5,8.5 L6,12 L13.5,4.5".into(), String::new()),
            Icon::ChevronDown => ("M3.5,6 L8,10.5 L12.5,6".into(), String::new()),
            Icon::ChevronRight => ("M6,3.5 L10.5,8 L6,12.5".into(), String::new()),
            Icon::ChevronUp => ("M3.5,10 L8,5.5 L12.5,10".into(), String::new()),
            Icon::Columns => (format!("{} M6,2.5 V13.5 M10,2.5 V13.5", rounded_rect(1.5, 2.5, 14.5, 13.5, 1.0)), String::new()),
            Icon::Copy => (
                format!("{} M10.5,5.5 V3.5 A1,1 0 0 0 9.5,2.5 H3.5 A1,1 0 0 0 2.5,3.5 V9.5 A1,1 0 0 0 3.5,10.5 H5.5", rounded_rect(5.5, 5.5, 13.5, 13.5, 1.0)),
                String::new(),
            ),
            Icon::Dash => ("M4,8 H12".into(), String::new()),
            // Straight sides and a handle two rows above the lid: at 13 px a tapered bin anti-aliases into a grey block.
            Icon::Delete => (
                "M2.5,4.5 H13.5 M6,4.5 V2 H10 V4.5 M3.5,4.5 V12.5 A1,1 0 0 0 4.5,13.5 H11.5 A1,1 0 0 0 12.5,12.5 V4.5 \
                 M6.5,7 V11 M9.5,7 V11"
                    .into(),
                String::new(),
            ),
            Icon::Download => ("M8,2.5 V10.5 M4.5,7 L8,10.5 L11.5,7 M2.5,13.5 H13.5".into(), String::new()),
            Icon::Upload => ("M8,13.5 V5.5 M4.5,9 L8,5.5 L11.5,9 M2.5,2.5 H13.5".into(), String::new()),
            Icon::SourceControl => (
                "M5,13.5 V2.5 M2.5,5 L5,2.5 L7.5,5 M11,2.5 V13.5 M8.5,11 L11,13.5 L13.5,11".into(),
                String::new(),
            ),
            Icon::Hide => (format!("{EYE} {} M2.5,2.5 L13.5,13.5", circle(8.0, 8.0, 2.0)), String::new()),
            Icon::PanelMaximize => ("M2.5,6 V2.5 H6 M10,2.5 H13.5 V6 M13.5,10 V13.5 H10 M6,13.5 H2.5 V10".into(), String::new()),
            Icon::PanelRestore => ("M6,2.5 V6 H2.5 M10,2.5 V6 H13.5 M13.5,10 H10 V13.5 M6,13.5 V10 H2.5".into(), String::new()),
            Icon::Play => ("M4.5,2.5 V13.5 L13,8 Z".into(), String::new()),
            Icon::Refresh => ("M13.5,8 A5.5,5.5 0 1 1 11.89,4.11 M11.89,1.5 V4.11 H9.3".into(), String::new()),
            Icon::Reset => ("M2.5,8 A5.5,5.5 0 1 0 4.11,4.11 M4.11,1.5 V4.11 H6.7".into(), String::new()),
            Icon::Search => (format!("{} M10,10 L13.5,13.5", circle(6.75, 6.75, 4.25)), String::new()),
            Icon::Show => (format!("{EYE} {}", circle(8.0, 8.0, 2.0)), String::new()),
            Icon::Stop => (rounded_rect(3.5, 3.5, 12.5, 12.5, 1.0), String::new()),
            Icon::Container => ("M8,1.5 L14,4.75 V11.25 L8,14.5 L2,11.25 V4.75 Z M2,4.75 L8,8 L14,4.75 M8,8 V14.5".into(), String::new()),
            Icon::Database => (
                "M2.5,4.5 A5.5,2 0 1 0 13.5,4.5 A5.5,2 0 1 0 2.5,4.5 V11.5 A5.5,2 0 0 0 13.5,11.5 V4.5 M2.5,8 A5.5,2 0 0 0 13.5,8"
                    .into(),
                String::new(),
            ),
            Icon::Folder => (
                "M1.5,4 A1,1 0 0 1 2.5,3 H6 L7.5,4.5 H13.5 A1,1 0 0 1 14.5,5.5 V12 A1,1 0 0 1 13.5,13 H2.5 A1,1 0 0 1 1.5,12 Z".into(),
                String::new(),
            ),
            Icon::FolderOpen => (
                "M1.5,12.5 V4 A1,1 0 0 1 2.5,3 H6 L7.5,4.5 H12 A1,1 0 0 1 13,5.5 V7 M1.5,12.5 L3.6,7.6 A1,1 0 0 1 4.5,7 H14 \
                 A0.5,0.5 0 0 1 14.45,7.7 L12.4,12.4 A1,1 0 0 1 11.5,13 H2 A0.5,0.5 0 0 1 1.5,12.5 Z"
                    .into(),
                String::new(),
            ),
            Icon::Globe => (format!("{} M1.75,8 H14.25 M8,1.75 A3,6.25 0 0 1 8,14.25 A3,6.25 0 0 1 8,1.75 Z", circle(8.0, 8.0, 6.25)), String::new()),
            Icon::Keyboard => (
                format!(
                    "{} M4.5,6.5 H5 M7.25,6.5 H7.75 M10,6.5 H10.5 M4.5,9 H5 M7.25,9 H7.75 M10,9 H10.5 M5.5,11 H10.5",
                    rounded_rect(1.5, 3.5, 14.5, 13.0, 1.0)
                ),
                String::new(),
            ),
            Icon::List => ("M5.5,4.5 H13.5 M5.5,8 H13.5 M5.5,11.5 H13.5".into(), format!("{}{}{}", circle(2.75, 4.5, 1.0), circle(2.75, 8.0, 1.0), circle(2.75, 11.5, 1.0))),
            // Upright (⋮), as Windows' and VS Code's "more" stands; dots of 1.3 (about 2 px at 13 px - at 1 they were
            // faint specks).
            Icon::More => (String::new(), format!("{}{}{}", circle(8.0, 3.75, 1.3), circle(8.0, 8.0, 1.3), circle(8.0, 12.25, 1.3))),
            Icon::Settings => (cog(), String::new()),
            Icon::Terminal => (format!("{} M4.5,6 L6.5,8 L4.5,10 M8.5,10.5 H11.5", rounded_rect(1.5, 2.5, 14.5, 13.5, 1.0)), String::new()),
            Icon::Error => (format!("{} M5.75,5.75 L10.25,10.25 M10.25,5.75 L5.75,10.25", circle(8.0, 8.0, 6.0)), String::new()),
            Icon::Info => (format!("{} M8,7.5 V11.5", circle(8.0, 8.0, 6.0)), circle(8.0, 5.25, 1.0)),
            // The ? sits inside the ring with room around it: its hook 3.6 across and the dot 1.7 (it was 4.2 and 1.8,
            // which filled the ring). Much smaller than this it turns into a smudge of pixels at 13 px.
            Icon::Question => (
                format!("{} M6.22,6.64 A1.78,1.78 0 1 1 8.81,8.23 C8.26,8.51 8,8.85 8,9.36 V9.7", circle(8.0, 8.0, 6.0)),
                circle(8.0, 11.35, 0.85),
            ),
            Icon::Warning => (
                "M7.13,2.5 A1,1 0 0 1 8.87,2.5 L14.35,12 A1,1 0 0 1 13.48,13.5 H2.52 A1,1 0 0 1 1.65,12 Z M8,6 V9".into(),
                circle(8.0, 11.0, 1.0),
            ),
        };
        Shape { stroke, fill }
    }

    /// How much room its drawing gets, so no icon looks bigger than another ([`Optical`]).
    pub fn optical(self) -> Optical {
        match self {
            Icon::Error
            | Icon::Info
            | Icon::Question
            | Icon::Globe
            | Icon::Refresh
            | Icon::Reset
            | Icon::Settings
            | Icon::Search
            | Icon::Warning => Optical::Round,
            Icon::Close
            | Icon::Add
            | Icon::Back
            | Icon::Check
            | Icon::ChevronDown
            | Icon::ChevronRight
            | Icon::ChevronUp
            | Icon::Dash
            | Icon::More => Optical::Mark,
            _ => Optical::Box,
        }
    }

    /// The drawing as it is served: moved to the middle of the grid and scaled until its longer side is its
    /// [`Optical`] size - whatever size it was drawn at in [`Icon::shape`].
    fn fitted(self) -> (Commands, Commands) {
        let shape = self.shape();
        let (stroke, fill) = (parse(&shape.stroke), parse(&shape.fill));
        let Some((x0, y0, x1, y1)) = bounds(&[&stroke, &fill]) else {
            return (stroke, fill);
        };
        let longest = (x1 - x0).max(y1 - y0).max(f32::EPSILON);
        let (centre, k) = (((x0 + x1) / 2.0, (y0 + y1) / 2.0), self.optical().extent() / longest);
        (rescale(&stroke, centre, k), rescale(&fill, centre, k))
    }

    /// Whether its points go to the pixel grid ([`fit`]). Snapping keeps straight lines one pixel wide; the cog is all
    /// short diagonals, and snapped its teeth come out uneven - a dotted ring at 16 px.
    fn snaps(self) -> bool {
        self != Icon::Settings
    }

    /// The asset path that draws it at `size`: the pixels are in the path, so the stroke can be 1 px at that size.
    pub fn path(self, size: IconSize) -> SharedString {
        format!("icon/{}/{}.svg", self.name(), drawn_size(size) as u32).into()
    }
}

/// How big an icon's drawing is inside its box, by its outline: the same width reads bigger as a square than as a
/// circle, and a lone line or chevron reads bigger still - so each gets its own share of the 16 grid, and every icon
/// looks the same size beside the others (the keyline idea of Material's and Fluent's icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optical {
    /// Circles, rings and triangles: 13 of 16.
    Round,
    /// Squares, folders, boxes, most things: 12 of 16.
    Box,
    /// Open marks - ✕, +, ✓, chevrons, arrows, a dash, …: 10 of 16.
    Mark,
}

impl Optical {
    /// The longer side of the drawing, on the 16 grid.
    pub const fn extent(self) -> f32 {
        match self {
            Optical::Round => 13.0,
            Optical::Box => 12.0,
            Optical::Mark => 10.0,
        }
    }
}

/// The pixels an icon of `size` is drawn at: at the UI scale, whole - so its lines can sit on the pixel grid.
pub fn drawn_size(size: IconSize) -> f32 {
    (size.pixels() * ui_scale()).round().max(1.0)
}

/// The SVG for a path under `icon/` - None for any other path. Fitted to its optical size ([`Icon::fitted`]), then
/// drawn in the pixels of its size ([`fit`]), stroked 1 px.
pub fn asset(path: &str) -> Option<String> {
    let rest = path.strip_prefix("icon/")?.strip_suffix(".svg")?;
    let (name, size) = rest.split_once('/')?;
    let size = size.parse::<u32>().ok().filter(|s| (1..=512).contains(s))? as f32;
    let icon = Icon::ALL.iter().find(|i| i.name() == name)?;
    let (stroke, fill) = icon.fitted();
    Some(shapes::svg_document(size, size, 1.0, &fit(&stroke, size, !icon.snaps()), &fit(&fill, size, true)))
}

/// The layout icons of the title bar and the guide: the parts of the window, filled while shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutIcon {
    /// Customize Layout: the frame with a sidebar and a panel.
    Customize,
    /// The sidebar on the left.
    SidebarLeft,
    /// The sidebar on the right.
    SidebarRight,
    /// The bottom panel.
    Panel,
}

/// A 16 × 16 layout icon - the same drawing as the Command Palette's ([`shapes::layout`]); `filled` fills the part it
/// stands for, as VS Code does while it is shown.
pub fn layout_icon(kind: LayoutIcon, filled: bool, color: gpui::Hsla) -> impl IntoElement {
    let name = match kind {
        LayoutIcon::Customize => "compact",
        LayoutIcon::SidebarLeft => "sidebar-left",
        LayoutIcon::SidebarRight => "sidebar-right",
        LayoutIcon::Panel => "panel",
    };
    gpui::svg().path(shapes::layout(name, filled)).size(px(16.0)).flex_none().text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_an_svg_stroked_one_pixel_at_its_size() {
        for icon in Icon::ALL {
            for size in [IconSize::Small, IconSize::Medium, IconSize::Nav, IconSize::Large] {
                let path = icon.path(size);
                let svg = asset(&path).unwrap_or_else(|| panic!("{path}"));
                assert!(svg.starts_with("<svg"), "{path}");
                let shape = icon.shape();
                assert!(!shape.stroke.is_empty() || !shape.fill.is_empty(), "{path}");
                if !shape.stroke.is_empty() {
                    assert!(svg.contains(r#"stroke-width="1""#), "{path}");
                }
            }
        }
    }

    #[test]
    fn lines_land_in_the_middle_of_a_pixel_and_arcs_still_meet_their_ends() {
        // At 13 px a line at 8 on the grid (6.5) and one at 4.5 (3.66 -> 3.5) cover one pixel row each.
        assert_eq!(fit(&parse("M2.5,8 H13.5"), 13.0, false), "M2.5,6.5H10.5");
        assert_eq!(fit(&parse("M2,4.5 V8"), 13.0, false), "M1.5,3.5V6.5");
        // A circle's two halves: the radius is half the snapped width.
        assert_eq!(fit(&parse(&circle(8.0, 8.0, 6.0)), 13.0, false), "M1.5,6.5A5,5 0 1 0 11.5,6.5A5,5 0 1 0 1.5,6.5Z");
        // At 16 px the grid is the pixels: nothing moves.
        assert_eq!(fit(&parse("M3.5,3.5 L12.5,12.5"), 16.0, false), "M3.5,3.5L12.5,12.5");
    }

    #[test]
    fn every_icon_fills_its_optical_size_in_the_middle_of_the_grid() {
        for icon in Icon::ALL {
            let (stroke, fill) = icon.fitted();
            let (x0, y0, x1, y1) = bounds(&[&stroke, &fill]).unwrap();
            let longest = (x1 - x0).max(y1 - y0);
            assert!((longest - icon.optical().extent()).abs() < 0.01, "{icon:?}: {longest}");
            assert!(((x0 + x1) / 2.0 - 8.0).abs() < 0.01 && ((y0 + y1) / 2.0 - 8.0).abs() < 0.01, "{icon:?}");
        }
    }

    #[test]
    fn a_circle_counts_its_bulge_not_only_its_ends() {
        // Its two halves end on the left; the right half reaches 8 further.
        assert_eq!(bounds(&[&parse(&circle(8.0, 8.0, 4.0))]), Some((4.0, 4.0, 12.0, 12.0)));
    }

    #[test]
    fn names_are_unique_and_unknown_paths_are_not_icons() {
        let mut names: Vec<_> = Icon::ALL.iter().map(|i| i.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Icon::ALL.len());
        assert_eq!(asset("icon/nothing/13.svg"), None);
        assert_eq!(asset("icon/close/0.svg"), None);
        assert_eq!(asset("shape/spinner.svg"), None);
    }
}
