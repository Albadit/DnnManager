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

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// GPUI's margin and per-side padding methods - none of them is used: spacing between elements is flex + gap,
    /// inside a container one `.p()` or `.pad()` (design-system.md, Spacing).
    const FORBIDDEN: &[&str] = &["m", "mt", "mb", "ml", "mr", "mx", "my", "pt", "pb", "pl", "pr", "px", "py"];

    /// Whether `line` calls one of them: `.mt(`, `.px_2(`, `.m_auto(`… (`px(` alone is GPUI's length, not a call on
    /// an element).
    fn forbidden_call(line: &str) -> Option<String> {
        let mut rest = line;
        while let Some(i) = rest.find('.') {
            rest = &rest[i + 1..];
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if rest[name.len()..].starts_with('(') {
                let base = name.split('_').next().unwrap_or("");
                if FORBIDDEN.contains(&base) && (name == base || name[base.len()..].starts_with('_')) {
                    return Some(name);
                }
            }
        }
        None
    }

    fn scan(dir: &Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for (n, line) in text.lines().enumerate() {
                    // Comments, and this test's own examples.
                    if line.trim_start().starts_with("//") || line.contains("forbidden_call(") {
                        continue;
                    }
                    if let Some(call) = forbidden_call(line) {
                        found.push(format!("{}:{}: .{call}(", path.display(), n + 1));
                    }
                }
            }
        }
    }

    #[test]
    fn the_scan_knows_a_margin_from_a_length() {
        assert_eq!(forbidden_call("div().mt(px(4.0))").as_deref(), Some("mt"));
        assert_eq!(forbidden_call("d.px_2()").as_deref(), Some("px_2"));
        assert_eq!(forbidden_call("div().m_auto()").as_deref(), Some("m_auto"));
        assert_eq!(forbidden_call("div().pad(px(3.0), px(0.0)).w(px(4.0))"), None);
        assert_eq!(forbidden_call("div().p(px(6.0)).max_w(px(600.0))"), None);
    }

    #[test]
    fn no_margins_or_per_side_padding_anywhere() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut found = Vec::new();
        scan(&crates, &mut found);
        assert!(found.is_empty(), "margins or per-side padding (use gap and .p / .pad):\n{}", found.join("\n"));
    }
}
