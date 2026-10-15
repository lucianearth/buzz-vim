//! Grayscale only. Uses the xterm 256-color gray ramp (232..=255), which
//! every terminal lucian is likely to use renders the same way, so there is
//! nothing to configure. Hierarchy comes from brightness, bold and dim.

use ratatui::style::{Color, Modifier, Style};

pub fn gray(level: u8) -> Color {
    // 0 = near black, 23 = near white.
    Color::Indexed(232 + level.min(23))
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
pub fn key() -> Style {
    Style::new()
        .fg(gray(23))
        .bg(gray(5))
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
