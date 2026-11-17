# GPUI 0.2.2 - where to look

Base: `https://docs.rs/gpui/0.2.2/gpui/` - append the page. Source: `~/.cargo/registry/src/*/gpui-0.2.2/src/`.

## The model

| Topic | Docs page | Source |
|---|---|---|
| Application, windows | `struct.Application.html`, `struct.App.html`, `struct.Window.html` | `app.rs`, `window.rs` |
| State: entities, contexts | `struct.Entity.html`, `struct.WeakEntity.html`, `struct.Context.html`, `_ownership_and_data_flow/index.html` | `app/entity_map.rs`, `app/context.rs` |
| Views | `trait.Render.html`, `trait.RenderOnce.html`, `struct.AnyView.html` | `view.rs` |
| Events between entities | `trait.EventEmitter.html`, `struct.Subscription.html` | `app.rs` (`subscribe`, `observe`) |
| Globals | `trait.Global.html` | `global.rs` |
| Async | `struct.Task.html`, `struct.BackgroundExecutor.html`, `struct.ForegroundExecutor.html` | `executor.rs` |
| Tests | `attr.test.html`, `struct.TestAppContext.html` | `app/test_context.rs` |

## Elements

| Element | Docs page | Source |
|---|---|---|
| `div` | `fn.div.html`, `struct.Div.html`, `struct.Interactivity.html` | `elements/div.rs` |
| Popups on top | `fn.deferred.html`, `fn.anchored.html`, `struct.Anchored.html` | `elements/deferred.rs`, `elements/anchored.rs` |
| Custom paint / measuring | `fn.canvas.html` | `elements/canvas.rs` |
| Long lists | `fn.uniform_list.html`, `fn.list.html`, `struct.ListState.html` | `elements/uniform_list.rs`, `elements/list.rs` |
| Images, SVG | `fn.img.html`, `fn.svg.html` | `elements/img.rs`, `elements/svg.rs` |
| Text | `struct.StyledText.html`, `struct.TextStyle.html`, `struct.TextSystem.html` | `elements/text.rs`, `text_system.rs` |
| Animation | `trait.AnimationExt.html`, `struct.Animation.html` | `elements/animation.rs` |

## Styling and input

| Topic | Docs page | Source |
|---|---|---|
| Style methods | `trait.Styled.html`, `struct.StyleRefinement.html` | `styled.rs`, `style.rs` |
| Lengths, colours | `fn.px.html`, `fn.rems.html`, `fn.relative.html`, `fn.rgb.html`, `fn.hsla.html` | `geometry.rs`, `color.rs` |
| Mouse, hover, click, drag | `trait.InteractiveElement.html`, `trait.StatefulInteractiveElement.html` | `elements/div.rs`, `interactive.rs` |
| Children | `trait.ParentElement.html` | `element.rs` |
| Focus | `struct.FocusHandle.html`, `trait.Focusable.html` | `window.rs` |
| Actions, key bindings | `macro.actions.html`, `derive.Action.html`, `struct.KeyBinding.html`, `struct.Keymap.html` | `action.rs`, `keymap/` |
| Text input | `trait.EntityInputHandler.html` | `input.rs` |
| Cursors | `enum.CursorStyle.html` | `platform.rs`, Windows mapping in `platform/windows/util.rs` |
| Scrolling | `struct.ScrollHandle.html`, `struct.UniformListScrollHandle.html` | `elements/div.rs` |

## dnnm-ui (crates/ui) - use before writing new

| Need | Helper |
|---|---|
| Glyph icon / drawn icon / spinner | `ui::icon`, `ui::line_icon`, `ui::shape_icon` + `ui::shapes`, `ui::spinner` |
| Buttons, links, switch, check box | `ui::button`, `ui::icon_button(_sized)`, `ui::link`, `ui::switch`, `ui::check_box`, `ui::status_dot` |
| Tooltip lined up with its element | `.tip(text)`, `.tip_beside(text)` (trait `ui::Tip`) |
| ⓘ beside a title or label | `ui::info_tip`, `ui::with_tip`, `ui::with_tip_sized` |
| Where a button is (for its popup) | `ui::Anchor` (`measure()`, `below(right)`, `contains`) |
| Context menu with submenus | `ui::menu::{ContextMenu, MenuItem, render}` |
| Dialog frame, fields, select, radio | `ui::dialog::{frame, frame_with_tip, ok_cancel, label, hint, field_error, text_field, check, radio, select, option}` |
| Text field | `ui::text_input::TextInput` |
| Fade and pulse | `ui::fade_in`, `ui::pulse` (respect `Theme::animations`) |
| Find highlights | `ui::LineMarks`, `ui::marked_text`, `ui::find_colors` |
| Spacing | `ui::Pad` (`.pad(vertical, horizontal)`), `ui::spacing` |
| Theme | `ui::theme::Theme::get(cx)` - colours, `text.*` sizes, `metrics.*` |
