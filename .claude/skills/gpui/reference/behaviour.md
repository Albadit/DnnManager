# GPUI 0.2.2 - behaviour learnt in this project

Each item was checked in GPUI's source (`~/.cargo/registry/src/*/gpui-0.2.2/src/`) or measured in the window.
Re-check when GPUI is upgraded; add new findings here with how they were verified.

## Drawing and redraws

- **`window.refresh()` during drawing is dropped** (`window.rs`: it only acts when `invalidator.not_drawing()`).
  From a `canvas` prepaint or a render, ask for the next frame instead:
  `cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()).ok())` - what `ui::Tooltip` does. (`window.on_next_frame`
  looked unreliable here once, but Windows repaints every vsync and runs next-frame callbacks then - see
  [app-model.md](app-model.md); that test was more likely the screenshot timing. Either works; keep the one in use.)
- **Views render every frame they're drawn** unless `.cached()`; a view's `cx.notify()` redraws the window.
- **Measuring an element:** a child `canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
  .absolute().inset_0()` records its window bounds each frame (`ui::Anchor` does this). Absolute children are placed
  against their parent in Taffy - no `relative()` needed for that.

## Animation

- **`with_animation` runs from the first frame the element (by its id) is drawn** and keeps its start while the element
  stays on screen (`elements/animation.rs`, element state). A new id restarts it - key the id on the state animated
  to (`NamedInteger("switch-slide", on)`) so a flip back starts a new one.
- **Animate a change, not an appearance:** remember what was shown last with `window.use_keyed_state(id, cx, ..)` (kept
  while drawn frame after frame) and animate only when it differs - else a switch slides every time its page opens.
- **Duration and easing decide whether it is seen**: 150 ms with a cubic ease-out did 80 % of a switch's move in the
  first ~50 ms and read as a jump (frames captured every ~33 ms). The project uses `ui::MOTION` (200 ms) with
  `ui::motion_easing` (GPUI's `ease_in_out`) - use those, and check new motion with a burst of captures.
- **Frames keep coming while animating**: Windows' backend redraws on every vsync (`platform/windows/platform.rs`,
  VSyncProvider) and an unfinished animation asks for the next frame.

## Things this app had wrong (found by the GPUI review)

- **A `cx.notify()` / `window.refresh()` from a `canvas` measuring during the draw is dropped** - the Projects table's
  width, the tour card's height and a select list's height waited for some other redraw. Now `cx.defer` after the
  frame (`behaviour` above). Check any new measuring canvas for this.
- **Subscriptions kept on `Workspace._subscriptions` for a page that closes pile up**, and an `observe` of a long-lived
  entity (the store) runs on after the page is gone - an observer ends only when it or the *observed* entity is
  dropped. Keep a page's subscriptions with the page (`Modal::Settings(page, subs)`).
- **A key binding takes the key before `on_key_down`**: Settings' Ctrl+F never reached its key handler - the global
  `FindInPanel` binding took it. Handle it as the action and route it to what is in front.
- **Disk calls in `render`** (a log file's size) run every frame - read them with the data, off the UI thread.

## Layering and popups

- **`deferred(...)` draws after everything else, by priority** (`with_priority(n)`; higher on top). This project:
  popups 2; a context menu 3 over its outside-press catcher at 2.
- **`anchored()` always moves its content inside the window**, whatever the fit mode (`elements/anchored.rs`: the
  snap to the window's edges runs after the anchor switch). `AnchoredPositionMode::Window` puts it at a window
  position whatever its parent.
- **`on_mouse_down_out`** fires for a press anywhere outside the element - including on a submenu drawn outside a
  menu's bounds and on the button that opened it. Exclude those, or use a full-window catcher under the popup.
- **`occlude()`** stops presses and hover reaching what is under the element. A catcher layer that only closes a
  popup should occlude, so the press does nothing else.

## Hover, tooltips, cursors

- **One `group_hover` per element**: a second call replaces the first - set colour and everything else in one call.
- **`hover` / `group_hover` / `active` can't change size or layout**: they are applied at paint time only - during
  layout and prepaint there is no hitbox to test (`elements/div.rs` ~1615, 1655, 1797). Colours, borders, opacity
  work; `w`, `h`, padding don't (a 1 px line given `w(4)` on hover stayed 1 px - measured). Give the element its full
  size always and change only its colour. Calling `hover`, `tooltip`, `on_drag` or `on_hover` twice panics in debug.
- **A tooltip opens at the mouse** (`window.rs::prepaint_tooltip`: origin = mouse + 1). To line it up with its
  element, the tooltip view must know the element's bounds - `ui::Tip` measures the element and the tooltip view
  places itself with `anchored().position_mode(Window)`, after one frame to learn its own size.
- **Windows cursors**: GPUI's Windows backend maps only arrow, I-beam, crosshair, hand, resize left-right and
  up-down, and not-allowed (`platform/windows/util.rs`); anything else shows the arrow. The four-way Move pointer
  needs the window subclass in `dnnm_system::move_pointer` (answers `WM_SETCURSOR`).

## Closing, the tray, quitting

- **Closing the last window quits the app** on Windows whatever the app does in `on_window_closed`
  (`platform/windows/platform.rs` ~716: `PostQuitMessage` once no window is left). "Keep running when closed" works
  only because `Workspace` answers `on_window_should_close` with false and hides the window itself
  (`crates/tray`, `ShowWindow(SW_HIDE)`) - keep it that way; GPUI has no tray API and `cx.hide()` does nothing on
  Windows. Details in [windows-platform.md](windows-platform.md).

## Layout

- **Text in a flex row doesn't wrap** unless its box may shrink: `.min_w_0()` (or `flex_1().min_w_0()`), or a
  width. Without it a long path pushes the dialog or toast wider.
- **`truncate()` / `text_ellipsis()` on text that doesn't wrap only clips it in a shrinking flex item**
  (`elements/text.rs`: for non-wrapping text the first measured size is kept, and that first measure is at full
  width). Clamp wrapping text to one line instead - `line_clamp(1).text_ellipsis()` is measured again whenever its
  width changes; `ui::one_line` does this. Checked in the Output's stage list at 150 px: "bondforwe…", and again after
  dragging it wider. An absolutely placed line was worse (measured before its box had a width: only "…").
- **A plain `div()` around a flex child may leave it at its content width**: the Output's summary rows shrank to
  "Warnings 0" inside `div().flex_none().pad(..)`; making that wrapper `flex().flex_col()` stretched them again.
- **No text baselines**: GPUI's text doesn't report one to Taffy, so `items_baseline()` doesn't line text up. An
  icon beside text is centred on the line box - which is the capitals' middle - and lowered by
  `ui::info_tip_drop` to sit in the letters' visual middle.
- **Crisp 1 px lines in an SVG**: put the path on half pixels (`M1.5,…`) with `stroke-width="1"` at 1:1 size; on
  whole pixels or a 1.3 stroke the line is smeared over two pixels.
  An SVG drawn smaller than its grid (16 grid at 13 px) loses that: `ui::icons` redraws each icon in the pixels of
  its size and snaps the points (`fit`). GPUI rasterises an svg at 2x (`SMOOTH_SVG_SCALE_FACTOR`, svg_renderer.rs:9)
  and places it at its layout box (window.rs `paint_svg`); checked by screenshots at 100 % display scaling.
- **An svg doesn't take its parent's text colour**: `Svg::paint` reads only its own `style.text.color`
  (elements/svg.rs:110) - without one it draws nothing. For a hover colour, set it on the svg with `group_hover`.
- **The title bar drag**: an element with `window_control_area(Drag)` must `occlude()`, and buttons inside it
  occlude too - else the root's focus handling swallows the drag, or a click becomes a drag.

### Select keys and a draggable scroll bar (measured 2026-11-18)

- `dialog::select`'s keys are actions in a "Select" key context on the box (`track_focus`). Deeper than the dialogs' "Modal", so its `escape` wins; with the list shut the handler calls `cx.propagate()` and the keymap tries the next binding (Modal's CloseView) - window.rs:3834-3846. Checked in the window: Esc shut the list and left the page; Down / Page Down / Home / Enter moved and chose as expected.
- `ui::scrollbar` is a `canvas` that inserts a `BlockMouseExceptScroll` hitbox in prepaint (only while there is something to scroll) and registers window mouse listeners in paint; the drag (one at a time) lives in a thread-local and moves the `ScrollHandle` with `set_offset`. Checked: a 150 px drag scrolled the version list ~55 rows, the list stayed open and no row was chosen.
- A view's own "pressed elsewhere" close must not run on a press on the overlay: `on_mouse_down_out` is capture and geometric, so the select now does the closing itself and checks the list's measured bounds.

### Keyboard navigation (measured 2026-11-20, in the window with the stand-in IIS)

- **No Tab by itself**: 0.2.2 has tab stops (`tab_index`, div.rs:637), `focus_next` / `focus_prev` (window.rs:1413)
  and clicks a focused element with `on_click` when Enter or Space comes *up* (div.rs:2183) - nothing binds Tab.
  `ui::focus` binds it and the workspace handles it. A binding wins over `on_key_down`, so the terminal takes Tab back
  with `NoAction` in its context (keymap.rs:173: a `NoAction` stops shallower bindings).
- **A shadow with no blur draws nothing on Windows**: the shadow shader integrates over `blur_radius`
  (shaders.hlsl, `shadow_fragment`), so a crisp "outline" shadow (blur 0, spread 2) is invisible. The focus ring is
  the border instead.
- **A mouse press focuses a focusable element** (div.rs:2025) unless an earlier listener `prevent_default`s - and
  the element's own `on_mouse_down` runs first in the bubble phase. `ui::focus` does that, so clicks don't move the
  focus (the projects table keeps its arrows).
- **A removed element's focus handle lives a frame longer**: `window.focused()` still returns it, but
  `root.contains_focused()` is false - test that to see the focus was left on nothing.
- **Two draws can happen before an `on_next_frame` callback runs**: comparing "this frame" with "the frame before"
  missed a menu that closed; compare with what the last check saw.
- `window.use_keyed_state(id, cx, |_, cx| cx.focus_handle())` gives a free-function element (a menu) a focus handle
  that lives while it is drawn and is new each time it opens.
