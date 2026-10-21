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
    /// The Q box: a quick question to #quick.
    Quick,
    /// A reply box for a tab-0 row, into its thread.
    Reply,
    /// Asking before archiving the tab picked in the tab menu.
    Archive,
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
            Mode::Quick => "Q",
            Mode::Reply => "REPLY",
            Mode::Archive => "ARCHIVE?",
        }
    }
}

/// (keys, what, shown in the footer strip)
pub fn legend(mode: Mode) -> &'static [(&'static str, &'static str, bool)] {
    match mode {
        Mode::Normal => &[
            ("j k ↑↓", "move", false),
            ("n", "latest", true),
            ("0", "needs you", false),
            ("1-9", "tab", false),
            ("!", "next ⚑, oldest first", false),
            ("H L ⇥", "prev/next tab", false),
            ("< >", "move this tab left / right", false),
            ("T", "tab order", false),
            ("⏎ l →", "thread", false),
            ("i", "write", true),
            ("+", "👍", false),
            ("s", "sections", false),
            ("o", "image", true),
            ("O", "images in Preview (a thread: all of them)", false),
            ("f", "links", true),
            ("f", "copy a code block", false),
            ("⌫ ^o", "back to where a link took you from", false),
            ("[ ]", "narrower / wider (or drag the right edge)", false),
            ("a", "agents", true),
            ("c", "checklist", true),
            ("w", "routing: how each stop was routed", false),
            ("t ⏎ s o", "routing: window, sample, another, thread", false),
            (
                "Q ^q",
                "ask #quick; again in the box: + this channel",
                false,
            ),
            ("r", "agents screen: reset the picked agent here", false),
            ("?", "help", true),
            ("r", "reply in thread", false),
            ("⏎ r", "tab 0: open / reply", false),
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
            ("⏎", "send", false),
            ("⇧⏎", "new line", true),
            ("^j", "new line", false),
            (
                "```",
                "code block: ⏎ is a new line until you close it",
                false,
            ),
            ("@", "mention", true),
            ("⇥", "pick mention / indent", false),
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
            ("a", "archive it", true),
            ("esc", "done", true),
        ],
        Mode::Confirm => &[("y", "upload to Buzz", true), ("n esc", "cancel", true)],
        Mode::Reset => &[("y ⏎", "reset", true), ("n esc", "cancel", true)],
        Mode::Quick => &[
            ("⏎", "send to #quick", true),
            (
                "^q",
                "+ this channel and its latest messages (again: off)",
                true,
            ),
            ("⇧⏎ ^j", "new line", true),
            ("esc", "close (keeps the text)", true),
        ],
        Mode::Archive => &[("y ⏎", "archive", true), ("n esc", "cancel", true)],
        Mode::Reply => &[
            ("⏎", "send into its thread", true),
            ("⇧⏎ ^j", "new line", true),
            ("esc", "close (keeps the text)", true),
        ],
        Mode::Viewer => &[
            ("← →", "prev / next image", true),
            ("esc q o", "close", true),
            ("O", "Preview (a thread: all of them, one window)", true),
        ],
        Mode::Help => &[("esc ?", "close", true)],
        Mode::Hint => &[
            ("letters", "follow that link, copy that code", true),
            ("esc", "cancel", true),
        ],
    }
}
