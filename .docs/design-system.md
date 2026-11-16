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

## Spacing

- **No margins, no per-side padding.** `.m*`, `.mt/.mb/.ml/.mr/.mx/.my`, `*_auto` margins and `.pt/.pb/.pl/.pr/.px/.py`
  aren't used.
- **Between elements: flex and gap.** A parent that lays out children is `.flex()` / `.flex_col()` with `.gap(..)`;
  siblings spaced differently are grouped into nested flex wrappers, each with its own gap. Centring is
  `justify_center` / `items_center`; pushing to the end is `justify_between` or a `flex_1()` sibling.
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
| Title bar / status bar height | 30 / 24 | same |
| Sidebar width / icons-only width | 230 / 48 | 190 / 40 |
| Gap around the cards (sidebar, page, panel) | 5, rounded 6, bordered | 0, flush, divided by lines |
| Panel height | 230 | same |
| Control height, search height | 30, 26 | same |
| Radius: controls, check boxes, cards | 4, 3, 6 | same |
| Table row height | 38 | same |

The sidebar shows only its icons below a window width of 1,100.

## Controls

| Control | Spec | Rust |
|---|---|---|
| Button | 30 high, padding 14 × 6, radius 4; Secondary: `card_bg` / `card_border`, `hover_bg` on hover, 50 % when disabled; Primary: `accent`, semibold, `accent_hover`, 45 % disabled | `ui::button` |
| Icon button | 28 × 28, transparent, `hover_bg`, 35 % disabled | `ui::icon_button` |
| Link | `link_fg`, underlined on hover | `ui::link` |
| Toggle switch | track 32 × 18 radius 9 (`input_border` off, `accent` on), knob 12 `on_accent`, label 8 to the right | `ui::switch` |
| Table check box | 16, radius 3, `accent` filled with a check when ticked | `ui::check_box` |
| Sidebar entry | 14 px text, margin 8 × 1, padding 6/9/8/9, radius 6, a 16 px glyph in a 30 px slot, label 6 to its right | `Workspace::render_sidebar` |
| Panel tab | 12 px, padding 8 × 3, radius 4, `sidebar_selected` when chosen | `Workspace::render_panel` |
| Table header | `header_bg`, `text_muted` semibold, padding 8 × 6, line under it | `ProjectsPage` |
| Table cell | padding 8 × 0; a column as wide as its widest text + 16, at least its minimum | `Widths::measure` |
| Window buttons | 46 wide; close turns `#C42B1C` with white glyph on hover | `Workspace::render_title_bar` |
| Command center | 22 high, radius 6, `command_center_*` colours, 38 % of the window, at most 600 | `Workspace::render_title_bar` |
| Layout icons, Columns icon | Path data, stroked 1 / 1.5 px, filled while the part is shown | `ui::icons::layout_icon`, `columns_icon` |

## Not there yet

- Animations: a pulsing dot while a site changes, an indeterminate progress bar, popup fades.
- Drop shadows are GPUI's presets (`shadow_md`, `shadow_lg`, `shadow_xl`).
- Windows high-contrast themes.
- UI scale (Settings → General) is saved but not applied; GPUI follows the display's scaling.
