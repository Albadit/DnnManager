# GPUI 0.2.2 on Windows: platform layer reference

Source root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2/`.
Path shorthands: `W/` = `src/platform/windows/`, `P` = `src/platform.rs`, `win` = `src/window.rs`,
`app` = `src/app.rs`, `ctx` = `src/app/context.rs`. Line numbers are for 0.2.2. "UNVERIFIED" = inferred,
not confirmed by reading the code or by a run.

Already covered elsewhere (not repeated here): the cursor mapping only has arrow/ibeam/cross/hand/size-we/size-ns/no;
the vsync thread calls RedrawWindow every vsync; other cursors need a WM_SETCURSOR subclass; the title-bar Drag area
must occlude.

---

## 1. How a Windows window works

### Process and threads
- `WindowsPlatform::new` calls `OleInitialize`, creates the D3D11/DXGI devices and the DirectWrite text system, and makes a
  hidden message-only window (`HWND_MESSAGE`, class `Zed::PlatformWindow`) that receives GPUI's private messages
  (`W/platform.rs:91-161`, `W/platform.rs:1096`). If anything fails, it shows a `MessageBoxW` "Failed to launch" and then
  **panics** with `.unwrap()` (`P:154-160`, `W/util.rs:194-203`).
- Main-thread tasks go into a flume channel, followed by `PostMessageW(WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD)` to the
  message window (`W/dispatcher.rs:82-92`). The message window drains the channel (`W/platform.rs:748-753`).
- Background tasks run on the WinRT `ThreadPool` at high priority. Timers (`dispatch_after`) use `ThreadPoolTimer`
  (`W/dispatcher.rs:47-67`).
- `run()` calls `on_finish_launching`, starts the vsync thread, then runs a plain `GetMessageW`/`DispatchMessageW` loop.
  There is no `TranslateMessage` here: GPUI calls it itself for unhandled keys. When the loop ends it calls the quit
  callback (`W/platform.rs:308-321`).
- Every app window uses the class `Zed::Window`. The class is registered once with `CS_HREDRAW|CS_VREDRAW`, a black
  background brush and the icon loaded at platform start (`W/window.rs:1147-1162`). The icon is resource ID **1** of the
  exe (`W/platform.rs:1000-1015`). Without an icon resource with ID 1, the window has no icon.

### Private messages (`WM_USER+n`, `W/events.rs:22-28`)
| Message | Sent by | Effect |
|---|---|---|
| `+1` CURSOR_STYLE_CHANGED | `set_cursor_style` sends it to every window (`W/platform.rs:553-564`) | stores the HCURSOR and calls SetCursor if it changed between none and some (`W/events.rs:1067-1082`) |
| `+2` CLOSE_ONE_WINDOW | WM_DESTROY (`W/events.rs:263-281`) | removes the HWND from the list; **`PostQuitMessage` when the list becomes empty** (`W/platform.rs:716-722, 732-745`) |
| `+3` TASK_DISPATCHED | dispatcher | runs foreground runnables |
| `+5` FORCE_UPDATE_WINDOW | device-lost recovery | `draw_window(force_render=true)` (`W/events.rs:109`) |
| `+6` KEYBOARD_LAYOUT_CHANGED | WM_INPUTLANGCHANGE (`W/events.rs:1167-1178`) | calls `on_keyboard_layout_change` (`W/platform.rs:774-784`) |
| `+7` GPU_DEVICE_LOST | vsync thread | renderer rebuilds its resources (`W/events.rs:1187-1194`) |
Each one is checked against a random `validation_number` (`W/platform.rs:713-716`).

### WM_* handling map (`W/events.rs:41-110`; anything not listed goes to `DefWindowProcW`)
| Message | Handling |
|---|---|
| WM_CREATE | with a hidden title bar, `SWP_FRAMECHANGED` forces WM_NCCALCSIZE (`:766-773`) |
| WM_NCCALCSIZE | only when the title bar is hidden and the window is not fullscreen: shrinks the client rect by the frame thickness (SM_CXSIZEFRAME+SM_CXPADDEDBORDER) on left, right and bottom. Top inset: 0 on Win10, `round(dpi/96)` px on Win11, the full frame when maximized (`:699-747, :1494-1536`). When maximized with an auto-hide taskbar, it pulls in 1 px on the taskbar edge so the taskbar can still pop up (`:719-745`) |
| WM_NCHITTEST | see "Custom title bar" below (`:856-923`) |
| WM_NCMOUSEMOVE | sent to GPUI as `MouseMove` in client coordinates (`:925-947`), which keeps the hit test current over the caption |
| WM_NCL/R/MBUTTONDOWN, NCLBUTTONDBLCLK | sent to GPUI as `MouseDown`. If GPUI stops propagation or prevents default, it returns 0, so no native drag or maximize happens. Left-down on HTMIN/MAX/CLOSE is recorded and swallowed (`:63-80, :949-998`) |
| WM_NC*BUTTONUP | sent to GPUI. Then, if the button-up lands on the same HTMIN/HTMAX/HTCLOSE as the button-down: `SW_MINIMIZE`, a maximize/`SW_NORMAL` toggle, or `PostMessage(WM_CLOSE)` (`:1000-1065`) |
| WM_L/R/M/XBUTTONDOWN/UP | `SetCapture`/`ReleaseCapture`; click count is GPUI's own `ClickState`, using the system double-click time and size and updated on WM_SETTINGCHANGE (`:445-503`, `W/window.rs:1037-1110`) |
| WM_MOUSEMOVE / WM_MOUSELEAVE | TrackMouseEvent; hover status callback (`:283-328, :1258-1279`) |
| WM_MOUSEWHEEL / WM_MOUSEHWHEEL | see section 4 (`:520-623`) |
| WM_KEYDOWN/UP, WM_SYSKEYDOWN/UP, WM_CHAR, WM_DEADCHAR | see section 4 (`:330-443`) |
| WM_SYSCOMMAND | swallows `SC_KEYMENU` only if GPUI handled the preceding sys key (`:1130-1139`) |
| WM_IME_STARTCOMPOSITION / WM_IME_COMPOSITION | IMM32: places the composition and candidate windows at the caret; marked text and result text (`:612-697`) |
| WM_ENTERSIZEMOVE / WM_ENTERMENULOOP | `SetTimer(USER_TIMER_MINIMUM)`; each WM_TIMER drains the main-thread queue and paints, so the app keeps running inside the modal move/size loop (`:216-250`) |
| WM_SIZE | when minimized, stashes the `request_frame` callback, so **no frames while minimized**; otherwise resizes the swap chain and calls the resize callback (`:170-214`) |
| WM_MOVE | updates the origin and, if the centre crossed monitors, the display; calls the moved callback (`:120-152`) |
| WM_GETMINMAXINFO | applies `window_min_size` (logical size × scale + border) (`:154-168`) |
| WM_DPICHANGED | new scale = dpi/96; `SetWindowPos` to the rect Windows suggests; when maximized, calls the resize code itself (`:775-822`) |
| WM_DISPLAYCHANGE | if the window's monitor is gone: `ShowWindow(SW_SHOWNORMAL)` and look up the new monitor (`:828-854`) |
| WM_PAINT | `draw_window` then `ValidateRect` (`:252-254, :1196-1205`) |
| WM_SHOWWINDOW(TRUE) | draws at once (`:1180-1185`) |
| WM_ACTIVATE | the active-status callback is **spawned** on the foreground executor, so it is async, not inside the message (`:749-764`) |
| WM_CLOSE | `should_close` callback; returning false cancels the close (`:256-261`) |
| WM_DESTROY | `close` callback (GPUI removes the Window), then CLOSE_ONE_WINDOW (`:263-281`) |
| WM_SETCURSOR | returns the GPUI cursor except on resize edges or when the window is disabled (`:1084-1104`) |
| WM_SETTINGCHANGE | wparam≠0: reloads double-click, border offset, wheel lines/chars, auto-hide taskbar. wparam=0 with "ImmersiveColorSet": checks light/dark again (`:1106-1165`) |

### Frame scheduling
1. The vsync thread waits on `DwmFlush`. If that returns in under 1 ms (monitor asleep, desktop occluded) it sleeps one
   DWM interval instead, 16.67 ms if unknown (`W/vsync.rs:22-56`). Then it calls `RedrawWindow(RDW_INVALIDATE)` on every
   window (`W/platform.rs:240-273`).
2. WM_PAINT → `request_frame` → GPUI draws **only if the view tree is dirty** (`cx.notify`/refresh) or `force_render`.
   Otherwise it only presents again while `needs_present`, or **for 1 s after the last input while the window is active**
   (`win:1037-1060`). An idle window draws nothing; an active window presents at refresh rate for 1 s after each input.
3. `Present(0, 0)`: no sync interval, because pacing comes from the vsync thread (`W/directx_renderer.rs:204-207`).

### DirectX renderer facts
- D3D11 on the first DXGI adapter that supports feature level 11.1, 11.0 or 10.1 (`W/directx_devices.rs:121-141, 160-170`).
  If none does, `get_adapter` fails, and platform creation fails with it (the panic above).
- By default it uses a DirectComposition swap chain with `WS_EX_NOREDIRECTIONBITMAP`, FLIP_SEQUENTIAL, 3 buffers,
  **premultiplied alpha** (`W/window.rs:409-411`, `W/directx_renderer.rs:1021-1045, 1389`). Setting env
  `GPUI_DISABLE_DIRECT_COMPOSITION=1` (or `true`) gives a plain HWND swap chain with `ALPHA_MODE_IGNORE`, which means no
  transparency (`W/directx_renderer.rs:29, 1047-1077`, `W/platform.rs:138-139`).
- Format BGRA8; paths use 4× MSAA in an intermediate texture (`W/directx_renderer.rs:30-32, 398-503`).
- `draw_surfaces` is a no-op on Windows, so `PaintSurface` (video) is not drawn (`W/directx_renderer.rs:572-577`).
- `gpu_specs()` reports `is_software_emulated` (WARP) (`W/directx_renderer.rs:579-606`).

### Device lost
The vsync thread checks `GetDeviceRemovedReason` every tick (`W/platform.rs:252-261, 1022-1031`). On loss it sleeps 350 ms,
recreates the devices (5 tries, 100 ms apart, `W/directx_devices.rs:23-41`), sends DEVICE_LOST to the platform window,
the text system and each window, sleeps 200 ms, then forces a redraw (`W/platform.rs:1033-1094`). If every try fails, it
only logs, and the window may stay blank (`W/platform.rs:1050-1056`, `W/directx_renderer.rs:209-224`).

---

## 2. Window API cheat sheet (behaviour on Windows)

### `WindowOptions` (`P:1089-1141`, defaults `P:1221-1245`)
| Field | Windows behaviour |
|---|---|
| `window_bounds: Option<WindowBounds>` | `None` → active window's bounds + (0,35), otherwise the display's `default_bounds()`: 1024×700-ish centred, clipped to the monitor (`win:906-927`, `P:291-299`). If the centre of the bounds is not on the chosen display, Windows silently uses `default_bounds()` (`W/window.rs:1309-1313`). `Maximized(_)`/`Fullscreen(_)` call `zoom()`/`toggle_fullscreen()` after creation (`win:1000-1006`) |
| `titlebar: Option<TitlebarOptions>` | `appears_transparent: true` hides the native caption (custom title bar). **`titlebar: None` also hides it** (`unwrap_or(true)`, `W/window.rs:380-384`). `title` sets the HWND text (`W/window.rs:385-392`). `traffic_light_position` is ignored |
| `focus` | **unused on Windows** (`P:1171-1175` dead_code attr) |
| `show` | true: `SetWindowPlacement` at creation. false: the placement is saved and applied by the first `activate_window()` (`W/window.rs:471-478, 290-315, 690-697`) |
| `kind` | `PopUp` → `WS_EX_TOOLWINDOW` with style 0 (no caption or frame, not in the taskbar, **not topmost**). `Normal` and `Floating` are the same: `WS_EX_APPWINDOW` + `WS_SYSMENU` (`W/window.rs:394-408`). There is no owner/parent window, so no real modal child windows |
| `is_resizable` | adds `WS_THICKFRAME|WS_MAXIMIZEBOX` (`W/window.rs:399-401`) |
| `is_minimizable` | adds `WS_MINIMIZEBOX` (`W/window.rs:403-405`) |
| `is_movable` | false → WM_NCHITTEST is not handled, so there is no Drag area and no custom resize (`W/events.rs:863-865`) |
| `display_id` | index into `EnumDisplayMonitors` order. **`unwrap()` panics if the index is out of range** (`W/window.rs:415-419`, `W/display.rs:35-36`) |
| `window_background` | applied once at creation (`win:998`); see below |
| `window_min_size` | WM_GETMINMAXINFO (`W/events.rs:154-168`) |
| `app_id`, `window_decorations`, `tabbing_identifier` | no-ops on Windows (`P:534, 541`); `window_decorations()` always returns `Decorations::Server` (`P:538-540`) |

### Bounds, display, DPI
- `window.bounds() -> Bounds<Pixels>`: logical client origin and size (`W/window.rs:519-521, 160-165`).
- `window.window_bounds() -> WindowBounds`: comes from `GetWindowPlacement.rcNormalPosition`, so it gives the restore
  size even when maximized. Use it to save window state (`W/window.rs:166-197`).
- `window.viewport_size()`, `window.scale_factor()` (= monitor DPI/96, `W/window.rs:89-92`), `window.display(cx)`
  (`win:1800-1805`). These update on resize, move and DPI change via `bounds_changed` (`win:1675-1685`).
- Display `bounds()` is the **whole monitor (rcMonitor), not the work area**, so the taskbar is included
  (`W/display.rs:38, 194-196`). `DisplayId` is the enumeration index and is not stable. `uuid()` is a hash of the device
  name, so save that instead (`W/display.rs:39, 190-192, 241-247`).
- `cx.observe_window_bounds(window, |this, window, cx| ..)` (`ctx:429`).
- `window.resize(size)`: async `SetWindowPos(SWP_NOMOVE)` (`W/window.rs:539-566`).

### State
- `window.minimize_window()` → `ShowWindowAsync(SW_MINIMIZE)` (`win:4117`, `W/window.rs:778-780`).
- `window.zoom_window()` → `SW_MAXIMIZE` only. **It does not toggle**, so restore the window yourself via the Max control
  area or the raw HWND (`win:1741`, `W/window.rs:782-790`). Called before the window is shown, it sets the initial
  state instead.
- `window.is_maximized()` → `IsZoomed`, false when fullscreen (`W/window.rs:154-156`).
- `window.toggle_fullscreen()` / `is_fullscreen()`: strips the caption and frame styles and covers the monitor; toggling
  again restores the saved style and rect (`W/window.rs:238-288, 792-802`).
- `window.activate_window()`: restores from minimized, `SetActiveWindow`, `SetFocus`, **injects a fake Alt press and
  release with `SendInput`** to get around the foreground lock, then `SetForegroundWindow` (`W/window.rs:690-742`).
  `window.is_window_active()` compares with `GetActiveWindow` (`W/window.rs:744-746`).
- `window.set_window_title(&str)` → `SetWindowTextW` (`win:1779`, `W/window.rs:752-756`).
- Not available on Windows (no-ops): `start_window_move`, `start_window_resize`, `show_window_menu`, `set_client_inset`
  (only stores the value), `request_decorations`, `set_window_edited`, `set_app_id`, `show_character_palette`,
  `titlebar_double_click` (`P:514-548`, `win:1746-1797`). The `window_shadow.rs` example relies on these and does not
  work on Windows (`examples/window_shadow.rs:18-23, 86, 164`).
- There is **no per-window hide/show API**. `cx.hide()` and `cx.activate()` are no-ops (`W/platform.rs:368-370`). For a
  tray app, use `HasWindowHandle` → HWND → `ShowWindow(SW_HIDE)` (`W/window.rs:483-491`).

### Custom title bar: `window_control_area`
- `div().window_control_area(WindowControlArea::{Drag, Close, Max, Min})` (`src/elements/div.rs:581, 1004`) adds a
  window-control hitbox (`win:3324-3327`, `src/elements/div.rs:1848-1852`).
- WM_NCHITTEST asks GPUI which control hitbox is in `window.mouse_hit_test`. **That is the hit test from the last
  processed mouse move, not the WM_NCHITTEST point** (`win:1133-1147`). It maps Drag→HTCAPTION, Close→HTCLOSE,
  Max→HTMAXBUTTON, Min→HTMINBUTTON (`W/events.rs:867-879`).
- With no control area under the mouse and a hidden title bar: DefWindowProc results are kept for resize edges; there is
  a synthetic HTTOP band `SM_CYFRAME` px high at the top when not maximized; everything else is HTCLIENT
  (`W/events.rs:884-922`).
- HTCAPTION gets native behaviour: drag via the modal move loop (Aero Snap). A double-click toggles maximize via
  DefWindowProc **unless a GPUI handler consumed the mouse-down** (`W/events.rs:60-62, 984-997`).
- Close/Max/Min act on button-up over the same area (section 1). HTMAXBUTTON is what Win11 uses for the Snap Layouts
  flyout (UNVERIFIED that it appears; GPUI does nothing special for it).

### Appearance
- `window.appearance() -> WindowAppearance` (`win:1711`); `cx.window_appearance()` (`app:1029`). Only `Light` or `Dark`
  is ever reported. It comes from `UISettings.GetColorValue(Foreground)`: a light foreground means dark mode
  (`W/util.rs:175-192`).
- Change notifications: `window.observe_window_appearance(|window, cx| ..)` (`win:1321`) or
  `cx.observe_window_appearance(window, |this, window, cx| ..)` (`ctx:465`). They fire on WM_SETTINGCHANGE
  "ImmersiveColorSet", which also switches the **native** title bar's DWM dark mode (`W/events.rs:1141-1165`,
  `W/util.rs:150-165`). High-contrast themes are not detected (UNVERIFIED: no code for it found).

### Background appearance (`window.set_background_appearance`, `win:1789`; `W/window.rs:758-776`)
Uses the undocumented `SetWindowCompositionAttribute`, and only on build ≥ 17763 (`W/window.rs:1320-1324`):
`Opaque` = ACCENT_DISABLED, `Transparent` = ACCENT_ENABLE_TRANSPARENTGRADIENT, `Blurred` = **acrylic**
(ACCENT_ENABLE_ACRYLICBLURBEHIND). **There is no Mica support.** Transparency only shows where the scene alpha < 1 and
DirectComposition is on.

### Activation, close, quit
- `cx.observe_window_activation(window, |this, window, cx| ..)` (`ctx:447`). This runs from a spawned task after WM_ACTIVATE.
- `window.on_window_should_close(cx, |window, cx| -> bool)` (`win:4329-4339`). This is the **only** close hook, and
  there is one per window: setting it again replaces it (`W/window.rs:828-830`). Alt+F4, the Close area and
  `PostMessage(WM_CLOSE)` all go through it.
- When the **last GPUI HWND is destroyed, the app quits** (`PostQuitMessage`) no matter what `on_window_closed` does
  (`W/platform.rs:716-722`). Windows opened with `show:false` still count. Then the quit callbacks run, and `on_app_quit`
  futures get **100 ms** (`app:56, 686-715, 1771-1790`). Tray pattern: return false from `on_window_should_close` and hide
  the HWND. Keep the window alive.
- `cx.quit()` → `PostQuitMessage` on the next tick (`W/platform.rs:324-328`).

---

## 3. Platform services cheat sheet

| API | Windows implementation |
|---|---|
| `cx.write_to_clipboard(ClipboardItem)` / `read_from_clipboard()` (`app:1041-1056`) | Win32 clipboard. Writes **only the first entry**: text as CF_UNICODETEXT plus GPUI hash/metadata formats, or an image (PNG, GIF and JPEG are converted to PNG; SVG as "image/svg+xml") (`W/clipboard.rs:150-230`). Reads text, images and CF_HDROP files (`W/clipboard.rs:41-90, 248-282`). `OpenClipboard(None)` failures are only logged (`W/clipboard.rs:114-131`) |
| `cx.open_url(&str)` (`app:1078`) | `ShellExecuteW("open")` on a background thread; an empty string is ignored; errors only logged (`W/platform.rs:424-436, 839-859`). From an elevated process the opened browser is elevated too (UNVERIFIED, standard ShellExecute behaviour) |
| `cx.reveal_path(&Path)` (`app:1138`) | `SHOpenFolderAndSelectItems`; if that fails with "file not found", opens the parent folder (`W/platform.rs:480-492, 861-900`) |
| `cx.open_with_system(&Path)` | `ShellExecuteW` (`W/platform.rs:494-506`) |
| `cx.prompt_for_paths(PathPromptOptions{files,directories,multiple,prompt})` → `Receiver<Result<Option<Vec<PathBuf>>>>` (`app:1116`) | **Native** `IFileOpenDialog`. `directories` → `FOS_PICKFOLDERS` (folders only); `files` is ignored; `multiple` → ALLOWMULTISELECT; `prompt` → OK button label; **no file-type filters**; cancel → `Ok(None)` (`W/platform.rs:442-455, 902-945`). Mixed files and folders not supported (`:475-478`) |
| `cx.prompt_for_new_path(dir, suggested_name)` (`app:1129`) | **Native** `IFileSaveDialog`. Folder is set if `dir` canonicalizes; only filter is "All files (*.*)" (`W/platform.rs:457-473, 947-998`) |
| (both dialogs) | Owner = the active GPUI window, or none (`W/platform.rs:228-238`). The dialog runs modally **inside a foreground task** (`W/platform.rs:448-452`), so its nested loop still dispatches GPUI messages (UNVERIFIED: re-entrancy effects not tested) |
| `window.prompt(level, msg, detail, &answers, cx)` → `oneshot::Receiver<usize>` (`win:4141-4176`) | Default builder is the **native `TaskDialogIndirect`** (`app:657`, `W/window.rs:606-688`). Caption fixed to "Info"/"Warning"/"Critical", with matching icon. `PromptButton::Ok`→IDOK, `Cancel`→IDCANCEL, `Other`→negative IDs. Closing with Esc or X when there is no Cancel button drops the sender, so the receiver gets `Canceled` (`W/window.rs:676-682`). Needs Common Controls v6, which the gpui manifest provides (below). `cx.set_prompt_builder(..)` replaces it with an in-window GPUI prompt (`app:1976-1990`) |
| `cx.write_credentials(url, user, &[u8])` / `read_credentials(url)` / `delete_credentials(url)` (`app:1058-1076`) | Windows Credential Manager, `CRED_TYPE_GENERIC`, target **`zed:url=<url>`**, `CRED_PERSIST_LOCAL_MACHINE` (`W/util.rs:109-111`, `W/platform.rs:578-650`). Runs on the **foreground** executor. A missing entry makes `CredReadW` fail, so `read_credentials` returns **`Err`, not `Ok(None)`** (`W/platform.rs:610-617`) |
| `cx.keyboard_layout()` / `cx.on_keyboard_layout_change(..)` (`app:723-745`) | id from `GetKeyboardLayoutNameW` (KLID like "00000409"), name from HKLM `...\Keyboard Layouts\<id>\Layout Text` (`W/keyboard.rs:95-112`) |
| System settings | Not public. Wheel lines/chars (`SPI_GETWHEELSCROLLLINES/CHARS`) and auto-hide taskbar are read per window and refreshed on WM_SETTINGCHANGE (`W/system_settings.rs:32-100`). `cx.should_auto_hide_scrollbars()` = `UISettings.AutoHideScrollBars` (`W/platform.rs:566-568, 1016-1020`) |
| `cx.app_path()` | `std::env::current_exe()` (`W/platform.rs:544-546`) |
| `cx.path_for_auxiliary_executable` | always `Err("not yet implemented")` (`W/platform.rs:548-551`) |
| `cx.restart()` (`app:1153-1158`) | Spawns `powershell.exe -command` with a script that waits for the PID to exit and then `Start-Process`es the exe, then quits (`W/platform.rs:330-366`). The path is put inside a double-quoted PowerShell string, so a path containing `$` or `"` breaks it. The child inherits the elevation (UNVERIFIED) |
| `cx.set_menus(..)` | **Only stored**; there is no native menu bar on Windows (`W/platform.rs:516-522`) |
| `cx.set_dock_menu(..)` / `update_jump_list(..)` | taskbar **Jump List** tasks; clicking one goes to `on_app_menu_action` (`W/platform.rs:197-226, 755-772`, `W/destination_list.rs`) |
| System tray | **None in GPUI** (no `Shell_NotifyIcon` anywhere in the crate) |
| `cx.register_url_scheme` | `Err("unimplemented")` (`W/platform.rs:652-654`) |
| Accessibility | **No UI Automation provider** (no `WM_GETOBJECT`/UIA in the crate), so screen readers see an empty window |

---

## 4. Windows-specific gotchas

### Things that panic
- `HasDisplayHandle::display_handle()` → `unimplemented!()` (`W/window.rs:494-499`). Any crate that asks a GPUI window
  for a display handle (some `raw-window-handle` consumers do) will panic. Use only `window_handle()`.
- `cx.hide_other_apps()` / `unhide_other_apps()` → `unimplemented!()` (`W/platform.rs:372-380`).
- `WindowOptions.display_id` that no longer exists (monitor unplugged since it was saved) → `unwrap()` on `None`
  (`W/window.rs:416`). Check it against `cx.displays()` first.
- `WindowsDisplay::new_with_handle` uses `expect`/`unwrap` on the monitor info (`W/display.rs:66-75, 101-106`).
- Platform creation failure (no D3D11 adapter, OLE, DirectWrite) → message box, then panic (`P:154-160`).
- A missing font panics only with the `test-support` feature; normally it logs and uses the system UI font
  (`W/direct_write.rs:481-498`).
- `window.prompt` while the prompt builder is taken → `unreachable!` (`win:4153-4155`). With the native builder it is put
  back at once, so this only happens with a re-entrant custom builder.

### Fonts
- DirectWrite with **grayscale antialiasing only, never ClearType/subpixel** (`DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE`,
  `W/direct_write.rs:752-761, 912-919`). The rendering mode is DirectWrite's recommendation, with OUTLINE mapped to
  NATURAL_SYMMETRIC (`W/direct_write.rs:731-750`). The shader uses gamma and grayscale enhanced contrast from the system
  `IDWriteRenderingParams1`, read once (`W/directx_renderer.rs:608-640`).
- `.SystemUIFont` resolves to the `SPI_GETICONTITLELOGFONT` face, normally "Segoe UI" (`W/direct_write.rs:460-472, 1819-1840`).
- Colour glyphs: only the COLR format is handled (`W/direct_write.rs:892`).

### High DPI and multiple monitors
- Per-monitor-v2 DPI awareness and Common Controls v6 come from the manifest the **default `windows-manifest` feature**
  embeds as RT_MANIFEST ID 1 (`resources/windows/gpui.manifest.xml`, `build.rs:262-275`, `Cargo.toml:38-42, 83`).
  Turning off default features loses both: DPI virtualization (blurry) and TaskDialog failing (UNVERIFIED). An app that
  embeds its own manifest (e.g. `requireAdministrator`) will clash with resource ID 1 (UNVERIFIED), so merge the two.
- All coordinates are logical = physical / (dpi/96) (`W/util.rs:167-173`). Initial bounds are converted with the scale
  of the window at creation (`W/window.rs:463-470`). On mixed-DPI setups the first placement on another monitor may be
  scaled wrong until WM_DPICHANGED corrects it (UNVERIFIED).
- When a monitor is disconnected, the window is restored and its display looked up again (`W/events.rs:828-854`).

### Keyboard
- **AltGr**: right-Alt + left-Ctrl counts as AltGr (both modifiers cleared) only for the layouts in a hard-coded list
  (de, fr, es, pl, nl, pt-PT, sv, no, fi, tr, ru, cs, hu, sl, sk, ro) (`W/keyboard.rs:114-145`, `W/events.rs:1463-1490`).
  Other AltGr layouts (e.g. Albanian 041C, en-GB) report `ctrl+alt`, so a `ctrl-alt-*` binding can win over typing the
  character. `current_modifiers()` reads the layout from the registry **on every call** (`W/events.rs:1463-1468`).
- **Alt / menu**: WM_SYSKEYDOWN goes to GPUI first. If a binding handles it, the following `SC_KEYMENU` is swallowed;
  if not, DefWindowProc runs, so Alt+F4 and Alt+Space keep working (`W/events.rs:330-354, 1130-1139`). WM_SYSKEYUP
  always returns 0 (`W/events.rs:356-368`), so tapping Alt alone does not open the system menu (UNVERIFIED in a run).
- Modifier-only keys produce `ModifiersChanged`, de-duplicated (`W/events.rs:1313-1340`). The Windows key = `platform`
  modifier (`W/events.rs:1483`). `function` is always false.
- Keys not handled by GPUI are passed to `TranslateMessage` → WM_CHAR → `replace_text_in_range` on the focused input
  handler; control characters are dropped and surrogate pairs joined (`W/events.rs:396-407, 428-435, 1207-1240`).
- `ctrl-shift-0` cannot be triggered (Windows bug, `W/events.rs:370-371`).

### IME
IMM32 only, no TSF. `update_ime_position` is a no-op (`W/window.rs:860-862`). The composition window is placed only on
WM_IME_STARTCOMPOSITION, from `selected_text_range` + `bounds_for_range` (`W/events.rs:616-655`). While text is marked,
keydowns go straight to IME translation (`W/events.rs:384-391`). Dead keys become marked text (`W/events.rs:437-443`).

### Mouse wheel
`ScrollDelta::Lines`, **not pixels**: notches × `SPI_GETWHEELSCROLLLINES`. With Shift it becomes horizontal and uses
`WHEELSCROLLCHARS` (`W/events.rs:527-571`). The horizontal wheel is negated (`W/events.rs:595`). `touch_phase` is always
`Moved`, and there is no precision-touchpad pixel path. If the user setting is "one screen at a time"
(`WHEEL_PAGESCROLL` = 0xFFFFFFFF), it is used as the line count, giving huge deltas (UNVERIFIED in a run).

### Cursor
`set_cursor_style` sends a message to **all** windows (`W/platform.rs:553-564`). `CursorStyle::None` → no cursor
(`W/util.rs:134`).

### Other
- No frames while minimized. A hidden window is drawn as soon as it is shown (`W/events.rs:175-178, 1180-1185`).
  Whether a hidden window receives WM_PAINT from the vsync invalidation is UNVERIFIED.
- File drag and drop in: OLE `IDropTarget`, CF_HDROP only, always DROPEFFECT_COPY (`W/window.rs:879-1035`).
- The window class has a black background brush (`W/window.rs:1158`). A black flash on first show or resize is
  possible (UNVERIFIED).
- `PopUp` windows are not topmost and have no owner (`W/window.rs:394-395`). A "dialog" window is a separate top-level
  window that does not block its parent.
- Closing the last window ends the process even with tray state alive (see section 2).
