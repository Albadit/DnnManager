# Design system

DNN Manager looks like VS Code's Modern themes: every colour and size is a token in one place, and every view takes
them from there.

## Where

| What | Rust |
|---|---|
| Colours, light and dark (VS Code Light Modern, Dark Modern) | `Colors::light()`, `Colors::dark()` in [`theme.rs`](../crates/ui/src/theme.rs) |
| Radii, heights, gaps, text sizes | `Metrics`, `TextSizes` in `theme.rs` |
| Every icon (`Icon`, its path data), the layout icons | [`icons.rs`](../crates/ui/src/icons.rs), [`shapes.rs`](../crates/ui/src/shapes.rs) |
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

Settings → General's font size (11-18) scales all of them in these proportions (`TextSizes::for_body`). Fonts: Segoe UI; the Output panel Cascadia Mono. On macOS and Linux GPUI falls back to the system's fonts. Icons
are drawn, not a font (see [Icons](#icons)), so they look the same everywhere.

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

The window, as VS Code's: the activity bar (the page entries, their icons alone), the primary side bar beside it (on
the Source Control page its view - `SourceControlSide`, the page's files; empty on the others) and
the secondary one across the page (empty for now; Ctrl+B / Ctrl+Alt+B), the page, the panel under the parts Panel
Alignment says, the status bar. Primary Side Bar Position puts the activity bar and the primary side bar on the left
or the right.

## Controls

| Control | Spec | Rust |
|---|---|---|
| Button | 30 high whatever it holds (text, an icon, both - an icon-only ⋮ beside text buttons lines up), 14 in from its sides, radius 4; Secondary: `card_bg` / `card_border`, `hover_bg` on hover, 50 % when disabled; Primary: `accent`, semibold, `accent_hover`, 45 % disabled | `ui::button` |
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
| Window buttons | Windows and Linux: ours at the right, 46 wide; close turns `#C42B1C` with a white icon on hover. macOS: the system's traffic lights at the left (12, 11 - centred in the bar), ours left out, the logo starting 78 px in (`MAC_CAPTION`) | `Workspace::render_title_bar`, `main` (`traffic_light_position`) |
| Command center | 26 high, radius 6, `command_center_*` colours, 38 % of the window, at most 600 | `Workspace::render_title_bar` |
| Title bar dragging | The bar is Windows' caption (`WindowControlArea::Drag`) and `.occlude()`d, so the workspace's focus handling doesn't swallow the press; every button on it is `.occlude()`d too, so a press there is a click, not a move | `Workspace::render_title_bar` |
| Info tip | A 12 px muted ⓘ beside a title or label, its explanation in a tooltip; centred on the line and then lowered about a tenth of the text's size (1 px beside a label, 2 beside a page's title, `ui::info_tip_drop`), so it sits in the middle of the letters as seen rather than on the capitals | `ui::info_tip`, `ui::with_tip`, `ui::with_tip_sized`, `ui::dialog::frame_with_tip` |
| Select | The box with a chevron; the list an overlay (`deferred` + `anchored`) under it, at least as wide as the box, scrolling past 320 px with a scroll bar (a 6 px `scroll_thumb` thumb in a 12 px strip at its right, as tall as the share seen, 8 px and `scroll_thumb_hover` under the mouse or while dragged; drag it, or press beside it to jump there). A press on the box focuses and opens it; Down / Up / Enter / Space open it from the keyboard, Up / Down / Home / End / Page Up / Page Down move the highlight (one highlight for mouse and keys), Enter or Space choose, Esc shuts the list (then the dialog), a press outside shuts it. Rows can include group headings (muted, bold, skipped by the keys and the mouse; the options under them indented 20 px), an option can have a tip, and the box can have a tip that is hidden while the list is open. The compact look is 24 px high with caption text, used by the Logs pickers. Every select in the app is this one: dialogs, Settings, the Logs pickers | `ui::dialog::{select, select_rows, SelectRow, SelectLook}`, `ui::scrollbar` |
| Keyboard shortcut | Each of a command's shortcuts a 22 px chip (click: record a new one there), then **+** (or "Add shortcut") to record another; written and kept as the OS writes them - `Ctrl+J, Ctrl+Shift+J`, on macOS `⌘J, ⇧⌘J` | `SettingsPage::render_keyboard`, `settings_keyboard`, `ui::keys` |
| Sash | The side bars' and panel's edges, as grab area the gap (4 px in Compact): three 2 px dots in its middle, or in Compact a 1 px `sidebar_border` line; under the mouse and while dragged an `accent` line with round ends as thick as the sash (the same on every edge); side bars and panel stop at their smallest size (a drag doesn't close them); the panel leaves the page at least 300 px of the window; where the primary side bar's and the panel's sashes meet, an invisible corner (the four-way Move pointer - GPUI's Windows backend has none, so `dnnm_system::move_pointer` subclasses the window and answers `WM_SETCURSOR` with it; elsewhere a crosshair) drags both and lights both lines | `Workspace::render` (`Sash.cs`) |
| Toasts | The page's bottom right, 16 px in - over the page, above the panel (the window's bottom right while the panel is maximized): the running operation (title, Cancel, the progress bar), then one message at a time | `Workspace::render_toasts` |
| Output while a run goes on | The lines keep the newest in view while the view is at the end (scrolled up to read, it stays); the stage list brings the stage that runs into view | `Workspace::render_run` |
| Output header | The pill (a spinner while running), title, issues; "start → end" and the time ("0.25s", "18s" by the second while running, "2m 26s"); the stage list "Stages · done/all", each stage with its time - 220 px wide, dragged by its edge from 150 to 520 (kept as `outputStagesWidth`), the edge a 1 px line lit 4 px wide in the accent under the mouse and while dragged; stage names, the title and the summary's values cut short with "…" (`ui::one_line`) | `Workspace::render_run`, `Workspace::stages_sash`, `span` |
| Panel buttons (`PanelButton`) | Clear, Find, a 1 × 16 line, maximize/restore (`Icon::PanelMaximize` / `PanelRestore`), hide (✕) - all at `SIZE` - 26 × 26, `log_muted`; the terminal's + and ⌄ the same | `Workspace::render_panel`, `panel_button` (terminal) |
| Source Control tabs | VS Code's: 35 px on `window_bg`, a tab a file - its icon, its name in its change's colour (italic while it is the preview), a muted "Server ↔ Project" / "Server" / "Project", a 20 px close button on the tab shown and the one under the mouse (its place kept on the others); the tab shown on `card_bg` with a 1 px `accent` line along its top; a 1 px `card_border` between tabs. Under them the file's breadcrumbs (folders in `text_muted`, chevrons, the file's icon and name) | `source_control_page` |
| Source Control diff | Side by side in two equal halves, the server's copy left and the project's right; the words that differ in a changed line on `diff_removed_text_bg` / `diff_added_text_bg` over their line's colour, `diff_filler` where a side has no line, the overview ruler (14 px, removed in `diff_ruler_removed` on its left half, added in `diff_ruler_added` on its right) under the scroll bar; a file only one side has drawn alone, as an editor; 44 px line numbers in `text_muted`; a removed line on `diff_removed_bg`, an added one on `diff_added_bg` (VS Code's, the same in both themes), a changed row both; the change shown last marked by a 1 px `focus_border` line above it; the mono font at caption size | `source_control_page` |
| Source Control code | Syntax colours from `highlight.rs` in `syntax_keyword`, `syntax_string`, `syntax_comment`, `syntax_number`, `syntax_tag`, `syntax_attribute` (VS Code's Light / Dark Modern); **Inline** shows one column with both line numbers and a `-` / `+` sign | `source_control_page` |
| Source Control rows | 22 px; a file's icon by its kind (`ui::file_icons`): a bold 9 px tag for code (C#, VB, JS, TS, {}, <>, #, @, Aa) or a line icon (picture, archive, database, cog, globe for .resx, cube for .dll, text page), in Seti's colours (a darker set in the light theme), the plain page in `text_muted` for any other; a row's own buttons only while the mouse is on it, so the name and folder have the whole width otherwise; the section the top row is in held at the top (`window_bg`, a 1 px `card_border` edge below), pushed up by the next section; the app's scroll bar (`ui::scrollbar`) over the list's right edge; VS Code's staging - **+** (`Icon::Add`) stages, **−** (`Icon::Dash`) unstages, a **Staged Changes** section above Changes; the primary button is **Commit** (`Icon::Check`) and nothing else, no status line under it - a spinner takes Refresh's place in the title while something runs | `source_control_page`, `ui::file_icons` |
| Change letters | M `log_warn`, A `success_text`, I / R `link_fg`, C / D / S `error_text`, bold, on the right of a file's row | `source_control_page` |
| Output and Logs text | Wraps in the panel's width; selectable across lines (`ui::selection::LinesSelection`): the selection a highlight of `accent` at 35 %, over Find's marks; I-beam cursor; Copy / Select all with the primary modifier and a right-click menu | `ui::selection`, `workspace::render_run`, `logs_view` |
| Text field caret | 1 px, 1.2 × the font size high (not the line box), in the line's middle; blinks every 530 ms (Windows' default) while focused, solid again after each edit or move | `ui::text_input` |
| Find bar (`PanelSearch`) | Over the shown tab's top right: a 260 px box with Aa / ab / .* inside it, the count ("3 of 18", "No results"), ↑ ↓ and ✕ (13 px); matches `log_warn` at 30 %, the current one at 75 % | `Workspace::render_find_bar`, `ui::marked_text`, `dnnm_logs::search` |
| Context menu | Rows 4 px in from the sides, separators edge to edge; a submenu opens beside its entry on the side where it fits in the window; a press anywhere outside it (panel and title bar too) or Esc closes it, and does nothing else | `ui::menu` |
| Popup from a button | Opens under its button, 4 px below, left edges level (right edges for a button at the right, like ⋮ and the terminal's ⌄): the button's place is measured with `ui::Anchor`; a second click on the button closes it; the button's tip is left out while it is open. Every popup (menus, select lists, Columns, pickers) is drawn on the top layer, `deferred(...).with_priority(2)` (a menu's outside-press catcher 2, the menu 3) | `ui::Anchor` |
| Keep-warm flame | 13 px high in a box of the path's proportions (GPUI scales an SVG to its box's width), centred in a glyph's 15 px square; the red dot at the bottom right | `flame_icon`, `flame_width` |
| Layout icons | Path data, stroked 1 px, filled while the part is shown | `ui::icons::layout_icon`, `ui::shapes::layout` |

## Keyboard

Everything works without the mouse (`ui::focus`):

- **Tab / Shift+Tab** go through every control in the order it is drawn (the title bar, the activity bar, the page,
  the panel, the status bar), wrapping at the ends. A control is a tab stop through `ui::focusable` (with a border:
  buttons, check boxes) or `ui::focusable_bare` (without: icon buttons, links, tabs, menu rows); `ui::button`,
  `icon_button`, `link`, `switch`, `check_box`, `dialog::check` / `radio`, text fields and selects are already.
  A disabled control is not a tab stop.
- **Enter / Space** press the focused control (GPUI clicks it as the key comes up; a "Control" key context keeps a
  page's own Enter from also running). A control opened on a mouse press (Help, the gear, a shortcut chip) also has
  an `on_click` for the keyboard.
- **The ring**: the control's 1 px border in `focus_border` (a filled button's in its text colour), and
  `hover_bg` inside for a control without a background; a tab shows it on its underline. Only while the keyboard
  is in use - from a Tab or a key press on a control until the next mouse press. A mouse press clicks a control
  without taking the focus.
- **Layers** (`ui::focus::trap`): Settings / Troubleshoot, dialogs, prompts, the palette, the tour, menus and the
  Columns popup hold the focus while open - Tab goes round their controls only. Opening, one takes the focus (its
  first control from the keyboard, unless it took it itself); closing, it gives it back to what had it.
- **Menus**: open on their first item from the keyboard; Down / Up and Tab move, Enter or Space runs an item or
  opens a submenu (its items come next), Esc closes.
- **The projects table** is one tab stop: Up / Down choose the row (a 2 px `focus_border` bar at its left while
  the table has the keyboard), Enter opens it, Space ticks, Right / Left expand, F5 starts, Shift+F10 its menu. The
  details take the keyboard as they open; Esc goes back to the same row.
- **The terminal** keeps Tab and Shift+Tab for the shell; Ctrl+J (hide the panel) leaves it.
- **Each OS's keys** (`ui::keys::Os`): the app's shortcuts are bound as `secondary-…` - Ctrl, ⌘ on macOS - except
  Ctrl+Tab and Ctrl+\` (⌘Tab and ⌘\` are macOS's); a default in `commands.rs` is written for Windows and
  `Shortcut::default_on` gives the OS its own. macOS writes shortcuts with its signs, ⌃⌥⇧⌘ in that order (`⇧⌘P`,
  `⌘⌫`); Windows `Ctrl+Shift+P`; Linux the same with `Super` for the Windows key. A tooltip gets its shortcut from
  the command (`shortcuts::with_shortcut`), so it shows the user's own. Never write `Ctrl` into a label.
- **Text fields** (`ui::text_input`, its editing in `ui::text_edit`): each OS's own keys (`text_input::keys(os)`) -
  copy / paste / cut / select all / undo / redo with the primary modifier; words with Ctrl+arrows, or ⌥+arrows on
  macOS (Ctrl+→ goes to the next word's start on Windows, to the word's end on macOS and Linux, as their own fields
  do); the ends with Home / End, also ⌘← / ⌘→ and ⌃A / ⌃E on macOS. Up and Down are left to the list a field
  filters. A run of typing is one undo step (a space starts the next); a paste is its own. Double-click selects a
  word, triple-click the line, a drag goes on past the field; a right-click opens Undo / Redo / Cut / Copy / Paste /
  Select all with their keys. A text wider than the field scrolls to keep the caret in sight.

## Icons

Every icon is an `Icon` (`ui::icons`): one enum, its path data in one place - change a path in `Icon::shape` and
it changes everywhere. Draw one with `ui::icon(Icon::Delete, colour, cx)` or `ui::icon_button(id, Icon::Delete, …)`:
no size and no pixels at the call. A role other than the standard one is an `IconSize`, through `ui::icon_sized` /
`ui::icon_button_sized`; never a glyph, a literal path or a number.

| `IconSize` | Pixels | For |
|---|---|---|
| `Medium` (the default) | 13 | Every action and state icon: icon buttons (28 × 28), buttons with a label, the title and status bars, toasts, errors, check lists, tips, the panel and find bar |
| `Small` | 10 | Chevrons, check marks, sort arrows, the window's buttons, the Output pill |
| `Nav` | 16 | The activity bar, Settings' categories, the title bar's Help (?) beside the 16 px layout icons |
| `Large` | 26 | A prompt's own icon |

`icons::SIZE*` hold the same pixels, for laying out around an icon (a box, an offset) - not for drawing one.
`LABEL_GAP` (8) is the space between an icon and its label.

- **Every icon the same size to the eye.** An icon's drawing is measured (its arcs' bulges included) and scaled into the
  middle of the grid until its longer side is its `Optical` extent: `Round` 13 of 16 (circles, rings, the triangle),
  `Box` 12 (squares, folders, most things), `Mark` 10 (✕, +, ✓, chevrons, arrows, the dash, …). The same width looks
  bigger as a square than as a circle, and bigger still as a bare line - so they get different room and come out
  even. A path can be drawn at any scale in `Icon::shape`; give a new icon its class in `Icon::optical`.
- **Stroked 1 px, on the pixel grid.** The paths are drawn on a 16 × 16 grid; the asset source (`icons::asset`, path
  `icon/<name>/<px>.svg`) redraws them for the size they're shown at, the UI scale included (`icons::drawn_size`),
  with a 1 px stroke and every point of a line moved to the middle of a pixel (`fit`), so a line covers one row of
  pixels instead of two half-bright ones. Arcs keep meeting their snapped ends; the few solid dots (ⓘ, …, the list's
  bullets) are fills, left soft. Crisp at 100 % display scaling; at 125 % / 150 % GPUI scales the whole window and
  lines land between pixels again.
- Small sizes get simple drawings: square corners on the caption buttons (a 1 px rounded corner at 10 px is a smudge),
  a bin with straight sides, a cog with six teeth, not eight. The cog isn't snapped (`Icon::snaps`): it is all short
  diagonals, and snapped its teeth come out uneven. Check a new or changed icon at all four sizes - a contact sheet of `Icon::ALL` rendered
  from `icons::asset` shows them side by side.
- An svg takes no colour from its parent: an icon whose colour changes on hover sets it on the svg and uses
  `group_hover` (`ui::info_tip`, the password field's eye).
- `ui::icon` puts an icon in a box 2 px larger than it, centred; rows centre it with `items_center`.
- An icon in front of text that may wrap uses `ui::line_icon` (`items_start` row): its box is one line of that text
  high, so it centres on the first line.
- Measured in the running app (bounding boxes of the drawn pixels): icons and their text share a centre line within
  half a pixel. One place needed 1 px to get there, commented: the palette's check marks (`top(-1)`).
- The terminal crate doesn't depend on dnnm-ui and repeats the sizes (`ICON_SIZE`, `ICON_SIZE_SMALL`); the app hands
  it the `Icon` paths (`PanelIcons`), so its icons are the same drawings.
- `ui::shapes` holds the layout icons (16 px, stroked 1 px) and the spinner: the layout icons (filled while
  shown or chosen - Panel Alignment's always, the panel's place being the picture; the activity bar's icon
  `activity-bar-left/right`, the side bars' `sidebar-left/right`, the menu bar's VS Code's `layout-menubar`; Customize Layout puts one in front of each option and the tick after its title), the spinner
  (`ui::spinner`, turning once a second with animations on). `ui::shape_icon` puts one in an icon's box.

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
- Screen readers: GPUI 0.2.2 has no accessibility tree, so the keyboard works but nothing is announced.
- UI scale (Settings → General) is saved but not applied; GPUI follows the display's scaling.
