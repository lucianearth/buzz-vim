//! `BUZZ_VIM_LOG=/path` appends diagnostics there. Off by default; the TUI
//! owns the screen, so there is nowhere else to print.

use std::io::Write;

pub fn line(s: impl AsRef<str>) {
    let Ok(p) = std::env::var("BUZZ_VIM_LOG") else {
        return;
    };
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    // It records opened URLs; only you should read it.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    if let Ok(mut f) = o.open(p) {
        let _ = writeln!(
            f,
            "{} {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            s.as_ref()
        );
    }
}
