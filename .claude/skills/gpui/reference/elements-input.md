# GPUI 0.2.2 - elements, interactivity and input

All paths are relative to `gpui-0.2.2/` in the cargo registry. `div.rs` = `src/elements/div.rs`.
Everything below was read from the 0.2.2 source; anything not confirmed is marked UNVERIFIED.

## 1. Elements and ids

### Element lifecycle
- `trait Element: 'static + IntoElement` (src/element.rs:51) has two associated state types,
  `RequestLayoutState` and `PrepaintState` (element.rs:54, 58), and three phases:
  - `request_layout(id, inspector_id, window, cx) -> (LayoutId, RequestLayoutState)` (element.rs:73): asks Taffy for a layout node.
  - `prepaint(id, inspector_id, bounds, &mut request_layout, window, cx) -> PrepaintState` (element.rs:83): bounds are known. Hitboxes, the dispatch-tree node, focus handles and tooltips are registered here.
  - `paint(id, inspector_id, bounds, &mut rl, &mut pp, window, cx)` (element.rs:95): draws, and registers mouse, key and action listeners.
- `Drawable` enforces the order. A second `request_layout` panics with "must call request_layout only once" (element.rs:394), and prepaint without a layout panics (element.rs:443).
- Each element gets a dispatch-tree node during prepaint (`dispatch_tree.push_node()`, element.rs:419). Key contexts and action listeners hang off these nodes.
- Window frame order (window.rs:2036-2082): the root is prepainted, then deferred draws sorted by priority (window.rs:2042-2045), then **one** of prompt / active-drag preview / tooltip (window.rs:2050-2063). The hit test is computed (window.rs:2065), then everything is painted in the same order. While a drag is active, no tooltip is drawn (the `else` at window.rs:2061).

### Conversion traits
- `IntoElement { type Element; fn into_element(self); fn into_any_element(self) }` (element.rs:113-125). Every `IntoElement` also gets `FluentBuilder` (`.when`, `.map`, ...) (element.rs:127).
- `Render` (element.rs:131): `fn render(&mut self, &mut Window, &mut Context<Self>) -> impl IntoElement`. Implemented by views, which live in an `Entity`.
- `RenderOnce` (element.rs:147): `fn render(self, &mut Window, &mut App)`. It takes ownership and has no entity. Use `#[derive(IntoElement)]` to get a `Component<C>` wrapper (element.rs:182).
- `ParentElement::child / children / extend` (element.rs:156-177).
- `AnyElement` (element.rs:574) is a type-erased element in a per-frame arena (`ELEMENT_ARENA`, element.rs:580). Useful methods: `prepaint_at`, `prepaint_as_root`, `layout_as_root`, `downcast_mut` (element.rs:586-655).

### Ids and global ids
- `ElementId` variants (window.rs:4864-4883): `View(EntityId)`, `Integer(u64)`, `Name(SharedString)`, `Uuid`, `FocusHandle(FocusId)`, `NamedInteger(SharedString,u64)`, `Path`, `CodeLocation`, `NamedChild(Box<ElementId>, SharedString)`.
  `From` impls exist for `usize`, `i32`, `&'static str`, `SharedString`, `(&str, usize)`, `(&str, u64)`, `(&str, u32)`, `(SharedString, usize)`, `(&str, EntityId)`, `&FocusHandle`, `(ElementId, impl Into<SharedString>)` and others (window.rs:4922-5000).
- `GlobalElementId` is the whole id stack (`SmallVec<[ElementId; 32]>`, element.rs:276). When an element has an id, `Drawable` pushes it onto `window.element_id_stack` around every phase (element.rs:354-357, 413-416).
- Views push `ElementId::View(entity_id)` (src/view.rs:32-33, 152-153). Ids only have to be unique **within the nearest ancestor that has an id or is a view** (element.rs:63-64).
- `RenderOnce` components push `ElementId::Name(type_name::<C>())` (element.rs:223, 245, 261). Two sibling instances of the same component therefore share a namespace. Give each instance's inner `.id(...)` a unique value (for example `(name, ix)`), or the two instances share hover, click, scroll and focus state.
- Manual namespacing: `window.with_element_namespace(id, f)` (window.rs:2566) and `window.with_id(id, f)` (window.rs:2602).
- `InteractiveElement::id(id) -> Stateful<Self>` (div.rs:607). `Stateful<E>` (div.rs:2903) forwards all `Element` methods (div.rs:2932-2990) and adds `StatefulInteractiveElement` (div.rs:2916).

### Why stable ids matter: element state
- State is keyed by `(GlobalElementId, TypeId)` (window.rs:670). `with_element_state` looks in `next_frame` first, then takes the state out of `rendered_frame` (window.rs:2643-2647).
- `Frame::finish` only carries over states that were **accessed** this frame (window.rs:805-812). An element that is not rendered for one frame loses its state: scroll offset, focus handle, hover and pressed state, the active tooltip.
- A div without an id gets `None` from `with_optional_element_state` (window.rs:2711-2736), so `InteractiveElementState` does not exist for it. That struct holds `focus_handle`, `clicked_state`, `hover_state`, `pending_mouse_down`, `scroll_offset` and `active_tooltip` (div.rs:2568-2575).
  Without an id there is no click, active style, on_hover, tooltip or retained scroll. That is why those methods are only on `StatefulInteractiveElement` (div.rs:1041-1182).
- An id that changes between frames (for example one that contains an index that shifts) resets all of that state.
- `window.use_keyed_state(key, cx, init) -> Entity<S>` (window.rs:2578) and `use_state` (keyed by the caller's location, window.rs:2612) store an entity in element state. It lives only while it is rendered every frame.

## 2. Cheat sheet (all listeners are `Fn(&E, &mut Window, &mut App) + 'static`; wrap with `cx.listener(...)`, src/app/context.rs:252)

Phase legend: **B** = bubble, **C** = capture. "hovered" means `hitbox.is_hovered(window)`, which respects occlusion.

### InteractiveElement (any `Div`, no id needed) - div.rs:596-1037
| Method | What / phase |
|---|---|
| `group(name)` | names this element's hitbox for `group_*` styles of **descendants** (div.rs:601). Pushed during paint and popped after the children (div.rs:1844, 1876), so only children see it. The innermost group with that name wins (`.last()`, div.rs:2885). |
| `id(id) -> Stateful<Self>` | enables stateful features (div.rs:607) |
| `track_focus(&FocusHandle)` | makes the element focusable, ties the handle to its dispatch node, focuses it on left/any mouse down (div.rs:616, 2025-2037) |
| `tab_stop(bool)`, `tab_index(isize)`, `tab_group()` | tab order (div.rs:628-654) |
| `key_context("Ctx a=b")` | sets the `KeyContext` of this dispatch node (div.rs:658, applied at div.rs:2358). The string is parsed by `KeyContext::parse`; a parse failure is only logged (`log_err`). |
| `hover(f)` / `group_hover(g, f)` | style refinements while hovered (div.rs:670, 680). **Debug builds panic if `hover` is called twice** (`debug_assert!` "hover style already set", div.rs:671-674). |
| `on_mouse_down(button, f)` | B, that button, hovered (div.rs:121-135) |
| `capture_any_mouse_down(f)` / `on_any_mouse_down(f)` | C / B, any button, hovered (div.rs:141-167) |
| `on_mouse_up(button, f)` | B, that button, hovered (div.rs:173) |
| `capture_any_mouse_up` / `on_any_mouse_up` | C / B, any button, hovered (div.rs:193-219) |
| `on_mouse_down_out(f)` | C, **any** button, mouse **outside the bounds** (geometric `!hitbox.contains(pos)`, not occlusion-aware) (div.rs:226-236) |
| `on_mouse_up_out(button, f)` | C, that button, `!hovered` (occlusion-aware, unlike down_out) (div.rs:243-257) |
| `on_mouse_move(f)` | B, hovered only (div.rs:263) |
| `on_drag_move::<T>(f)` | C, fires on **every** mouse move while a drag of type `T` is active, wherever the mouse is. `DragMoveEvent { event, bounds, .. }`, `.drag(cx) -> &T` (div.rs:282-306, 62-85). |
| `on_scroll_wheel(f)` | B, `hitbox.should_handle_scroll` (passes through `block_mouse_except_scroll`) (div.rs:312-322) |
| `capture_action::<A>(f)` | C. In bubble it calls `cx.propagate()` (div.rs:328-343). |
| `on_action::<A>(f)` | B (div.rs:349). Handling an action **stops propagation by default** (window.rs:4056). |
| `on_boxed_action(&dyn Action, f)` | B, dynamic action type (div.rs:367) |
| `on_key_down` / `capture_key_down` | B / C on the focus path (div.rs:387-413) |
| `on_key_up` / `capture_key_up` | B / C (div.rs:419-442) |
| `on_modifiers_changed(f)` | no phase; runs leaf to root on the focus path (div.rs:448, window.rs:3918) |
| `drag_over::<S>(f(style, &S, w, cx))` | style while a drag of `S` hovers this element (div.rs:938, applied at div.rs:2530-2535) |
| `group_drag_over::<S>(g, f)` | same, for a group hitbox (div.rs:957) |
| `on_drop::<T>(f)` | B on mouse up, hovered, drag type matches; then `stop_propagation` + `refresh` (div.rs:976, 2089-2119) |
| `can_drop(pred(&dyn Any, w, cx) -> bool)` | gates both `on_drop` and the drag_over styles (div.rs:986, 2106, 2514-2519) |
| `occlude()` | `HitboxBehavior::BlockMouse`: everything behind gets no hover, moves, clicks or scroll (div.rs:997, window.rs:596) |
| `block_mouse_except_scroll()` | `BlockMouseExceptScroll`: blocks the same things but lets the scroll wheel through (div.rs:1013, window.rs:624) |
| `window_control_area(area)` | platform title-bar / caption hit areas (div.rs:1004) |
| `focus(f)` | style when this handle is focused (div.rs:1020, applied at div.rs:2490) |
| `in_focus(f)` | style when focus is inside this element: `within_focused` (div.rs:1030, applied at div.rs:2484) |
| `debug_selector(f)` | test-support bounds lookup; does nothing otherwise (div.rs:709-721) |

### StatefulInteractiveElement (needs `.id(..)`) - div.rs:1041-1182
| Method | What / phase |
|---|---|
| `focusable()` | creates and keeps its own focus handle in element state (div.rs:1043, 1584-1599) |
| `overflow_scroll / overflow_x_scroll / overflow_y_scroll` | sets `Overflow::Scroll`; the offset lives in element state unless `track_scroll` is used (div.rs:1049-1065, 1601-1613) |
| `scrollbar_width(len)` | reserves layout space for a scrollbar (div.rs:1071) |
| `track_scroll(&ScrollHandle)` | shares the offset, bounds and child bounds with a handle (div.rs:1077) |
| `anchor_scroll(Option<ScrollAnchor>)` | records the element's origin so `ScrollAnchor::scroll_to` can scroll a non-direct child into view (div.rs:1083, 3015-3037, 1368-1370) |
| `active(f)` / `group_active(g, f)` | style while pressed (**any** mouse button down, div.rs:2335-2347; cleared on mouse up, C, div.rs:2323-2327) |
| `on_click(f)` | `ClickEvent`; left button only (div.rs:2139); fires in B on mouse up (div.rs:2235-2245) |
| `on_drag(value, ctor(&T, cursor_offset, w, cx) -> Entity<W: Render>)` | starts a drag after moving more than `DRAG_THRESHOLD` = 2px (div.rs:47, 2159). **Debug panics if called twice** (div.rs:508-511). |
| `on_hover(f(&bool))` | B on mouse move; false while the button is down or a drag is active (div.rs:2261-2276). Debug panics if called twice (div.rs:529). |
| `tooltip(build -> AnyView)` / `hoverable_tooltip(...)` | delayed tooltip (div.rs:1161, 1172). Debug panics if a second tooltip is set (div.rs:542, 561). |

### Cursor
`.cursor(CursorStyle)`, `.cursor_pointer()`, `.cursor_default()`, `.cursor_text()` and the other `cursor_*` methods are generated by the `cursor_style_methods!` macro (gpui-macros-0.2.2/src/styles.rs:159-186, used at src/styled.rs:31). They set `style.mouse_cursor`, and that inserts a hitbox (div.rs:1696).
The frontmost **hovered** hitbox that has a cursor wins: `Frame::cursor_style` walks the requests in reverse (window.rs:762-773). A child without a cursor therefore shows the parent's cursor. During a drag the drag source's cursor is applied to the whole window (div.rs:1834-1837).

### ScrollHandle (div.rs:3068-3249) - `Rc<RefCell<..>>`, clone freely
`new()`, `offset() -> Point` (negative when scrolled down), `max_offset() -> Size`, `bounds()` (viewport), `bounds_for_item(ix) -> Option<Bounds>`, `top_item()`, `bottom_item()`, `children_count()`, `scroll_to_item(ix)` (scrolls the minimum needed to show it), `scroll_to_top_of_item(ix)`, `scroll_to_bottom()`, `set_offset(Point)`, `logical_scroll_top/bottom() -> (ix, Pixels)`.
- Child bounds are the layout bounds of **direct children only**, recorded in `Div::prepaint` (div.rs:1373-1382). `scroll_to_item` is applied during the next prepaint (div.rs:1396-1398, 3162-3200). If the index does not exist yet, the request is kept until it does (div.rs:3197).
- None of the mutators notify the view. Call `cx.notify()` yourself after `scroll_to_item` or `set_offset` (the data_table example does that, examples/data_table.rs:358-359).
- The offset is clamped to `[-max, 0]` on every prepaint (div.rs:1754-1759). `max_offset` and `bounds` are only valid after a frame has been painted.
- `UniformListScrollHandle` is a separate type for `uniform_list` (examples/data_table.rs:256, 441).

## 3. Event dispatch rules

### Mouse
- Hit test: hitboxes are walked **front to back** (in reverse insertion order). Stop at the first `BlockMouse`. `hover_hitbox_count` is cut at the first `BlockMouseExceptScroll` (window.rs:775-797). The hitbox bounds are intersected with the content mask (window.rs:779), so clipped parts of children do not get hits.
- A div only inserts a hitbox if it needs one: a non-normal behavior, a cursor, a group, scroll, focus, hover or group-hover style, any mouse/click/drag/drop listener, or a tooltip (div.rs:1693-1712). A plain styled div is transparent to the mouse.
- Mouse listeners are **not** filtered by the hit test at dispatch time. Every listener of the frame runs, and each one checks `hitbox.is_hovered` itself (for example div.rs:128-131).
  Order: **capture** runs in paint order, back to front (root first) (window.rs:3693-3701). **Bubble** runs in reverse, front to back (window.rs:3703-3711). Children and deferred overlays are painted later, so they bubble first.
- `cx.stop_propagation()` (src/app.rs:1712) stops the remaining listeners. In bubble that means the ones behind or above; in capture it skips the rest of capture **and** all of bubble (window.rs:3698, 3704).
- `window.prevent_default()` (window.rs:1874) does **not** stop propagation. It is a flag that later handlers check with `default_prevented()`. GPUI uses it to stop a parent from stealing focus (div.rs:2030-2035) and to skip setting the active state (div.rs:2336). It is reset for every event (window.rs:3586).
- Clicks: on left mouse down (B, hovered) the event is stored in `pending_mouse_down` (div.rs:2137-2145). On mouse up, in capture: if still hovered the pending event is moved to `captured_mouse_down`, otherwise it is discarded (div.rs:2219-2232). In bubble the click listeners fire with `ClickEvent::Mouse{down, up}` (div.rs:2235-2245).
  So the press and the release must both land on the same hitbox, and the element must have an id. If a drag starts (>2px), the pending press is cleared and no click fires (div.rs:2162, 2172).
- `MouseButton::Right`: `on_click` never fires for it (the left button is checked at div.rs:2139), so `ClickEvent::is_right_click()` (src/interactive.rs:219) is never true from a div. Use `on_mouse_down(MouseButton::Right, ..)` for context menus. `active` styling does react to any button (div.rs:2335).
- Double-click: there is no separate event. `click_count` is set by the platform. On Windows it comes from `GetDoubleClickTime` and `SM_CXDOUBLECLK`/`SM_CYDOUBLECLK` and resets when the button changes (src/platform/windows/window.rs:1025-1081).
  `on_click` fires on **every** release: first with `click_count()==1`, then with `2` (`ClickEvent::click_count` reads `up.click_count`, interactive.rs:256-261). Check `event.click_count() == 2` (or `>= 2`) and make the single-click action safe to run first.
- Keyboard click: when a focused element with `on_click` gets `KeyUp` of `enter` or `space` without modifiers, it fires `ClickEvent::Keyboard` (div.rs:2179-2209). This listener is only registered if the element was focused **at paint time** and has an id. `ClickEvent::position()` is then the bottom-left of the bounds (interactive.rs:197-202).
- Focus on click: `track_focus`/`focusable` elements call `window.focus` on mouse down of any button (B, hovered) unless `default_prevented`, then call `prevent_default` so ancestors do not take focus (div.rs:2025-2037).
  Clicking a non-focusable area does **not** blur. There is no blur listener in div.rs; `window.blur()` is explicit (window.rs:1397). (UNVERIFIED that nothing else in gpui blurs on click.)
- `window.focus(handle)` clears pending multi-key input and refreshes. It does nothing if the handle is already focused or focus is disabled (window.rs:1386-1394).
- When a drag is active, every mouse move refreshes the window and **mouse up ends the drag** whether or not someone dropped it (window.rs:3716-3727). External file drags arrive as an `ExternalPaths` drag; use `on_drop::<ExternalPaths>` (window.rs:3622-3661).

### Keyboard
- Dispatch path = the root-to-focused path in the **rendered** frame's dispatch tree. **If nothing is focused, the path is just the root node** (`focus_node_id_in_rendered_frame` falls back to `root_node_id`, window.rs:3982-3990). So `on_action` and `on_key_down` on inner divs never fire until something inside is focused. Give the window's root view a focus handle and focus it.
- Order for a `KeyDown` (window.rs:3730-3850):
  1. keystroke interceptors (window.rs:3779);
  2. keymap matching → **actions** dispatched (window.rs:3834-3846). If a handler stops propagation (the default for actions), the key event ends here;
  3. otherwise `on_key_down` capture (root→leaf) then bubble (leaf→root) (window.rs:3878-3907);
  4. modifiers-changed listeners, then keystroke observers (window.rs:3852-3870).
- **A key bound to an action never reaches `on_key_down` if a handler handled it.** If no listener exists for the first matched binding, the next binding is tried (the loop at window.rs:3834).
- Windows text input: if the KeyDown was handled (`!propagate`), `TranslateMessage` is skipped and **no WM_CHAR / text input** is produced (src/platform/windows/events.rs:397-406). Calling `cx.stop_propagation()` in `on_key_down` therefore swallows typed characters.
- Multi-stroke bindings (`"ctrl-k ctrl-s"`): pending input waits **1 second**, then the keys are replayed (window.rs:3801-3831). The pending state is dropped if focus changes (window.rs:3786).
- Action dispatch (window.rs:3992-4089): global `cx.on_action` capture → element capture (root→leaf) → element bubble (leaf→root; `propagate_event=false` is set before each handler, window.rs:4056) → global `cx.on_action` bubble (window.rs:4066-4078).
  `cx.propagate()` (app.rs:1720) inside a handler lets the action continue to ancestors. App-level `cx.on_action` only runs if no element handled the action (app.rs:1693-1695).
- `window.dispatch_action(Box<dyn Action>, cx)` is **deferred** (`cx.defer`) and targets the currently focused node (window.rs:1476-1488). `focus_handle.dispatch_action(..)` targets that handle's node (window.rs:367).
- `window.is_action_available(&action, cx)` (window.rs:1884) checks whether some node on the focus path has a listener for it (key_dispatch.rs:381-393).

### Keymap and contexts
- `cx.bind_keys([KeyBinding::new("ctrl-s", Save, Some("Editor"))])` (src/app.rs:1677). `KeyBinding::new` **panics** if the keystrokes or the context fail to parse (src/keymap/binding.rs:33-45). Several keystrokes are separated by whitespace (binding.rs:57-58).
- Actions: `actions!(ns, [A, B])` derives `Clone, PartialEq, Default, Debug, Action` (src/action.rs:24-40). `NoAction` disables a binding (keymap.rs:173-186).
- Precedence (src/keymap.rs:128-206):
  - The **context depth** wins first: a binding that matches a deeper context beats a shallower one.
  - At equal depth, the binding **added later** wins (sort at keymap.rs:165-167).
  - **A binding with no context counts as the deepest depth** (`Some(contexts.len())`, keymap.rs:213). An unscoped `"escape"` binding therefore beats `"escape"` scoped to an ancestor context such as `"Dialog"`, unless Dialog is the deepest context on the path.
- `KeyContext::parse("Editor mode=full")` takes identifiers and `key=value` pairs separated by whitespace (src/keymap/context.rs:60-95). `KeyContext::new_with_defaults()` adds `os=windows` and the like (context.rs:31-47).
- Predicate syntax: `A`, `a == b`, `a != b`, `!A`, `A && B`, `A || B`, `Parent > Child` (context.rs:172-197, 354-358). An identifier is tested against **one** context entry (the deepest in the slice being evaluated) (context.rs:270-275). `"Editor && mode == full"` means both are on the same element's context.
- For display: `window.keystroke_text_for(&action)` gives the highest-precedence binding (window.rs:3566). `window.bindings_for_action(&action)` (window.rs:4223) leaves out shadowed bindings (key_dispatch.rs:395-445).

### Tab navigation
- No Tab handling is built in. Bind `tab`/`shift-tab` to your own actions and call `window.focus_next()` / `focus_prev()` (window.rs:1413-1432; examples/tab_stop.rs:35-40, 138-140).
- `cx.focus_handle()` starts with `tab_stop: false` (window.rs:282-294), so `focus_next` skips it until you call `.tab_stop(true)` on the handle. The div's `.tab_stop()` / `.tab_index()` only apply to the handle the div creates itself through `focusable()`; with `track_focus(&h)` they are **not** copied to `h` (the condition at div.rs:1584-1585). Configure `h` itself (examples/tab_stop.rs:17-21). (This reading of div.rs:1584 is from the source; not checked at runtime.)
- Tab order is sorted by tab-group path, then insertion (paint) order (src/tab_stop.rs:52-57). Tab stops are registered at paint (div.rs:1817-1819), so elements that are not painted are not in the order.

### Text input (EntityInputHandler)
- Implement `EntityInputHandler` (src/input.rs:10-75): `text_for_range`, `selected_text_range`, `marked_text_range`, `unmark_text`, `replace_text_in_range`, `replace_and_mark_text_in_range`, `bounds_for_range`, `character_index_for_point`.
- In a custom element's `paint`, call `window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx)` (window.rs:3400-3414; examples/input.rs:540-542). It is only registered when that handle is focused **at paint time**.
- Editing keys (backspace, arrows, `cmd-a` and so on) are ordinary actions bound under a context (examples/input.rs:569-591, 680-691). Characters arrive through `replace_text_in_range` (Windows: events.rs:428-435).

### Modal pattern (built from the pieces above)
- Overlay: a full-window `div().occlude()` (or `block_mouse_except_scroll()` if the content behind should still scroll), in `deferred(...)` with a high priority so it is painted last and hit first.
- Dialog: `.track_focus(&dialog_focus).key_context("Dialog").on_action(cancel)`. On open, save `window.focused(cx)` (window.rs:1380), then `dialog_focus.focus(window)`. On close, restore the saved handle. Bind `escape` with context `"Dialog"`; a context-free `escape` elsewhere ties at depth and the one added later wins (see Precedence).
- A deferred element keeps its logical dispatch parent (`DeferredDraw.parent_node`, window.rs:658), so actions still bubble from the popup to its owner. (UNVERIFIED: not traced through `prepaint_deferred_draws`.)
- Click outside: `on_mouse_down_out` on the panel. Remember that it is capture-phase and geometric, so it also fires for presses on sibling popups or submenus outside its bounds.

## 4. Gotchas (with sources)
1. **Hover and group_hover cannot change layout.** `compute_style_internal` gets `hitbox: None` during request_layout (div.rs:1615) and prepaint (div.rs:1655); hover is only applied at paint (div.rs:1797, 2497-2511). Size, padding, margin, display and position changes in `hover()` have no effect. Only paint-time properties work: background, border color, text color, opacity, shadow. The same holds for `active`: `clicked_state` is read during layout (div.rs:2543-2558), but layout is computed before the press is registered, so treat it as paint-only too (UNVERIFIED for layout effects of `active`).
2. Hover, group-hover and drag-over styles are ignored while a drag is active (`!cx.has_active_drag()`, div.rs:2498).
3. The hover-change redraw listener is only registered when the element has a hover style, a cursor or a drag-over style (div.rs:2068-2081), or a group_hover style (div.rs:2385-2401). It calls `cx.notify(current_view)`, so a **cached** parent view does not repaint a child's hover unless the notify reaches it. (UNVERIFIED for `AnyView::cached`.)
4. `Visibility::Hidden` (`.invisible()`) returns before **any** listeners are registered: no mouse, key or action handlers, no focus tab stop (div.rs:1809-1811). `Display::None` skips the children's prepaint (div.rs:1409).
5. Without an id: no `on_click`, `active`, `on_hover`, `tooltip`, retained scroll or self-created focus. These methods are only on `StatefulInteractiveElement` (div.rs:1041). Element state is `None` without a global id (window.rs:2729-2735).
6. Element state is dropped after one frame without rendering (window.rs:805-812). Conditionally rendered scroll containers reset to the top. Use a `ScrollHandle` stored in the view to keep the position.
7. One `ScrollHandle` per scroll container. It stores a single bounds/offset/child_bounds set, overwritten on each prepaint (div.rs:1374-1381, 1761-1764). Sharing one handle between two elements makes them overwrite each other.
8. Tooltip delay: `TOOLTIP_SHOW_DELAY` = 500 ms (div.rs:48). Hoverable tooltip hide delay: `HOVERABLE_TOOLTIP_HIDE_DELAY` = 500 ms (div.rs:49). A tooltip does not start while the mouse button is down (div.rs:2298, 2305). Mouse down or scroll hides a non-hoverable tooltip (div.rs:2675-2691). The builder is called once, when the delay ends (div.rs:2757-2760).
   Only one tooltip is drawn: the last requested (window.rs:2090). It goes at mouse + (1,1) and flips left or up at the window edge (window.rs:2104-2133).
9. Drag preview: `ctor` gets `cursor_offset = mouse - source_origin` (div.rs:2163). The preview is laid out with min-size and drawn at `mouse - cursor_offset` (window.rs:2055-2058), so its top-left follows the source's grab point. It is drawn **above everything**, including deferred elements. No tooltip is shown while dragging (window.rs:2061).
10. `on_drop` needs the exact `TypeId` of `T` (div.rs:2097-2099). A drop handler without a matching type does nothing; the drag still ends on mouse up (window.rs:3721-3725).
11. `on_mouse_down_out` is geometric (`hitbox.contains`, div.rs:232), not occlusion-aware, and fires for **every** button, including right-click and middle-click.
12. Actions stop at the first handler. A parent's `on_action` for the same action never runs unless the child calls `cx.propagate()` (window.rs:4056-4060).
13. If nothing is focused, only root-node listeners get keys and actions (window.rs:3989). Focus something on window open.
14. Focus-based styles and `handle_input` read the focus at paint time. After `focus()`, GPUI refreshes (window.rs:1393), so the next frame is correct; the current frame is not.
15. `capture_action` handlers do not stop propagation by default (unlike bubble). Call `cx.stop_propagation()` to swallow the action (div.rs:336-341; window.rs:4037-4041).
16. Scroll wheel: `on_scroll_wheel` and the built-in overflow scroll use `should_handle_scroll`, which is blocked by `occlude()` but **not** by `block_mouse_except_scroll()` (window.rs:514-516, 783-789). Built-in scroll notifies the view only when the offset changes (div.rs:2448-2450). Pixel delta uses `window.line_height()` for line-based wheels (div.rs:2414, 2421).
17. `MouseMoveEvent::dragging()` is true only for the **left** button (interactive.rs:356-358).
18. `ClickEvent::modifiers()` comes from the **mouse-up** event (interactive.rs:189). `first_focus()` reports `down.first_mouse` (interactive.rs:245-250).
19. Keyboard click (Enter/Space) fires on **KeyUp** (div.rs:2184). An `enter` bound to an action (for example a dialog's Confirm) runs on KeyDown **and** the focused button's `on_click` runs on KeyUp. Both happen. The KeyUp path checks `default_prevented` (div.rs:2185), but that flag is reset for every event (window.rs:3586). A `prevent_default` during KeyDown does not carry over; it would have to be a KeyUp handler that runs earlier, for example `capture_key_up`.
20. `KeyBinding::new` panics on a bad context string. `.key_context("...")` on an element only logs the error and drops the context (div.rs:663).

## 5. Patterns (assembled from the APIs above; signatures checked, snippets not compiled)

### Row with click, double-click, right-click and hover
```rust
div()
    .id(("row", ix))                       // stable, unique among siblings (section 1)
    .hover(|s| s.bg(theme.row_hover))      // paint-only properties (gotcha 1)
    .cursor_pointer()
    .on_click(cx.listener(move |this, ev: &ClickEvent, _w, cx| {
        if ev.click_count() >= 2 { this.open(ix, cx) } else { this.select(ix, cx) }
    }))                                    // fires for click 1 AND click 2 (section 3)
    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
        this.open_context_menu(ix, ev.position, cx);
        cx.stop_propagation();             // the row's parents don't see this right-press
    }))
```

### Keyboard root: make actions reachable
```rust
struct Root { focus: FocusHandle }
// in the window-open closure: let root = cx.new(|cx| Root { focus: cx.focus_handle() });
//                             root.read(cx).focus.focus(window);     // gotcha 13
impl Render for Root {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .key_context("Root")
            .on_action(cx.listener(|this, _: &Refresh, _w, cx| this.refresh(cx)))
            .child(/* ... */)
    }
}
// cx.bind_keys([KeyBinding::new("f5", Refresh, Some("Root"))]);
```

### Scrollable list that keeps its position and can reveal an item
```rust
// field: scroll: ScrollHandle  (kept in the view, not created in render)
div()
    .id("list")
    .overflow_y_scroll()
    .track_scroll(&self.scroll)            // one handle per container (gotcha 7)
    .children(items)                       // direct children = indexes for scroll_to_item
// later: self.scroll.scroll_to_item(ix); cx.notify();   // the handle does not notify
```

### Drag and drop between rows (as in examples/drag_drop.rs:88-116)
```rust
#[derive(Clone)] struct DraggedRow(usize);
impl Render for DraggedRow { fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div().child("...") } }

div().id(("row", ix))
    .on_drag(DraggedRow(ix), |d, _offset, _w, cx| cx.new(|_| d.clone()))
    .drag_over::<DraggedRow>(|s, _d, _w, _cx| s.bg(theme.drop_target))
    .on_drop(cx.listener(move |this, d: &DraggedRow, _w, cx| this.move_row(d.0, ix, cx)))
```

### Popup closed by an outside click or Escape
```rust
deferred(
    anchored().child(
        div()
            .occlude()                                       // nothing behind is hovered or clicked
            .track_focus(&self.menu_focus)
            .key_context("Menu")
            .on_action(cx.listener(|this, _: &Dismiss, w, cx| this.close(w, cx)))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, w, cx| this.close(w, cx)))
            .child(/* items */),
    ),
)
.with_priority(1)
// close(): restore the focus saved on open, then cx.notify()
// bind: KeyBinding::new("escape", Dismiss, Some("Menu"))
```
Note: `anchored()` and `deferred(..).with_priority(..)` are covered in another note; they are not described here.

## 6. Quick answers
- "Why doesn't my `on_click` fire?" Check for a missing `.id()`, a press that started on another element, the press turning into a drag (>2px with `on_drag`), a right or middle button, or an element in front that is `occlude`d.
- "Why doesn't my key handler fire?" Nothing on the path is focused; a binding consumed the key as an action; or the element is `invisible()`.
- "Why does my hover make the layout jump or do nothing?" Hover can't change layout (gotcha 1); use view state set by `on_hover` and re-render instead.
- "Why does the scroll reset?" The id changed, or the element was not rendered for a frame; keep a `ScrollHandle` in the view.
- "Why does Escape go to the wrong handler?" Check context depth, then the order in which the bindings were added. A binding without a context counts as the deepest depth.
