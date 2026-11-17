//! The images the app draws as SVG - the keep-warm flame, from the path data of `Themes/Icons.xaml`. GPUI draws an
//! `svg()` as a mask in the element's text colour, so the fill here doesn't matter.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

use dnnm_keepwarm::view::{FLAME_FILLED, FLAME_OUTLINE, flame_svg};

pub const FLAME_OUTLINE_SVG: &str = "flame/outline.svg";
pub const FLAME_FILLED_SVG: &str = "flame/filled.svg";

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            FLAME_OUTLINE_SVG => Some(Cow::Owned(flame_svg(FLAME_OUTLINE, "#000").into_bytes())),
            FLAME_FILLED_SVG => Some(Cow::Owned(flame_svg(FLAME_FILLED, "#000").into_bytes())),
            // The other drawn icons (layout icons, the spinner, maximize): dnnm-ui's.
            other => dnnm_ui::shapes::asset(other).map(|svg| Cow::Owned(svg.into_bytes())),
        })
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([FLAME_OUTLINE_SVG, FLAME_FILLED_SVG].into_iter().filter(|p| p.starts_with(path)).map(Into::into).collect())
    }
}

/// The SVG to draw for a flame path of `dnnm_keepwarm::view::flame`.
pub fn flame_asset(path: &str) -> &'static str {
    if path == FLAME_OUTLINE { FLAME_OUTLINE_SVG } else { FLAME_FILLED_SVG }
}
