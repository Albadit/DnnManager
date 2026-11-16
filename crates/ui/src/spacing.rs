//! The spacing rule: no margins and no per-side padding anywhere. Space **between** elements is their parent's flex
//! `gap`; space **inside** a container (a section, a card, a button, a wrap) is one padding call - `.p(x)` when it is
//! the same all round, else [`Pad::pad`], CSS's `padding: <vertical> <horizontal>`.

use gpui::{Pixels, Styled};

/// One padding call with two values, as CSS's `padding: 6px 14px`.
pub trait Pad: Styled + Sized {
    /// `vertical` above and below, `horizontal` left and right.
    fn pad(mut self, vertical: Pixels, horizontal: Pixels) -> Self {
        let style = self.style();
        style.padding.top = Some(vertical.into());
        style.padding.bottom = Some(vertical.into());
        style.padding.left = Some(horizontal.into());
        style.padding.right = Some(horizontal.into());
        self
    }
}

impl<T: Styled> Pad for T {}
