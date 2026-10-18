//! The goofy bits. Kept to a few spots: the banner, empty-channel lines, the
//! "thinking" critter for agents, and the little burst when a message sends.

pub const BANNER: &[&str] = &["█▄▄ █ █ ▀█ ▀█   █ █ █ █▀▄▀█", "█▄█ █▄█ █▄ █▄   ▀▄▀ █ █ ▀ █"];

pub const EMPTY_LINES: &[&str] = &[
    "nothing here yet. the bees are waiting.",
    "quiet channel. press i and say something.",
    "bzz. no messages.",
];

/// A dot drifting left-to-right while something loads or an agent thinks.
pub fn flight(tick: u64, width: usize) -> String {
    let w = width.max(8);
    let pos = (tick as usize) % w;
    let mut s = " ".repeat(pos);
    s.push('●');
    s
}

/// Ramp level for the logo dot: dim to bright and back, about 2.4 s a
/// breath at the 150 ms tick.
pub fn breath(tick: u64) -> u8 {
    const LOW: u64 = 11;
    const HIGH: u64 = 23;
    let half = HIGH - LOW;
    let t = tick % (2 * half);
    (LOW + if t <= half { t } else { 2 * half - t }) as u8
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
    fn breath_stays_on_the_ramp_and_moves_one_step_a_tick() {
        let levels: Vec<u8> = (0..60).map(super::breath).collect();
        assert!(levels.iter().all(|l| (11..=23).contains(l)));
        assert!(levels.windows(2).all(|w| w[0].abs_diff(w[1]) == 1));
    }

    #[test]
    fn think_frames_share_a_width() {
        let w = super::THINK[0].width();
        assert!(super::THINK.iter().all(|f| f.width() == w));
    }
}
