# GPUI 0.2.2 - the application model

Source: crates.io `gpui-0.2.2`. All paths are relative to the crate root
(`~/.cargo/registry/src/index.crates.io-*/gpui-0.2.2/`). `file:line` refers to that exact version.
"Inferred" means it follows from the code as quoted but was not run; "UNVERIFIED" means not checked.

## 1. The model in brief

- **`Application`** wraps `Rc<AppCell>`, a `RefCell<App>` (app.rs:61-63, 125). `Application::run(|cx: &mut App| ..)`
  borrows the App mutably for the launch callback (app.rs:174-183).
- **`App`** owns every entity's state, all windows and globals. Docs: "The root context granting access to the
  application's global state. This context owns all entities' data" (docs/contexts.md:7-9).
- **`Entity<T>`** is a ref-counted handle; it "doesn't truly _own_ the state, but it can be used to access the state
  from its true owner, the `App`" (_ownership_and_data_flow.rs:16-18). Clone increments the count, drop decrements it.
  An entity whose `T: Render` is a *view* (docs/contexts.md:31-33).
- **`WeakEntity<T>`** does not keep the entity alive; `upgrade()` gives `Option<Entity<T>>` (entity_map.rs:681-686) and
  `update`/`read_with`/`update_in` return `Err("entity released")` once it is gone (entity_map.rs:691-740).
- **`Context<'a, T>`** is `App` plus "which particular entity it is tied to" (_ownership_and_data_flow.rs:36); it
  `Deref`s/`DerefMut`s to `App` (context.rs:25-37), so anything taking `&mut App` also takes `&mut Context<T>`
  (docs/contexts.md:11-13).
- **`Window`** is *not* a context. It is passed next to `&mut App`/`&mut Context<T>` (docs/contexts.md:27-29;
  `Render::render(&mut self, window: &mut Window, cx: &mut Context<Self>)`, element.rs:131-134).
- **`AsyncApp`** holds a `Weak<AppCell>` and is `'static`, so it can be held across `.await`; every call is fallible
  ("app was released", async_context.rs:15-30). **`AsyncWindowContext`** = `AsyncApp` + `AnyWindowHandle`
  (async_context.rs:266-271). `TestAppContext` is the same idea but panics instead of erroring (docs/contexts.md:19-21).
- **Data flow**: mutate through `entity.update(cx, |state, cx| ..)`, then `cx.notify()` ("Notify observers") or
  `cx.emit(Event)` for typed events (requires `impl EventEmitter<E> for T`); other entities react via
  `cx.observe(..)` / `cx.subscribe(..)`, which return a `Subscription` you keep or `.detach()`
  (_ownership_and_data_flow.rs:38-139, examples/ownership_post.rs:13-34).
- **Effects queue**: `notify`, `emit`, `defer`, global changes, `refresh_windows` and entity creation are queued as
  `Effect`s (app.rs:2230-2251) and flushed when the *outermost* `App::update` finishes (app.rs:759-777, 1201-1256).
  Observers therefore run after your update closure returns, not inline.

## 2. API cheat sheet (signatures copied from source)

### Entities
```rust
// AppContext (gpui.rs:115-189); App's impl at app.rs:2113, Context's at context.rs:739
fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Self::Result<Entity<T>>;
fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R> where R: Send + 'static;
// Entity<T> (entity_map.rs:416-460)
pub fn read<'a>(&self, cx: &'a App) -> &'a T
pub fn read_with<R, C: AppContext>(&self, cx: &C, f: impl FnOnce(&T, &App) -> R) -> C::Result<R>
pub fn update<R, C: AppContext>(&self, cx: &mut C, update: impl FnOnce(&mut T, &mut Context<T>) -> R) -> C::Result<R>
pub fn write<C: AppContext>(&self, cx: &mut C, value: T) -> C::Result<()>          // replaces + notify (444-449)
pub fn update_in<R, C: VisualContext>(&self, cx: &mut C,
    update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R) -> C::Result<R>
pub fn downgrade(&self) -> WeakEntity<T>
// WeakEntity<T> (entity_map.rs:681-740): upgrade() -> Option<Entity<T>>; update/update_in/read_with -> anyhow::Result<R>
// Context<T> (context.rs:45-57)
pub fn entity(&self) -> Entity<T>;  pub fn weak_entity(&self) -> WeakEntity<T>;  pub fn entity_id(&self) -> EntityId
```
`C::Result<R>` is `R` for `App`/`Context` and `anyhow::Result<R>` for `AsyncApp` (async_context.rs:23-24).

### Notify, emit, observe, subscribe (Context<T>, context.rs)
```rust
pub fn notify(&mut self)                                                       // 229
pub fn emit<Evt>(&mut self, event: Evt) where T: EventEmitter<Evt>, Evt: 'static  // 723
pub fn observe<W>(&mut self, entity: &Entity<W>,
    on_notify: impl FnMut(&mut T, Entity<W>, &mut Context<T>) + 'static) -> Subscription           // 63
pub fn observe_in<V2>(&mut self, observed: &Entity<V2>, window: &mut Window,
    on_notify: impl FnMut(&mut T, Entity<V2>, &mut Window, &mut Context<T>) + 'static) -> Subscription // 317
pub fn subscribe<T2, Evt>(&mut self, entity: &Entity<T2>,
    on_event: impl FnMut(&mut T, Entity<T2>, &Evt, &mut Context<T>) + 'static) -> Subscription     // 98
pub fn subscribe_in<Emitter, Evt>(&mut self, emitter: &Entity<Emitter>, window: &Window,
    on_event: impl FnMut(&mut T, &Entity<Emitter>, &Evt, &mut Window, &mut Context<T>) + 'static) -> Subscription // 355
pub fn observe_global<G: 'static>(&mut self, f: impl FnMut(&mut T, &mut Context<T>) + 'static) -> Subscription // 176
pub fn observe_global_in<G: Global>(&mut self, window: &Window,
    f: impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static) -> Subscription                 // 671
pub fn on_release(&self, on_release: impl FnOnce(&mut T, &mut App) + 'static) -> Subscription      // 135
pub fn observe_release<T2>(&self, entity: &Entity<T2>,
    on_release: impl FnOnce(&mut T, &mut T2, &mut Context<T>) + 'static) -> Subscription           // 151
pub fn on_release_in(&mut self, window: &Window,
    on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static) -> Subscription              // 397
```
App-level: `App::observe` (app.rs:780), `App::subscribe` (app.rs:865), `App::observe_new` (app.rs:1568),
`App::observe_release` (app.rs:1589), `Window::observe` (window.rs:1551), `Window::subscribe` (window.rs:1580).
`Subscription::detach(self)` / `Subscription::join(a, b)` (subscription.rs:175-194).

### Listeners for elements
```rust
pub fn listener<E: ?Sized>(&self, f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static)
    -> impl Fn(&E, &mut Window, &mut App) + 'static          // context.rs:252 - captures a WeakEntity
pub fn processor<E, R>(&self, f: impl Fn(&mut T, E, &mut Window, &mut Context<T>) -> R + 'static)
    -> impl Fn(E, &mut Window, &mut App) -> R + 'static      // context.rs:264 - captures a STRONG Entity
```

### Async
```rust
// Context<T>
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static, R: 'static        // context.rs:237
pub fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R>
    where AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static          // context.rs:661
// App / Window
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static // app.rs:1417
pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static                         // window.rs:1663
// Task<T> (executor.rs:56-108): #[must_use]; impl Future
pub fn ready(val: T) -> Self;  pub fn detach(self);
pub fn detach_and_log_err(self, cx: &App)        // only for Task<Result<T, E>>, E: Debug
// Timers
pub fn timer(&self, duration: Duration) -> Task<()>   // BackgroundExecutor, executor.rs:357
pub use smol::Timer;                                  // gpui.rs:94  (Timer::after(d).await, examples/window.rs:191)
pub fn background_executor(&self) -> &BackgroundExecutor   // App, app.rs:1402
// AsyncApp: update(&self, f: impl FnOnce(&mut App) -> R) -> Result<R> (async_context.rs:142); refresh() (124)
// AsyncWindowContext: update(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> Result<R> (284);
//                     on_next_frame (298); spawn (330)
```
Typical shape: `cx.spawn_in(window, async move |this, cx| { let r = cx.background_spawn(work).await;
this.update_in(cx, |this, window, cx| { ..; cx.notify(); }) }).detach();`

### Deferral and frames
```rust
pub fn defer(&mut self, f: impl FnOnce(&mut App) + 'static)                         // App, app.rs:1434
pub fn defer_in(&mut self, window: &Window,
    f: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static)                 // Context, context.rs:305
pub fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static)  // Window, window.rs:1541
pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) // Window, window.rs:1644
pub fn on_next_frame(&self, window: &mut Window,
    f: impl FnOnce(&mut T, &mut Window, &mut Context<T>) + 'static)                 // Context, context.rs:292
pub fn request_animation_frame(&self)                                               // Window, window.rs:1654
pub fn refresh(&mut self)                                                           // Window, window.rs:1367
pub fn refresh_windows(&mut self)                                                   // App, app.rs:755
```

### Focus
```rust
pub fn focus_handle(&self) -> FocusHandle                      // App, app.rs:2029
impl FocusHandle {                                              // window.rs:312-375
    pub fn tab_index(mut self, index: isize) -> Self;  pub fn tab_stop(mut self, tab_stop: bool) -> Self;
    pub fn focus(&self, window: &mut Window);  pub fn is_focused(&self, window: &Window) -> bool;
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool;
    pub fn within_focused(&self, window: &Window, cx: &mut App) -> bool;
    pub fn dispatch_action(&self, action: &dyn Action, window: &mut Window, cx: &mut App);
    pub fn downgrade(&self) -> WeakFocusHandle; }
pub trait Focusable: 'static { fn focus_handle(&self, cx: &App) -> FocusHandle; }   // window.rs:440
// Window: focus(&mut self, &FocusHandle) 1386; blur() 1397; disable_focus() 1407; focused(&self, &App) 1380; focus_next() 1413
// Context: focus_self(&mut self, window) 710 (deferred); focus_view(&mut self, &Entity<W>, window) 287
// Context listeners (all -> Subscription): on_focus 532, on_focus_in 558, on_blur 581, on_focus_lost 608,
//   on_focus_out(&mut self, handle, window, impl FnMut(&mut T, FocusOutEvent, &mut Window, &mut Context<T>)) 627
```

### Globals
```rust
pub trait Global: 'static {}                                   // global.rs:22 (marker)
// App: set_global(G) 1497; global::<G>() -> &G 1457 (panics if absent); try_global 1466; has_global 1451;
//      global_mut 1474; default_global::<G: Default>() -> &mut G 1486; remove_global 1510;
//      observe_global::<G>(impl FnMut(&mut App)) -> Subscription 1522
// BorrowAppContext (gpui.rs:241-280), works on App and Context:
fn update_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R where G: Global;
fn update_default_global<G, R>(&mut self, f: impl FnOnce(&mut G, &mut Self) -> R) -> R where G: Global + Default;
// Static sugar: G::global(cx), G::update_global(cx, |g, cx| ..), G::set_global(cx, g)  (global.rs:30-75)
```

### Element state
```rust
pub fn use_state<S: 'static>(&mut self, cx: &mut App,
    init: impl FnOnce(&mut Self, &mut Context<S>) -> S) -> Entity<S>            // window.rs:2612 (#[track_caller])
pub fn use_keyed_state<S: 'static>(&mut self, key: impl Into<ElementId>, cx: &mut App,
    init: impl FnOnce(&mut Self, &mut Context<S>) -> S) -> Entity<S>            // window.rs:2578
pub fn with_element_state<S, R>(&mut self, global_id: &GlobalElementId,
    f: impl FnOnce(Option<S>, &mut Self) -> (R, S)) -> R                         // window.rs:2628
pub fn with_optional_element_state<S, R>(&mut self, global_id: Option<&GlobalElementId>,
    f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>)) -> R         // window.rs:2711
```

### Actions, keys, windows, app services
```rust
pub fn dispatch_action(&mut self, action: Box<dyn Action>, cx: &mut App)        // Window, window.rs:1476 (deferred)
pub fn dispatch_action(&mut self, action: &dyn Action)                          // App, app.rs:1879 (active window or global)
pub fn on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Self) + 'static)   // App, app.rs:1696
pub fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>)           // App, app.rs:1677
pub fn open_window<V: 'static + Render>(&mut self, options: crate::WindowOptions,
    build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> anyhow::Result<WindowHandle<V>>  // app.rs:943
// WindowHandle<V>::update(&self, cx, impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R) -> Result<R> (window.rs:4695)
pub fn quit(&self)                                                               // app.rs:749
pub fn on_app_quit<Fut>(&self, on_quit: impl FnMut(&mut App) -> Fut + 'static) -> Subscription  // app.rs:1771
pub fn on_window_closed(&self, on_closed: impl FnMut(&mut App) + 'static) -> Subscription       // app.rs:1806
pub fn on_window_should_close(&self, cx: &App, f: impl Fn(&mut Window, &mut App) -> bool + 'static) // window.rs:4329
pub fn open_url(&self, url: &str)                                                // app.rs:1078
pub fn write_to_clipboard(&self, item: ClipboardItem)                            // app.rs:1041
pub fn read_from_clipboard(&self) -> Option<ClipboardItem>                       // app.rs:1053
// write_to_primary/read_from_primary exist only on Linux/FreeBSD (app.rs:1035, 1047)
```
`WindowOptions` fields (platform.rs:1089-1134): `window_bounds`, `titlebar`, `focus`, `show`, `kind`, `is_movable`,
`is_resizable`, `is_minimizable`, `display_id`, `window_background`, `app_id`, `window_min_size`,
`window_decorations` (Wayland only), `tabbing_identifier` (macOS). Defaults: focus/show/movable/resizable/minimizable
all `true`, `window_bounds: None`, a default titlebar (platform.rs:1221-1243).

## 3. Gotchas and rules

1. **Re-entrancy panics.** `update` moves the entity out of the map ("lease", entity_map.rs:108-123). Reading or
   updating it again while leased panics with **"cannot {read|update} {type} while it is already being updated"**
   (entity_map.rs:116, 138, 181-187). Inside `entity.update(cx, |this, cx| ..)`, use `this`, never
   `cx.entity().read(cx)`. A view is leased only while its own `render()` runs (view.rs:47); children render later in
   `request_layout`, so a child may read its parent.
2. **Same window updated twice.** `update_window_id` takes the `Window` out of `cx.windows` for the duration
   (app.rs:1366); a nested `handle.update(..)` on the same window returns `Err("window not found")` (app.rs:1366,
   1388). `read_window` panics: "attempted to read a window that is already on the stack" (app.rs:2203). Inside
   `open_window`'s builder the window is not inserted yet (app.rs:949, 967), so handle-based updates fail there too
   (inferred). `Window::defer` swallows that error with `.ok()` (window.rs:1544).
3. **Global re-entrancy.** `update_global::<G>` removes `G` while the closure runs (app.rs:1539-1546), so
   `cx.global::<G>()` inside it panics "no state of type .. exists" (app.rs:1457-1462).
   `global_mut`/`default_global` notify observers even when you only read (app.rs:1476, 1488).
4. **Dropped `Task` = cancelled.** "If you drop a task it will be cancelled immediately" (executor.rs:54); `Task` is
   `#[must_use]` (executor.rs:56). Store it in a field (replacing it cancels the old one) or `.detach()`.
   `detach_and_log_err` spawns on the foreground executor (executor.rs:92-96).
5. **Dropped `Subscription` = unsubscribed** (subscription.rs:156-161, 197-200). `.detach()` keeps the callback until the
   observed entity is released (subscription.rs:172-177). Observer/subscriber callbacks hold a weak handle to the
   emitter and remove themselves once it's gone (app.rs:849-859, 898-911).
6. **Subscriptions start at the end of the current effect cycle.** `observe`/`subscribe`/`observe_global` and the focus
   listeners activate through `defer` (app.rs:835-839, 880-887; context.rs:188, 551, 575, 600). `on_release`
   activates immediately (context.rs:146).
7. **notify vs emit.** `notify()` is untyped and deduped: one pending Notify per entity per flush
   (app.rs:1180-1186, 2042). It also invalidates windows that rendered the entity (§4). `emit()` is typed, needs
   `EventEmitter<E>` (gpui.rs:237), is queued every time with no dedupe (context.rs:728-732) and never invalidates a
   view. If the UI must change, call `notify()` as well.
8. **notify during drawing doesn't schedule anything.** If a window tracks the entity and is mid-draw,
   `invalidate_view` records the view as dirty but does not set the window dirty or queue observers
   (window.rs:116-126). `cx.notify()` inside `render`/prepaint/paint is lost until something else redraws. Use
   `cx.defer` / `window.defer` / `on_next_frame` (the defer path runs when the phase is `None` again, window.rs:1977).
9. **Leaks.** `cx.listener` captures a `WeakEntity` (context.rs:256); `cx.processor` captures a strong `Entity`
   (context.rs:268). `observe_self`/`subscribe_self` capture a strong `self.entity()` in a callback stored by the
   App (context.rs:91-94, 128-131): detaching them probably keeps the entity alive forever (inferred). Entity handles
   inside detached closures or long-lived tasks keep entities alive; prefer `WeakEntity` (`cx.spawn` already hands
   you one, context.rs:243). Tests get leak detection (`leak-detection` feature, on with `test-support`,
   Cargo.toml:45, 56-57; set `LEAK_BACKTRACE` for allocation sites, entity_map.rs:783-830).
10. **Releases are batched.** Entities are freed (and `on_release` runs) at the top of each `flush_effects` loop, not
    at drop (app.rs:1203, 1261-1276). Dropping a focused `FocusHandle` blurs that window (app.rs:1278-1298).
11. **Async contexts are fallible.** `AsyncApp` methods return `Err("app was released")` (async_context.rs:30);
    `as_mut` isn't available in async contexts (async_context.rs:61-68, 393-400). `AsyncApp::update` does
    `RefCell::borrow_mut` on the App (async_context.rs:142-145, app.rs:83). Calling it synchronously while the App is
    already borrowed (e.g. a `cx.on_drop` guard, context.rs:275-284, dropped inside an update) would panic with a
    RefCell borrow error (inferred, UNVERIFIED at runtime).
12. **Foreground tasks run on the UI thread.** `ForegroundExecutor::spawn` dispatches to the main thread and asserts it
    is polled/dropped there (executor.rs:471-540). Blocking work inside `cx.spawn` blocks the UI; send it to
    `cx.background_spawn` (needs `Send`, executor.rs:145-150).
13. **Spawning from render.** `render` runs every frame for uncached views (§4). A `cx.spawn` there spawns once per
    frame, and the `Task` is cancelled at once unless stored or detached (executor.rs:54). Start work from
    events/constructors and keep the `Task` in a field (inferred from the above).
14. **`request_animation_frame` must be called while a view renders.** It calls `current_view()`, which unwraps
    `rendered_entity_stack.last()` (window.rs:1654-1656, 3361-3364). The doc comment's "otherwise refresh the entire
    window" fallback is not in the code.
15. **Element state.** `with_element_state`/`use_state`/`use_keyed_state` only work during
    layout/prepaint/paint (debug assert "this method can only be called during request_layout, prepaint, or paint",
    window.rs:169-176, 2636). State lives only while the element is rendered in consecutive frames
    (Frame::finish keeps accessed keys only, window.rs:805-812). `use_state` keys on the caller's code location
    (window.rs:2617-2619), so in a loop or list every item shares one state: use `use_keyed_state` (doc,
    window.rs:2608-2610). A nested call with the same id and type panics "reentrant call to with_element_state for the
    same state type and element id" (window.rs:2677-2679). Changing the state entity notifies the view that created
    it (window.rs:2591-2594).
16. **Focus is reported per frame.** `window.focus()` sets the id and calls `refresh()` (window.rs:1386-1394).
    `on_focus`/`on_blur`/`on_focus_in`/`on_focus_out` fire only from `Window::draw`, by comparing the focus paths of
    the previous and new frames (window.rs:1939-1971). `contains_focused`/`within_focused` use the *last rendered*
    dispatch tree (window.rs:243-262). `Context::focus_self` is deferred (context.rs:715-717).
17. **Actions.** `Window::dispatch_action` is deferred to the end of the effect cycle and targets the focus captured
    at call time (window.rs:1476-1488). `App::on_action` handlers run only in the bubble phase, after element handlers,
    unless an element handler calls `cx.propagate()` (app.rs:1693-1705). `bind_keys` queues `RefreshWindows`
    (app.rs:1679).
18. **Quit.** `on_app_quit` futures get `SHUTDOWN_TIMEOUT` = 100 ms (app.rs:56, 698-720). After that,
    `foreground_executor()` panics "Can't spawn on main thread after on_app_quit" (app.rs:1408-1410; `spawn`
    debug-panics, 1422-1424). On **Windows**, closing the last window posts `WM_QUIT` itself
    (platform/windows/platform.rs:718-721, 732-745). The `on_window_closed` + `cx.quit()` pattern in
    examples/on_window_close_quit.rs:42-47 is for other platforms. `on_window_should_close` returning `false` keeps
    the window open (window.rs:4327-4338).
19. **`open_window` draws once synchronously** before returning; on Windows this avoids a crash (app.rs:959-964).
20. **`as_mut`/`GpuiBorrow` always notifies on drop** (app.rs:2415-2419); `Entity::write` always notifies
    (entity_map.rs:444-449).

## 4. Performance facts

- **Which windows `notify` invalidates.** At the end of every `draw`, the window records every entity read or updated
  during the frame (`EntityMap::read/lease/insert` add to `accessed_entities`, entity_map.rs:98-99, 110-111, 132-133;
  recorded at window.rs:1974, 1983-1996 → app.rs:808-833). `App::notify(id)` calls `invalidate_view` on those windows
  only; if no window tracks the entity, it just queues observers (app.rs:2034-2054). Notifying an entity that no
  window read last frame redraws nothing.
- **Which views are marked dirty.** At the next draw, `invalidate_entities` → `mark_view_dirty` marks the notified id
  and its ancestor *views* from the last frame's dispatch tree (window.rs:1998-2004, 1304-1318; view_path at
  key_dispatch.rs:575-587). A plain model (not a rendered view) has no view path, so notifying it marks no view
  dirty. It only sets the window dirty (window.rs:119-121).
- **Uncached views render every frame.** The root is re-rendered every draw (window.rs:2036-2037), and an
  `Entity<V>` used as an element always calls `render` (view.rs:40-52). Only `AnyView::cached(style)` skips work
  (view.rs:99-106).
- **`.cached()` re-renders when** any of these holds (view.rs:208-213): bounds, content mask or text style changed
  (the `ViewCacheKey`, view.rs:22-26); its own entity id is in `dirty_views` (it or a descendant view was notified);
  or `window.refreshing` is set. Otherwise it replays the previous prepaint/paint ranges (view.rs:215-222, 280).
  - A cached view that reads a *model* in `render` does **not** re-render when that model notifies, because the
    model id never lands in the view's path. It must `cx.observe(&model, |_, _, cx| cx.notify())` (inferred from
    view.rs:212 + window.rs:1304-1318).
  - Re-rendering a cached view sets `refreshing = true` for its subtree (view.rs:225, 276), so **nested cached views
    re-render too**.
  - `refreshing` is set by `window.refresh()` (window.rs:1367-1372), `window.focus()`/`blur()` (1386-1404), resize
    (`bounds_changed`, 1680), `cx.refresh_windows()` (app.rs:1321-1328), `bind_keys`/`clear_key_bindings`
    (app.rs:1679, 1685). **Any focus change throws away every cached view in that window for one frame.**
  - Accessed entities of a reused cached view are re-registered, so the window keeps tracking them (view.rs:217-218).
  - The inspector's picking mode disables caching (view.rs:168-169).
- **Frame scheduling on Windows.** A `VSyncProvider` thread calls `RedrawWindow(.., RDW_INVALIDATE)` on every window
  each vblank (platform/windows/platform.rs:240-272) → `WM_PAINT` → `draw_window` → the request-frame callback
  (events.rs:252-254, 1196-1202). The callback (window.rs:1025-1066):
  1. runs `on_next_frame` callbacks first (1026-1035), which is why they need a frame tick;
  2. if `invalidator.is_dirty() || force_render`: `draw` + `present`, then clears the arena (1044-1054);
  3. else if `needs_present` **or (window active and last input < 1 s ago)**: re-`present` the old scene without
     re-rendering, "to prevent the display from underclocking the refresh rate" (1037-1042, 1055-1058).
     `last_input_timestamp` is set by every `dispatch_event` (window.rs:3582).
  So a `notify` waits at most one vblank before the frame. An idle window does no render work, only the vsync wakeup.
- **Element arena.** Elements are allocated in a thread-local arena of 1 MiB chunks (window.rs:211-213), cleared
  *after* `present` "to reduce latency" (window.rs:1048-1051). It grows by whole 1 MiB chunks and keeps them
  (`clear` resets, never frees; arena.rs:102-110, 130-139).
- **Effects flush loop.** Each loop iteration first releases dropped entities and focus handles (app.rs:1202-1204).
  Observer sets are cloned (`Rc`) per effect (app.rs:1304-1318). Many `emit`s cost one listener pass each; repeated
  `notify`s within one flush cost one (app.rs:1183-1185).
- **Tests differ.** With `test-support`, every `flush_effects` synchronously draws dirty windows (app.rs:1236-1249).
  In the real app, drawing waits for the vsync paint.
- **Timers.** Prefer `cx.background_executor().timer(d)` over `smol::Timer`: it goes through the dispatcher
  (`dispatch_after`, executor.rs:357-367), so test clocks (`advance_clock`, executor.rs:395) control it. `now()` is
  documented for fake timers (executor.rs:347-352).
- **UNVERIFIED**: the per-frame cost of `present` alone on DirectX, and whether `RedrawWindow` keeps waking minimized
  windows (events.rs:176-187 parks `request_frame` while minimized, which suggests no frames then).
