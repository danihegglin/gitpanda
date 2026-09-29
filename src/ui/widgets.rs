//! Small shared building blocks.

use gpui::{Div, ElementId, SharedString, Stateful, div, prelude::*, px};

use super::theme::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Primary,
    Danger,
    Ghost,
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, tone: Tone) -> Stateful<Div> {
    let (bg, fg, hover, border) = match tone {
        Tone::Normal => (BG_HIGHLIGHT, FG, 0x33395a, BORDER_HI),
        Tone::Primary => (BLUE0, 0xe6ecff, 0x4a6ab8, 0x4a6ab8),
        Tone::Danger => (0x4a2230, RED, 0x5c2a3b, 0x5c2a3b),
        Tone::Ghost => (0x00000000, FG_DARK, BG_HIGHLIGHT, 0x00000000),
    };
    let bg = if tone == Tone::Ghost { alpha(0, 0.0) } else { alpha(bg, 1.0) };
    let border = if tone == Tone::Ghost { alpha(0, 0.0) } else { alpha(border, 1.0) };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(26.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_color(c(fg))
        .text_size(px(12.))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(c(hover)))
        .child(label.into())
}

/// A compact icon-ish button used on hover in lists.
pub fn mini_button(id: impl Into<ElementId>, label: impl Into<SharedString>, color: u32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(7.))
        .rounded(px(4.))
        .bg(alpha(wash(color), 0.12))
        .text_color(c(color))
        .text_size(px(11.))
        .cursor_pointer()
        .hover(move |s| s.bg(alpha(wash(color), 0.25)))
        .child(label.into())
}

pub fn pill(text: impl Into<SharedString>, color: u32, filled: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(18.))
        .px(px(6.))
        .rounded(px(4.))
        .text_size(px(11.))
        .whitespace_nowrap()
        .when(filled, |d| d.bg(alpha(color, 0.9)).text_color(c(BG_DARKER)))
        .when(!filled, |d| {
            d.bg(alpha(wash(color), 0.1)).border_1().border_color(alpha(color, 0.5)).text_color(c(color))
        })
        .child(text.into())
}

pub fn status_color(letter: &str) -> u32 {
    match letter {
        "A" | "U" => GREEN,
        "D" => RED,
        "R" | "C" => MAGENTA,
        "!" => ORANGE,
        _ => YELLOW,
    }
}

pub fn status_badge(letter: &'static str) -> Div {
    let col = status_color(letter);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(4.))
        .bg(alpha(wash(col), 0.16))
        .text_color(c(col))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::BOLD)
        .child(letter)
}

pub fn avatar(name: &str, email: &str, size: f32) -> Div {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let col = person(email);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded_full()
        .bg(alpha(col, 0.22))
        .border_1()
        .border_color(alpha(col, 0.6))
        .text_color(c(col))
        .text_size(px(size * 0.4))
        .font_weight(gpui::FontWeight::BOLD)
        .child(if initials.is_empty() { "?".to_string() } else { initials })
}

/// The gitpanda mark: a panda face on a blue tile, drawn from
/// circles so it scales cleanly to any size.
pub fn logo(size: f32) -> Div {
    let dark = c(BG_DARKER);
    // A shape at (left, top) with the given width and height, all as
    // fractions of the tile.
    let blob = move |l: f32, t: f32, w: f32, h: f32, color: gpui::Hsla| {
        div()
            .absolute()
            .left(px(l * size))
            .top(px(t * size))
            .w(px(w * size))
            .h(px(h * size))
            .rounded_full()
            .bg(color)
    };
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.27))
        .bg(c(BLUE0))
        .child(blob(0.15, 0.17, 0.25, 0.25, dark)) // ears
        .child(blob(0.60, 0.17, 0.25, 0.25, dark))
        .child(blob(0.18, 0.24, 0.64, 0.60, c(FG))) // face
        .child(blob(0.31, 0.43, 0.16, 0.19, dark)) // eye patches
        .child(blob(0.53, 0.43, 0.16, 0.19, dark))
        .child(blob(0.375, 0.48, 0.055, 0.055, c(FG))) // eyes
        .child(blob(0.57, 0.48, 0.055, 0.055, c(FG)))
        .child(blob(0.445, 0.64, 0.11, 0.065, dark)) // nose
}

/// Section header in the side panels.
pub fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(10.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(DARK5))
        .child(text.into())
}

// ---- time ------------------------------------------------------------------

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn relative_time(ts: i64) -> String {
    let d = (now() - ts).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", d / 60),
        3600..86400 => format!("{}h ago", d / 3600),
        86400..2_592_000 => format!("{}d ago", d / 86400),
        _ => date(ts),
    }
}

/// `YYYY-MM-DD` (UTC).
pub fn date(ts: i64) -> String {
    let (y, m, d) = civil(ts.div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn date_time(ts: i64) -> String {
    let secs = ts.rem_euclid(86400);
    format!("{} {:02}:{:02} UTC", date(ts), secs / 3600, secs % 3600 / 60)
}

/// Days since the epoch to (year, month, day); Howard Hinnant's algorithm.
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::date(0), "1970-01-01");
        assert_eq!(super::date(1_700_000_000), "2023-11-14");
        assert_eq!(super::date(951_782_400), "2000-02-29");
    }
}
