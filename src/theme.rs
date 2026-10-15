//! Grayscale only, in exact RGB so every terminal shows the same shades.
//! Hierarchy comes from brightness, bold and dim. There is a dark and a
//! light ramp; which one follows the Mac's appearance (see `watch`).

use ratatui::style::{Color, Modifier, Style};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static LIGHT: AtomicBool = AtomicBool::new(false);

/// Step `level` (0 = canvas, 23 = strongest text) on the current ramp.
/// Dark runs near black to near white; light is the same steps mirrored.
pub fn gray(level: u8) -> Color {
    let l = level.min(23);
    let v = if LIGHT.load(Ordering::Relaxed) {
        250 - 10 * l
    } else {
        8 + 10 * l
    };
    Color::Rgb(v, v, v)
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
    Style::new().fg(gray(21))
}
pub fn bright() -> Style {
    Style::new().fg(gray(23)).add_modifier(Modifier::BOLD)
}
pub fn muted() -> Style {
    Style::new().fg(gray(13))
}
pub fn faint() -> Style {
    Style::new().fg(gray(8))
}
pub fn rule() -> Style {
    Style::new().fg(gray(6))
}
pub fn badge() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(gray(18))
        .add_modifier(Modifier::BOLD)
}
pub fn tab_active() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(gray(22))
        .add_modifier(Modifier::BOLD)
}
pub fn tab_idle() -> Style {
    Style::new().fg(gray(15)).bg(gray(4))
}
/// Key hints read as inverted chips.
pub fn key() -> Style {
    Style::new()
        .fg(gray(1))
        .bg(gray(17))
        .add_modifier(Modifier::BOLD)
}
pub fn mention() -> Style {
    Style::new()
        .fg(gray(23))
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}
pub fn code() -> Style {
    Style::new().fg(gray(19)).bg(gray(2))
}
pub fn code_label() -> Style {
    Style::new().fg(gray(10)).bg(gray(2))
}
