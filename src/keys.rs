//! The legend. One table drives the footer strip and the `?` overlay, so
//! they can't drift apart.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Sections,
    Order,
    Confirm,
    Viewer,
    Help,
    Hint,
    /// Asking before `!rotate`-ing an agent from the agents screen.
    Reset,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Sections => "SECTIONS",
            Mode::Order => "TAB ORDER",
            Mode::Confirm => "UPLOAD?",
            Mode::Viewer => "IMAGE",
            Mode::Help => "HELP",
            Mode::Hint => "LINK",
            Mode::Reset => "RESET?",
        }
    }
}

/// (keys, what, shown in the footer strip)
pub fn legend(mode: Mode) -> &'static [(&'static str, &'static str, bool)] {
    match mode {
        Mode::Normal => &[
            ("j k ↑↓", "move", true),
            ("n", "latest message (into its thread)", true),
            ("0-9", "tab", false),
            ("H L ⇥", "prev/next tab", false),
            ("< >", "move this tab left / right", false),
            ("T", "tab order", false),
            ("⏎ l →", "thread", true),
            ("i", "write", true),
            ("+", "👍", false),
            ("s", "sections", false),
            ("o", "image", true),
            ("O", "images in Preview (a thread: all of them)", false),
            ("f", "follow link", true),
            ("⌫ ^o", "back to where a link took you from", false),
            ("[ ]", "narrower / wider (or drag the right edge)", false),
            ("a", "agents", true),
            ("r", "agents screen: reset the picked agent here", false),
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
            ("m", "move this channel here", true),
            ("R", "reload from relay", true),
            ("esc", "cancel", true),
        ],
        Mode::Order => &[
            ("j k", "pick a tab", true),
            ("J K", "move it down / up", true),
            ("⏎", "go to it", true),
            ("esc", "done", true),
        ],
        Mode::Confirm => &[("y", "upload to Buzz", true), ("n esc", "cancel", true)],
        Mode::Reset => &[("y ⏎", "reset", true), ("n esc", "cancel", true)],
        Mode::Viewer => &[
            ("← →", "prev / next image", true),
            ("esc q o", "close", true),
            ("O", "Preview (a thread: all of them, one window)", true),
        ],
        Mode::Help => &[("esc ?", "close", true)],
        Mode::Hint => &[
            ("letters", "follow that link", true),
            ("esc", "cancel", true),
        ],
    }
}
