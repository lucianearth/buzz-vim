//! The goofy bits. Kept to a few spots: the banner, empty channels, the
//! "thinking" critter for agents, and the little burst when a message sends.

pub const BANNER: &[&str] = &["█▄▄ █ █ ▀█ ▀█   █ █ █ █▀▄▀█", "█▄█ █▄█ █▄ █▄   ▀▄▀ █ █ ▀ █"];

pub const BEE: &[&str] = &[
    r"        \     /       ",
    r"     .-. \   / .-.    ",
    r"    (   )-\ /-(   )   ",
    r"     `-'  (o)  `-'    ",
    r"        /|||||\       ",
    r"       ( ||||| )~     ",
    r"        \|||||/       ",
    r"          \_/         ",
];

pub const EMPTY_LINES: &[&str] = &[
    "nothing here yet. the bee is waiting.",
    "quiet channel. press i and say something.",
    "bzz. no messages.",
];

/// Little bee flying left-to-right while something loads or an agent thinks.
pub fn flight(tick: u64, width: usize) -> String {
    let w = width.max(8);
    let pos = (tick as usize) % w;
    let wing = if tick.is_multiple_of(2) { "ʚ" } else { "ɞ" };
    let mut s = " ".repeat(pos);
    s.push_str(wing);
    s.push('●');
    s
}

/// Spinner for agents that are mid-turn. Every frame is the same width so
/// the name next to it doesn't jiggle.
pub const THINK: &[&str] = &["( ·.·)", "(·.· )", "( -.-)", "( ·.·)", "(°.° )", "(·.· )"];

/// Shown for a moment after a message goes out.
pub const SENT: &[&str] = &["bzz!", "~bzzz~", "*whoosh*", "delivered ✦", "off it goes"];

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn think_frames_share_a_width() {
        let w = super::THINK[0].width();
        assert!(super::THINK.iter().all(|f| f.width() == w));
    }
}
