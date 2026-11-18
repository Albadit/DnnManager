# Design system

DNN Manager looks like VS Code's Modern themes: every colour and size is a token in one place, and every view takes
them from there.

## Where

| What | Rust |
|---|---|
| Colours, light and dark (VS Code Light Modern, Dark Modern) | `Colors::light()`, `Colors::dark()` in [`theme.rs`](../crates/ui/src/theme.rs) |
| Radii, heights, gaps, text sizes | `Metrics`, `TextSizes` in `theme.rs` |
| Glyphs (Segoe Fluent Icons code points), layout and column icons | [`icons.rs`](../crates/ui/src/icons.rs) |
| Buttons, links, switches, check boxes, status dots | [`lib.rs`](../crates/ui/src/lib.rs) |
| Menus, dialog frame and form pieces, the text field | [`menu.rs`](../crates/ui/src/menu.rs), [`dialog.rs`](../crates/ui/src/dialog.rs), [`text_input.rs`](../crates/ui/src/text_input.rs) |

Colour fields are in snake case (`card_border`). A colour with alpha is written `#AARRGGBB` (`ca(0x33000000)`).

The theme is a GPUI global (`Theme::get(cx)`), rebuilt when the settings' theme, font size or density changes, or the
system switches between light and dark while the theme is **System**.

## UI scale

Settings → General → UI scale (80-200 %) scales everything the app draws: every size is written with the app's
`px` ([`dnnm_ui::px`](../crates/ui/src/scale.rs), the terminal's `dnnm_terminal::scale::px`), which multiplies it by
the scale - GPUI 0.2.2 has no zoom of its own. Use it for every size, never `gpui::px`, except for the window's own
place and size on the screen (`main.rs`, the saved window bounds). A change applies at once: the theme is made again
(`Workspace::apply_theme` → `apply_ui_scale`).

## Spacing

- **No margins, no per-side padding.** `.m*`, `.mt/.mb/.ml/.mr/.mx/.my`, `*_auto` margins and `.pt/.pb/.pl/.pr/.px/.py`
  aren't used.
- **Between elements: flex and gap.** A parent that lays out children is `.flex()` / `.flex_col()` with `.gap(..)`;
  siblings spaced differently are grouped into nested flex wrappers, each with its own gap. Centring is
  `justify_center` / `items_center`; pushing to the end is `justify_between`, or a child that grows (`flex_1()`) and
  holds content. A title centred between unequal sides is a 3-column grid (`grid().grid_cols(3)`, `col_start`), as
  the palette's header.
- **No spacer elements**: no empty `div` to make room (a `flex_1()` pusher, a fixed-size blank). An element that needs
  extra room around it pads itself (a stage heading in the Output: `.pad(3, 0)`). An empty cell that keeps a table
  column's width in a header row is a column, not a spacer.
- The rules are checked by a test (`spacing::tests` in `crates/ui`): it fails on a margin or per-side padding call
  anywhere under `crates/`.
- **Inside a container** (a section, a card, a button, a row, a wrap): one padding call - `.p(x)` when equal all round,
  else `.pad(vertical, horizontal)` from [`dnnm_ui::Pad`](../crates/ui/src/spacing.rs), CSS's `padding: v h`.
- Absolute positions (`top`, `left`, `inset_0`) aren't spacing and stay.
- Text that wraps inside a flex child gets `.w_full()` (or `.min_w_0()`): GPUI otherwise measures it too narrow.

## Type

| Role | Size at the default font size 13 | Used for |
|---|---|---|
| `small` | 11 | Output status pill, gear menu group name |
| `hint` | 11.5 | Expanded row's fact names, Settings hints |
| `caption` | 12 | Title bar, status bar, command center, tooltips, Output lines |
| `body` | 13 | Everything else |
| `medium` | 14 | Sidebar entries, the empty-table text |
| `heading` | 15 | Settings sub-headings |
| `large` | 16 | Settings section title |
| `title` | 20 | Settings header |
| `display` | 22 | Page titles |

Settings → General's font size (11-18) scales all of them in these proportions (`TextSizes::for_body`). Fonts: Segoe UI; icons Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10, the
same code points); the Output panel Cascadia Mono. On macOS and Linux GPUI falls back to the system's fonts, so the
glyphs may be missing there.

## Frame

| Token | Default | Compact density |
|---|---|---|
| Title bar / status bar height | 36 / 28 | same |
| Activity bar width / side bars' default width | 50 / 230 | 41 / 190 |
| Gap around the cards (activity bar, side bars, page, panel) | 5, rounded 6, bordered | 0, flush, divided by lines |
| Panel height | 230 | same |
| Control height, search height | 30, 26 | same |
| Radius: controls, check boxes, cards | 4, 3, 6 | same |
| Table row height | 38 | same |

The window, as VS Code's: the activity bar (the page entries, their icons alone), the primary side bar beside it and
the secondary one across the page (both empty for now, Ctrl+B / Ctrl+Alt+B), the page, the panel under the parts Panel
Alignment says, the status bar. Primary Side Bar Position puts the activity bar and the primary side bar on the left
or the right.

## Controls

| Control | Spec | Rust |
|---|---|---|
| Button | 30 high, padding 14 × 6, radius 4; Secondary: `card_bg` / `card_border`, `hover_bg` on hover, 50 % when disabled; Primary: `accent`, semibold, `accent_hover`, 45 % disabled | `ui::button` |
| Icon button | 28 × 28, transparent, `hover_bg`, 35 % disabled | `ui::icon_button` |
| Link | `link_fg`, underlined on hover | `ui::link` |
| Toggle switch | track 32 × 18 radius 9 (`input_border` off, `accent` on), knob 12 `on_accent`, label 8 to the right; slides when flipped (Animations) | `ui::switch` |
| Table check box | 16, radius 3, `accent` filled with a check when ticked | `ui::check_box` |
| Activity bar entry | A 32 px square, its 16 px glyph in the middle, radius 6, 4 px between entries; as much room beside it as above it (8 px; 4 in Compact - the bar is 50 / 41 wide); what it is in its tooltip, beside it | `Workspace::render_sidebar` |
| Tooltip | The card (caption text, padding 8 × 4, at most 420 wide), 4 px from its element: under it and centred on it - above it without room below, moved in from the window's edges -, or beside it, centred on its height, for the activity bar's entries. Set with `.tip(text)` / `.tip_beside(text)` (`ui::Tip`), which measure the element; GPUI by itself puts a tooltip at the mouse | `ui::Tooltip`, `ui::tip_origin` |
| Panel tab | 12 px, 26 high, padding 8 beside the text, radius 4, `sidebar_selected` when chosen | `Workspace::render_panel` |
| Panel header | The tabs and the panel buttons, 4 px from the header's edges all round (as much beside them as above), a 1 px `card_border` line under it | `Workspace::render_panel` |
| Table header | `header_bg`, `text_muted` semibold, padding 8 × 6, line under it | `ProjectsPage` |
| Table cell | padding 8 × 0; a column as wide as its widest text + 16, at least its minimum | `Widths::measure` |
| Window buttons | 46 wide; close turns `#C42B1C` with white glyph on hover | `Workspace::render_title_bar` |
| Command center | 26 high, radius 6, `command_center_*` colours, 38 % of the window, at most 600 | `Workspace::render_title_bar` |
| Title bar dragging | The bar is Windows' caption (`WindowControlArea::Drag`) and `.occlude()`d, so the workspace's focus handling doesn't swallow the press; every button on it is `.occlude()`d too, so a press there is a click, not a move | `Workspace::render_title_bar` |
| Info tip | A 12 px muted ⓘ beside a title or label, its explanation in a tooltip; centred on the line and then lowered about a tenth of the text's size (1 px beside a label, 2 beside a page's title, `ui::info_tip_drop`), so it sits in the middle of the letters as seen rather than on the capitals | `ui::info_tip`, `ui::with_tip`, `ui::with_tip_sized`, `ui::dialog::frame_with_tip` |
| Select | The box with a chevron; the list an overlay (`deferred` + `anchored`) under it, at least as wide as the box, scrolling past 320 px with a scroll bar (a 6 px `scroll_thumb` thumb in a 12 px strip at its right, as tall as the share seen, 8 px and `scroll_thumb_hover` under the mouse or while dragged; drag it, or press beside it to jump there). A press on the box focuses and opens it; Down / Up / Enter / Space open it from the keyboard, Up / Down / Home / End / Page Up / Page Down move the highlight (one highlight for mouse and keys), Enter or Space choose, Esc shuts the list (then the dialog), a press outside shuts it - the Settings selects too; the Logs pickers share the scroll bar | `ui::dialog::select`, `ui::scrollbar`, `ui::with_scrollbar` |
| Keyboard shortcut | Each of a command's shortcuts a 22 px chip (click: record a new one there), then **+** (or "Add shortcut") to record another; kept as `Ctrl+J, Ctrl+Shift+J` | `SettingsPage::render_keyboard`, `settings_keyboard` |
| Sash | The side bars' and panel's edges, as grab area the gap (4 px in Compact): three 2 px dots in its middle, or in Compact a 1 px `sidebar_border` line; under the mouse and while dragged an `accent` line with round ends as thick as the sash (the same on every edge); side bars and panel stop at their smallest size (a drag doesn't close them); the panel leaves the page at least 300 px of the window; where the primary side bar's and the panel's sashes meet, an invisible corner (the four-way Move pointer - GPUI's Windows backend has none, so `dnnm_system::move_pointer` subclasses the window and answers `WM_SETCURSOR` with it; elsewhere a crosshair) drags both and lights both lines | `Workspace::render` (`Sash.cs`) |
| Toasts | The page's bottom right, 16 px in - over the page, above the panel (the window's bottom right while the panel is maximized): the running operation (title, Cancel, the progress bar), then one message at a time | `Workspace::render_toasts` |
| Output while a run goes on | The lines keep the newest in view while the view is at the end (scrolled up to read, it stays); the stage list brings the stage that runs into view | `Workspace::render_run` |
| Output header | The pill (a spinner while running), title, issues; "start → end" and the time ("0.25s", "18s" by the second while running, "2m 26s"); the stage list "Stages · done/all", each stage with its time - 220 px wide, dragged by its edge from 150 to 520 (kept as `outputStagesWidth`), the edge a 1 px line lit 4 px wide in the accent under the mouse and while dragged; stage names, the title and the summary's values cut short with "…" (`ui::one_line`) | `Workspace::render_run`, `Workspace::stages_sash`, `span` |
| Panel buttons (`PanelButton`) | Clear, Find, a 1 × 16 line, maximize/restore (drawn, 12 px, 1 px lines on the pixel grid), hide (✕ at `SIZE_CLOSE` 11) - 26 × 26, `log_muted`; the terminal's + and ⌄ the same | `Workspace::render_panel`, `panel_button` (terminal) |
| Text field caret | 1 px, 1.2 × the font size high (not the line box), in the line's middle; blinks every 530 ms (Windows' default) while focused, solid again after each edit or move | `ui::text_input` |
| Find bar (`PanelSearch`) | Over the shown tab's top right: a 260 px box with Aa / ab / .* inside it, the count ("3 of 18", "No results"), ↑ ↓ (13 px) and ✕ (11 px, as the panel's); matches `log_warn` at 30 %, the current one at 75 % | `Workspace::render_find_bar`, `ui::marked_text`, `dnnm_logs::search` |
| Context menu | Rows 4 px in from the sides, separators edge to edge; a submenu opens beside its entry on the side where it fits in the window; a press anywhere outside it (panel and title bar too) or Esc closes it, and does nothing else | `ui::menu` |
| Popup from a button | Opens under its button, 4 px below, left edges level (right edges for a button at the right, like ⋮ and the terminal's ⌄): the button's place is measured with `ui::Anchor`; a second click on the button closes it; the button's tip is left out while it is open. Every popup (menus, select lists, Columns, pickers) is drawn on the top layer, `deferred(...).with_priority(2)` (a menu's outside-press catcher 2, the menu 3) | `ui::Anchor` |
| Keep-warm flame | 13 px high in a box of the path's proportions (GPUI scales an SVG to its box's width), centred in a glyph's 15 px square; the red dot at the bottom right | `flame_icon`, `flame_width` |
| Layout icons, Columns icon | Path data, stroked 1 / 1.5 px, filled while the part is shown | `ui::icons::layout_icon`, `columns_icon` |

## Icons

Sizes by role, from `ui::icons` - never a number typed at the call:

| Constant | Size | For |
|---|---|---|
| `SIZE_SMALL` | 10 | Chevrons, check marks, sort arrows, the window's buttons, the Output pill |
| `SIZE` | 13 | Every action and state icon: icon buttons (28 × 28, the C# `IconButton`), buttons with a label, the title and status bars, toasts, errors, check lists, tips |
| `SIZE_NAV` | 16 | The activity bar and Settings' categories |
| `SIZE_LARGE` | 26 | A prompt's own icon |
| `LABEL_GAP` | 8 | Between an icon and its label |

- `ui::icon` puts a glyph in a box 2 px larger than it, centred; rows centre it with `items_center`.
- An icon in front of text that may wrap uses `ui::line_icon` (`items_start` row): its box is one line of that text
  high, so it centres on the first line.
- Measured in the running app (bounding boxes of the drawn pixels): icons and their text share a centre line within
  half a pixel. One place needed 1 px to get there, commented: the palette's check marks (`top(-1)`).
- The terminal crate doesn't depend on dnnm-ui and repeats the sizes (`ICON_SIZE`, `ICON_SIZE_SMALL`).
- Drawn icons - the C# app's path data, not glyphs - come from `ui::shapes` as SVGs: the layout icons (filled while
  shown or chosen - Panel Alignment's always, the panel's place being the picture; the activity bar's icon
  `activity-bar-left/right`, the side bars' `sidebar-left/right`, the menu bar's VS Code's `layout-menubar`; Customize Layout puts one in front of each option and the tick after its title), the spinner
  (`ui::spinner`, turning once a second with animations on) and maximize/restore. `ui::shape_icon` puts one in a
  glyph's box.

## Explanations

A fixed explanation of a page, card, field, check box or dialog is an info tip beside its title or label - not a line of
text under it. What the user must not miss stays visible text: a problem or validation error, live status (reading,
offline, what a server answered) and a warning that something can't be undone. A hint that is sometimes an
explanation and sometimes a problem is a tip while it explains and text while it reports the problem.

## Animations

| What | How | Rust |
|---|---|---|
| Menus, dialogs, Settings, the palette | Fade in over `ui::MOTION` (200 ms, ease-in-out) | `ui::fade_in` |
| Toggle switch | When flipped while shown: the knob slides and the track fades to its new colour over `ui::MOTION`; one that appears already on or off doesn't move | `ui::switch` |
| Keep-warm flame while it pings | Opacity 1 → 0.3 → 1 every 1.2 s | `ui::pulse`, `FLAME_PULSE` |
| The running operation's progress (its toast) | A third of a 2 px track slides across it every 1.4 s, eased at both ends; the whole track lit with animations off | `progress_bar` (workspace.rs) |
| Side bars and the panel shown or hidden | Slide open or shut over `ui::MOTION`: the width (the height for the panel) eases between 0 and its own; the content keeps its size and is cut off, so nothing in it moves (not when the panel is maximized) | `Workspace::render`, `Reveal` |

They play only while Settings → General's "Play animations" is on and Windows' animation effects are on
(`dnnm_system::motion::system_animations`); `Theme::animations` carries both.

## Not there yet

- Animations: a pulsing dot while a site changes, an indeterminate progress bar.
- Drop shadows are GPUI's presets (`shadow_md`, `shadow_lg`, `shadow_xl`).
- Windows high-contrast themes.
- UI scale (Settings → General) is saved but not applied; GPUI follows the display's scaling.
