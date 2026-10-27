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
    /// Asking before voting on a control request (a kind-7 reaction).
    Vote,
    /// `B`: whose Claude subscription this channel's agents spend.
    Subscription,
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
            Mode::Vote => "VOTE?",
            Mode::Subscription => "SUBSCRIPTION",
        }
    }
}

/// Where a key is listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Show {
    /// Only in the `?` overlay.
    Help,
    /// The footer strip and the overlay.
    Both,
    /// Only the footer (and a box's own legend): a box's moving and
    /// closing, which the overlay says once under "move".
    Footer,
}

impl Show {
    pub fn footer(self) -> bool {
        self != Show::Help
    }
    pub fn help(self) -> bool {
        self != Show::Footer
    }
}

/// (keys, what, where it shows)
pub type Key = (&'static str, &'static str, Show);
/// A titled run of keys: one section of the `?` overlay.
pub type Section = (&'static str, &'static [Key]);

use Show::{Both, Footer, Help};

const MOVE: Section = (
    "move",
    &[
        ("j k ↑↓", "move", Help),
        ("g G", "oldest / newest", Help),
        ("d u ^d ^u", "page / half page", Help),
        ("n", "latest", Both),
        ("h ← esc", "back / close", Help),
    ],
);
const TABS: Section = (
    "tabs",
    &[
        ("0-9", "go to tab (0 needs you)", Help),
        ("H L ⇥", "prev / next tab", Help),
        ("< >", "move tab left / right", Help),
        ("T", "tab order", Help),
        ("!", "next ⚑, oldest first", Help),
    ],
);
const MESSAGES: Section = (
    "messages",
    &[
        ("i", "write", Both),
        ("r", "reply in thread", Help),
        ("+", "👍", Help),
        ("⏎ l →", "open thread / tab 0 row", Help),
        ("o", "image", Both),
        ("O", "images in Preview", Help),
        ("f", "links", Both),
        ("⌫ ^o", "back from link", Help),
        ("^v", "paste image or file", Help),
    ],
);
const APP: Section = (
    "app",
    &[
        ("s", "sections", Help),
        ("R", "reload sections", Help),
        ("[ ]", "narrower / wider (or drag)", Help),
        ("?", "help", Both),
        ("^c", "quit", Help),
        ("mouse", "click picks, wheel scrolls", Help),
    ],
);
const SCREENS: Section = (
    "screens",
    &[
        ("a", "agents", Both),
        ("c", "checklist", Both),
        ("w", "routing", Help),
        ("C", "control", Help),
        ("B", "set Claude subscription", Help),
        ("q Q ^q", "ask #quick", Help),
    ],
);
const AGENTS: Section = ("agents (a)", &[("r", "reset the picked agent", Help)]);
const ROUTING: Section = (
    "routing (w)",
    &[
        ("t", "next time window", Help),
        ("⇥ ⇧⇥", "next / prev card", Help),
        ("⏎ l", "open: sample, why, record", Help),
        ("s", "another sample", Help),
        ("o", "open its thread", Help),
    ],
);
const CONTROL: Section = (
    "control (C)",
    &[
        ("⏎ l", "open request / topic / day", Help),
        ("1-9", "vote on a request", Help),
        ("o", "a note's images", Help),
        ("r", "reload #control", Help),
    ],
);
const INSERT: Section = (
    "insert (i)",
    &[
        ("esc", "normal mode", Footer),
        ("⏎", "send", Help),
        ("⇧⏎ ^j", "new line", Both),
        ("```", "code block: ⏎ = new line", Help),
        ("@", "mention", Both),
        ("⇥", "pick mention / indent", Help),
        // Said under "messages".
        ("^v", "paste image or file", Footer),
        ("^w ^u", "delete word / all", Help),
        ("drop file", "attach it (asks first)", Help),
    ],
);
const SECTIONS: Section = (
    "sections (s)",
    &[
        ("j k", "move", Footer),
        ("space x", "mark several", Both),
        ("⏎", "switch", Both),
        ("m", "move this channel here", Both),
        ("R", "reload from relay", Both),
        ("esc", "cancel", Footer),
    ],
);
const ORDER: Section = (
    "tab order (T)",
    &[
        ("j k", "pick a tab", Footer),
        ("J K", "move it down / up", Both),
        ("⏎", "go to it", Both),
        ("a", "archive it", Both),
        ("esc", "done", Footer),
    ],
);
/// Upload, reset, archive and vote all ask the same way.
const CONFIRM: Section = (
    "confirms (upload, reset, vote, archive)",
    &[("y ⏎", "yes", Both), ("n esc", "cancel", Both)],
);
const QUICK: Section = (
    "Q box (q Q ^q)",
    &[
        ("⏎", "send to #quick", Both),
        ("^q", "add / drop this channel", Both),
        ("⇧⏎ ^j", "new line", Footer),
        ("esc", "close (keeps the text)", Footer),
    ],
);
const REPLY: Section = (
    "reply (r on tab 0)",
    &[
        ("⏎", "send into its thread", Both),
        ("⇧⏎ ^j", "new line", Footer),
        ("esc", "close (keeps the text)", Footer),
    ],
);
const SUBSCRIPTION: Section = (
    "Claude subscription (B)",
    &[
        ("j k", "pick", Footer),
        ("⇥ h l", "this channel / default", Both),
        ("⏎", "use it (signs, publishes)", Both),
        ("esc", "close", Footer),
    ],
);
const VIEWER: Section = (
    "image (o)",
    &[
        ("← →", "prev / next image", Both),
        ("esc q o", "close", Footer),
        ("O", "open in Preview", Both),
    ],
);
const HINT: Section = (
    "links (f)",
    &[
        ("letters", "follow link / copy code", Both),
        ("esc", "cancel", Footer),
    ],
);

/// The `?` overlay, left column then right. A short screen clips the
/// bottom, so the boxes that list their own keys go last.
pub const HELP: [&[Section]; 2] = [
    &[MOVE, TABS, MESSAGES, APP, INSERT, REPLY, QUICK],
    &[
        SCREENS,
        AGENTS,
        ROUTING,
        CONTROL,
        SUBSCRIPTION,
        SECTIONS,
        ORDER,
        VIEWER,
        HINT,
        CONFIRM,
    ],
];

/// A mode's keys, by section.
pub fn sections(mode: Mode) -> &'static [Section] {
    match mode {
        // Footer order: n i o f a c ?.
        Mode::Normal => &[MOVE, TABS, MESSAGES, SCREENS, APP, AGENTS, ROUTING, CONTROL],
        Mode::Insert => &[INSERT],
        Mode::Sections => &[SECTIONS],
        Mode::Order => &[ORDER],
        Mode::Confirm | Mode::Reset | Mode::Archive | Mode::Vote => &[CONFIRM],
        Mode::Quick => &[QUICK],
        Mode::Reply => &[REPLY],
        Mode::Subscription => &[SUBSCRIPTION],
        Mode::Viewer => &[VIEWER],
        Mode::Help => &[("help", &[("esc ?", "close", Both)])],
        Mode::Hint => &[HINT],
    }
}

/// A mode's keys in order: the footer strip shows those with `footer()`.
pub fn legend(mode: Mode) -> impl Iterator<Item = &'static Key> + Clone {
    sections(mode).iter().flat_map(|s| s.1)
}
