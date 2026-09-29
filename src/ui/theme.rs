//! Tokyo Night (night variant), with a few derived surface tones.

use gpui::{Hsla, Rgba, rgb, rgba};

pub const BG: u32 = 0x1a1b26;
pub const BG_DARK: u32 = 0x16161e;
pub const BG_DARKER: u32 = 0x111118;
pub const BG_HIGHLIGHT: u32 = 0x292e42;
pub const BG_HOVER: u32 = 0x1f2335;
pub const BG_SELECT: u32 = 0x283457;
pub const BORDER: u32 = 0x101014;
pub const BORDER_HI: u32 = 0x2a2f45;
pub const TERMINAL_BLACK: u32 = 0x414868;
pub const FG: u32 = 0xc0caf5;
pub const FG_DARK: u32 = 0xa9b1d6;
pub const FG_GUTTER: u32 = 0x3b4261;
pub const COMMENT: u32 = 0x565f89;
pub const DARK3: u32 = 0x545c7e;
pub const DARK5: u32 = 0x737aa2;

pub const BLUE: u32 = 0x7aa2f7;
pub const BLUE0: u32 = 0x3d59a1;
pub const BLUE1: u32 = 0x2ac3de;
pub const CYAN: u32 = 0x7dcfff;
pub const MAGENTA: u32 = 0xbb9af7;
pub const PURPLE: u32 = 0x9d7cd8;
pub const ORANGE: u32 = 0xff9e64;
pub const YELLOW: u32 = 0xe0af68;
pub const GREEN: u32 = 0x9ece6a;
pub const GREEN1: u32 = 0x73daca;
pub const RED: u32 = 0xf7768e;

pub const DIFF_ADD: u32 = 0x20303b;
pub const DIFF_ADD_HI: u32 = 0x2c4a3f;
pub const DIFF_DEL: u32 = 0x37222c;
pub const DIFF_DEL_HI: u32 = 0x57303a;
pub const OURS_BG: u32 = 0x1e2a4a;
pub const THEIRS_BG: u32 = 0x2e2346;
pub const RESOLVED_BG: u32 = 0x1d2e2a;

/// Lane colours for the commit graph.
pub const LANES: [u32; 10] = [
    BLUE, MAGENTA, GREEN1, ORANGE, CYAN, RED, YELLOW, GREEN, PURPLE, BLUE1,
];

pub fn lane(i: u8) -> u32 {
    LANES[i as usize % LANES.len()]
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// A palette colour at the given opacity (0..=1).
pub fn alpha(hex: u32, a: f32) -> Rgba {
    rgba((hex << 8) | ((a.clamp(0.0, 1.0) * 255.0) as u32))
}

/// The colour to use for a translucent fill behind `hex`. Warm hues turn
/// muddy brown when thinned over navy, so they get a cool blue wash instead
/// and keep their own colour for text and borders.
pub fn wash(hex: u32) -> u32 {
    match hex {
        YELLOW | ORANGE | GREEN => BLUE,
        _ => hex,
    }
}

/// Selection highlight: a flat translucent tint of `hex`.
pub fn tint(hex: u32, a: f32) -> Rgba {
    alpha(wash(hex), a)
}

pub const UI_FONT: &str = if cfg!(target_os = "macos") { ".SystemUIFont" } else { "Inter" };
pub const MONO_FONT: &str = if cfg!(target_os = "macos") { "Menlo" } else { "DejaVu Sans Mono" };

/// Avatar colours: the cool half of the palette.
const PEOPLE: [u32; 6] = [BLUE, MAGENTA, GREEN1, CYAN, PURPLE, BLUE1];

/// Stable colour for a person, from their email.
pub fn person(email: &str) -> u32 {
    let h = email.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    PEOPLE[h as usize % PEOPLE.len()]
}
