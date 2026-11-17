//! The tour of the window - `Controls/GuidedTour.xaml(.cs)` with its stops from `MainWindow.Help.cs`: the window dimmed
//! but for one part at a time, ringed in blue, with a card beside it saying what it is for. Back / Next (← / →), Skip
//! tour (Esc), Finish on the last. While it is open nothing under it can be clicked; it follows the part as the window
//! is resized.
//!
//! The app knows where its parts are: it puts a [`target`] marker inside each part (an absolutely placed canvas that
//! records the part's bounds every frame into the shared [`TourTargets`]), and the overlay - over the whole window -
//! reads them in the same frame.

use dnnm_ui::px;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, BorderStyle, Bounds, ClickEvent, Context, Corners, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, Pixels, Point, SharedString, Size, Window, actions, canvas, div, fill, point, prelude::*, quad, rgba, size};

use dnnm_ui::theme::Theme;
use dnnm_ui::{ButtonKind, Pad, Tip, button, link};

use crate::markdown::bold_text;

/// A part of the window the tour points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TourPart {
    /// The top of the sidebar: Projects and New project (`SidebarTop`).
    SidebarPages,
    /// The Projects table (`ProjectsPage.TourTable`).
    ProjectsTable,
    /// The Projects page's toolbar: search, Columns, Only show running (`ProjectsPage.TourToolbar`).
    ProjectsToolbar,
    /// The title bar's search (`CommandCenter`).
    CommandCenter,
    /// The title bar's Update button, while there is an update.
    UpdateButton,
    /// The title bar's ? (`HelpButton`).
    HelpButton,
    /// The bottom panel: Output, Logs, Terminal (`PanelCard`).
    Panel,
    /// The status bar's IIS state and buttons (`IisStatus`).
    IisStatus,
    /// The status bar (`StatusBar`).
    StatusBar,
    /// The gear at the bottom of the sidebar (`ManageButton`).
    ManageButton,
}

/// What the app does before a stop is shown, so its part can be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourEnter {
    /// Projects, with no details open (`ShowTable`).
    ShowProjectsTable,
    /// The bottom panel open, without taking the keyboard.
    OpenPanel,
}

/// A stop: the part it is about, what to say about it (`**bold**` allowed), and what to do first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TourStop {
    pub part: TourPart,
    pub title: &'static str,
    pub text: &'static str,
    pub enter: Option<TourEnter>,
}

/// What of the window is shown - a part hidden by the layout isn't on the tour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TourLayout {
    pub sidebar_visible: bool,
    /// The title bar's search - hidden in a narrow window.
    pub command_center_visible: bool,
    /// Update - shown only while there is one.
    pub update_visible: bool,
    pub status_bar_visible: bool,
}

/// The tour's stops, in the order the eye goes round the window (`MainWindow.TourStops`).
pub fn stops(layout: TourLayout) -> Vec<TourStop> {
    let stop = |part, title, text, enter| TourStop { part, title, text, enter };
    let mut stops = Vec::new();
    if layout.sidebar_visible {
        stops.push(stop(
            TourPart::SidebarPages,
            "The pages",
            "**Projects** lists your websites. **New project** creates one - new, or from a backup.",
            None,
        ));
    }
    stops.push(stop(
        TourPart::ProjectsTable,
        "Your websites",
        "One row per website. The dot shows how it is doing: **green** is running, an **empty circle** is stopped, **orange** is busy. \
         Click a name for everything about it; right-click a row for all its actions.",
        Some(TourEnter::ShowProjectsTable),
    ));
    stops.push(stop(
        TourPart::ProjectsToolbar,
        "Find and filter",
        "Type to find a website. **Columns** chooses what the list shows; **Only show running** hides stopped websites. \
         Tick rows to start, stop or remove several websites at once.",
        Some(TourEnter::ShowProjectsTable),
    ));
    if layout.command_center_visible {
        stops.push(stop(
            TourPart::CommandCenter,
            "Search everything",
            "Find a website by name from any page. Type **>** first to run any command instead.",
            None,
        ));
    }
    if layout.update_visible {
        stops.push(stop(
            TourPart::UpdateButton,
            "A new version is ready",
            "**Update** installs it. DNN Manager closes and opens again by itself, where you were.",
            None,
        ));
    }
    stops.push(stop(
        TourPart::HelpButton,
        "Help",
        "Help for the page you are on (**F1**), the getting started guide, and this tour.",
        None,
    ));
    stops.push(stop(
        TourPart::Panel,
        "Output, Logs and Terminal",
        "**Output** shows what DNN Manager is doing, step by step. **Logs** shows what your websites wrote down - look here when something goes wrong. \
         **Ctrl+J** opens and closes this panel.",
        Some(TourEnter::OpenPanel),
    ));
    if layout.status_bar_visible {
        stops.push(stop(
            TourPart::IisStatus,
            "The web server",
            "**IIS running** in green means your websites can be opened. Its buttons restart or stop IIS - that affects **every website** on this computer.",
            None,
        ));
        stops.push(stop(
            TourPart::StatusBar,
            "This computer",
            "How much memory, processor and disk space is in use.",
            None,
        ));
    }
    if layout.sidebar_visible {
        stops.push(stop(
            TourPart::ManageButton,
            "Settings and more",
            "**Settings**, **Themes**, **Troubleshoot** and **Check for Updates**. To begin: **Settings → IIS** and **Settings → Database server** - click **Test** in each.",
            None,
        ));
    }
    stops
}

/// Where the window's parts are, as drawn this frame - written by the [`target`] markers, read and cleared by the tour.
#[derive(Clone, Default)]
pub struct TourTargets(Rc<RefCell<HashMap<TourPart, Bounds<Pixels>>>>);

impl TourTargets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, part: TourPart, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().insert(part, bounds);
    }

    /// Where `part` was drawn this frame (window coordinates).
    pub fn get(&self, part: TourPart) -> Option<Bounds<Pixels>> {
        self.0.borrow().get(&part).copied()
    }

    fn clear(&self) {
        self.0.borrow_mut().clear();
    }
}

/// The marker the app puts inside a part (made `.relative()`): it covers the part and records its bounds every frame
/// while it is drawn. Not in a `.cached()` view - a cached frame doesn't run it.
pub fn target(part: TourPart, targets: &TourTargets) -> impl IntoElement {
    let targets = targets.clone();
    canvas(move |bounds, _, _| targets.record(part, bounds), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

actions!(guided_tour, [TourNext, TourBack, TourSkip]);

/// → Next, ← Back, Esc Skip tour. Call once at the start.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", TourNext, Some("GuidedTour")),
        KeyBinding::new("left", TourBack, Some("GuidedTour")),
        KeyBinding::new("escape", TourSkip, Some("GuidedTour")),
    ]);
}

/// What the tour tells the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourEvent {
    /// Make this stop's part visible (open the panel, show Projects) - before it is looked for.
    Enter(TourEnter),
    /// The tour is over - `finished` when Finish ended it, false when it was skipped. The app removes the overlay, puts
    /// the panel back as it was before the tour (`Tour_Ended`) and gives the page the keyboard.
    Ended { finished: bool },
}

const GAP: f32 = 12.0;
const EDGE: f32 = 12.0;
const HOLE_PADDING: f32 = 4.0;
/// How many frames a stop waits for its part to be drawn (after its Enter) before it is passed over.
const WAIT_FRAMES: u8 = 3;

/// The overlay. The app adds it as the last child of its root (`.relative()`, the size of the window) while the tour
/// is open, and drops it on [`TourEvent::Ended`].
pub struct GuidedTour {
    stops: Vec<TourStop>,
    targets: TourTargets,
    index: usize,
    /// The first stop shown - Back goes no further.
    first: Option<usize>,
    /// A stop being looked for: its index, the direction to go on in when its part isn't drawn, and frames left.
    pending: Option<(usize, isize, u8)>,
    ended: bool,
    /// The hole as last drawn and the card's measured height - the card is placed from them, and placed again (one
    /// frame later) when the part moves or the card's wrapped text is taller than measured.
    hole: Rc<Cell<Option<Bounds<Pixels>>>>,
    overlay_size: Rc<Cell<Size<Pixels>>>,
    card_height: Rc<Cell<f32>>,
    focus: FocusHandle,
}

impl EventEmitter<TourEvent> for GuidedTour {}

impl GuidedTour {
    /// Starts the tour at its first stop whose part can be seen; takes the keyboard. `stops` from [`stops`]; `targets`
    /// the markers' shared record.
    pub fn new(stops: Vec<TourStop>, targets: TourTargets, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut tour = Self {
            stops,
            targets,
            index: 0,
            first: None,
            pending: None,
            ended: false,
            hole: Rc::new(Cell::new(None)),
            overlay_size: Rc::new(Cell::new(size(px(0.0), px(0.0)))),
            card_height: Rc::new(Cell::new(150.0)),
            focus,
        };
        if tour.stops.is_empty() {
            tour.ended = true;
            cx.defer_in(window, |_, _, cx| cx.emit(TourEvent::Ended { finished: false }));
        } else {
            tour.go(0, 1, cx);
        }
        tour
    }

    /// Shows stop `index` - or, when its part isn't drawn, the next one in `direction`. The stop's Enter goes first, and
    /// the part is looked for once the window has drawn that.
    fn go(&mut self, index: isize, direction: isize, cx: &mut Context<Self>) {
        if self.ended {
            return;
        }
        if index >= self.stops.len() as isize {
            self.end(true, cx);
            return;
        }
        let (index, direction) = if index < 0 { (0, 1) } else { (index as usize, direction) };
        if let Some(enter) = self.stops[index].enter {
            cx.emit(TourEvent::Enter(enter));
        }
        self.pending = Some((index, direction, WAIT_FRAMES));
        cx.notify();
    }

    /// Called from the overlay's paint when the pending stop's part was drawn - or wasn't, again.
    fn settle(&mut self, found: bool, cx: &mut Context<Self>) {
        let Some((index, direction, frames)) = self.pending else { return };
        if found {
            self.pending = None;
            self.index = index;
            self.first.get_or_insert(index);
            self.hole.set(None);
            cx.notify();
        } else if frames > 0 {
            self.pending = Some((index, direction, frames - 1));
            cx.notify();
        } else {
            self.pending = None;
            self.go(index as isize + direction, direction, cx);
        }
    }

    fn end(&mut self, finished: bool, cx: &mut Context<Self>) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.pending = None;
        cx.emit(TourEvent::Ended { finished });
        cx.notify();
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_none() {
            self.go(self.index as isize + 1, 1, cx);
        }
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_none() && self.first.is_some_and(|first| self.index > first) {
            self.go(self.index as isize - 1, -1, cx);
        }
    }

    pub fn skip(&mut self, cx: &mut Context<Self>) {
        self.end(false, cx);
    }

    /// The card: which stop, its title and text, Skip tour (not on the last), Back and Next / Finish.
    fn render_card(&self, stop: &TourStop, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let c = theme.colors;
        let last = self.index + 1 == self.stops.len();
        let can_back = self.first.is_some_and(|first| self.index > first);
        let card_height = self.card_height.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .pad(px(14.0), px(18.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(c.card_border)
            .bg(c.card_bg)
            .shadow_lg()
            .child(
                // Measures the card as laid out: a taller card than guessed is placed again.
                canvas(
                    move |bounds, window, cx| {
                        let h = f32::from(bounds.size.height);
                        if (h - card_height.get()).abs() > 0.5 {
                            card_height.set(h);
                            // Once this frame is done: a refresh asked for while drawing is dropped.
                            let handle = window.window_handle();
                            cx.defer(move |cx| {
                                handle.update(cx, |_, window, _| window.refresh()).ok();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            // Which stop and its title, 2 apart; the text 6 under them; the buttons 14 under the text.
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div().w_full().text_size(theme.text.caption).text_color(c.text_muted).child(format!(
                                    "{} of {}",
                                    self.index + 1,
                                    self.stops.len()
                                )),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_size(theme.text.medium)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(SharedString::from(stop.title)),
                            ),
                    )
                    .child(div().w_full().line_height(px(20.0)).child(bold_text(stop.text))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().when(!last, |d| {
                        d.child(
                            link("tour-skip", "Skip tour", cx)
                                .tip("End the tour (Esc) - Help opens it again")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.skip(cx))),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                button("tour-back", "Back", ButtonKind::Secondary, can_back, cx)
                                    .min_w(px(70.0))
                                    .tip("The previous part (←)")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.back(cx))),
                            )
                            .child(
                                button(
                                    "tour-next",
                                    if last { "Finish" } else { "Next" },
                                    ButtonKind::Primary,
                                    true,
                                    cx,
                                )
                                .min_w(px(70.0))
                                .tip(if last { "End the tour" } else { "The next part (→)" })
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.next(cx))),
                            ),
                    ),
            )
    }
}

/// The tour's backdrop (`TourBackdrop`): #80000000 in the light theme, #B3000000 in the dark.
fn backdrop(dark: bool) -> Hsla {
    rgba(if dark { 0x000000B3 } else { 0x00000080 }).into()
}

/// The hole around a part: 4 px larger, within the overlay.
pub fn hole_for(part: Bounds<Pixels>, overlay: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let grown = Bounds::new(
        point(part.origin.x - px(HOLE_PADDING), part.origin.y - px(HOLE_PADDING)),
        size(part.size.width + px(2.0 * HOLE_PADDING), part.size.height + px(2.0 * HOLE_PADDING)),
    );
    let hole = grown.intersect(&overlay);
    (hole.size.width > px(0.0) && hole.size.height > px(0.0)).then_some(hole)
}

/// The card's top left within the overlay (`PlaceCallout`): below the part, else above, right or left of it - wherever
/// it fits whole; over the middle of a part too big for any of those (the Projects table). Always inside the window,
/// on whole pixels. `hole` is relative to the overlay.
pub fn place_card(hole: Option<Bounds<Pixels>>, overlay: Size<Pixels>, card: Size<f32>) -> Point<f32> {
    let (w, h) = (f32::from(overlay.width), f32::from(overlay.height));
    let (x, y) = match hole {
        None => ((w - card.width) / 2.0, (h - card.height) / 2.0),
        Some(b) => {
            let (left, top) = (f32::from(b.origin.x), f32::from(b.origin.y));
            let (right, bottom) = (left + f32::from(b.size.width), top + f32::from(b.size.height));
            if h - bottom >= card.height + GAP + EDGE {
                (left, bottom + GAP)
            } else if top >= card.height + GAP + EDGE {
                (left, top - GAP - card.height)
            } else if w - right >= card.width + GAP + EDGE {
                (right + GAP, top)
            } else if left >= card.width + GAP + EDGE {
                (left - GAP - card.width, top)
            } else {
                (left + (right - left - card.width) / 2.0, top + (bottom - top - card.height) / 2.0)
            }
        }
    };
    let x = x.clamp(EDGE, (w - card.width - EDGE).max(EDGE)).round();
    let y = y.clamp(EDGE, (h - card.height - EDGE).max(EDGE)).round();
    point(x, y)
}

/// The card's width: 340, less in a narrow window, at least 200.
pub fn card_width(overlay_width: Pixels) -> f32 {
    (f32::from(overlay_width) - 2.0 * EDGE).clamp(200.0, 340.0)
}

impl Focusable for GuidedTour {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GuidedTour {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let shown = if self.ended || self.pending.is_some() { None } else { self.stops.get(self.index).copied() };
        let looking_for = self.pending.map(|(i, _, _)| self.stops[i].part);
        let part = shown.map(|s| s.part);
        let targets = self.targets.clone();
        let hole_cell = self.hole.clone();
        let size_cell = self.overlay_size.clone();
        let this = cx.entity().downgrade();
        let dim = backdrop(theme.dark);
        let ring = theme.colors.accent;

        // The backdrop and the ring, drawn from where the parts were drawn this frame. The markers record in their
        // prepaint, before this paints; the record is cleared after, so a part no longer drawn isn't pointed at.
        let stage = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                if let Some(wanted) = looking_for {
                    let found = targets.get(wanted).is_some();
                    let this = this.clone();
                    window.on_next_frame(move |_, cx| {
                        let _ = this.update(cx, |tour, cx| tour.settle(found, cx));
                    });
                    window.paint_quad(fill(bounds, dim));
                    targets.clear();
                    return;
                }
                let hole = part.and_then(|p| targets.get(p)).and_then(|b| hole_for(b, bounds));
                targets.clear();
                match hole {
                    None => window.paint_quad(fill(bounds, dim)),
                    Some(h) => {
                        let (l, t) = (h.origin.x, h.origin.y);
                        let (r, b) = (l + h.size.width, t + h.size.height);
                        let (bl, bt) = (bounds.origin.x, bounds.origin.y);
                        let (br, bb) = (bl + bounds.size.width, bt + bounds.size.height);
                        let rect = |x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels| {
                            Bounds::new(point(x0, y0), size((x1 - x0).max(px(0.0)), (y1 - y0).max(px(0.0))))
                        };
                        for piece in [rect(bl, bt, br, t), rect(bl, b, br, bb), rect(bl, t, l, b), rect(r, t, br, b)] {
                            window.paint_quad(fill(piece, dim));
                        }
                        window.paint_quad(quad(
                            h,
                            Corners::all(px(6.0)),
                            gpui::transparent_black(),
                            px(2.0),
                            ring,
                            BorderStyle::default(),
                        ));
                    }
                }
                // The card follows the part: placed again next frame when it moved or the window changed size.
                let relative = hole.map(|h| Bounds::new(h.origin - bounds.origin, h.size));
                if relative != hole_cell.get() || bounds.size != size_cell.get() {
                    hole_cell.set(relative);
                    size_cell.set(bounds.size);
                    window.refresh();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let overlay = self.overlay_size.get();
        let width = card_width(overlay.width);
        let at = place_card(self.hole.get(), overlay, size(width, self.card_height.get()));

        div()
            .id("guided-tour")
            .key_context("GuidedTour")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TourNext, _, cx| this.next(cx)))
            .on_action(cx.listener(|this, _: &TourBack, _, cx| this.back(cx)))
            .on_action(cx.listener(|this, _: &TourSkip, _, cx| this.skip(cx)))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // Nothing under it can be clicked.
            .occlude()
            .font_family(theme.font.clone())
            .text_size(theme.text.body)
            .text_color(theme.colors.text_primary)
            .child(stage)
            .when_some(shown, |d, stop| {
                d.child(div().absolute().left(px(at.x)).top(px(at.y)).w(px(width)).child(self.render_card(&stop, cx)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn stops_follow_the_layout() {
        let all = TourLayout {
            sidebar_visible: true,
            command_center_visible: true,
            update_visible: true,
            status_bar_visible: true,
        };
        let parts: Vec<TourPart> = stops(all).iter().map(|s| s.part).collect();
        assert_eq!(parts.len(), 10);
        assert_eq!(parts[0], TourPart::SidebarPages);
        assert_eq!(parts[9], TourPart::ManageButton);
        let bare = TourLayout {
            sidebar_visible: false,
            command_center_visible: false,
            update_visible: false,
            status_bar_visible: false,
        };
        let titles: Vec<&str> = stops(bare).iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Your websites", "Find and filter", "Help", "Output, Logs and Terminal"]);
        assert_eq!(stops(bare)[3].enter, Some(TourEnter::OpenPanel));
        assert!(stops(all)[1].text.contains("is busy. Click a name"));
    }

    #[test]
    fn the_hole_is_padded_and_kept_inside() {
        let overlay = b(0.0, 0.0, 1000.0, 700.0);
        assert_eq!(hole_for(b(100.0, 100.0, 50.0, 20.0), overlay), Some(b(96.0, 96.0, 58.0, 28.0)));
        assert_eq!(hole_for(b(0.0, 0.0, 50.0, 20.0), overlay), Some(b(0.0, 0.0, 54.0, 24.0)));
        assert_eq!(hole_for(b(2000.0, 0.0, 50.0, 20.0), overlay), None);
    }

    #[test]
    fn the_card_goes_below_above_beside_or_over_the_part() {
        let window = size(px(1000.0), px(700.0));
        let card = size(340.0, 150.0);
        // Below a part at the top.
        assert_eq!(place_card(Some(b(100.0, 10.0, 30.0, 20.0)), window, card), point(100.0, 42.0));
        // Above a part at the bottom.
        assert_eq!(place_card(Some(b(100.0, 650.0, 30.0, 20.0)), window, card), point(100.0, 488.0));
        // Right of a part as tall as the window.
        assert_eq!(place_card(Some(b(0.0, 0.0, 230.0, 700.0)), window, card), point(242.0, 12.0));
        // Over the middle of a part too big for any side.
        assert_eq!(place_card(Some(b(0.0, 0.0, 1000.0, 700.0)), window, card), point(330.0, 275.0));
        // No part: the middle of the window.
        assert_eq!(place_card(None, window, card), point(330.0, 275.0));
        // Kept inside on the right.
        assert_eq!(place_card(Some(b(900.0, 10.0, 30.0, 20.0)), window, card), point(648.0, 42.0));
        assert_eq!(card_width(px(300.0)), 276.0);
        assert_eq!(card_width(px(100.0)), 200.0);
    }
}
