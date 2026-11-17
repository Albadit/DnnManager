//! The UI scale for the terminal panel, as dnnm-ui's (`dnnm_ui::scale`) - this crate doesn't depend on dnnm-ui, so the
//! app sets both: every size here goes through [`px`].

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::Pixels;

/// The scale as `f32` bits; 1.0 until the app sets it.
static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000);

/// The scale, 1.0 for 100 % - the app passes dnnm-ui's.
pub fn set_ui_scale(scale: f32) {
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
}

/// `pixels` at the UI scale.
pub fn px(pixels: f32) -> Pixels {
    gpui::px(pixels * f32::from_bits(SCALE.load(Ordering::Relaxed)))
}
