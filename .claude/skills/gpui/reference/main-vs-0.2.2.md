# GPUI: Zed `main` vs the pinned crates.io 0.2.2

Surveyed 2026-11-17. "Local" = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2/`.
Every "absent in 0.2.2" below was checked with grep on the local copy unless marked UNVERIFIED.
Main-branch facts come from raw.githubusercontent.com / the GitHub API on that date; main moves daily.

## 1. Versions

- crates.io `gpui` max_version is **0.2.2, published 2025-10-22**. Earlier: 0.2.1 (2025-10-14), 0.2.0
  (2025-10-09); 0.1.0 (2022) and the `-test` pre-releases are yanked.
  Source: https://crates.io/api/v1/crates/gpui
- docs.rs shows 0.2.2 as latest; it builds docs only for `x86_64-unknown-linux-gnu`, so Windows-only items
  (`platform::windows`) are not on docs.rs. https://docs.rs/crate/gpui/latest
- 0.2.2 is a snapshot of Zed's monorepo from Oct 2025, published together with satellite crates under
  `gpui_*`/`gpui-macros` names 0.2.2 (local registry holds gpui_collections, gpui_refineable, gpui_util,
  gpui_sum_tree, gpui_http_client, gpui-macros ... all 0.2.2).
- **Main is not published.** `crates/gpui/Cargo.toml` on main still says `version = "0.2.2"`,
  `publish = true`, yet its code is ~13 months ahead. There is no tag or changelog tying main to a release;
  the version field is simply not bumped between ad-hoc publishes.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/Cargo.toml
- Consequence: code copied from Zed main (or main's docs/examples) often does not compile against 0.2.2.
  Use docs.rs/gpui/0.2.2 and the local source as the truth; use main only for ideas.

### The big structural change on main: platform split
- Main has separate crates `gpui_platform`, `gpui_windows`, `gpui_macos`, `gpui_linux`, `gpui_wgpu`,
  `gpui_web`, `gpui_ios`, `gpui_apple`, `gpui_shared_string`, `gpui_util`, `gpui_tokio`.
  https://api.github.com/repos/zed-industries/zed/contents/crates?ref=main
- `crates/gpui/src/platform/` on main has no `windows/`, `mac/`, `linux/`, `blade/` dirs any more (only
  app_menu, keyboard, keystroke, layer_shell, platform_info, popup, test, visual_test ...).
  https://api.github.com/repos/zed-industries/zed/contents/crates/gpui/src/platform?ref=main
  Local 0.2.2 still has `src/platform/{windows,mac,linux,blade}` in-tree (DirectX renderer at
  `src/platform/windows/directx_renderer.rs`).
- Entry point on main: `gpui_platform::application().run(|cx: &mut App| ...)`; `Application::new()` is gone,
  replaced by `Application::with_platform(Rc<dyn Platform>)`.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs ,
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/app.rs
  **In 0.2.2 use `Application::new()` / `Application::headless()`** (local `src/app.rs:132,146`).
- `gpui_platform` 0.1.0 deps per target: Windows -> `gpui_windows` (+ gpui `windows-manifest`), macOS ->
  `gpui_macos`, Linux -> `gpui_linux` (+ optional `gpui_wgpu`), wasm -> `gpui_web`.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml
- GPUI runs in the browser on main ("GPUI on the web (#50228)", 2026-02-26) and on iOS
  ("gpui_ios: Add iOS platform backend (#63068)", 2026-10-14).
  https://api.github.com/repos/zed-industries/zed/commits?path=crates/gpui/src/elements/text.rs

## 2. README and docs on main (summary)

README (https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/README.md):
- Hybrid immediate/retained-mode, GPU-accelerated; pre-1.0, expect breaking changes; needs latest stable Rust.
- Getting started: add `gpui` (+ `gpui_platform` on main), build an `Application`, `run`, then
  `App::open_window` with a root view. 0.2.2's README says `Application::new()` and "macOS or Linux"
  (local README.md) even though Windows works in 0.2.2.
- Platform notes (main): Windows needs no features (Win32 + DirectWrite; `font-kit` has no effect);
  Linux needs `wayland` and/or `x11`; macOS uses Metal and needs Xcode.
- Three "registers": **entities** (state, owned by App, Rc-like handles), **views** (entities that
  implement `Render`, re-rendered each frame into an element tree styled Tailwind-like, `div` as the
  workhorse), **elements** (low-level imperative layout/paint for lists, editors).
- Other services: actions (keystroke -> logical op), platform services on `App` (quit, open URL), an
  executor bound to the platform loop, and `#[gpui::test]` + `TestAppContext` for simulated input.

docs/ on main = `contexts.md`, `key_dispatch.md` (same two files as local 0.2.2 `docs/`).
https://api.github.com/repos/zed-industries/zed/contents/crates/gpui/docs?ref=main
- contexts.md: `App` is the root context and owns entity data; `Context<T>` is tied to one entity
  (notify, emit) and derefs to `App`; `AsyncApp`/`AsyncWindowContext` via `to_async` survive `.await` and
  make entity access fallible; `TestAppContext` panics on missing app/window. `Window` is not a context:
  pass `&mut App` alongside. `Entity<T>` with `T: Render` is a view. (local docs/contexts.md)
- key_dispatch.md: `actions!(ns, [A, B])` for unit actions; `.key_context("menu")` marks a subtree keymap
  contexts can target; `.on_action(cx.listener(...))` handles them; keymap JSON maps keys to
  `ns::Action`, with args in an array. https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/docs/key_dispatch.md
  **Pitfall:** both main's and 0.2.2's key_dispatch.md show a `#[gpui::action]` attribute, but
  gpui-macros 0.2.2 exports no such attribute macro - only `#[derive(Action)]`, `register_action!`,
  `#[derive(IntoElement)]`, `Render`, `AppContext`, `VisualContext`, `#[gpui::test]` (local
  `gpui-macros-0.2.2/src/gpui_macros.rs`). In 0.2.2 write `#[derive(Clone, PartialEq, Deserialize,
  JsonSchema, Action)]` with `#[action(namespace = ...)]` (doc example at local `src/action.rs:61`).
- `_ownership_and_data_flow.rs` (rustdoc module, also in 0.2.2): App owns all entities; `cx.new(..)`
  returns `Entity<T>`, an inert typed handle, Rc-counted; access via `entity.update(cx, |s, cx| ..)` /
  `entity.read(cx)`; `cx.notify()` signals change; `cx.observe(&e, ..)` and typed events via
  `EventEmitter<E>` + `cx.emit` + `cx.subscribe(&e, ..)` both return a `Subscription` you store or
  `.detach()`. Main's version starts from `gpui_platform::application()`; 0.2.2's from
  `Application::new()`. https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/_ownership_and_data_flow.rs
- Main adds `src/_accessibility.rs` (a11y doc module) - not in 0.2.2.
  https://api.github.com/repos/zed-industries/zed/contents/crates/gpui/src?ref=main

## 3. API differences a 0.2.2 user must know

Legend: [main only] = exists on main, grep finds nothing in local 0.2.2.

### App / Application (src/app.rs)
- [main only] `QuitMode` enum (`Default`, `LastWindowClosed`, `Explicit`), `Application::with_quit_mode`,
  `App::set_quit_mode`. In 0.2.2 quit on last window with `cx.on_window_closed(|cx| ..)` + `cx.quit()`
  (local `src/app.rs:1806`; example `examples/on_window_close_quit.rs`).
- [main only] `Application::with_platform`, `new_inaccessible`, `with_windowing`, `run_embedded`,
  `with_restart_arguments`; `App::thermal_state`/`on_thermal_state_change`;
  `App::set_cursor_hide_mode`/`cursor_hide_mode`/`is_cursor_visible`.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/app.rs
- Same in both: `with_assets`, `with_http_client`, `on_open_urls`, `on_reopen`, `keyboard_layout`,
  `on_keyboard_layout_change`, `refresh_windows`, `observe_keystrokes`, `set_prompt_builder`.

### Window / focus (src/window.rs)
- **Breaking:** main `FocusHandle::focus(&self, window: &mut Window, cx: &mut App)`; 0.2.2 is
  `focus(&self, window: &mut Window)` (local `src/window.rs:340`). 0.2.2 also has `window.focus(&handle)`
  (local `src/window.rs:1386`).
- 0.2.2 has `window.use_state` / `window.use_keyed_state` (local `src/window.rs:2578,2612`); the fetched
  first third of main's window.rs did not show them - UNVERIFIED whether main still has them.
- [main only] accessibility: `Window` holds an `a11y` field (AccessKit, "gpui: Accesskit support (#56065)",
  2026-05-27). https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/window.rs

### div / interactivity (src/elements/div.rs)
Main list: https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/elements/div.rs
- [main only] `focus_visible(..)` style (keyboard-only focus ring) - in 0.2.2 use `.focus(..)` (local
  div.rs:1020) or track keyboard-vs-mouse yourself.
- [main only] `on_aux_click`, `on_mouse_exit`, `on_file_drop_exit`, `on_mouse_pressure`, `on_pinch`,
  `capture_pinch`, `on_back_gesture`, `external_drag_payload`, `hover_listener_mode`.
- [main only] `tooltip_show_delay(..)` ("Make tooltip show delay configurable (#58291)", 2026-06-04).
  0.2.2 hard-codes `TOOLTIP_SHOW_DELAY` = 500 ms and `HOVERABLE_TOOLTIP_HIDE_DELAY` = 500 ms (local
  div.rs:48-49). Both have `tooltip(Fn(&mut Window,&mut App)->AnyView)` and `hoverable_tooltip`.
- [main only] ARIA on `StatefulInteractiveElement`: `role`, `aria_label`, `aria_description`,
  `aria_expanded`, `aria_selected`, `aria_value`, `on_a11y_action`, ... - none in 0.2.2 (grep `aria_`,
  `accesskit` = 0 hits), so 0.2.2 has no element-level accessibility API.
- Renamed on main: `ScrollWheelEvent` -> `ScrollEvent`, `on_scroll_wheel` deprecated for `on_scroll`
  (#65805, 2026-10-20). **0.2.2: use `on_scroll_wheel` + `ScrollWheelEvent`** (local
  `src/interactive.rs:363`, div.rs:312).
- Same in both: `hover`, `group_hover`, `active`, `group_active`, `focus`, `in_focus`, `track_focus`,
  `tab_stop`, `tab_index`, `tab_group`, `key_context`, `on_click`, `on_hover`, `on_drag`, `on_drag_move`,
  `on_drop`, `can_drop`, `drag_over`, `occlude`, `block_mouse_except_scroll`, `window_control_area`,
  `on_mouse_down_out`, `on_mouse_up_out`, `track_scroll`, `anchor_scroll`, `focusable`,
  `restrict_scroll_to_axis`, `debug_selector`.
- `scrollbar_width`: 0.2.2 on `StatefulInteractiveElement` (local div.rs:1071); main moved it to `Styled`.

### Styled (src/styled.rs)
Main: https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/styled.rs
- [main only] `text_ellipsis_start` (#45122, 2025-12-19), `text_ellipsis_middle`; `TextOverflow`
  gained `TruncateStart(..)` and `TruncateMiddle(..)`. 0.2.2 `TextOverflow` has only `Truncate(SharedString)`
  (local `src/style.rs:331`) -> end-truncation only; middle/start ellipsis must be done by hand
  (measure with `window.text_system()` and shorten the string yourself).
- [main only] `items_stretch`, `self_start/end/center/baseline/stretch/flex_start/flex_end`,
  `justify_evenly`, `aspect_ratio`, `aspect_square`, `flex_grow_0/1`, `flex_shrink_1`, `font_features`,
  `grid_cols_min_content/max_content`, `grid_rows_min_content/max_content`. (`Style.aspect_ratio` field
  exists in 0.2.2 but has no Styled helper; setting it through `style()` is untested.)
- Same in both: `truncate()` = `overflow_hidden().whitespace_nowrap().text_ellipsis()` (local
  styled.rs:123), `line_clamp`, `text_overflow`, `text_align`, `whitespace_nowrap`, grid basics, `debug`.
- `CursorStyle` in 0.2.2: Arrow, IBeam, Crosshair, ClosedHand, OpenHand, PointingHand, Resize* variants,
  ResizeColumn (+ more after line 60 of the enum; grep local `src/platform.rs`). Main variants UNVERIFIED.

### Elements
- [main only] `src/elements/container_query.rs`; main also has `gestures.rs`, `spring.rs` (spring
  animation), `debug_overlay.rs`, `hot_reload.rs`, `profiler`.
  https://api.github.com/repos/zed-industries/zed/contents/crates/gpui/src/elements?ref=main
- "gpui: Remove hand-written IntoElement impls for elements (#64864)" 2026-11-09 and
  "Impl `Cow<'static, str>` to IntoElement (#48585)" 2026-02-10: on 0.2.2 a `Cow<str>` is not an element;
  convert to `SharedString`. https://api.github.com/repos/zed-industries/zed/commits?path=crates/gpui/src/elements/div.rs

### Windows build (0.2.2 specific)
- 0.2.2 `build.rs` compiles HLSL with `fxc.exe`: `GPUI_FXC_PATH` env var, else PATH, else a hard-coded
  `Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe` (local build.rs:278-352). Main finds the SDK via registry
  ("windows: Automatically find Windows SDK when building gpui (#38711)", 2025-11-04).
  https://api.github.com/search/commits?q=repo:zed-industries/zed+gpui_windows+committer-date:2025-10-22..2026-03-31
- Features in 0.2.2 default: `font-kit`, `wayland`, `x11`, `windows-manifest` (local Cargo.toml:38-43);
  0.2.2 also has `macos-blade`, `runtime_shaders`, `screen-capture`, `inspector`, `test-support`.

## 4. Examples on main not in 0.2.2, and relevant fixes

Main examples dir: https://api.github.com/repos/zed-industries/zed/contents/crates/gpui/examples?ref=main
Descriptions: https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/README.md
New since 0.2.2 (local has 28 examples, none of these):
- `popover` - floating layers via `deferred` + `anchored` (the pattern also works in 0.2.2).
- `focus_visible` - keyboard-only focus styling (API main-only).
- `anchor` - anchored positioning.
- `testing` - `#[gpui::test]`, `TestAppContext`, actions, focus, window tests (pattern valid in 0.2.2).
- `a11y`, `system_notifications`, `move_entity_between_windows`, `window_movable`, `mouse_pressure`,
  `list_example` (bottom-aligned list + scrollbar), `svg_animation`, `active_state_bug`, `layer_shell`,
  `toggle_headless`, `view_example`.
- Run on main: `cargo run -p gpui --example <name>`; main examples import `gpui_platform::application`,
  so for 0.2.2 swap that for `Application::new()`.

Fixes on main after 2025-10-22 (not in 0.2.2) - relevant to Windows, text, tooltips, hover, cursors:
- Text: "Fix `TextLayout::layout` producing invalid text runs (#41224)" 2025-10-26; "Fix truncation
  flickering (#45373)" 2025-12-19; "Fix multiline text truncation (#57450)" 2026-05-21 (i.e. `line_clamp`
  + ellipsis is buggy in 0.2.2 - prefer single-line `truncate()`); "Pixel snapping (#54728)" 2026-04-24;
  "Only take a line wrapper when truncating text (#66720)" 2026-11-09.
  https://api.github.com/repos/zed-industries/zed/commits?path=crates/gpui/src/elements/text.rs
- Tooltips: "Make tooltip show delay configurable (#58291)"; "Long-press to activate tooltips (#63948)"
  2026-09-11; "gpui_windows: Keep child windows above GPUI content (#66368)" 2026-11-02.
- Hover/cursor: "Restore mouse cursor on window deactivation (#55155)" 2026-04-29; "Fix cursor reverting
  when a drag starts (#66540)" 2026-11-04; "Add an option to keep hover listeners active after keyboard
  input (#63614)" 2026-09-03 (suggests hover listeners pause after key input in 0.2.2 - UNVERIFIED); "Repaint cached views when
  inherited opacity changes (#65951)".
  https://api.github.com/search/commits?q=repo:zed-industries/zed+gpui+hover+committer-date:>2025-10-22
- Windows: "Fix GPUI prompts from bleeding clicks into lower windows (#41442)" 2025-10-29; "Fix Alt
  modifier stuck after Alt-Tab on Windows (#52220)" 2026-04-18; "gpui_windows: Fix double borrow crash
  from display change handler (#66855)" 2026-11-12; "Preserve native tab advances (#66468)"; "Composite
  COLR layers on the CPU (#66588)" (colour emoji); "Fix antialiased outline compositing on Windows
  (#66972)" 2026-11-15; render_to_image for headless capture (#63012).
  https://api.github.com/repos/zed-industries/zed/commits?path=crates/gpui_windows
  Search was best-effort: the GitHub search API reported 105-164 hits per query and only the first page
  was read; treat this list as a sample, not complete.

## 5. Zed `ui` crate idioms worth copying (on top of plain gpui)

- **Prelude.** `ui::prelude` re-exports `gpui::prelude::*`, core gpui types (`App`, `Context`, `Window`,
  `SharedString`, `div`, `px`, `rems`, `relative`, `RenderOnce`, `IntoElement`...), the common
  components (Button, IconButton, Label, Icon, Headline), `h_flex`/`v_flex`, spacing/typography helpers
  and theme access (`ActiveTheme`). One `use ui::prelude::*;` per view file.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/prelude.rs
  Copy: give the app's UI crate a `prelude` module with the same shape.
- **h_flex / v_flex.** Free functions returning `Div`: `h_flex()` = flex + flex_row + items_center,
  `v_flex()` = flex + flex_col; marked `#[track_caller]`. https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/components/stack.rs
- **Components = `#[derive(IntoElement)]` struct + `impl RenderOnce`.** Builder methods take and return
  `self`; `render(self, window, cx)` consumes it. Behaviour is factored into small traits shared by many
  components: `Clickable` (on_click, cursor), `Disableable`, `Toggleable` (selected), `FixedWidth`,
  `ButtonCommon` (id, style, size, tooltip, tab_index). `Button` wraps a `ButtonLike` base and delegates.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/components/button/button.rs
  (`#[derive(IntoElement)]` + `RenderOnce` both exist in 0.2.2.)
- **Tooltip.** A `Tooltip` view (`Render`) with title, optional meta line and key binding. Constructors
  return the closure `.tooltip()` wants: `Tooltip::text(t)`, `Tooltip::for_action_title(t, &Action)`,
  `Tooltip::element(f)`; others return `AnyView` (`simple`, `for_action`, `with_meta`). A shared
  `tooltip_container(cx, |div, cx| ..)` applies elevation, UI font, padding - so all tooltips look alike.
  Pattern: `.id(..).tooltip(Tooltip::text("Save"))` - needs a stateful (id'd) element.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/components/tooltip.rs
- **PopoverMenu.** `PopoverMenu::new(id).trigger(button).menu(|window, cx| Some(menu_entity))`, plus
  `anchor(Corner)`, `attach(Corner)`, `offset`, `with_handle(PopoverMenuHandle)` for programmatic
  show/hide/toggle. Layout: `deferred(anchored().snap_to_window_with_margin(..)...)` with an occluding
  div so the menu paints on top and blocks input below; the menu entity must be a `ManagedView`
  (Focusable + `EventEmitter<DismissEvent>`); dismiss = emit `DismissEvent`; outside mouse-down
  dismisses; focus is restored to the previous element on close. `trigger_with_tooltip` hides the
  tooltip while open. https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/components/popover_menu.rs
- **ContextMenu.** `ContextMenu::build(window, cx, |menu, window, cx| menu.header(..).entry(..)
  .separator().action(..).toggleable_entry(..).submenu(..))` returns `Entity<ContextMenu>`; keyboard
  via menu actions `SelectNext/SelectPrevious/SelectFirst/SelectLast/Confirm/Cancel` (+ child/parent
  for submenus); emits `DismissEvent`; `context(focus_handle)` makes action entries show bindings of
  that focus context. https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/components/context_menu.rs
- **visible_on_hover.** Blanket trait on `InteractiveElement + Styled`: element is `invisible()` and a
  `group_hover(group, |s| s.visible())` reveals it - the idiom for row action buttons. Pair with
  `.group("row")` on the parent. Uses only 0.2.2 APIs.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/traits/visible_on_hover.rs
- **Elevation.** `ElevationIndex` (Background, Surface, EditorSurface, ElevatedSurface, ModalSurface)
  maps to background colour + shadow stack; popovers/tooltips use ElevatedSurface, dialogs ModalSurface.
  Centralise surface styling instead of hand-picking bg/shadow per popup.
  https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/src/styles/elevation.rs
- **Component previews.** `RegisterComponent` derive + `Component` trait feed Zed's component gallery
  (`component_preview` crate). Zed-specific; optional to copy.

## Practical rules for a 0.2.2-pinned app

1. Never paste main code without checking each API against local 0.2.2 source; the version number lies.
2. Entry: `Application::new().with_assets(..).run(..)`; quit via `on_window_closed` + `cx.quit()`.
3. Focus: `handle.focus(window)` (no `cx`). Scroll: `on_scroll_wheel`. No `focus_visible`, no ARIA.
4. Truncation: `truncate()` / `text_ellipsis()` end-only; avoid relying on `line_clamp` + ellipsis
   (fixed on main in #57450). Tooltip delay fixed at 500 ms.
5. Copy Zed's `ui` idioms (prelude, h_flex/v_flex, RenderOnce components, Tooltip view, deferred +
   anchored popovers, DismissEvent, visible_on_hover) - they use APIs that exist in 0.2.2.
6. Upgrading off 0.2.2 means a git dependency on Zed (gpui + gpui_platform + gpui_windows ...), not a
   crates.io bump, until Zed publishes again.
