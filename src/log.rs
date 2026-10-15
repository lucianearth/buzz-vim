//! `BUZZ_VIM_LOG=/path` appends diagnostics there. Off by default; the TUI
//! owns the screen, so there is nowhere else to print.

use std::io::Write;

pub fn line(s: impl AsRef<str>) {
    let Ok(p) = std::env::var("BUZZ_VIM_LOG") else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(
            f,
            "{} {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            s.as_ref()
        );
    }
}
