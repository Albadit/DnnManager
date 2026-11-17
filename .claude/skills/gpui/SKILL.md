---
name: gpui
description: >-
  Writes clean, fast, idiomatic GPUI 0.2.2 code for DNN Manager's window: views and Render, elements (div, deferred,
  anchored, canvas, uniform_list, svg), styling, events, focus and actions, entities and contexts, async tasks, and
  the project's dnnm-ui building blocks, and GPUI's Windows backend. Has a reference built from a full read of GPUI
  0.2.2's source (every fact with its file:line), what differs on Zed's main branch, and what was measured in this
  app; looks anything else up in the source or docs.rs/gpui/0.2.2 before writing. Use for any change to GPUI code in crates/app, crates/ui, crates/dialogs, crates/terminal or
  crates/keepwarm - a new view or control, layout, popups and tooltips, hover and cursors, scrolling, keyboard,
  rendering speed - and to review GPUI code for quality.
---

# GPUI

DNN Manager's window is GPUI **0.2.2** (pinned in `Cargo.lock`). GPUI is pre-1.0 and changes between versions, so
nothing is written from memory: every type, method and behaviour used is checked in the docs or the source of this
exact version first.

## Where the truth is

| Source | Use it for |
|---|---|
| The reference files below | Read the one for the area first - each fact carries a `file:line` in GPUI 0.2.2's source, from a full read of it. |
| GPUI's source, `~/.cargo/registry/src/*/gpui-0.2.2/src/` (and `examples/`, `docs/`) | Anything the references don't cover, and checking a reference line before relying on it in a tricky spot. Grep it. |
| [docs.rs/gpui/0.2.2/gpui](https://docs.rs/gpui/0.2.2/gpui/) | Signatures and doc comments (built for Linux - Windows-only code isn't shown; use the source for that). |
| `crates/ui` (dnnm-ui) | The project's controls on top of GPUI - use them before writing a new one ([api-map.md](reference/api-map.md)). |
| [.docs/design-system.md](../../../.docs/design-system.md) | Sizes, colours, spacing and the rules every control follows. |

**Zed's `main` is not 0.2.2.** It moved on (platform crates, renamed events, accessibility, quit modes…); code or
docs from github.com/zed-industries/zed often don't compile here. Check every API against 0.2.2 -
[main-vs-0.2.2.md](reference/main-vs-0.2.2.md) lists the known differences.

## Reference files

| Area | File |
|---|---|
| Entities, contexts, notify / emit / observe / subscribe, async tasks, focus, globals, element state, redraws, caching, quit | [app-model.md](reference/app-model.md) |
| Element lifecycle, ids, every mouse / key / scroll / drag handler, event dispatch, actions and key bindings, tab order, text input | [elements-input.md](reference/elements-input.md) |
| Style defaults and every `Styled` method, Taffy layout, text (wrap, truncate, shaping), svg / img / canvas / anchored / deferred / animation / lists | [layout-style-text.md](reference/layout-style-text.md) |
| The Windows backend: messages, title bar hit-testing, frames, DPI, dialogs, clipboard, close / quit, what panics or does nothing on Windows | [windows-platform.md](reference/windows-platform.md) |
| What `main` has that 0.2.2 doesn't, and Zed `ui` idioms worth copying | [main-vs-0.2.2.md](reference/main-vs-0.2.2.md) |
| What this project learnt in the window (measured), and the docs page / source file per topic | [behaviour.md](reference/behaviour.md), [api-map.md](reference/api-map.md) |

## Workflow

1. **Read what is there.** The view or control being changed, and how its neighbours do the same thing. Reuse a
   dnnm-ui helper or an existing pattern before adding one.
2. **Look the API up** for every GPUI item you are not certain of: the reference file for the area, then the 0.2.2
   source when the behaviour matters (timing, layout, hit-testing, Windows support). Never take an API from Zed's
   `main` without finding it in 0.2.2.
3. **Write it** by the rules below.
4. **Build and try it in the window**, never only in tests: `cargo build -p dnnmanager --target-dir target/ui`, run
   with `DNNMANAGER_GPUI_FIXTURE` ([.docs/testing.md](../../../.docs/testing.md)), screenshot and measure what
   changed - alignment in pixels, hover, the popup's place, the cursor. Report what could not be tried.
5. **Gate:** `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`, CRLF and no BOM.
6. **Update** design-system.md when a size, colour, layer or rule changes, and add what you learnt about GPUI to
   reference/behaviour.md (with how it was verified).

## Rules for clean GPUI code

- **State in entities, look in `render`.** `render` only reads state and builds elements; it never starts work,
  writes files or blocks. Changes go through `cx.listener(...)` / `entity.update(...)` and end with `cx.notify()`.
- **Sizes through the app's `px`** (`dnnm_ui::px`, re-exported by `crates/ui`): it applies Settings' UI scale. Not
  `gpui::px` - except the window's own bounds on the screen.
- **Theme, not literals.** Colours, fonts, radii and sizes come from `Theme::get(cx)` and `icons::SIZE*`; a new
  value goes into the theme or design-system.md, not inline.
- **dnnm-ui first.** Buttons, icon buttons, inputs, selects, menus, check boxes, tooltips (`.tip()`), info tips and
  dialog frames exist - extend them rather than restyle a `div` in place.
- **Popups:** `deferred(anchored()...)` with `.with_priority(2)` (menus 3 over their outside-press catcher at 2),
  placed from the button's `ui::Anchor`, not from the mouse; Esc and a press outside close them.
- **Ids:** every element with interaction, hover, tooltip, scroll or animation gets a stable, unique `.id(...)`.
- **Text wraps:** a flex child holding text that may be long needs `.min_w_0()` (or a width), or it pushes past its
  parent instead of wrapping.
- **Small renders.** Big lists use `uniform_list`/`list`, not hundreds of children; hoist work that doesn't change
  per frame out of `render`; clone `SharedString`/`Rc`, not owned `String`s, into closures.
- **No timers for drawing.** Animate with `with_animation`; redraw with `cx.notify()`; never poll.
- **Comments say why** (a GPUI limitation worked around, a measured size), in the codebase's plain style.

## Report

Name what was changed and how it was checked in the window (screenshots, pixel measurements), what GPUI pages or
source files decided it, and anything left untried.
