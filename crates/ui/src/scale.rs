//! The UI scale (Settings → General → UI scale): every size the app gives - widths, gaps, paddings, radii, fonts,
//! icons - goes through [`px`], which multiplies it by the scale. GPUI 0.2.2 has no zoom of its own (its scale is the
//! monitor's DPI, and its `rem` unit isn't used here), so the app's own `px` is the zoom: everything is laid out larger
//! or smaller in the same window coordinates, so mouse positions, drags and hit-testing stay right.

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::Pixels;

/// The scale as `f32` bits; 1.0 until the settings set it.
static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000);

/// `percent` of the normal size (80 to 200, as the setting allows); takes effect at the next draw.
pub fn set_ui_scale(percent: u32) {
    SCALE.store(scale_of(percent).to_bits(), Ordering::Relaxed);
}

/// The scale for `percent`, kept to what the setting allows (80 to 200).
fn scale_of(percent: u32) -> f32 {
    percent.clamp(80, 200) as f32 / 100.0
}

/// The UI scale now: 1.0 is 100 %.
pub fn ui_scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// `pixels` at the UI scale - use this, not `gpui::px`, for every size in the app.
pub fn px(pixels: f32) -> Pixels {
    gpui::px(pixels * ui_scale())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The scale itself isn't set here: it is global, and the crate's other tests measure with `px` at the same time.
    #[test]
    fn a_percent_is_kept_to_what_the_setting_allows() {
        assert_eq!(scale_of(100), 1.0);
        assert_eq!(scale_of(150), 1.5);
        assert_eq!(scale_of(999), 2.0);
        assert_eq!(scale_of(10), 0.8);
        assert_eq!(ui_scale(), 1.0, "100 % until the settings say otherwise");
    }
}
