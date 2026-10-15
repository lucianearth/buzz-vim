//! The legend. One table drives the footer strip and the `?` overlay, so
//! they can't drift apart.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Sections,
    Confirm,
    Viewer,
    Help,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Sections => "SECTIONS",
            Mode::Confirm => "UPLOAD?",
            Mode::Viewer => "IMAGE",
            Mode::Help => "HELP",
        }
    }
}

/// (keys, what, shown in the footer strip)
pub fn legend(mode: Mode) -> &'static [(&'static str, &'static str, bool)] {
    match mode {
        Mode::Normal => &[
            ("j k ↑↓", "move", true),
            ("1-9", "tab", false),
            ("H L ⇥", "prev/next tab", false),
            ("⏎ →", "thread", true),
            ("i", "write", true),
            ("+", "👍", false),
            ("s", "sections", false),
            ("o", "image", true),
            ("[ ]", "narrower / wider (or drag the right edge)", false),
            ("P", "next color palette", false),
            ("a", "agents", true),
            ("?", "help", true),
            ("r", "reply in thread", false),
            ("h ← esc", "back (thread, agents)", false),
            ("g G", "oldest / newest", false),
            ("d u", "page down / up", false),
            ("^d ^u", "half page down / up", false),
            ("^v", "paste image or file", false),
            ("R", "reload sections", false),
            ("q", "quit", false),
            ("mouse", "click to pick, wheel scrolls", false),
        ],
        Mode::Insert => &[
            ("esc", "normal mode", true),
            ("⏎", "send", true),
            ("⇧⏎ ^j", "new line", true),
            (
                "```",
                "code block: ⏎ is a new line until you close it",
                false,
            ),
            ("@", "mention", true),
            ("⇥", "pick mention / indent", true),
            ("^v", "paste image or file", true),
            ("^w", "delete word", false),
            ("^u", "clear", false),
            ("drop file", "attach it (asks first)", false),
        ],
        Mode::Sections => &[
            ("j k", "move", true),
            ("space", "mark several", true),
            ("⏎", "switch", true),
            ("R", "reload from relay", true),
            ("esc", "cancel", true),
        ],
        Mode::Confirm => &[("y", "upload to Buzz", true), ("n esc", "cancel", true)],
        Mode::Viewer => &[
            ("← →", "prev / next image", true),
            ("esc q o", "close", true),
        ],
        Mode::Help => &[("esc ?", "close", true)],
    }
}
