//! How the WPF app shows keep warm, word for word: the row's flame and texts (`ProjectRow.cs`, `ProjectMenu.cs`,
//! `ProjectsPage.xaml(.cs)`), what switching it writes to the activity log (`ServerStore.ToggleKeepWarm`), the overview's
//! Keep warm card (`ProjectView.xaml(.cs)`) and the flame itself (`KeepWarmFlame` in `ButtonStyles.xaml`, `FlameOutline`
//! and `FlameFilled` in `Icons.xaml`, `KeepWarmFg` in the themes).

use std::time::Duration;

use crate::model::{KeepWarmState, KeepWarmStatus};
use crate::plan::KeepWarmPlan;
use crate::rules;

/// How a site's keep-warm flame looks: an outline while off; filled while on - in the flame's colour when warm,
/// pulsing while a request is on its way or due, grey while paused, with a red dot while failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepWarmLook {
    Off,
    Warm,
    Busy,
    Paused,
    Problem,
}

pub fn look(status: &KeepWarmStatus) -> KeepWarmLook {
    match status.state {
        KeepWarmState::Off => KeepWarmLook::Off,
        KeepWarmState::Warm => KeepWarmLook::Warm,
        KeepWarmState::Waiting | KeepWarmState::WarmingUp => KeepWarmLook::Busy,
        KeepWarmState::Paused => KeepWarmLook::Paused,
        KeepWarmState::Failing | KeepWarmState::Stopped => KeepWarmLook::Problem,
    }
}

/// For the flame's pulse.
pub fn busy(status: &KeepWarmStatus) -> bool {
    look(status) == KeepWarmLook::Busy
}

/// What switching it does: "Keep warm" or "Stop keeping warm" (the flame's accessible name, the menu item).
pub fn action(on: bool) -> &'static str {
    if on { "Stop keeping warm" } else { "Keep warm" }
}

/// The flame's (and the overview switch's) tooltip: only what a click does.
pub fn tip(on: bool) -> &'static str {
    if on {
        "Stop keeping this website ready - it may open slowly after a while without visits"
    } else {
        "Keep this website ready, so it opens fast even after a while without visits"
    }
}

/// Why keep warm can't be switched on; None when it can, or is on. `has_url`: the site has a web binding.
pub fn unavailable(on: bool, has_url: bool) -> Option<&'static str> {
    if on || has_url { None } else { Some("The site has no http or https binding to request") }
}

/// A site with an address can be kept warm; one that is kept warm can always be switched off.
pub fn can_toggle(on: bool, has_url: bool) -> bool {
    on || has_url
}

/// The row menu item's tooltip: how it is going while on, why it can't be while off.
pub fn menu_tooltip(status: &KeepWarmStatus, has_url: bool) -> Option<String> {
    if status.is_on() { Some(status.text.clone()) } else { unavailable(false, has_url).map(str::to_string) }
}

/// The expanded row's fact.
pub const DETAILS_LABEL: &str = "Keep warm";

/// The bulk button: `all_warm` - every selected site that can be is kept warm already.
pub fn bulk_tooltip(all_warm: bool) -> &'static str {
    if all_warm {
        "Stop keeping the selected websites warm"
    } else {
        "Keep the selected websites warm, so they open fast after a while without visits"
    }
}

pub fn bulk_name(all_warm: bool) -> &'static str {
    if all_warm { "Stop keeping the selected websites warm" } else { "Keep the selected websites warm" }
}

/// The activity log's line when the user switches a site off.
pub fn switched_off_log(site: &str) -> String {
    format!("No longer keeping '{site}' warm.")
}

/// The activity log's line when the user switches a site on.
pub fn switched_on_log(site: &str, ping_minutes: u32) -> String {
    format!(
        "Keeping '{site}' warm - while DNN Manager runs it requests the site at least every {} (sooner when its app pool needs it) and warms it up again after a recycle.",
        rules::span(Duration::from_secs(60 * u64::from(ping_minutes)))
    )
}

// ─── The overview's Keep warm card ───────────────────────────────────

pub const CARD_TITLE: &str = "Keep warm";
pub const CARD_HINT: &str = "While DNN Manager runs - minimized too - it requests the site before IIS would shut it down for being idle, and warms it up again after a recycle, so its next page opens at once instead of after DNN starting up (3 to 10 seconds). No browser: a small request DNN answers in milliseconds, none while the site is in use.";
/// The button (and its tooltip), enabled while the site is kept warm.
pub const CHECK_NOW: &str = "Check now";

/// One line of the card's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub label: &'static str,
    pub value: String,
    pub warning: bool,
}

/// How the site is kept warm - its IIS idle time-out, how often it is requested, and where.
pub fn facts(site: &str, plan: &KeepWarmPlan) -> Vec<Fact> {
    let fact = |label, value: String| Fact { label, value, warning: false };
    let setting = Duration::from_secs(60 * u64::from(plan.ping_minutes));
    // Not skipped while in use when the idle time-out is too short for two intervals (about a minute).
    let in_use = if rules::may_skip_when_in_use(site, plan.interval, plan.idle_timeout) {
        ", none while the site is in use"
    } else {
        ""
    };
    let idle = match plan.idle_timeout {
        None => "unknown - the app pool's settings couldn't be read".to_string(),
        Some(idle) if idle.is_zero() => {
            "none - IIS never shuts the site's worker process down for being idle".to_string()
        }
        Some(idle) => format!(
            "{} - IIS shuts the site's worker process down after that long without a request",
            rules::span(idle)
        ),
    };
    let requests = if plan.interval < setting {
        format!(
            "every {} at most{in_use} - the settings' {} would come too close to the idle time-out",
            rules::span(plan.interval),
            rules::span(setting)
        )
    } else {
        format!("every {} at most{in_use}", rules::span(plan.interval))
    };
    let mut list = vec![fact("IIS idle time-out", idle), fact("Requests", requests)];
    match &plan.target {
        Some(target) => {
            list.push(fact("Keep-alive page", target.display_url(&plan.ping_path)));
            list.push(fact(
                "Warm-up page",
                format!("{} - when the site has no worker process", target.display_url(&plan.warm_up_path)),
            ));
            list.push(fact(
                "Sent to",
                format!(
                    "{}:{} as {} - this PC, whatever DNS says",
                    target.connect_host, target.port, target.host_header
                ),
            ));
        }
        None => list.push(Fact {
            label: "Requests go to",
            value: "nowhere - the site has no http or https binding to request".into(),
            warning: true,
        }),
    }
    list
}

// ─── The flame ───────────────────────────────────────────────────────

/// Keep warm's flame: Codicons "flame" (Microsoft, https://github.com/microsoft/vscode-codicons, CC BY 4.0). The whole
/// icon - an outline: its inner contour is a hole (even-odd fill) - while a site isn't kept warm.
pub const FLAME_OUTLINE: &str = "M8.1694 2.38161C8.44582 2.23863 8.72367 2.14217 8.96199 2.08199C8.98644 2.62292 9.15365 3.15614 9.38208 3.66553C9.7057 4.38719 10.1817 5.12315 10.6465 5.83569C10.6644 5.86313 10.6823 5.89054 10.7002 5.91792C11.1553 6.61534 11.5997 7.2963 11.9379 7.97958C12.2887 8.68856 12.5034 9.35877 12.5034 10C12.5034 11.1529 12.1584 12.1473 11.5114 12.8484C10.8704 13.5429 9.88689 14 8.50338 14C7.10629 14 6.13556 13.5958 5.45709 12.9749C4.77045 12.3465 4.33513 11.4518 4.09927 10.3914C3.87708 9.39256 4.07301 8.49755 4.33374 7.84072C4.37616 7.73386 4.42008 7.63386 4.46386 7.54138L4.58938 7.79243C4.97001 8.55369 5.89571 8.86226 6.65697 8.48162C7.50314 8.05854 7.73725 7.02244 7.37909 6.24601C7.00688 5.43912 6.71741 4.43545 6.97653 3.65811C7.17125 3.07394 7.63415 2.65846 8.1694 2.38161ZM4.11056 6.18914L4.10959 6.19037L4.10785 6.19258L4.1029 6.19894L4.08731 6.21936C4.07451 6.23635 4.057 6.26009 4.03559 6.29026C3.99281 6.35055 3.93432 6.43679 3.86679 6.54649C3.732 6.76546 3.55959 7.08054 3.40429 7.47178C3.0944 8.25245 2.84483 9.35744 3.12312 10.6086C3.38774 11.7982 3.89791 12.9035 4.78195 13.7126C5.67415 14.5292 6.89927 15 8.50338 15C10.1211 15 11.3875 14.4571 12.2463 13.5266C13.0989 12.6027 13.5034 11.3471 13.5034 10C13.5034 9.14123 13.2179 8.31144 12.8341 7.53604C12.4688 6.7979 11.9948 6.07168 11.5492 5.38919C11.5274 5.35579 11.5057 5.32249 11.484 5.2893C11.0111 4.56435 10.5805 3.89406 10.2945 3.25635C10.0087 2.61894 9.89683 2.07627 9.99247 1.59806C10.0219 1.45117 9.98382 1.29885 9.88885 1.18301C9.79389 1.06716 9.65198 1 9.50218 1C9.08022 1 8.377 1.14838 7.70997 1.49339C7.03689 1.84154 6.33312 2.42606 6.02784 3.34189C5.63773 4.51222 6.08683 5.83198 6.47104 6.66488C6.64207 7.03564 6.49357 7.44529 6.20976 7.5872C5.94247 7.72084 5.61745 7.6125 5.48381 7.34521L4.9494 6.27639C4.87302 6.12365 4.72397 6.02044 4.55412 6.0027C4.38427 5.98497 4.21685 6.05547 4.11056 6.18914Z";

/// Its outer contour alone - a filled flame - while a site is kept warm.
pub const FLAME_FILLED: &str = "M4.11056 6.18914L4.10959 6.19037L4.10785 6.19258L4.1029 6.19894L4.08731 6.21936C4.07451 6.23635 4.057 6.26009 4.03559 6.29026C3.99281 6.35055 3.93432 6.43679 3.86679 6.54649C3.732 6.76546 3.55959 7.08054 3.40429 7.47178C3.0944 8.25245 2.84483 9.35744 3.12312 10.6086C3.38774 11.7982 3.89791 12.9035 4.78195 13.7126C5.67415 14.5292 6.89927 15 8.50338 15C10.1211 15 11.3875 14.4571 12.2463 13.5266C13.0989 12.6027 13.5034 11.3471 13.5034 10C13.5034 9.14123 13.2179 8.31144 12.8341 7.53604C12.4688 6.7979 11.9948 6.07168 11.5492 5.38919C11.5274 5.35579 11.5057 5.32249 11.484 5.2893C11.0111 4.56435 10.5805 3.89406 10.2945 3.25635C10.0087 2.61894 9.89683 2.07627 9.99247 1.59806C10.0219 1.45117 9.98382 1.29885 9.88885 1.18301C9.79389 1.06716 9.65198 1 9.50218 1C9.08022 1 8.377 1.14838 7.70997 1.49339C7.03689 1.84154 6.33312 2.42606 6.02784 3.34189C5.63773 4.51222 6.08683 5.83198 6.47104 6.66488C6.64207 7.03564 6.49357 7.44529 6.20976 7.5872C5.94247 7.72084 5.61745 7.6125 5.48381 7.34521L4.9494 6.27639C4.87302 6.12365 4.72397 6.02044 4.55412 6.0027C4.38427 5.98497 4.21685 6.05547 4.11056 6.18914Z";

/// The paths' bounds: WPF draws them `Stretch="Uniform"` into the flame's box, so the box fits these, not 16×16.
pub const FLAME_VIEW_BOX: &str = "3 1 10.5034 14";
/// The flame's height in the rows and the card (and the bulk button's icon): the 13 px of the glyphs beside it. GPUI
/// scales an SVG to its box's width, so the box takes the path's proportions - `flame_width`.
pub const FLAME_SIZE: f32 = 13.0;

/// The flame's width for a height: the path's 10.5 × 14.
pub fn flame_width(height: f32) -> f32 {
    height * 10.5034 / 14.0
}
/// The failing dot: 6×6 at the bottom right, 2 px past the right edge and 1 px past the bottom.
pub const FLAME_DOT_SIZE: f32 = 6.0;
pub const FLAME_DOT_OVERHANG: (f32, f32) = (2.0, 1.0);
/// The pulse while busy: opacity 1 → 0.3 in 0.6 s, back again, forever - only while seen, not in efficiency mode,
/// not with animations off.
pub const FLAME_PULSE: (f32, f32, Duration) = (1.0, 0.3, Duration::from_millis(600));
/// `KeepWarmFg`: the flame while the site is kept warm.
pub const KEEP_WARM_FG_DARK: u32 = 0xFF8A3D;
pub const KEEP_WARM_FG_LIGHT: u32 = 0xD9480F;

/// Which colour a flame takes - the theme's brush of that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlameBrush {
    /// `TextMuted`.
    TextMuted,
    /// `KeepWarmFg`.
    KeepWarmFg,
}

/// How to draw the flame for a look: which path, which colour, whether the red (`ErrorText`) dot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlameStyle {
    pub path: &'static str,
    pub brush: FlameBrush,
    pub dot: bool,
    pub pulse: bool,
}

pub fn flame(look: KeepWarmLook) -> FlameStyle {
    let filled = |brush, dot, pulse| FlameStyle { path: FLAME_FILLED, brush, dot, pulse };
    match look {
        KeepWarmLook::Off => FlameStyle { path: FLAME_OUTLINE, brush: FlameBrush::TextMuted, dot: false, pulse: false },
        KeepWarmLook::Warm => filled(FlameBrush::KeepWarmFg, false, false),
        KeepWarmLook::Busy => filled(FlameBrush::KeepWarmFg, false, true),
        KeepWarmLook::Paused => filled(FlameBrush::TextMuted, false, false),
        KeepWarmLook::Problem => filled(FlameBrush::KeepWarmFg, true, false),
    }
}

/// The flame as an SVG document in `fill` (e.g. "#FF8A3D"), for an `svg()` element: even-odd, as WPF fills a path.
pub fn flame_svg(path: &str, fill: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{FLAME_VIEW_BOX}"><path fill="{fill}" fill-rule="evenodd" d="{path}"/></svg>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::tests::site;
    use crate::settings::KeepWarmSettings;
    use dnnm_application::iis::IisBinding;

    #[test]
    fn looks_and_texts_follow_the_state() {
        let status = |state| KeepWarmStatus::new(state, "x");
        assert_eq!(look(&KeepWarmStatus::off()), KeepWarmLook::Off);
        assert!(busy(&status(KeepWarmState::WarmingUp)) && busy(&status(KeepWarmState::Waiting)));
        assert_eq!(look(&status(KeepWarmState::Stopped)), KeepWarmLook::Problem);
        assert_eq!(flame(KeepWarmLook::Off).path, FLAME_OUTLINE);
        assert!(flame(KeepWarmLook::Problem).dot);
        assert_eq!(flame(KeepWarmLook::Paused).brush, FlameBrush::TextMuted);
        assert_eq!(action(true), "Stop keeping warm");
        assert_eq!(unavailable(false, false), Some("The site has no http or https binding to request"));
        assert!(!can_toggle(false, false) && can_toggle(true, false));
        assert!(switched_on_log("shop", 5).contains("at least every 5 minutes (sooner"));
        assert!(flame_svg(FLAME_FILLED, "#FF8A3D").contains("fill-rule=\"evenodd\""));
    }

    #[test]
    fn the_card_says_how_and_where() {
        let s = site(vec![IisBinding::parse("http", "*:80:shop.dnndev.me", false)]);
        let plan = KeepWarmPlan::new(
            &KeepWarmSettings { ping_minutes: 30, ..Default::default() },
            &s,
            Some(Duration::from_secs(20 * 60)),
        );
        let f = facts("shop", &plan);
        assert_eq!(
            f[0].value,
            "20 minutes - IIS shuts the site's worker process down after that long without a request"
        );
        assert_eq!(
            f[1].value,
            "every 8 minutes at most, none while the site is in use - the settings' 30 minutes would come too close to the idle time-out"
        );
        assert_eq!(f[2].value, "http://shop.dnndev.me/KeepAlive.aspx");
        assert_eq!(f[4].value, "127.0.0.1:80 as shop.dnndev.me - this PC, whatever DNS says");
        let none = facts("shop", &KeepWarmPlan::new(&KeepWarmSettings::default(), &site(vec![]), None));
        assert!(none[2].warning && none[0].value.starts_with("unknown"));
    }
}
