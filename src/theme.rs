//! Every color sits on one 24-step brightness ramp, in exact RGB so every
//! terminal shows the same shades. Hierarchy comes from brightness, bold and
//! dim. There is a dark and a light ramp; which one follows the Mac's
//! appearance (see `watch`).
//!
//! Palettes tint the ramp with soft pastels, one hue per role, and keep each
//! step's luminance exactly. Under a grayscale filter every palette looks the
//! same as plain gray.

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

static LIGHT: AtomicBool = AtomicBool::new(false);
static PALETTE: AtomicUsize = AtomicUsize::new(0);

/// What a color is for. Each palette gives a role its own hue.
#[derive(Clone, Copy)]
pub enum Role {
    /// Canvas, cards, rules, chip text.
    Surface,
    /// Body text and names.
    Text,
    /// Selected tab, key chips, badges, mentions.
    Accent,
    /// Secondary text: times, hints.
    Muted,
    /// Code blocks.
    Code,
}

/// (name, [(hue in degrees, chroma)] per role in `Role` order). Chroma is
/// OKLab; pastel stays low. Surfaces are the faintest so the canvas stays calm.
pub const PALETTES: &[(&str, [(f32, f32); 5])] = &[
    ("gray", [(0.0, 0.0); 5]),
    (
        "honey",
        [
            (75.0, 0.018),
            (60.0, 0.035),
            (300.0, 0.07),
            (140.0, 0.04),
            (230.0, 0.05),
        ],
    ),
    (
        "sea",
        [
            (240.0, 0.022),
            (220.0, 0.03),
            (35.0, 0.07),
            (175.0, 0.04),
            (290.0, 0.05),
        ],
    ),
    (
        "garden",
        [
            (150.0, 0.02),
            (130.0, 0.03),
            (0.0, 0.07),
            (95.0, 0.045),
            (260.0, 0.05),
        ],
    ),
];

pub fn palette() -> usize {
    PALETTE.load(Ordering::Relaxed)
}

pub fn palette_name() -> &'static str {
    PALETTES[palette()].0
}

/// Select a palette by name; unknown names leave it alone.
pub fn set_palette(name: &str) {
    if let Some(i) = PALETTES.iter().position(|(n, _)| *n == name) {
        PALETTE.store(i, Ordering::Relaxed);
    }
}

pub fn next_palette() -> &'static str {
    PALETTE.store((palette() + 1) % PALETTES.len(), Ordering::Relaxed);
    palette_name()
}

/// Step `level` (0 = canvas, 23 = strongest text) on the current ramp.
/// Dark runs near black to near white; light is the same steps mirrored.
pub fn gray(level: u8) -> Color {
    tone(Role::Surface, level)
}

/// Step `level` for `role`: the ramp's gray, tinted by the palette.
pub fn tone(role: Role, level: u8) -> Color {
    let l = level.min(23);
    let v = if LIGHT.load(Ordering::Relaxed) {
        250 - 10 * l
    } else {
        8 + 10 * l
    };
    let (hue, chroma) = PALETTES[palette()].1[role as usize];
    if chroma == 0.0 {
        return Color::Rgb(v, v, v);
    }
    let [r, g, b] = tint(v, hue, chroma);
    Color::Rgb(r, g, b)
}

fn lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn enc(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Relative luminance (Rec. 709) of linear RGB.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// OKLab to linear sRGB (Björn Ottosson's matrices).
fn oklab(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

/// The gray `v` with hue and chroma added, then OKLab lightness solved so
/// the luminance equals the gray's. Chroma shrinks until the color fits sRGB.
fn tint(v: u8, hue: f32, chroma: f32) -> [u8; 3] {
    let y = lin(v as f32 / 255.0);
    let (sin, cos) = hue.to_radians().sin_cos();
    let mut c = chroma;
    for _ in 0..12 {
        let (a, b) = (c * cos, c * sin);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if luma(oklab(mid, a, b)) < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let rgb = oklab((lo + hi) / 2.0, a, b);
        if rgb.iter().all(|&x| (-0.0005..=1.0005).contains(&x)) {
            return rgb.map(|x| (enc(x.clamp(0.0, 1.0)) * 255.0).round() as u8);
        }
        c *= 0.75;
    }
    [v, v, v]
}

pub fn set_light(on: bool) {
    LIGHT.store(on, Ordering::Relaxed);
}

/// Pick the ramp now and re-check every 2 s, so flipping the Mac between
/// light and dark follows while the app is open. `BUZZ_VIM_THEME=light|dark`
/// pins it; off macOS it stays dark.
pub fn watch() {
    match std::env::var("BUZZ_VIM_THEME").as_deref() {
        Ok("light") => return set_light(true),
        Ok("dark") => return,
        _ => {}
    }
    if !cfg!(target_os = "macos") {
        return;
    }
    set_light(!mac_dark());
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            set_light(!mac_dark());
        }
    });
}

/// `AppleInterfaceStyle` is "Dark" in dark mode and missing in light mode.
fn mac_dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark")
}

pub fn text() -> Style {
    Style::new().fg(tone(Role::Text, 21))
}
pub fn bright() -> Style {
    Style::new()
        .fg(tone(Role::Text, 23))
        .add_modifier(Modifier::BOLD)
}
pub fn muted() -> Style {
    Style::new().fg(tone(Role::Muted, 13))
}
pub fn faint() -> Style {
    Style::new().fg(tone(Role::Muted, 8))
}
pub fn rule() -> Style {
    Style::new().fg(gray(6))
}
pub fn badge() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(tone(Role::Accent, 18))
        .add_modifier(Modifier::BOLD)
}
pub fn tab_active() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(tone(Role::Accent, 22))
        .add_modifier(Modifier::BOLD)
}
pub fn tab_idle() -> Style {
    Style::new().fg(gray(15)).bg(gray(4))
}
/// Key hints read as inverted chips.
pub fn key() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(tone(Role::Accent, 17))
        .add_modifier(Modifier::BOLD)
}
pub fn mention() -> Style {
    Style::new()
        .fg(tone(Role::Accent, 23))
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}
pub fn code() -> Style {
    Style::new().fg(tone(Role::Code, 19)).bg(gray(2))
}
pub fn code_label() -> Style {
    Style::new().fg(tone(Role::Code, 10)).bg(gray(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every palette step keeps its gray's luminance, so a grayscale filter
    /// shows exactly the plain ramp (within rounding).
    #[test]
    fn palettes_match_the_gray_ramp() {
        for &(name, roles) in PALETTES {
            for v in (8..=238).step_by(10) {
                let y = lin(v as f32 / 255.0);
                for (hue, chroma) in roles {
                    let c = tint(v as u8, hue, chroma);
                    let got = luma(c.map(|x| lin(x as f32 / 255.0)));
                    let gray_of = enc(got) * 255.0;
                    assert!(
                        (gray_of - v as f32).abs() < 1.5,
                        "{name} {v} {hue}: {c:?} reads as {gray_of}, y {y}"
                    );
                }
            }
        }
    }
}
