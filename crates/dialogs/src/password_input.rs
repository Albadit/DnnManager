//! A password field (`Controls/PasswordInput.xaml`): what is typed shows as dots - never the password itself. It takes
//! typing (also through an IME), Backspace, Ctrl+A to replace it all, Ctrl+V to paste; it never copies or cuts. It
//! uses the text field's own key bindings (`dnnm_ui::text_input`, context "TextInput"), so it needs nothing bound of
//! its own. The cursor stays at the end - a password is typed again, not edited in the middle.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, LayoutId, Pixels, ShapedLine, Style, TextRun, UTF16Selection, Window, div,
    fill, point, prelude::*, px, relative, size,
};

use dnnm_ui::text_input::{Backspace, Clear, Delete, Paste, SelectAll};
use dnnm_ui::theme::Theme;

/// The password changed. The event holds nothing: the view reads [`PasswordInput::password`] when it needs it.
#[derive(Debug, Clone, Copy)]
pub struct PasswordChanged;

pub struct PasswordInput {
    focus_handle: FocusHandle,
    content: String,
    all_selected: bool,
}

impl EventEmitter<PasswordChanged> for PasswordInput {}

const DOT: char = '•';

impl PasswordInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { focus_handle: cx.focus_handle(), content: String::new(), all_selected: false }
    }

    pub fn password(&self) -> &str {
        &self.content
    }

    pub fn set_password(&mut self, password: &str, cx: &mut Context<Self>) {
        if self.content != password {
            self.content = password.to_string();
            self.all_selected = false;
            cx.emit(PasswordChanged);
            cx.notify();
        }
    }

    fn edit(&mut self, new_text: &str, cx: &mut Context<Self>) {
        if self.all_selected {
            self.content.clear();
            self.all_selected = false;
        }
        self.content.push_str(&new_text.replace(['\r', '\n'], ""));
        cx.emit(PasswordChanged);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.all_selected {
            self.content.clear();
            self.all_selected = false;
        } else {
            self.content.pop();
        }
        cx.emit(PasswordChanged);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.all_selected {
            self.backspace(&Backspace, window, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.all_selected = !self.content.is_empty();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(&text, cx);
        }
    }

    /// Esc lets the key go on - to close what the field is in - as an empty text field does.
    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        cx.propagate();
    }

    fn dots(&self) -> String {
        std::iter::repeat_n(DOT, self.content.chars().count()).collect()
    }
}

impl EntityInputHandler for PasswordInput {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        // What the IME is told is the dots - never the password.
        let dots = self.dots();
        actual_range.replace(0..dots.encode_utf16().count());
        Some(dots)
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.content.chars().count();
        Some(UTF16Selection { range: if self.all_selected { 0..end } else { end..end }, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.chars().count())
    }
}

struct DotsElement {
    input: Entity<PasswordInput>,
}

impl IntoElement for DotsElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DotsElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<ShapedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<ShapedLine> {
        let dots = self.input.read(cx).dots();
        let style = window.text_style();
        let run = TextRun {
            len: dots.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        Some(window.text_system().shape_line(dots.into(), font_size, &[run], None))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        line: &mut Option<ShapedLine>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let input = self.input.read(cx);
        let focus_handle = input.focus_handle.clone();
        let all_selected = input.all_selected;
        let colors = Theme::get(cx).colors;
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let Some(line) = line.take() else { return };
        let end = line.width;
        if all_selected {
            window.paint_quad(fill(
                Bounds::new(bounds.origin, size(end, bounds.size.height)),
                colors.accent.opacity(0.3),
            ));
        }
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if focus_handle.is_focused(window) && !all_selected {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left() + end, bounds.top()), size(px(1.), bounds.size.height)),
                colors.text_primary,
            ));
        }
    }
}

impl Render for PasswordInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .w_full()
            .overflow_hidden()
            .child(DotsElement { input: cx.entity() })
    }
}

impl Focusable for PasswordInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
