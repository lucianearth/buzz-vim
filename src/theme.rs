//! The ramp from `buzz_vim_core::palette` as ratatui styles. Which ramp,
//! dark or light, follows the Mac's appearance (see `watch`).

use buzz_vim_core::palette;
pub use buzz_vim_core::palette::{Role, hue, nth};
use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static LIGHT: AtomicBool = AtomicBool::new(false);

fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::Rgb(r, g, b)
}

fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// Step `level` in `hue` at full color.
pub fn ink(hue: f32, level: u8) -> Color {
    rgb(palette::ink(hue, level, light()))
}

/// Step `level` (0 = canvas, 23 = strongest text) on the current ramp.
/// Dark runs near black to near white; light is the same steps mirrored.
pub fn gray(level: u8) -> Color {
    tone(Role::Surface, level)
}

/// Step `level` for `role`: the ramp's gray, tinted by the role's hue.
pub fn tone(role: Role, level: u8) -> Color {
    rgb(palette::tone(role, level, light()))
}

/// A fixed "#rrggbb" (the Routing page's stream and outcome colours), the
/// same on both ramps. On the light ramp `dark` stands in, when given: a
/// hue darkened to read as text on the light page.
pub fn hex(hex: &str, dark: Option<&str>) -> Color {
    let h = match dark {
        Some(d) if light() => d,
        _ => hex,
    }
    .trim_start_matches('#');
    let p = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .unwrap_or(0)
    };
    Color::Rgb(p(0), p(2), p(4))
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
/// A person's name in its own color. A few steps under the brightest
/// text, where there is room in sRGB for the hue to show.
pub fn name(key: &str) -> Style {
    Style::new()
        .fg(ink(hue(key), 18))
        .add_modifier(Modifier::BOLD)
}
/// A chip (key hint, active tab) in `hue`.
pub fn chip(hue: f32) -> Style {
    Style::new()
        .fg(gray(1))
        .bg(ink(hue, 17))
        .add_modifier(Modifier::BOLD)
}
pub fn link() -> Style {
    Style::new()
        .fg(tone(Role::Accent, 20))
        .add_modifier(Modifier::UNDERLINED)
}
pub fn code() -> Style {
    Style::new().fg(tone(Role::Code, 19)).bg(gray(2))
}
pub fn code_label() -> Style {
    Style::new().fg(tone(Role::Code, 10)).bg(gray(2))
}
