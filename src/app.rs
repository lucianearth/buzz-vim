//! App state and input handling. Rendering is in `ui.rs`, relay IO in `sync.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;

use crate::checklist::Checklists;
use crate::clip::Pending;
use crate::config::Loop;
use crate::control::Board;
use crate::draft::Draft;
use crate::keys::Mode;
use crate::live::ConnState;
use crate::media::Media;
use crate::mention;
use crate::model::Store;
use crate::needs::{self, Row};
use crate::readstate::{self, ReadState};
use crate::routing::{self, Decision, Node, Window};
use crate::sections::Section;
use crate::telemetry::Agents;
use crate::unread::{self, Next};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Timeline,
    Thread,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Message(Pane, String),
    Compose,
    Section(usize),
    ConfirmYes,
    ConfirmNo,
    CloseThread,
    /// The reading-width slider on the body's right edge.
    Handle,
    /// A row in the checklist pane.
    ChecklistItem(usize),
    /// Tab 0, Needs you.
    NeedsTab,
    /// A row on tab 0.
    NeedsRow(usize),
    /// A box on the routing view.
    RoutingNode(Node),
}

/// A piece of a link on screen, from the last draw.
#[derive(Clone, Debug)]
pub struct LinkHit {
    pub rect: Rect,
    pub url: String,
    /// (message id, link number in it): the pieces of one wrapped link share it.
    pub key: (String, usize),
}

/// A cursor move this big is g / G: to the first or last message.
const JUMP: usize = 100_000;

/// Where a link took you from, for Backspace / ^o.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub channel: String,
    pub selected: Option<String>,
    /// Open thread root and the reply picked in it.
    pub thread: Option<(String, Option<String>)>,
}

/// Things the user asked for that need the network. `main` executes them.
pub enum Action {
    OpenChannel(String),
    Send {
        channel: String,
        root: Option<String>,
        parent: Option<String>,
    },
    ThumbsUp {
        target: String,
    },
    /// `!rotate` to one agent: a fresh session in that channel.
    Reset {
        channel: String,
        root: Option<String>,
        agent: String,
    },
    /// Archive a channel for everyone (kind 9002, `archived` true).
    Archive(String),
    Upload,
    LoadOlder(String),
    ReloadSections,
    /// Move a channel into a section (`None` = the unsectioned group).
    MoveChannel {
        channel: String,
        section: Option<String>,
    },
    /// A section's tabs in a new order (synced, see sections::LANE_ORDER).
    Order {
        section: String,
        ids: Vec<String>,
    },
    LoadImage(String),
    EncodeImage((String, u16, u16)),
    /// Decode the pending upload so the confirm box can show it.
    PreviewPending,
    /// Read a dropped file (or the clipboard, for None) off the UI loop;
    /// files can be hundreds of MB.
    ReadAttachment(Option<std::path::PathBuf>),
    OpenThread {
        channel: String,
        root: String,
    },
    /// Hand a URL to the OS (browser, Buzz Desktop).
    OpenUrl(String),
    /// Put text on the system clipboard.
    Copy(String),
    /// Download images and open them in the OS viewer (Preview), all in
    /// one window, showing `current` first.
    OpenImages {
        urls: Vec<String>,
        current: usize,
    },
    /// A Q: a new top-level message in #quick, @mentioning `agent`.
    /// `text` is what was typed, back in the box if the relay refuses.
    SendQuick {
        channel: String,
        content: String,
        agent: String,
        text: String,
    },
    /// A reply written somewhere other than the compose box (tab 0's
    /// inline reply): into `root`'s thread, or the channel for None.
    SendDraft {
        channel: String,
        root: Option<String>,
        draft: Draft,
        /// Notified too, without an @ in the text: the row's asker.
        to: Vec<String>,
    },
    /// Look up these people's names (the Q agent's, in #quick).
    WantProfiles(Vec<String>),
    /// Read the control channel's notes, replies and reactions.
    LoadControl,
    /// A vote on a control request: a kind-7 reaction to the note.
    React {
        target: String,
        emoji: String,
        label: String,
    },
    SaveState,
    Quit,
}

pub struct App {
    pub store: Store,
    pub mode: Mode,
    pub conn: ConnState,
    pub tabs: Vec<String>,
    pub active: usize,
    pub sections: Vec<Section>,
    pub sections_loaded: bool,
    pub channels_loaded: bool,
    /// Section ids whose channels are the tabs.
    pub chosen: Vec<String>,
    pub picker_cursor: usize,
    /// The tab the reorder pop-up (T) is on.
    pub order_cursor: usize,
    pub picker_marks: HashSet<String>,
    /// Marks changed since the picker opened. Until then Enter switches to
    /// the section under the cursor.
    pub picker_touched: bool,
    pub focus: Pane,
    /// channel -> selected top-level message (None = follow newest)
    pub selected: HashMap<String, String>,
    /// open thread: (channel, root) and selected reply
    pub thread: Option<(String, String)>,
    pub thread_selected: Option<String>,
    pub draft: Draft,
    pub thread_draft: Draft,
    pub mention_pick: usize,
    pub pending: Option<Pending>,
    pub uploading: bool,
    pub status: Option<(String, Instant)>,
    pub sent_flash: Option<(usize, Instant)>,
    pub tick: u64,
    /// The empty/loading screen on show and the tick it appeared, so the
    /// garden grows from the start each time one comes up.
    pub empty: Option<(String, u64)>,
    pub empty_prev: Option<(String, u64)>,
    /// The full-screen agents view (`a`).
    pub agents_view: bool,
    /// The card picked on the agents view (j/k), for `r`.
    pub agents_cursor: usize,
    /// Agent (pubkey, name) waiting on the y/n before its reset, and the
    /// (channel, thread root) it was asked about.
    pub reset_ask: Option<(String, String, String, Option<String>)>,
    /// The channel `a` in the tab menu asked about archiving.
    pub archive_ask: Option<String>,
    pub agents: Agents,
    pub media: Media,
    pub picker: Option<Picker>,
    pub viewer: Option<String>,
    /// Every image in the pane the viewer was opened from, oldest first.
    pub viewer_list: Vec<String>,
    pub hits: Vec<(Rect, Hit)>,
    pub links: Vec<LinkHit>,
    /// Hint letters typed so far in `f` mode.
    pub hint_typed: String,
    /// A channel outside the chosen sections that a link opened, shown as
    /// one extra tab at the end until you leave it.
    pub extra: Option<String>,
    /// Where links took you from, newest last.
    pub jumps: Vec<Jump>,
    /// Timeline/thread page height from the last draw, for ^d/^u.
    pub page: usize,
    /// Line offset into a message taller than the pane: (message id, lines
    /// from its top). usize::MAX = its bottom. Keyed by id, so moving to
    /// another message drops it.
    pub peek: Option<(String, usize)>,
    /// From the last draw of the focused pane, when its cursor message is
    /// taller than the pane: (id, message height, pane height, offset shown).
    pub tall: Option<(String, usize, usize, usize)>,
    /// Reading width from the slider, in columns; None = full width.
    pub read_width: Option<u16>,
    /// Widest the body can be, from the last draw (for `[` `]`).
    pub body_max: u16,
    pub body_x: u16,
    /// Dragging the slider, and when it was last clicked (double-click resets).
    pub dragging: bool,
    pub handle_click: Option<std::time::Instant>,
    pub actions: Vec<Action>,
    /// Drafts of messages in flight, restored if the relay rejects them.
    /// event id -> (channel, thread root, the draft it came from)
    pub pending_sends: HashMap<String, (String, Option<String>, Draft)>,
    pub relay_host: String,
    /// channel -> (bot I last tagged there, when). Saved, so it survives
    /// restarts even when that message is older than the loaded history.
    pub last_bot: HashMap<String, (String, u64)>,
    /// Text put in a blank draft on entering insert; Esc drops it untouched.
    prefill: Option<String>,
    /// Read positions shared with Desktop and our other devices.
    pub read: ReadState,
    /// channel -> the bell's last place there (see [`unread::bell`]).
    pub bell: HashMap<String, (u64, Next)>,
    /// Identities the relay has archived: left out of @ completion.
    pub archived: HashSet<String>,
    /// A message to keep at the top of its pane while it is the cursor
    /// (the bell lands with it at the top).
    pub anchor: Option<(Pane, String)>,
    /// The operator loop's checklist records, every channel's.
    pub checklists: Checklists,
    /// The checklist pane (`c`) and the item picked in it.
    pub checklist_view: bool,
    pub checklist_cursor: usize,
    /// The Q box (`Q` / `^q`): its text, and whether it carries this channel.
    pub quick_draft: Draft,
    pub quick_include: bool,
    pub loop_: Loop,
    /// Tab 0, Needs you (`0`): on screen instead of the current tab.
    pub needs_view: bool,
    pub needs_cursor: usize,
    /// Tab 0's rows as last drawn; the cursor picks from these.
    pub needs: Vec<Row>,
    /// The row `!` went to last.
    pub needs_jump: Option<String>,
    /// The row an inline reply (`r` on tab 0) answers, and its text.
    pub reply_to: Option<Row>,
    pub reply_draft: Draft,
    /// Sends from the reply box, by event id, and the row each answered:
    /// a failed one goes back into the box, not a compose box.
    pub reply_sends: HashMap<String, Row>,
    /// The row the last reply answered, until sync knows its event id.
    pub reply_sent: Option<Row>,
    /// The routing view (`w`): how the watcher routed each stop it judged.
    pub routing_view: bool,
    pub routing_window: Window,
    /// The box picked (j k ⇥).
    pub routing_node: Node,
    /// The sample on show from that box: its nth newest decision.
    pub routing_sample: Option<usize>,
    /// Why for the sample, and the step picked in it.
    pub routing_why: Option<usize>,
    /// The Control view (`C`): #control's notes.
    pub control_view: bool,
    pub control: Board,
    /// The topic open (the day list), and the note open in it.
    pub control_topic: Option<String>,
    pub control_note: Option<String>,
    /// The row picked on the topic or day list.
    pub control_cursor: usize,
    /// Rows scrolled down on a note.
    pub control_scroll: usize,
    /// Tallest scroll from the last draw.
    pub control_max_scroll: usize,
    /// The action asked about (y/n) before voting, and the request it is on.
    pub control_vote: Option<usize>,
    pub control_vote_on: Option<String>,
    /// Today for the overview's sparklines (YYYY-MM-DD); None = the UTC
    /// date now. Tests pin it.
    pub control_today: Option<String>,
}

impl App {
    pub fn new(me: String, picker: Option<Picker>) -> Self {
        App {
            store: Store::new(me),
            mode: Mode::Normal,
            conn: ConnState::Connecting,
            tabs: vec![],
            active: 0,
            sections: vec![],
            sections_loaded: false,
            channels_loaded: false,
            chosen: vec![],
            picker_cursor: 0,
            order_cursor: 0,
            picker_marks: HashSet::new(),
            picker_touched: false,
            focus: Pane::Timeline,
            selected: HashMap::new(),
            thread: None,
            thread_selected: None,
            draft: Draft::default(),
            thread_draft: Draft::default(),
            mention_pick: 0,
            pending: None,
            uploading: false,
            status: None,
            sent_flash: None,
            tick: 0,
            empty: None,
            empty_prev: None,
            agents_view: false,
            agents_cursor: 0,
            reset_ask: None,
            archive_ask: None,
            agents: Agents::default(),
            media: Media::default(),
            picker,
            viewer: None,
            viewer_list: vec![],
            hits: vec![],
            links: vec![],
            hint_typed: String::new(),
            extra: None,
            jumps: vec![],
            page: 20,
            peek: None,
            tall: None,
            read_width: None,
            body_max: 80,
            body_x: 0,
            dragging: false,
            handle_click: None,
            actions: vec![],
            pending_sends: HashMap::new(),
            relay_host: String::new(),
            last_bot: HashMap::new(),
            prefill: None,
            read: ReadState::new(readstate::new_id(), readstate::new_id()),
            bell: HashMap::new(),
            archived: HashSet::new(),
            anchor: None,
            checklists: Checklists::default(),
            checklist_view: false,
            checklist_cursor: 0,
            quick_draft: Draft::default(),
            quick_include: false,
            loop_: Loop::default(),
            needs_view: false,
            needs_cursor: 0,
            needs: vec![],
            needs_jump: None,
            reply_to: None,
            reply_draft: Draft::default(),
            reply_sends: HashMap::new(),
            reply_sent: None,
            routing_view: false,
            routing_window: Window::Day,
            routing_node: Node::Seen,
            routing_sample: None,
            routing_why: None,
            control_view: false,
            control: Board::new(crate::control::DEFAULT_CHANNEL),
            control_topic: None,
            control_note: None,
            control_cursor: 0,
            control_scroll: 0,
            control_max_scroll: 0,
            control_vote: None,
            control_vote_on: None,
            control_today: None,
        }
    }

    /// Whether a tick should trigger a redraw (spinners, flashes, loading).
    pub fn animating(&self) -> bool {
        self.uploading
            || self.conn != ConnState::Online
            || self.agents.by_pubkey.values().any(|a| a.is_busy())
            || self
                .sent_flash
                .is_some_and(|(_, t)| t.elapsed().as_millis() < 1600)
            || self
                .status
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed().as_secs() < 7)
            || self
                .media
                .images
                .values()
                .any(|s| matches!(s, crate::media::Slot::Loading))
            || self
                .current()
                .and_then(|c| self.store.channels.get(c))
                .is_none_or(|c| !c.loaded || c.timeline.is_empty())
    }

    pub fn say(&mut self, s: impl Into<String>) {
        let s = s.into();
        crate::log::line(format!("status: {s}"));
        self.status = Some((s, Instant::now()));
    }

    /// `r` on the agents view: ask before resetting the picked agent.
    fn ask_reset(&mut self) {
        let Some(channel) = self.current().map(str::to_string) else {
            return;
        };
        let rows = self.agents.rows(&self.store, Some(&channel));
        let Some(r) = rows.get(self.agents_cursor.min(rows.len().saturating_sub(1))) else {
            return;
        };
        let root = self
            .thread
            .as_ref()
            .filter(|t| t.0 == channel)
            .map(|t| t.1.clone());
        self.reset_ask = Some((r.pubkey.clone(), r.name.clone(), channel, root));
        self.mode = Mode::Reset;
    }

    /// Send `!rotate` from you to the agent, in this channel (in the open
    /// thread when there is one, for thread-scoped sessions). The harness
    /// starts that agent's next turn here with a fresh session.
    fn reset_agent(&mut self) {
        self.mode = Mode::Normal;
        let Some((agent, name, channel, root)) = self.reset_ask.take() else {
            return;
        };
        self.agents.rotated(&agent, &channel);
        self.actions.push(Action::Reset {
            channel,
            root,
            agent,
        });
        self.actions.push(Action::SaveState);
        self.say(format!("resetting {name}…"));
    }

    pub fn current(&self) -> Option<&str> {
        self.tabs.get(self.active).map(String::as_str)
    }

    pub fn draft_mut(&mut self) -> &mut Draft {
        if self.focus == Pane::Thread && self.thread.is_some() {
            &mut self.thread_draft
        } else {
            &mut self.draft
        }
    }

    /// Where typing goes right now: (channel, thread root).
    pub fn draft_target(&self) -> Option<(String, Option<String>)> {
        match (&self.thread, self.focus) {
            (Some((c, r)), Pane::Thread) => Some((c.clone(), Some(r.clone()))),
            _ => self.current().map(|c| (c.to_string(), None)),
        }
    }

    /// The draft for a channel or thread, if it is still open.
    pub fn draft_for(&mut self, channel: &str, root: Option<&str>) -> Option<&mut Draft> {
        match root {
            Some(r) => match &self.thread {
                Some((c, tr)) if c == channel && tr == r => Some(&mut self.thread_draft),
                _ => None,
            },
            None if self.current() == Some(channel) => Some(&mut self.draft),
            None => None,
        }
    }

    pub fn draft_ref(&self) -> &Draft {
        if self.focus == Pane::Thread && self.thread.is_some() {
            &self.thread_draft
        } else {
            &self.draft
        }
    }

    /// Rebuild tabs from the chosen sections (or every channel when there
    /// are no sections), keeping the active channel if it is still there.
    pub fn rebuild_tabs(&mut self) {
        let keep = self.current().map(str::to_string);
        let mut tabs: Vec<String> = vec![];
        let chosen: Vec<&Section> = if self.chosen.is_empty() {
            self.sections.iter().take(1).collect()
        } else {
            self.sections
                .iter()
                .filter(|s| self.chosen.contains(&s.id))
                .collect()
        };
        for s in chosen {
            for c in &s.channels {
                if !tabs.contains(c) && self.store.channels.get(c).is_none_or(|ch| !ch.archived) {
                    tabs.push(c.clone());
                }
            }
        }
        if tabs.is_empty() {
            let mut all: Vec<&crate::model::Channel> = self
                .store
                .channels
                .values()
                .filter(|c| c.is_member && !c.archived && !c.name.is_empty())
                .collect();
            all.sort_by_key(|c| (c.is_dm, c.name.to_lowercase()));
            tabs = all.into_iter().map(|c| c.id.clone()).collect();
        }
        match &self.extra {
            Some(x) if tabs.contains(x) => self.extra = None,
            Some(x) => tabs.push(x.clone()),
            None => {}
        }
        self.tabs = tabs;
        self.active = keep
            .and_then(|k| self.tabs.iter().position(|t| *t == k))
            .unwrap_or(0);
        self.open_current();
    }

    pub fn open_current(&mut self) {
        if let Some(id) = self.current().map(str::to_string) {
            let ch = self.store.channel_mut(&id);
            ch.unread.clear();
            ch.mentioned.clear();
            self.actions.push(Action::OpenChannel(id));
        }
    }

    /// A thread's replies are seen through now.
    pub fn mark_thread(&mut self, root: &str) {
        self.read
            .mark(&unread::thread_key(root), crate::sync::now());
    }

    /// The bell: the newest activity in this tab, read or not; a thread
    /// opens on its first new reply, at the top. The same place again
    /// until something newer arrives. Never leaves the tab.
    fn bell(&mut self) {
        let Some(c) = self.current().map(str::to_string) else {
            return;
        };
        let Some(b) = self.store.channels.get(&c).and_then(|ch| {
            unread::bell(
                ch,
                &self.store.me,
                &self.read,
                crate::sync::now(),
                self.bell.get(&c),
            )
        }) else {
            return;
        };
        self.bell.insert(c, b.clone());
        self.land(b.1);
    }

    fn land(&mut self, n: Next) {
        let Some(c) = self.current().map(str::to_string) else {
            return;
        };
        self.peek = None;
        match n {
            Next::Message(id) => {
                self.thread = None;
                self.thread_selected = None;
                self.focus = Pane::Timeline;
                let last = self.timeline_ids().last() == Some(&id);
                self.set_cursor(id.clone(), last);
                self.anchor = Some((Pane::Timeline, id));
            }
            Next::Reply { root, id } => {
                self.actions.push(Action::OpenThread {
                    channel: c.clone(),
                    root: root.clone(),
                });
                self.thread = Some((c, root.clone()));
                self.focus = Pane::Thread;
                let last = self.thread_ids().last() == Some(&id);
                self.set_cursor(id.clone(), last);
                self.anchor = Some((Pane::Thread, id));
                self.mark_thread(&root);
            }
        }
    }

    fn switch_tab(&mut self, i: usize) {
        self.needs_view = false;
        self.routing_view = false;
        self.control_view = false;
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.thread = None;
            self.focus = Pane::Timeline;
            self.drop_extra();
            self.open_current();
            self.actions.push(Action::SaveState);
        }
    }

    /// The extra tab goes away once you are on another one.
    fn drop_extra(&mut self) {
        let Some(x) = self.extra.clone() else { return };
        if self.current() == Some(x.as_str()) {
            return;
        }
        self.extra = None;
        if let Some(i) = self.tabs.iter().position(|t| *t == x) {
            self.tabs.remove(i);
            if self.active > i {
                self.active -= 1;
            }
        }
    }

    // ---------- tab 0: needs you ----------

    /// Tab 0's rows from what we hold now.
    pub fn needs_rows(&self) -> Vec<Row> {
        let bot = |pk: &str| needs::is_bot(&self.store, &self.agents, &self.checklists, pk);
        needs::rows(&self.store, &self.checklists, bot, crate::sync::now())
    }

    fn open_needs(&mut self) {
        self.needs_view = true;
        self.routing_view = false;
        self.control_view = false;
        self.agents_view = false;
        self.checklist_view = false;
        self.needs = self.needs_rows();
    }

    /// Open a row: its message in its thread (or the thread, or the channel).
    fn open_row(&mut self, row: &Row) {
        self.agents_view = false;
        self.control_view = false;
        self.checklist_view = false;
        self.follow(&row.link());
        self.needs_view = false;
    }

    fn picked_row(&mut self) -> Option<Row> {
        self.needs = self.needs_rows();
        self.needs_cursor = self.needs_cursor.min(self.needs.len().saturating_sub(1));
        self.needs.get(self.needs_cursor).cloned()
    }

    /// `!`: the next Needs-an-answer row, oldest first, round again after
    /// the last, quickfix style.
    fn next_needs(&mut self) {
        let rows = self.needs_rows();
        let Some(r) = needs::next_answer(&rows, self.needs_jump.as_deref()).cloned() else {
            return self.say("⚑ nothing needs you");
        };
        let n = needs::answer_count(&rows);
        let i = rows.iter().position(|x| x.key == r.key).unwrap_or(0);
        self.needs_jump = Some(r.key.clone());
        self.open_row(&r);
        let who = if r.who.is_empty() {
            "watcher".to_string()
        } else {
            self.store.name_of(&r.who)
        };
        self.say(format!("⚑ {}/{n}  {who}: {}", i + 1, r.text));
    }

    /// `r` on tab 0: a reply box for the picked row, into its thread.
    fn reply_row(&mut self) {
        let Some(r) = self.picked_row() else {
            return self.say("nothing to reply to");
        };
        // Text typed for another row doesn't follow to this one.
        if self.reply_to.as_ref().is_some_and(|o| o.key != r.key) {
            self.reply_draft.clear();
        }
        self.reply_to = Some(r);
        self.mode = Mode::Reply;
    }

    fn send_reply(&mut self) {
        if self.reply_draft.text.trim().is_empty() {
            return self.say("type a reply first");
        }
        let Some(r) = self.reply_to.take() else {
            self.mode = Mode::Normal;
            return;
        };
        let draft = std::mem::take(&mut self.reply_draft);
        self.reply_sent = Some(r.clone());
        let to = (!r.who.is_empty()).then_some(r.who).into_iter().collect();
        self.actions.push(Action::SendDraft {
            channel: r.channel,
            root: r.thread,
            draft,
            to,
        });
        self.mode = Mode::Normal;
    }

    fn key_reply(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let d = &mut self.reply_draft;
        match k.code {
            // The text stays for next time, on the same row.
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter if alt => d.insert("\n"),
            KeyCode::Char('j') if ctrl => d.insert("\n"),
            KeyCode::Enter => self.send_reply(),
            KeyCode::Char('w') if ctrl => d.delete_word(),
            KeyCode::Char('u') if ctrl => d.clear(),
            KeyCode::Char('a') if ctrl => d.home(),
            KeyCode::Char('e') if ctrl => d.end(),
            KeyCode::Char(c) if !ctrl => d.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Tab => d.insert(&" ".repeat(crate::draft::TAB)),
            KeyCode::Backspace => d.backspace(),
            KeyCode::Delete => d.delete(),
            KeyCode::Left => d.left(),
            KeyCode::Right => d.right(),
            KeyCode::Home => d.home(),
            KeyCode::End => d.end(),
            KeyCode::Up => {
                d.vertical(false);
            }
            KeyCode::Down => {
                d.vertical(true);
            }
            _ => {}
        }
    }

    /// Keys on tab 0. True when the key was its own.
    fn key_needs(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => self.needs_cursor += 1,
            KeyCode::Char('k') | KeyCode::Up => {
                self.needs_cursor = self.needs_cursor.saturating_sub(1)
            }
            KeyCode::Char('g') | KeyCode::Home => self.needs_cursor = 0,
            KeyCode::Char('G') | KeyCode::End => self.needs_cursor = usize::MAX,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(r) = self.picked_row() {
                    self.needs_jump = Some(r.key.clone());
                    self.open_row(&r);
                }
            }
            KeyCode::Char('r') => self.reply_row(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => self.needs_view = false,
            // These act on the channel under tab 0, out of sight.
            KeyCode::Char('i')
            | KeyCode::Char('+')
            | KeyCode::Char('o')
            | KeyCode::Char('O')
            | KeyCode::Char('n')
            | KeyCode::Char('d')
            | KeyCode::Char('u')
            | KeyCode::Char('<')
            | KeyCode::Char('>')
            | KeyCode::PageDown
            | KeyCode::PageUp => {}
            _ => return false,
        }
        true
    }

    // ---------- control ----------

    fn open_control(&mut self) {
        self.control_view = true;
        self.needs_view = false;
        self.routing_view = false;
        self.agents_view = false;
        self.checklist_view = false;
        if !self.control.loaded {
            self.actions.push(Action::LoadControl);
        }
    }

    /// The note on screen, if one is open.
    pub fn control_open_note(&self) -> Option<&crate::control::Note> {
        self.control_note
            .as_deref()
            .and_then(|id| self.control.note(id))
    }

    /// The day the overview's sparklines end on.
    pub fn control_day(&self) -> String {
        self.control_today
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string())
    }

    /// The overview's picks, top to bottom: the open requests in the strip
    /// (ids), then the topic tiles (names).
    pub fn control_overview(&self) -> (Vec<String>, Vec<String>) {
        let reqs = self
            .control
            .open_requests()
            .iter()
            .take(crate::control::STRIP_MAX)
            .map(|n| n.id.clone())
            .collect();
        let tiles = self
            .control
            .tiles(&self.control_day())
            .into_iter()
            .map(|t| t.topic)
            .collect();
        (reqs, tiles)
    }

    /// Rows of the list on screen: the overview's picks, or the open
    /// topic's days.
    fn control_rows(&self) -> usize {
        match &self.control_topic {
            None => {
                let (r, t) = self.control_overview();
                r.len() + t.len()
            }
            Some(t) => self.control.days(t).len(),
        }
    }

    fn control_open(&mut self) {
        let i = self.control_cursor;
        match self.control_topic.clone() {
            None => {
                let (reqs, tiles) = self.control_overview();
                if let Some(id) = reqs.get(i) {
                    // Straight to the request; back returns here.
                    self.control_note = Some(id.clone());
                    self.control_scroll = 0;
                } else if let Some(t) = tiles.get(i - reqs.len()) {
                    self.control_topic = Some(t.clone());
                    self.control_cursor = 0;
                }
            }
            Some(t) => {
                if let Some(n) = self.control.days(&t).get(i) {
                    self.control_note = Some(n.id.clone());
                    self.control_scroll = 0;
                }
            }
        }
    }

    fn control_back(&mut self) {
        if self.control_note.take().is_some() {
            return;
        }
        if let Some(t) = self.control_topic.take() {
            // Back on the tile it came from.
            let (reqs, tiles) = self.control_overview();
            self.control_cursor = reqs.len() + tiles.iter().position(|r| *r == t).unwrap_or(0);
            return;
        }
        self.control_view = false;
    }

    /// The request a vote is asked about.
    pub fn control_vote_note(&self) -> Option<&crate::control::Note> {
        self.control_vote_on
            .as_deref()
            .and_then(|id| self.control.note(id))
    }

    /// A digit on a request: ask before voting that action, or say why not.
    fn ask_vote(&mut self, id: &str, action: usize) {
        let Some(n) = self.control.note(id) else {
            return;
        };
        match self.control.can_vote(n, action, &self.store.me) {
            Ok(()) => {
                self.control_vote = Some(action);
                self.control_vote_on = Some(id.to_string());
                self.mode = Mode::Vote;
            }
            Err(why) => self.say(format!("can't vote: {why}")),
        }
    }

    fn send_vote(&mut self) {
        self.mode = Mode::Normal;
        let Some(i) = self.control_vote.take() else {
            return;
        };
        let Some(n) = self
            .control_vote_on
            .take()
            .and_then(|id| self.control.note(&id))
        else {
            return;
        };
        let Some(a) = n.actions.get(i) else { return };
        // Asked again: the request may have closed while the box was up.
        if let Err(why) = self.control.can_vote(n, i, &self.store.me) {
            return self.say(format!("can't vote: {why}"));
        }
        self.actions.push(Action::React {
            target: n.id.clone(),
            emoji: a.emoji.clone(),
            label: a.name.clone(),
        });
    }

    /// Open the note's images in the viewer.
    fn control_images(&mut self) {
        let urls: Vec<String> = self
            .control_open_note()
            .map(|n| {
                n.media
                    .iter()
                    .filter(|m| m.mime.starts_with("image/"))
                    .map(|m| m.url.clone())
                    .collect()
            })
            .unwrap_or_default();
        if urls.is_empty() {
            return self.say("no images on this note");
        }
        self.viewer_list = urls;
        self.mode = Mode::Viewer;
        self.show_image(0);
    }

    /// Keys on the Control view. True when the key was its own.
    fn key_control(&mut self, k: KeyEvent) -> bool {
        let on_note = self.control_note.is_some();
        let page = self.page.max(1);
        match k.code {
            KeyCode::Char('C') => self.control_view = false,
            KeyCode::Char('r') => {
                self.actions.push(Action::LoadControl);
                self.say("reloading #control");
            }
            KeyCode::Char(c @ '1'..='9') if on_note => {
                let Some(id) = self
                    .control_open_note()
                    .filter(|n| n.is_request())
                    .map(|n| n.id.clone())
                else {
                    return false;
                };
                self.ask_vote(&id, c as usize - '1' as usize);
            }
            // On the overview a digit votes the request picked, or the only
            // one; with none open it leaves for the tab, as elsewhere.
            KeyCode::Char(c @ '1'..='9') if self.control_topic.is_none() => {
                let (reqs, _) = self.control_overview();
                let id = match reqs.get(self.control_cursor) {
                    Some(id) => id.clone(),
                    None if reqs.len() == 1 => reqs[0].clone(),
                    None if reqs.is_empty() => return false,
                    None => {
                        self.say("pick a request with j k first");
                        return true;
                    }
                };
                self.ask_vote(&id, c as usize - '1' as usize);
            }
            KeyCode::Char('o') if on_note => self.control_images(),
            KeyCode::Char('j') | KeyCode::Down if on_note => {
                self.control_scroll = (self.control_scroll + 1).min(self.control_max_scroll)
            }
            KeyCode::Char('k') | KeyCode::Up if on_note => {
                self.control_scroll = self.control_scroll.saturating_sub(1)
            }
            KeyCode::Char('d') | KeyCode::PageDown if on_note => {
                self.control_scroll = (self.control_scroll + page).min(self.control_max_scroll)
            }
            KeyCode::Char('u') | KeyCode::PageUp if on_note => {
                self.control_scroll = self.control_scroll.saturating_sub(page)
            }
            KeyCode::Char('g') | KeyCode::Home if on_note => self.control_scroll = 0,
            KeyCode::Char('G') | KeyCode::End if on_note => {
                self.control_scroll = self.control_max_scroll
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.control_cursor =
                    (self.control_cursor + 1).min(self.control_rows().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.control_cursor = self.control_cursor.saturating_sub(1)
            }
            KeyCode::Char('g') | KeyCode::Home => self.control_cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.control_cursor = self.control_rows().saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !on_note => self.control_open(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc | KeyCode::Backspace => {
                self.control_back()
            }
            // Writing, threads and reactions would act on the channel
            // hidden under the view.
            KeyCode::Char('i')
            | KeyCode::Char('+')
            | KeyCode::Char('o')
            | KeyCode::Char('O')
            | KeyCode::Char('n')
            | KeyCode::Char('f')
            | KeyCode::Char('l')
            | KeyCode::Enter
            | KeyCode::Right
            | KeyCode::Char('<')
            | KeyCode::Char('>')
            | KeyCode::Char('[')
            | KeyCode::Char(']')
            | KeyCode::Char('s')
            | KeyCode::Char('T')
            | KeyCode::Char('R')
            | KeyCode::Char('d')
            | KeyCode::Char('u')
            | KeyCode::PageDown
            | KeyCode::PageUp => {}
            _ => return false,
        }
        true
    }

    // ---------- routing ----------

    fn open_routing(&mut self) {
        self.routing_view = true;
        self.control_view = false;
        self.needs_view = false;
        self.agents_view = false;
        self.checklist_view = false;
        self.routing_sample = None;
        self.routing_why = None;
    }

    /// The decisions the view draws: every channel's we read.
    pub fn routing_decisions(&self) -> Vec<Decision> {
        self.checklists.decisions()
    }

    /// The sampled decision on show, if any.
    pub fn routing_picked(&self, ds: &[Decision]) -> Option<Decision> {
        let n = self.routing_sample?;
        let now = crate::sync::now();
        routing::sample(ds, now, self.routing_window, self.routing_node, n).cloned()
    }

    fn pick_box(&mut self, n: Node) {
        self.routing_node = n;
        // A sample on show follows to the new box, unless it's empty.
        if self.routing_sample.is_some() {
            self.routing_sample = Some(0);
            if self.routing_picked(&self.routing_decisions()).is_none() {
                self.routing_sample = None;
            }
        }
        self.routing_why = None;
    }

    /// The box `by` boxes on in drawing order: `wrap` goes round.
    fn step_box(&mut self, by: isize, wrap: bool) {
        let ds = self.routing_decisions();
        let nodes = routing::flow(&ds, crate::sync::now(), self.routing_window).nodes();
        let n = nodes.len() as isize;
        let at = nodes
            .iter()
            .position(|x| *x == self.routing_node)
            .unwrap_or(0) as isize;
        let to = if wrap {
            (at + by).rem_euclid(n)
        } else {
            (at + by).clamp(0, n - 1)
        };
        self.pick_box(nodes[to as usize]);
    }

    fn sample_box(&mut self) {
        let ds = self.routing_decisions();
        let f = routing::flow(&ds, crate::sync::now(), self.routing_window);
        if f.count(self.routing_node) == 0 {
            let w = self.routing_window.label();
            return self.say(format!(
                "nothing in {} in the last {w}",
                self.routing_node.label()
            ));
        }
        self.routing_sample = Some(self.routing_sample.map_or(0, |n| n + 1));
        self.routing_why = None;
    }

    fn open_decision_thread(&mut self) {
        let ds = self.routing_decisions();
        match self.routing_picked(&ds).and_then(|d| d.link()) {
            Some(url) => {
                self.routing_view = false;
                self.follow(&url);
            }
            None => self.say("no thread on this decision"),
        }
    }

    /// Open the picked Why step's record: a message in its thread, else
    /// copy the event id.
    fn open_step(&mut self) {
        let ds = self.routing_decisions();
        let Some(d) = self.routing_picked(&ds) else {
            return;
        };
        let steps = routing::why(&d);
        let Some(s) = self
            .routing_why
            .and_then(|i| steps.get(i.min(steps.len() - 1)))
        else {
            return;
        };
        match (&s.link, &s.event) {
            (Some(url), _) => {
                self.routing_view = false;
                self.follow(&url.clone());
            }
            (None, Some(id)) => {
                self.actions.push(Action::Copy(id.clone()));
                self.say(format!("copied {} event id", s.title));
            }
            (None, None) => self.say("no record for this step"),
        }
    }

    /// Keys on the routing view. True when the key was its own.
    fn key_routing(&mut self, k: KeyEvent) -> bool {
        let why = self.routing_why;
        match k.code {
            KeyCode::Char('w') => self.routing_view = false,
            KeyCode::Char('t') => {
                let i = Window::ALL.iter().position(|w| *w == self.routing_window);
                self.routing_window = Window::ALL[(i.unwrap_or(0) + 1) % Window::ALL.len()];
                self.routing_sample = self.routing_sample.map(|_| 0);
                self.routing_why = None;
            }
            KeyCode::Char('j') | KeyCode::Down if why.is_some() => {
                self.routing_why = why.map(|i| i.saturating_add(1))
            }
            KeyCode::Char('k') | KeyCode::Up if why.is_some() => {
                self.routing_why = why.map(|i| i.saturating_sub(1))
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if why.is_some() => {
                self.open_step()
            }
            KeyCode::Char('j') | KeyCode::Down => self.step_box(1, false),
            KeyCode::Char('k') | KeyCode::Up => self.step_box(-1, false),
            KeyCode::Tab => self.step_box(1, true),
            KeyCode::BackTab => self.step_box(-1, true),
            KeyCode::Char('g') | KeyCode::Home => self.step_box(-99, false),
            KeyCode::Char('G') | KeyCode::End => self.step_box(99, false),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if self.routing_picked(&self.routing_decisions()).is_some() {
                    self.routing_why = Some(0);
                } else {
                    self.routing_sample = None;
                    self.sample_box();
                }
            }
            KeyCode::Char('s') if self.routing_sample.is_some() => self.sample_box(),
            KeyCode::Char('o') if self.routing_sample.is_some() => self.open_decision_thread(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc | KeyCode::Backspace => {
                if why.is_some() {
                    self.routing_why = None;
                } else if self.routing_sample.is_some() {
                    self.routing_sample = None;
                } else {
                    self.routing_view = false;
                }
            }
            _ => return false,
        }
        true
    }

    // ---------- links ----------

    /// Each link on screen once, at its first piece, in screen order.
    pub fn link_starts(&self) -> Vec<(Rect, String)> {
        let mut seen = HashSet::new();
        self.links
            .iter()
            .filter(|l| seen.insert(l.key.clone()))
            .map(|l| (l.rect, l.url.clone()))
            .collect()
    }

    pub fn here(&self) -> Option<Jump> {
        Some(Jump {
            channel: self.current()?.to_string(),
            selected: self.current().and_then(|c| self.selected.get(c).cloned()),
            thread: self
                .thread
                .as_ref()
                .map(|(_, r)| (r.clone(), self.thread_selected.clone())),
        })
    }

    pub fn follow(&mut self, url: &str) {
        use crate::links::Target;
        if let Some(text) = url.strip_prefix(crate::links::COPY) {
            self.actions.push(Action::Copy(text.to_string()));
            return;
        }
        match crate::links::target(url) {
            Target::Message {
                channel,
                id,
                thread,
            } => {
                let known = self.store.channels.get(&channel);
                let l = crate::links::land(known, id, thread);
                let jump = Jump {
                    selected: l.selected,
                    thread: l.thread,
                    channel,
                };
                let newest = known.and_then(|c| c.timeline.last());
                let here = self.here();
                let same = here.as_ref().is_some_and(|h| {
                    h.channel == jump.channel
                        && match (&h.thread, &jump.thread) {
                            (Some((a, _)), Some((b, _))) => a == b,
                            // Following the bottom is already at the newest.
                            (None, None) => {
                                h.selected == jump.selected
                                    || (h.selected.is_none() && jump.selected.as_ref() == newest)
                            }
                            _ => false,
                        }
                });
                if same {
                    // Already here: just pick the reply, keep the timeline as
                    // is, and show it (a link followed from the routing view).
                    self.routing_view = false;
                    if let Some((_, Some(pick))) = jump.thread {
                        self.thread_selected = Some(pick);
                        self.focus = Pane::Thread;
                    }
                    return;
                }
                if let Some(h) = here {
                    self.jumps.push(h);
                }
                self.goto(jump);
            }
            Target::Desktop => {
                self.actions.push(Action::OpenUrl(url.to_string()));
                self.say("🐝 opening in Buzz Desktop");
            }
            Target::Web => {
                self.actions.push(Action::OpenUrl(url.to_string()));
                self.say(format!("opening {url}"));
            }
        }
    }

    fn back(&mut self) {
        match self.jumps.pop() {
            Some(j) => self.goto(j),
            None => self.say("nowhere to go back to"),
        }
    }

    fn goto(&mut self, j: Jump) {
        self.needs_view = false;
        self.routing_view = false;
        let left = self.current() != Some(j.channel.as_str());
        let i = match self.tabs.iter().position(|t| *t == j.channel) {
            Some(i) => i,
            None => {
                if let Some(x) = self.extra.take() {
                    self.tabs.retain(|t| *t != x);
                }
                self.extra = Some(j.channel.clone());
                self.tabs.push(j.channel.clone());
                self.tabs.len() - 1
            }
        };
        self.active = i;
        self.drop_extra();
        if left {
            self.open_current();
        }
        match j.selected {
            Some(s) => self.selected.insert(j.channel.clone(), s),
            None => self.selected.remove(&j.channel),
        };
        self.peek = None;
        match j.thread {
            Some((root, pick)) => {
                self.actions.push(Action::OpenThread {
                    channel: j.channel.clone(),
                    root: root.clone(),
                });
                self.mark_thread(&root);
                self.thread = Some((j.channel, root));
                self.thread_selected = pick;
                self.focus = Pane::Thread;
            }
            None => {
                self.thread = None;
                self.thread_selected = None;
                self.focus = Pane::Timeline;
            }
        }
        self.actions.push(Action::SaveState);
    }

    fn key_hint(&mut self, k: KeyEvent) {
        let KeyCode::Char(c) = k.code else {
            self.mode = Mode::Normal;
            return;
        };
        self.hint_typed.push(c);
        let starts = self.link_starts();
        let labels = crate::links::hint_labels(starts.len());
        if let Some(i) = labels.iter().position(|l| *l == self.hint_typed) {
            self.mode = Mode::Normal;
            self.follow(&starts[i].1.clone());
        } else if !labels.iter().any(|l| l.starts_with(&self.hint_typed)) {
            self.mode = Mode::Normal;
        }
    }

    // ---------- selection ----------

    fn timeline_ids(&self) -> Vec<String> {
        self.current()
            .and_then(|c| self.store.channels.get(c))
            .map(|c| c.timeline.clone())
            .unwrap_or_default()
    }

    fn thread_ids(&self) -> Vec<String> {
        let Some((ch, root)) = &self.thread else {
            return vec![];
        };
        let mut ids = vec![root.clone()];
        if let Some(c) = self.store.channels.get(ch) {
            ids.extend(c.replies.get(root).cloned().unwrap_or_default());
        }
        ids
    }

    /// The message the cursor is on in the focused pane.
    pub fn cursor_message(&self) -> Option<String> {
        match self.focus {
            Pane::Thread => self
                .thread_selected
                .clone()
                .or_else(|| self.thread_ids().last().cloned()),
            Pane::Timeline => {
                let c = self.current()?;
                self.selected
                    .get(c)
                    .cloned()
                    .or_else(|| self.timeline_ids().last().cloned())
            }
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let ids = if self.focus == Pane::Thread {
            self.thread_ids()
        } else {
            self.timeline_ids()
        };
        if ids.is_empty() {
            return;
        }
        // g / G go straight to the first or last message, top or bottom.
        let jump = delta.unsigned_abs() >= JUMP;
        // A message taller than the pane pages through its lines first,
        // like a pager, and only then moves on.
        if let Some((id, bh, h, off)) = self.tall.clone().filter(|_| !jump) {
            if self.cursor_message().as_deref() == Some(id.as_str()) {
                let step = delta.unsigned_abs() * 3;
                let end = bh.saturating_sub(h);
                if delta > 0 && off < end {
                    self.peek = Some((id.clone(), (off + step).min(end)));
                    let last = ids.last() == Some(&id);
                    self.set_cursor(id, last);
                    return;
                }
                if delta < 0 && off > 0 {
                    self.peek = Some((id.clone(), off.saturating_sub(step)));
                    let last = ids.last() == Some(&id);
                    self.set_cursor(id, last);
                    return;
                }
            }
        }
        let cur = self
            .cursor_message()
            .and_then(|m| ids.iter().position(|i| *i == m))
            .unwrap_or(ids.len() - 1);
        let next = (cur as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
        if next == 0 && delta < 0 && self.focus == Pane::Timeline {
            if let Some(c) = self.current().map(str::to_string) {
                self.actions.push(Action::LoadOlder(c));
            }
        }
        // A tall message starts at the end you come in from: its bottom going
        // up, its top going down (even as the newest, which would show its end).
        // g shows the top of the first, G the bottom of the last.
        let bottom = if jump { delta > 0 } else { delta < 0 };
        let off = if bottom { usize::MAX } else { 0 };
        if next == cur && !jump {
            return;
        }
        self.peek = Some((ids[next].clone(), off));
        self.set_cursor(ids[next].clone(), next == ids.len() - 1);
    }

    fn set_cursor(&mut self, id: String, at_end: bool) {
        match self.focus {
            Pane::Thread => self.thread_selected = (!at_end).then_some(id),
            Pane::Timeline => {
                if let Some(c) = self.current().map(str::to_string) {
                    if at_end {
                        self.selected.remove(&c);
                    } else {
                        self.selected.insert(c, id);
                    }
                }
            }
        }
    }

    fn open_thread(&mut self, insert: bool) {
        let Some(ch) = self.current().map(str::to_string) else {
            return;
        };
        let Some(msg) = self.cursor_message() else {
            return;
        };
        // From inside a thread, Enter just keeps the same thread.
        let root = self
            .store
            .channels
            .get(&ch)
            .and_then(|c| c.messages.get(&msg))
            .and_then(|m| m.root.clone())
            .unwrap_or(msg);
        self.actions.push(Action::OpenThread {
            channel: ch.clone(),
            root: root.clone(),
        });
        self.mark_thread(&root);
        self.thread = Some((ch, root));
        self.thread_selected = None;
        self.focus = Pane::Thread;
        if insert {
            self.enter_insert();
        }
    }

    fn close_thread(&mut self) {
        // What arrived while it was open is seen too.
        if let Some((_, root)) = self.thread.clone() {
            self.mark_thread(&root);
        }
        self.thread = None;
        self.thread_selected = None;
        self.focus = Pane::Timeline;
    }

    fn thumbs_up(&mut self) {
        let Some(target) = self.cursor_message() else {
            return;
        };
        self.actions.push(Action::ThumbsUp { target });
    }

    fn send(&mut self) {
        if self.draft_ref().is_empty() {
            return;
        }
        let (channel, root, parent) = match (&self.thread, self.focus) {
            (Some((c, r)), Pane::Thread) => (c.clone(), Some(r.clone()), Some(r.clone())),
            _ => match self.current() {
                Some(c) => (c.to_string(), None, None),
                None => return,
            },
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Some(bot) = self
            .draft_ref()
            .mention_pubkeys()
            .into_iter()
            .find(|pk| self.is_agent(pk))
        {
            self.last_bot.insert(channel.clone(), (bot, now));
            self.actions.push(Action::SaveState);
        }
        self.prefill = None;
        self.actions.push(Action::Send {
            channel,
            root,
            parent,
        });
        // Sending is the end of typing: back to normal mode, like Esc.
        self.mode = Mode::Normal;
    }

    // ---------- mentions ----------

    fn is_agent(&self, pk: &str) -> bool {
        self.store.profiles.get(pk).is_some_and(|p| p.is_agent)
    }

    /// The bot my latest message in this channel tagged: from loaded
    /// history (so sends from Desktop count too) or what we saved, whichever
    /// is newer.
    pub fn last_bot_in(&self, channel: &str) -> Option<String> {
        let seen = self.store.channels.get(channel).and_then(|c| {
            c.messages
                .values()
                .filter(|m| m.author == self.store.me && !m.deleted)
                .filter_map(|m| {
                    let bot = m.mentions.iter().find(|pk| self.is_agent(pk))?;
                    Some((bot.clone(), m.created_at))
                })
                .max_by_key(|(_, at)| *at)
        });
        let saved = self.last_bot.get(channel).cloned();
        [seen, saved]
            .into_iter()
            .flatten()
            .max_by_key(|(_, at)| *at)
            .map(|(pk, _)| pk)
    }

    /// Insert mode. A blank draft starts with `@bot ` for the bot last
    /// tagged here, as if it had been typed and Tab-completed.
    fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        let d = self.draft_ref();
        if !d.text.is_empty() || !d.attachments.is_empty() {
            return;
        }
        let Some((channel, _)) = self.draft_target() else {
            return;
        };
        let Some(pk) = self.last_bot_in(&channel) else {
            return;
        };
        let Some(name) = self
            .store
            .profiles
            .get(&pk)
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
        else {
            return;
        };
        self.draft_mut().complete_mention(0, &name, &pk);
        self.prefill = Some(self.draft_ref().text.clone());
    }

    /// Candidates for the `@query` being typed: the thread's and channel's
    /// people first, then our agents (mentioning one who isn't here adds
    /// them on send). Names already mentioned in the draft are left out.
    pub fn mention_candidates(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.draft_ref().mention_query() else {
            return vec![];
        };
        let q = q.to_lowercase();
        let (ch, root) = match (&self.thread, self.focus) {
            (Some((c, r)), Pane::Thread) => (Some(c.as_str()), Some(r.as_str())),
            _ => (self.current(), None),
        };
        let Some(ch) = ch else {
            return vec![];
        };
        let mut skip = self.archived.clone();
        skip.extend(self.draft_ref().mention_pubkeys());
        let mut out: Vec<(String, String)> =
            mention::candidates(&self.store, &self.agents, ch, root, &skip)
                .into_iter()
                .map(|(pk, n)| (n, pk))
                .filter(|(n, _)| n.to_lowercase().contains(&q))
                .collect();
        // Stable: within prefix matches, the order above holds.
        out.sort_by_key(|(n, _)| !n.to_lowercase().starts_with(&q));
        out.truncate(6);
        out
    }

    fn accept_mention(&mut self) -> bool {
        let cands = self.mention_candidates();
        let Some((name, pk)) = cands
            .get(self.mention_pick.min(cands.len().saturating_sub(1)))
            .cloned()
        else {
            return false;
        };
        let Some((at, _)) = self.draft_ref().mention_query() else {
            return false;
        };
        self.draft_mut().complete_mention(at, &name, &pk);
        self.mention_pick = 0;
        true
    }

    // ---------- Q ----------

    /// The channel a Q can add: none while tab 0 or the routing view is
    /// shown, since the tab under it is hidden.
    pub fn quick_here(&self) -> Option<&str> {
        if self.needs_view || self.routing_view {
            None
        } else {
            self.current()
        }
    }

    /// `Q` / `^q`: open the Q box; pressed again while it's open, the
    /// current channel goes along (or comes off again).
    fn quick(&mut self) {
        let name = &self.loop_.quick_channel;
        if self.mode == Mode::Quick {
            match crate::quick::can_add(&self.store, name, self.quick_here()) {
                Ok(()) => self.quick_include = !self.quick_include,
                Err(e) => self.say(e),
            }
            return;
        }
        if let Some(msg) = crate::quick::unavailable(&self.store, name, self.channels_loaded) {
            return self.say(msg);
        }
        let Some(q) = crate::quick::find_channel(&self.store, name) else {
            return;
        };
        let members = q.members.clone();
        self.actions.push(Action::WantProfiles(members));
        self.quick_include = false;
        self.mode = Mode::Quick;
    }

    fn key_quick(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let d = &mut self.quick_draft;
        match k.code {
            // The text stays for next time.
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter if alt => d.insert("\n"),
            KeyCode::Char('j') if ctrl => d.insert("\n"),
            KeyCode::Enter => self.send_quick(),
            KeyCode::Char('w') if ctrl => d.delete_word(),
            KeyCode::Char('u') if ctrl => d.clear(),
            KeyCode::Char('a') if ctrl => d.home(),
            KeyCode::Char('e') if ctrl => d.end(),
            KeyCode::Char(c) if !ctrl => d.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Tab => d.insert(&" ".repeat(crate::draft::TAB)),
            KeyCode::Backspace => d.backspace(),
            KeyCode::Delete => d.delete(),
            KeyCode::Left => d.left(),
            KeyCode::Right => d.right(),
            KeyCode::Home => d.home(),
            KeyCode::End => d.end(),
            KeyCode::Up => {
                d.vertical(false);
            }
            KeyCode::Down => {
                d.vertical(true);
            }
            _ => {}
        }
    }

    /// The Q as it will be sent, or why it can't be: (#quick id, agent
    /// pubkey, content).
    pub fn quick_message(&self) -> Result<(String, String, String), String> {
        let here = if self.quick_include {
            let here = self.quick_here().ok_or("no channel open to add")?;
            let root = self
                .thread
                .as_ref()
                .filter(|t| t.0 == here)
                .map(|t| t.1.as_str());
            Some((here, root))
        } else {
            None
        };
        let l = &self.loop_;
        let a = crate::quick::ask(
            &self.store,
            &l.quick_channel,
            &l.quick_agent,
            &self.quick_draft.text,
            here,
        )?;
        Ok((a.channel, a.agent, a.content))
    }

    fn send_quick(&mut self) {
        match self.quick_message() {
            Ok((channel, agent, content)) => {
                let text = std::mem::take(&mut self.quick_draft).text;
                self.actions.push(Action::SendQuick {
                    channel,
                    content,
                    agent,
                    text,
                });
                self.quick_include = false;
                self.mode = Mode::Normal;
            }
            Err(e) => self.say(e),
        }
    }

    // ---------- input ----------

    pub fn on_term(&mut self, ev: TermEvent) {
        match ev {
            TermEvent::Key(k) if k.kind != crossterm::event::KeyEventKind::Release => {
                self.on_key(k)
            }
            TermEvent::Mouse(m) => self.on_mouse(m),
            TermEvent::Paste(s) => self.on_paste(s),
            _ => {}
        }
    }

    fn on_paste(&mut self, s: String) {
        let text = s.replace("\r\n", "\n").replace('\r', "\n");
        // The Q box takes text only, a dropped path included.
        if self.mode == Mode::Quick {
            self.quick_draft.insert(&text);
            return;
        }
        if self.mode == Mode::Reply {
            self.reply_draft.insert(&text);
            return;
        }
        // Nor does the Control view: a paste there would land in the
        // channel hidden under it (or swap the vote box for Insert).
        if self.control_view {
            return self.say("nothing to paste into on the Control view");
        }
        // Nor does the routing view.
        if self.routing_view {
            return;
        }
        // Tab 0 has no compose box: the channel under it mustn't get it.
        if self.needs_view {
            return self.say("to paste a reply, pick a row and press r");
        }
        let paths = crate::clip::dropped_paths(&s);
        if let Some(p) = paths.first() {
            self.say(format!("reading {}…", p.display()));
            self.actions.push(Action::ReadAttachment(Some(p.clone())));
            return;
        }
        if self.mode != Mode::Insert {
            self.mode = Mode::Insert;
        }
        self.draft_mut().insert(&text);
    }

    fn paste_clipboard(&mut self) {
        self.actions.push(Action::ReadAttachment(None));
    }

    /// A file or clipboard image has been read: ask before uploading it.
    pub fn attach(&mut self, p: Pending) {
        // Don't pull the rug from under a popup or the confirm box.
        if !matches!(self.mode, Mode::Normal | Mode::Insert) {
            self.say(format!("{} is ready; drop or paste it again", p.name));
            return;
        }
        self.pending = Some(p);
        self.mode = Mode::Confirm;
        self.actions.push(Action::PreviewPending);
    }

    fn on_key(&mut self, mut k: KeyEvent) {
        // With all keys reported as escape codes (kitty, WezTerm, Ghostty),
        // Caps Lock comes as a state bit on the lowercase key.
        if k.state.contains(KeyEventState::CAPS_LOCK)
            && !k
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if let KeyCode::Char(c) = k.code {
                k.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
            }
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') && self.mode == Mode::Reply {
            if self.reply_draft.is_empty() {
                self.mode = Mode::Normal;
            } else {
                self.reply_draft.clear();
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('c') {
            // In the Q box: clear it, then close it; never quit from there.
            if self.mode == Mode::Quick {
                if self.quick_draft.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.quick_draft.clear();
                }
                return;
            }
            if self.mode == Mode::Insert && !self.draft_ref().is_empty() {
                self.draft_mut().clear();
                self.mode = Mode::Normal;
            } else {
                self.actions.push(Action::Quit);
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('v') && self.routing_view && self.mode == Mode::Normal {
            return;
        }
        if ctrl && k.code == KeyCode::Char('v') && self.needs_view && self.mode == Mode::Normal {
            return self.say("to paste a reply, pick a row and press r");
        }
        if ctrl && k.code == KeyCode::Char('v') && matches!(self.mode, Mode::Normal | Mode::Insert)
        {
            self.paste_clipboard();
            return;
        }
        if ctrl
            && k.code == KeyCode::Char('q')
            && matches!(self.mode, Mode::Normal | Mode::Insert | Mode::Quick)
        {
            self.quick();
            return;
        }
        match self.mode {
            Mode::Quick => self.key_quick(k),
            Mode::Reply => self.key_reply(k),
            Mode::Insert => self.key_insert(k),
            Mode::Normal => self.key_normal(k),
            Mode::Sections => self.key_sections(k),
            Mode::Order => self.key_order(k),
            Mode::Confirm => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.mode = Mode::Insert;
                    self.actions.push(Action::Upload);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.pending = None;
                    self.mode = Mode::Normal;
                    self.say("not uploaded");
                }
                _ => {}
            },
            Mode::Viewer => match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') | KeyCode::Enter => {
                    self.viewer = None;
                    self.mode = Mode::Normal;
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('k') | KeyCode::Up => {
                    self.step_viewer(-1)
                }
                KeyCode::Char('O') => self.open_in_os(),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('j') | KeyCode::Down => {
                    self.step_viewer(1)
                }
                _ => {}
            },
            Mode::Hint => self.key_hint(k),
            Mode::Reset => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.reset_agent(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.reset_ask = None;
                    self.mode = Mode::Normal;
                    self.say("not reset");
                }
                _ => {}
            },
            Mode::Vote => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.send_vote(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.control_vote = None;
                    self.control_vote_on = None;
                    self.mode = Mode::Normal;
                    self.say("no vote sent");
                }
                _ => {}
            },
            Mode::Archive => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.archive(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.archive_ask = None;
                    self.mode = Mode::Order;
                    self.say("not archived");
                }
                _ => {}
            },
            Mode::Help => {
                if matches!(
                    k.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    fn key_insert(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let cands = self.mention_candidates();
        match k.code {
            KeyCode::Esc => {
                if self.prefill.take().as_deref() == Some(self.draft_ref().text.as_str()) {
                    self.draft_mut().clear();
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Tab if !cands.is_empty() => {
                self.accept_mention();
            }
            KeyCode::Tab => self.draft_mut().insert(&" ".repeat(crate::draft::TAB)),
            KeyCode::Up if !cands.is_empty() => {
                self.mention_pick = self.mention_pick.saturating_sub(1)
            }
            KeyCode::Down if !cands.is_empty() => {
                self.mention_pick = (self.mention_pick + 1).min(cands.len() - 1)
            }
            KeyCode::Enter if alt => self.draft_mut().insert("\n"),
            KeyCode::Char('j') if ctrl => self.draft_mut().insert("\n"),
            KeyCode::Enter
                if !cands.is_empty()
                    && self
                        .draft_ref()
                        .mention_query()
                        .is_some_and(|(_, q)| !q.is_empty()) =>
            {
                self.accept_mention();
            }
            KeyCode::Enter if self.draft_ref().in_code_fence() => self.draft_mut().insert("\n"),
            KeyCode::Enter => self.send(),
            KeyCode::Char('w') if ctrl => self.draft_mut().delete_word(),
            KeyCode::Char('u') if ctrl => {
                let d = self.draft_mut();
                d.text.clear();
                d.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.draft_mut().home(),
            KeyCode::Char('e') if ctrl => self.draft_mut().end(),
            KeyCode::Char(c) if !ctrl => {
                self.draft_mut().insert(c.encode_utf8(&mut [0; 4]));
                self.mention_pick = 0;
            }
            KeyCode::Backspace => self.draft_mut().backspace(),
            KeyCode::Delete => self.draft_mut().delete(),
            KeyCode::Left => self.draft_mut().left(),
            KeyCode::Right => self.draft_mut().right(),
            KeyCode::Home => self.draft_mut().home(),
            KeyCode::End => self.draft_mut().end(),
            KeyCode::Up => {
                self.draft_mut().vertical(false);
            }
            KeyCode::Down => {
                self.draft_mut().vertical(true);
            }
            _ => {}
        }
    }

    fn key_normal(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.needs_view && !ctrl && self.key_needs(k) {
            return;
        }
        if self.control_view {
            if !ctrl && self.key_control(k) {
                return;
            }
            // As on the routing view: only keys that leave it, or open
            // another view, reach the channel hidden under it.
            let leaves = match k.code {
                KeyCode::Char(c) if !ctrl => {
                    matches!(
                        c,
                        'q' | 'Q' | '0'..='9' | '!' | 'L' | 'H' | 'c' | 'a' | 'w' | '?'
                    )
                }
                KeyCode::Tab | KeyCode::BackTab => true,
                _ => false,
            };
            if !leaves {
                return;
            }
        }
        if self.routing_view {
            if !ctrl && self.key_routing(k) {
                return;
            }
            // One gate for the channel hidden under the view: past the view's
            // own keys, only those that leave it or open another view get
            // through (^c and ^q are handled before this).
            let leaves = match k.code {
                KeyCode::Char('o') => ctrl,
                KeyCode::Char(c) if !ctrl => {
                    matches!(c, 'q' | '0'..='9' | '!' | 'L' | 'H' | 'c' | 'a' | '?' | 'f')
                }
                _ => false,
            };
            if !leaves {
                return;
            }
        }
        // ^d ^u page tab 0's rows, not the channel under it.
        if self.needs_view && ctrl && matches!(k.code, KeyCode::Char('d') | KeyCode::Char('u')) {
            let step = (self.page / 2).max(1);
            self.needs_cursor = if k.code == KeyCode::Char('d') {
                self.needs_cursor.saturating_add(step)
            } else {
                self.needs_cursor.saturating_sub(step)
            };
            return;
        }
        let n = self.tabs.len();
        match k.code {
            // Tab 0 is Needs you; the channels are 1 to 9.
            KeyCode::Char('0') => self.open_needs(),
            KeyCode::Char(c @ '1'..='9') => self.switch_tab(c as usize - '1' as usize),
            KeyCode::Char('!') => self.next_needs(),
            // H and L go round through tab 0.
            KeyCode::Char('L') | KeyCode::Tab => match (self.needs_view, n) {
                (_, 0) => self.open_needs(),
                (true, _) => self.switch_tab(0),
                (false, _) if self.active + 1 >= n => self.open_needs(),
                (false, _) => self.switch_tab(self.active + 1),
            },
            KeyCode::Char('H') | KeyCode::BackTab => match (self.needs_view, n) {
                (_, 0) => self.open_needs(),
                (true, _) => self.switch_tab(n - 1),
                (false, _) if self.active == 0 => self.open_needs(),
                (false, _) => self.switch_tab(self.active - 1),
            },
            KeyCode::Char('d') if ctrl => self.move_cursor((self.page / 2).max(1) as isize),
            KeyCode::Char('u') if ctrl => self.move_cursor(-((self.page / 2).max(1) as isize)),
            KeyCode::Char('j') | KeyCode::Down if self.agents_view => self.agents_cursor += 1,
            KeyCode::Char('k') | KeyCode::Up if self.agents_view => {
                self.agents_cursor = self.agents_cursor.saturating_sub(1)
            }
            KeyCode::Char('r') if self.agents_view => self.ask_reset(),
            KeyCode::Char('j') | KeyCode::Down if self.checklist_view => {
                self.checklist_cursor = self.checklist_cursor.saturating_add(1)
            }
            // Thread and paging keys would act on the timeline under the
            // pane, out of sight.
            KeyCode::Enter
            | KeyCode::Right
            | KeyCode::Char('l')
            | KeyCode::Char('r')
            | KeyCode::Char('d')
            | KeyCode::Char('u')
            | KeyCode::PageDown
            | KeyCode::PageUp
                if self.checklist_view => {}
            KeyCode::Char('k') | KeyCode::Up if self.checklist_view => {
                self.checklist_cursor = self.checklist_cursor.saturating_sub(1)
            }
            KeyCode::Char('g') | KeyCode::Home if self.checklist_view => self.checklist_cursor = 0,
            KeyCode::Char('G') | KeyCode::End if self.checklist_view => {
                self.checklist_cursor = usize::MAX
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc | KeyCode::Char('c')
                if self.checklist_view =>
            {
                self.checklist_view = false;
            }
            KeyCode::Char('c') => {
                self.needs_view = false;
                self.routing_view = false;
                self.control_view = false;
                self.checklist_view = true;
                self.agents_view = false;
                self.checklist_cursor = 0;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quick(),
            KeyCode::Char('w') => self.open_routing(),
            KeyCode::Char('C') => self.open_control(),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('d') | KeyCode::PageDown => self.move_cursor(self.page.max(1) as isize),
            KeyCode::Char('u') | KeyCode::PageUp => self.move_cursor(-(self.page.max(1) as isize)),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(-(JUMP as isize)),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(JUMP as isize),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if self.focus == Pane::Timeline {
                    self.open_thread(false);
                }
            }
            KeyCode::Char('r') => self.open_thread(true),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc if self.agents_view => {
                self.agents_view = false;
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                if self.thread.is_some() {
                    self.close_thread();
                }
            }
            KeyCode::Char('i') => self.enter_insert(),
            KeyCode::Char('a') => {
                self.needs_view = false;
                self.routing_view = false;
                self.control_view = false;
                self.agents_view = !self.agents_view;
                self.checklist_view = false;
            }
            KeyCode::Char('+') => self.thumbs_up(),
            KeyCode::Char('s') => {
                self.picker_marks = self.chosen.iter().cloned().collect();
                self.picker_touched = false;
                self.picker_cursor = self
                    .sections
                    .iter()
                    .position(|s| self.chosen.contains(&s.id))
                    .unwrap_or(0);
                self.mode = Mode::Sections;
            }
            KeyCode::Char('R') => self.actions.push(Action::ReloadSections),
            KeyCode::Char('<') | KeyCode::Char('>') => {
                if let Some(c) = self.current().map(str::to_string) {
                    self.shift_tab(&c, if k.code == KeyCode::Char('<') { -1 } else { 1 });
                }
            }
            KeyCode::Char('T') => {
                self.order_cursor = self.active;
                self.mode = Mode::Order;
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('o') if ctrl => self.back(),
            KeyCode::Backspace => self.back(),
            KeyCode::Char('f') => {
                if self.links.is_empty() {
                    self.say("no links or code on screen");
                } else {
                    self.hint_typed.clear();
                    self.mode = Mode::Hint;
                }
            }
            KeyCode::Char('o') => self.open_viewer(),
            KeyCode::Char('O') => self.open_in_os(),
            KeyCode::Char('n') => self.bell(),
            KeyCode::Char('[') => self.nudge_width(-4),
            KeyCode::Char(']') => self.nudge_width(4),
            _ => {}
        }
    }

    /// Every image in the focused pane, oldest first, and where the
    /// cursor's message is in that list (its image, or the nearest above).
    fn pane_images(&self) -> (Vec<String>, Option<usize>) {
        let Some(id) = self.cursor_message() else {
            return (vec![], None);
        };
        let ids = if self.focus == Pane::Thread {
            self.thread_ids()
        } else {
            self.timeline_ids()
        };
        let mut list = vec![];
        let mut start = None;
        for mid in &ids {
            let Some((_, m)) = self.store.find_message(mid) else {
                continue;
            };
            let first = list.len();
            list.extend(
                m.media
                    .iter()
                    .filter(|x| x.mime.starts_with("image/"))
                    .map(|x| x.url.clone()),
            );
            if *mid == id {
                start = if list.len() > first {
                    Some(first)
                } else {
                    list.len().checked_sub(1)
                };
            }
        }
        (list, start)
    }

    /// Full-screen image viewer. Opens on the selected message's image (or
    /// the nearest one above it) and pages through every image in the pane.
    fn open_viewer(&mut self) {
        match self.pane_images() {
            (list, Some(i)) => {
                self.viewer_list = list;
                self.mode = Mode::Viewer;
                // The one on screen and its neighbours queue first, then the
                // rest, so paging never waits.
                self.show_image(i);
                for u in self.viewer_list.clone() {
                    self.actions.push(Action::LoadImage(u));
                }
            }
            _ => self.say("no images here"),
        }
    }

    /// `O`: in a thread, every image in it, in one Preview window. In the
    /// channel, the image on screen in the viewer, or the selected
    /// message's images.
    fn open_in_os(&mut self) {
        let mut current = 0;
        let urls = if self.focus == Pane::Thread {
            let (list, start) = self.pane_images();
            // The one on screen in the viewer, else the selected message's.
            current = self
                .viewer
                .as_ref()
                .and_then(|v| list.iter().position(|u| u == v))
                .or(start)
                .unwrap_or(0);
            list
        } else if let Some(u) = self.viewer.clone() {
            vec![u]
        } else {
            self.cursor_message()
                .and_then(|id| self.store.find_message(&id))
                .map(|(_, m)| {
                    m.media
                        .iter()
                        .filter(|x| x.mime.starts_with("image/"))
                        .map(|x| x.url.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        if urls.is_empty() {
            self.say("no images here");
        } else {
            self.actions.push(Action::OpenImages { urls, current });
        }
    }

    fn show_image(&mut self, i: usize) {
        let Some(u) = self.viewer_list.get(i).cloned() else {
            return;
        };
        // Fetch the neighbours too, so paging is instant.
        for j in [Some(i), i.checked_sub(1), Some(i + 1)]
            .into_iter()
            .flatten()
        {
            if let Some(n) = self.viewer_list.get(j) {
                self.actions.push(Action::LoadImage(n.clone()));
            }
        }
        self.viewer = Some(u);
    }

    fn step_viewer(&mut self, delta: isize) {
        let n = self.viewer_list.len();
        let at = self
            .viewer
            .as_ref()
            .and_then(|u| self.viewer_list.iter().position(|x| x == u))
            .unwrap_or(0);
        let next = (at as isize + delta).clamp(0, n.saturating_sub(1) as isize) as usize;
        if next != at {
            self.show_image(next);
        }
    }

    fn key_sections(&mut self, k: KeyEvent) {
        let n = self.sections.len();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down if n > 0 => {
                self.picker_cursor = (self.picker_cursor + 1).min(n - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_cursor = self.picker_cursor.saturating_sub(1)
            }
            KeyCode::Char(' ') | KeyCode::Char('x') => self.toggle_section(self.picker_cursor),
            KeyCode::Enter => self.apply_sections(),
            KeyCode::Char('m') => self.move_here(),
            KeyCode::Char('R') => self.actions.push(Action::ReloadSections),
            _ => {}
        }
    }

    fn key_order(&mut self, k: KeyEvent) {
        let n = self.tabs.len();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('T') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down if n > 0 => {
                self.order_cursor = (self.order_cursor + 1).min(n - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.order_cursor = self.order_cursor.saturating_sub(1)
            }
            KeyCode::Char('J') | KeyCode::Char('K') => {
                if let Some(c) = self.tabs.get(self.order_cursor).cloned() {
                    self.shift_tab(&c, if k.code == KeyCode::Char('K') { -1 } else { 1 });
                    if let Some(i) = self.tabs.iter().position(|t| *t == c) {
                        self.order_cursor = i;
                    }
                }
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.switch_tab(self.order_cursor);
            }
            KeyCode::Char('a') => {
                if let Some(c) = self.tabs.get(self.order_cursor).cloned() {
                    self.archive_ask = Some(c);
                    self.mode = Mode::Archive;
                }
            }
            _ => {}
        }
    }

    /// `y` on the archive ask: the tab goes now, the relay hears after.
    /// Back in the tab menu, on the tab that took its place.
    fn archive(&mut self) {
        self.mode = Mode::Order;
        let Some(channel) = self.archive_ask.take() else {
            return;
        };
        self.store.channel_mut(&channel).archived = true;
        if self.extra.as_ref() == Some(&channel) {
            self.extra = None;
        }
        if self.thread.as_ref().is_some_and(|t| t.0 == channel) {
            self.thread = None;
            self.focus = Pane::Timeline;
        }
        self.rebuild_tabs();
        self.order_cursor = self.order_cursor.min(self.tabs.len().saturating_sub(1));
        self.actions.push(Action::Archive(channel));
    }

    /// The relay said no: the tab comes back.
    pub fn unarchive(&mut self, channel: &str) {
        self.store.channel_mut(channel).archived = false;
        self.rebuild_tabs();
    }

    /// The section a tab comes from: the first of today's that holds it.
    fn tab_section(&self, channel: &str) -> Option<&Section> {
        let mut today = self
            .sections
            .iter()
            .filter(|s| self.chosen.is_empty() || self.chosen.contains(&s.id));
        if self.chosen.is_empty() {
            return today
                .next()
                .filter(|s| s.channels.iter().any(|c| c == channel));
        }
        today.find(|s| s.channels.iter().any(|c| c == channel))
    }

    /// Move a tab one place left (-1) or right (1) within its section,
    /// here at once and synced to the phone.
    fn shift_tab(&mut self, channel: &str, by: isize) {
        if !self.sections_loaded {
            return self.say("sections are still loading");
        }
        let Some(s) = self.tab_section(channel) else {
            return self.say("tabs keep their order inside a section; pick sections with s");
        };
        let (section, name) = (s.id.clone(), s.name.clone());
        // Past channels that hold a place here but show no tab in it (a
        // starred one shown under Starred): swapping with them moves nothing.
        let mut ids = s.channels.clone();
        loop {
            let Some(next) = crate::sections::shift(&ids, channel, by) else {
                return self.say(format!("end of {name}"));
            };
            ids = next;
            let i = ids.iter().position(|c| c == channel).unwrap_or(0);
            let past = &ids[(i as isize - by) as usize];
            if self.tabs.contains(past) && self.tab_section(past).is_some_and(|t| t.id == section) {
                break;
            }
        }
        if let Some(s) = self.sections.iter_mut().find(|s| s.id == section) {
            s.channels = ids.clone();
        }
        self.rebuild_tabs();
        self.actions.push(Action::Order { section, ids });
    }

    /// Move the current channel into the section under the cursor.
    fn move_here(&mut self) {
        let (Some(ch), Some(s)) = (self.current(), self.sections.get(self.picker_cursor)) else {
            return;
        };
        let section = match s.id.as_str() {
            "★" | "dms" => return self.say(format!("can't move a channel into {}", s.name)),
            "channels" => None,
            id => Some(id.to_string()),
        };
        if self.store.channels.get(ch).is_some_and(|c| c.is_dm) {
            return self.say("DMs don't go in sections");
        }
        if s.channels.iter().any(|c| c == ch) {
            return self.say(format!("already in {}", s.name));
        }
        self.actions.push(Action::MoveChannel {
            channel: ch.to_string(),
            section,
        });
    }

    fn toggle_section(&mut self, i: usize) {
        if let Some(s) = self.sections.get(i) {
            self.picker_touched = true;
            if !self.picker_marks.remove(&s.id) {
                self.picker_marks.insert(s.id.clone());
            }
        }
    }

    fn apply_sections(&mut self) {
        // The marks start as today's choice, so Enter without toggling
        // anything means "switch to this one".
        if !self.picker_touched || self.picker_marks.is_empty() {
            if let Some(s) = self.sections.get(self.picker_cursor) {
                self.picker_marks = HashSet::from([s.id.clone()]);
            }
        }
        // Keep sidebar order.
        self.chosen = self
            .sections
            .iter()
            .filter(|s| self.picker_marks.contains(&s.id))
            .map(|s| s.id.clone())
            .collect();
        self.mode = Mode::Normal;
        self.extra = None;
        self.rebuild_tabs();
        self.actions.push(Action::SaveState);
    }

    /// Narrowest the slider goes.
    pub const MIN_WIDTH: u16 = 30;

    fn nudge_width(&mut self, d: i32) {
        let cur = self.read_width.unwrap_or(self.body_max).min(self.body_max) as i32;
        self.set_width((cur + d).max(0) as u16);
        self.actions.push(Action::SaveState);
    }

    /// At or past the full width means full width, so widening the
    /// terminal later keeps filling it.
    fn set_width(&mut self, w: u16) {
        self.read_width = (w < self.body_max).then_some(w.max(Self::MIN_WIDTH));
    }

    fn link_at(&self, col: u16, row: u16) -> Option<String> {
        self.links
            .iter()
            .find(|l| {
                let r = l.rect;
                col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
            })
            .map(|l| l.url.clone())
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| {
                m.column >= r.x
                    && m.column < r.x + r.width
                    && m.row >= r.y
                    && m.row < r.y + r.height
            })
            .map(|(_, h)| h.clone());
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.set_width(m.column.saturating_sub(self.body_x));
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                self.actions.push(Action::SaveState);
            }
            MouseEventKind::Down(MouseButton::Left) if hit == Some(Hit::Handle) => {
                let now = std::time::Instant::now();
                let double = self
                    .handle_click
                    .is_some_and(|t| now.duration_since(t).as_millis() < 400);
                self.handle_click = Some(now);
                if double {
                    self.read_width = None;
                    self.actions.push(Action::SaveState);
                } else {
                    self.dragging = true;
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if self.needs_view {
                    let c = self.needs_cursor as isize + d.signum();
                    self.needs_cursor = c.max(0) as usize;
                    return;
                }
                // The routing view scrolls nothing; the channel under it stays put.
                if self.control_view {
                    // The wheel scrolls the note, or moves the list's pick.
                    if self.control_note.is_some() {
                        let s = self.control_scroll as isize + d;
                        self.control_scroll = s.clamp(0, self.control_max_scroll as isize) as usize;
                    } else {
                        let c = self.control_cursor as isize + d.signum();
                        let n = self.control_rows() as isize;
                        self.control_cursor = c.clamp(0, (n - 1).max(0)) as usize;
                    }
                    return;
                }
                if self.routing_view {
                    return;
                }
                if self.mode == Mode::Sections {
                    self.picker_cursor = (self.picker_cursor as isize + d.signum())
                        .clamp(0, self.sections.len().saturating_sub(1) as isize)
                        as usize;
                    return;
                }
                if let Some(Hit::Message(pane, _)) = &hit {
                    self.focus = *pane;
                }
                self.move_cursor(d);
            }
            MouseEventKind::Down(MouseButton::Left)
                if matches!(self.mode, Mode::Normal | Mode::Insert | Mode::Hint)
                    && self.link_at(m.column, m.row).is_some() =>
            {
                self.mode = Mode::Normal;
                if let Some(url) = self.link_at(m.column, m.row) {
                    self.follow(&url);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => match hit {
                // Under the routing view only the tab row and its boxes take clicks.
                _ if self.routing_view
                    && !matches!(
                        hit,
                        Some(Hit::Tab(_) | Hit::NeedsTab | Hit::RoutingNode(_) | Hit::Handle)
                    ) => {}
                Some(Hit::Tab(i)) => self.switch_tab(i),
                Some(Hit::NeedsTab) => self.open_needs(),
                Some(Hit::NeedsRow(_)) if self.mode != Mode::Normal => {}
                Some(Hit::NeedsRow(i)) => {
                    // Click a row to pick it, again to open it.
                    if self.needs_cursor == i {
                        self.on_key(KeyEvent::from(KeyCode::Enter));
                    } else {
                        self.needs_cursor = i;
                    }
                }
                Some(Hit::Message(pane, id)) => {
                    if self.mode == Mode::Insert {
                        self.mode = Mode::Normal;
                    }
                    let same =
                        self.focus == pane && self.cursor_message().as_deref() == Some(id.as_str());
                    self.focus = pane;
                    let ids = if pane == Pane::Thread {
                        self.thread_ids()
                    } else {
                        self.timeline_ids()
                    };
                    let at_end = ids.last() == Some(&id);
                    self.set_cursor(id, at_end);
                    // Click the selected message again to open its thread.
                    if same && pane == Pane::Timeline {
                        self.open_thread(false);
                    }
                }
                Some(Hit::Compose) => {
                    if self.mode == Mode::Normal {
                        self.enter_insert();
                    }
                }
                Some(Hit::Section(i)) => {
                    self.picker_cursor = i;
                    self.toggle_section(i);
                }
                Some(Hit::ConfirmYes) => self.on_key(KeyEvent::from(KeyCode::Char('y'))),
                Some(Hit::ConfirmNo) => self.on_key(KeyEvent::from(KeyCode::Char('n'))),
                Some(Hit::CloseThread) if self.agents_view => self.agents_view = false,
                Some(Hit::CloseThread) if self.checklist_view => self.checklist_view = false,
                Some(Hit::ChecklistItem(i)) => self.checklist_cursor = i,
                Some(Hit::RoutingNode(_)) if self.mode != Mode::Normal => {}
                Some(Hit::RoutingNode(n)) => {
                    // Click a box to pick it, again to sample it.
                    if self.routing_node == n && self.routing_sample.is_none() {
                        self.on_key(KeyEvent::from(KeyCode::Enter));
                    } else {
                        self.pick_box(n);
                    }
                }
                Some(Hit::CloseThread) => self.close_thread(),
                Some(Hit::Handle) | None => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On tab 0 nothing reaches the channel underneath: paging, the wheel,
    /// paste and ^v stay on tab 0, and row clicks count only in normal mode.
    #[test]
    fn tab_zero_keeps_input_off_the_hidden_channel() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into()];
        app.store.channel_mut("a").timeline = vec!["m1".into(), "m2".into()];
        app.selected.insert("a".into(), "m1".into());
        app.on_key(KeyEvent::from(KeyCode::Char('0')));
        assert!(app.needs_view);
        app.page = 10;
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.needs_cursor, 5);
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.needs_cursor, 0);
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.needs_cursor, 1);
        app.on_term(TermEvent::Paste("hello".into()));
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.draft.is_empty()
                && app
                    .actions
                    .iter()
                    .all(|a| !matches!(a, Action::ReadAttachment(_)))
        );
        assert_eq!(app.selected.get("a").map(String::as_str), Some("m1"));
        // A row click in another mode does nothing.
        app.hits = vec![(Rect::new(0, 3, 80, 1), Hit::NeedsRow(4))];
        app.mode = Mode::Help;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.needs_cursor, 1);
        app.mode = Mode::Normal;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.needs_cursor, 4);
    }

    #[test]
    fn q_asks_in_quick_and_again_adds_this_channel() {
        let key = |app: &mut App, k: KeyEvent| app.on_key(k);
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        let mut app = App::new("me".into(), None);
        app.channels_loaded = true;
        let here = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let c = app.store.channel_mut(here);
        c.name = "buzz-vim".into();
        c.timeline = vec!["m1".into(), "m2".into()];
        app.tabs = vec![here.into()];
        // No #quick: says so, opens nothing, makes nothing.
        key(&mut app, KeyEvent::from(KeyCode::Char('Q')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|s| s.0.contains("no #quick"))
        );
        assert!(app.actions.is_empty());
        let q = crate::quick::CHANNEL_ID;
        let qc = app.store.channel_mut(q);
        qc.name = "quick".into();
        qc.members = vec!["me".into(), "tb".into()];
        app.store.profiles.entry("tb".into()).or_default().name = "taskbot".into();
        // First press: just the question.
        key(&mut app, ctrl_q);
        assert_eq!(app.mode, Mode::Quick);
        assert!(
            matches!(app.actions.first(), Some(Action::WantProfiles(m)) if m.contains(&"tb".to_string()))
        );
        app.actions.clear();
        // Q is a letter in the box, not the key.
        for c in "Qhow?".chars() {
            key(&mut app, KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(!app.quick_include);
        key(&mut app, ctrl_q);
        assert!(app.quick_include);
        key(&mut app, ctrl_q);
        assert!(!app.quick_include);
        key(&mut app, ctrl_q);
        let (ch, agent, content) = app.quick_message().unwrap();
        assert_eq!((ch.as_str(), agent.as_str()), (q, "tb"));
        assert!(
            content.starts_with("[Q]\nContext: #buzz-vim (channel 2130646b"),
            "{content}"
        );
        assert!(content.contains("m1 m2") && content.ends_with("\n\n@taskbot Qhow?"));
        // Esc keeps the text; Enter sends it and empties the box.
        key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        key(&mut app, KeyEvent::from(KeyCode::Char('Q')));
        assert_eq!(app.quick_draft.text, "Qhow?");
        assert!(!app.quick_include);
        key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.quick_draft.is_empty());
        assert!(app.actions.iter().any(|a| matches!(a,
            Action::SendQuick { channel, agent, content, text }
            if channel == q && agent == "tb" && content == "[Q]\n@taskbot Qhow?" && text == "Qhow?")));
        // ^c in the box clears it, then closes it; it never quits.
        app.actions.clear();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        key(&mut app, KeyEvent::from(KeyCode::Char('Q')));
        key(&mut app, KeyEvent::from(KeyCode::Char('x')));
        key(&mut app, ctrl_c);
        assert!(app.quick_draft.is_empty() && app.mode == Mode::Quick);
        key(&mut app, ctrl_c);
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.actions.iter().any(|a| matches!(a, Action::Quit)));
    }

    /// On tab 0 the channel under it is hidden: a Q can't add it.
    #[test]
    fn q_on_tab_zero_adds_no_channel() {
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        let mut app = App::new("me".into(), None);
        app.channels_loaded = true;
        app.store.channel_mut("a").name = "hidden".into();
        app.tabs = vec!["a".into()];
        let qc = app.store.channel_mut(crate::quick::CHANNEL_ID);
        qc.name = "quick".into();
        qc.members = vec!["me".into(), "tb".into()];
        app.store.profiles.entry("tb".into()).or_default().name = "taskbot".into();
        app.on_key(KeyEvent::from(KeyCode::Char('0')));
        assert!(app.needs_view);
        // Lower-case q opens the box too; it no longer quits.
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert_eq!(app.mode, Mode::Quick);
        assert!(!app.actions.iter().any(|a| matches!(a, Action::Quit)));
        app.on_key(KeyEvent::from(KeyCode::Char('x')));
        app.on_key(ctrl_q);
        assert!(!app.quick_include);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|s| s.0 == "no channel open to add")
        );
        app.quick_include = true;
        assert_eq!(app.quick_message(), Err("no channel open to add".into()));
        app.quick_include = false;
        let (_, _, content) = app.quick_message().unwrap();
        assert_eq!(content, "[Q]\n@taskbot x");
    }

    #[test]
    fn c_opens_the_checklist_and_moves_in_it() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into()];
        app.store.channel_mut("a").timeline = vec!["m1".into(), "m2".into()];
        let key = |app: &mut App, c| app.on_key(KeyEvent::from(c));
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Char('c'));
        assert!(app.checklist_view && !app.agents_view);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.checklist_cursor, 1);
        // j moved the list, not the timeline.
        assert_eq!(app.selected.get("a"), None);
        key(&mut app, KeyCode::Esc);
        assert!(!app.checklist_view);
        key(&mut app, KeyCode::Char('c'));
        key(&mut app, KeyCode::Char('c'));
        assert!(!app.checklist_view);
    }

    #[test]
    fn m_moves_the_current_channel() {
        let mut app = App::new("me".into(), None);
        app.sections = ["a", "b", "channels"]
            .map(|id| Section {
                id: id.into(),
                name: id.into(),
                channels: vec![format!("{id}-ch")],
            })
            .to_vec();
        app.chosen = vec!["a".into()];
        app.rebuild_tabs();
        for code in [KeyCode::Char('s'), KeyCode::Char('j'), KeyCode::Char('m')] {
            app.on_key(KeyEvent::from(code));
        }
        let moved = |app: &App, want: Option<&str>| {
            app.actions.iter().any(|a| {
                matches!(a, Action::MoveChannel { channel, section }
                if channel == "a-ch" && section.as_deref() == want)
            })
        };
        assert!(moved(&app, Some("b")));
        app.actions.clear();
        for code in [KeyCode::Char('j'), KeyCode::Char('m')] {
            app.on_key(KeyEvent::from(code));
        }
        assert!(moved(&app, None));
        app.actions.clear();
        // Its own section is a no-op.
        for code in [KeyCode::Char('k'), KeyCode::Char('k'), KeyCode::Char('m')] {
            app.on_key(KeyEvent::from(code));
        }
        assert!(app.actions.is_empty());
    }

    #[test]
    fn enter_switches_section() {
        let mut app = App::new("me".into(), None);
        app.sections = ["a", "b"]
            .map(|id| Section {
                id: id.into(),
                name: id.into(),
                channels: vec![format!("{id}-ch")],
            })
            .to_vec();
        app.chosen = vec!["a".into()];
        for code in [KeyCode::Char('s'), KeyCode::Char('j'), KeyCode::Enter] {
            app.on_key(KeyEvent::from(code));
        }
        assert_eq!(app.chosen, vec!["b".to_string()]);
        assert_eq!(app.tabs, vec!["b-ch".to_string()]);
        // Space marks several.
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('k'),
            KeyCode::Char(' '),
            KeyCode::Enter,
        ] {
            app.on_key(KeyEvent::from(code));
        }
        assert_eq!(app.chosen, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn caps_lock_state_types_capitals() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into()];
        app.mode = Mode::Insert;
        for c in ['h', 'é', '1'] {
            app.on_key(KeyEvent::new_with_kind_and_state(
                KeyCode::Char(c),
                KeyModifiers::NONE,
                crossterm::event::KeyEventKind::Press,
                KeyEventState::CAPS_LOCK,
            ));
        }
        assert_eq!(app.draft_ref().text, "HÉ1");
    }

    #[test]
    fn g_and_shift_g_skip_paging_a_tall_message() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into()];
        app.store.channel_mut("a").timeline = vec!["m1".into(), "m2".into()];
        app.selected.insert("a".into(), "m1".into());
        // On a tall first message, G goes to the bottom of the last at once.
        app.tall = Some(("m1".into(), 100, 10, 0));
        app.on_key(KeyEvent::from(KeyCode::Char('G')));
        assert_eq!(app.selected.get("a"), None);
        assert_eq!(app.peek, Some(("m2".into(), usize::MAX)));
        // On a tall last message, g goes to the top of the first.
        app.tall = Some(("m2".into(), 100, 10, 50));
        app.on_key(KeyEvent::from(KeyCode::Char('g')));
        assert_eq!(app.selected.get("a").map(String::as_str), Some("m1"));
        assert_eq!(app.peek, Some(("m1".into(), 0)));
    }

    #[test]
    fn links_jump_and_come_back() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into(), "b".into()];
        app.store.channel_mut("a").timeline = vec!["m1".into()];
        // Into a channel outside the tabs: an extra tab, then back.
        app.follow("buzz://message?channel=x&id=r1");
        assert_eq!(app.tabs, vec!["a", "b", "x"]);
        assert_eq!(app.current(), Some("x"));
        // Not loaded, so it opens as a thread to fetch it.
        assert_eq!(app.thread, Some(("x".into(), "r1".into())));
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(app.tabs, vec!["a", "b"]);
        assert_eq!(app.current(), Some("a"));
        assert_eq!(app.thread, None);
        // A loaded top-level message is picked in the timeline; ^o goes back.
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        app.follow("buzz://message?channel=a&id=m1");
        assert_eq!(
            (app.current(), app.selected.get("a")),
            (Some("a"), Some(&"m1".to_string()))
        );
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(app.current(), Some("b"));
        // Leaving the extra tab drops it.
        app.follow("buzz://channel/x");
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        assert_eq!(app.tabs, vec!["a", "b"]);
        // One extra tab to another still opens the new channel.
        app.follow("buzz://channel/x");
        app.actions.clear();
        app.follow("buzz://channel/y");
        assert_eq!(app.tabs, vec!["a", "b", "y"]);
        assert!(matches!(app.actions.first(), Some(Action::OpenChannel(c)) if c == "y"));
        // Following a link to where you already are adds no step.
        let n = app.jumps.len();
        app.follow("buzz://channel/y");
        assert_eq!(app.jumps.len(), n);
        // A reply in the open thread picks it, keeping the timeline selection.
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        app.store.channel_mut("a").timeline = vec!["m1".into(), "m2".into()];
        app.selected.insert("a".into(), "m1".into());
        app.thread = Some(("a".into(), "m1".into()));
        let n = app.jumps.len();
        app.follow("buzz://message?channel=a&id=r9&thread=m1");
        assert_eq!(app.jumps.len(), n);
        assert_eq!(app.thread_selected.as_deref(), Some("r9"));
        assert_eq!(app.selected.get("a").map(String::as_str), Some("m1"));
        // The newest message while following the bottom is where you are.
        app.thread = None;
        app.selected.remove("a");
        app.follow("buzz://message?channel=a&id=m2");
        assert_eq!(app.jumps.len(), n);
        // Desktop and web links go to the OS.
        app.follow("buzz://pr?id=1");
        assert!(matches!(app.actions.last(), Some(Action::OpenUrl(u)) if u == "buzz://pr?id=1"));
        // A code block copies its text.
        app.follow("copy:let x = 1;\nlet y = 2;");
        assert!(
            matches!(app.actions.last(), Some(Action::Copy(t)) if t == "let x = 1;\nlet y = 2;")
        );
    }

    #[test]
    fn l_opens_thread() {
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let root = EventBuilder::new(Kind::Custom(9), "hi")
            .tags([Tag::parse(["h", "c1"]).unwrap()])
            .sign_with_keys(&k)
            .unwrap();
        app.store.apply(&root);
        app.tabs = vec!["c1".into()];
        app.on_key(KeyEvent::from(KeyCode::Char('l')));
        assert_eq!(app.thread, Some(("c1".into(), root.id.to_hex())));
    }

    #[test]
    fn viewer_queues_the_thread_and_o_opens_all_of_it() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["ch".into()];
        let msg = |id: &str, at: u64, root: Option<&str>, imgs: &[&str]| crate::model::Message {
            id: id.into(),
            author: "me".into(),
            created_at: at,
            content: String::new(),
            edited: false,
            deleted: false,
            root: root.map(str::to_string),
            mentions: vec![],
            media: imgs
                .iter()
                .map(|u| crate::model::Media {
                    url: u.to_string(),
                    mime: "image/png".into(),
                    dim: None,
                    name: None,
                })
                .collect(),
            system: false,
        };
        let ch = app.store.channel_mut("ch");
        ch.messages.insert("t".into(), msg("t", 5, None, &["top"]));
        ch.messages.insert("r".into(), msg("r", 10, None, &["a"]));
        ch.messages
            .insert("r1".into(), msg("r1", 20, Some("r"), &["b", "c"]));
        ch.messages
            .insert("r2".into(), msg("r2", 30, Some("r"), &["d"]));
        ch.timeline = vec!["t".into(), "r".into()];
        ch.replies
            .insert("r".into(), vec!["r1".into(), "r2".into()]);
        app.thread = Some(("ch".into(), "r".into()));
        app.focus = Pane::Thread;
        app.thread_selected = Some("r1".into());
        app.on_key(KeyEvent::from(KeyCode::Char('o')));
        let loads: Vec<&str> = app
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::LoadImage(u) => Some(u.as_str()),
                _ => None,
            })
            .collect();
        // The one on screen and its neighbours first, then the whole thread.
        assert_eq!(&loads[..3], ["b", "a", "c"]);
        assert!(["a", "b", "c", "d"].iter().all(|u| loads.contains(u)));
        assert!(!loads.contains(&"top"));
        app.actions.clear();
        // O in a thread: every image in it, in thread order, opened on the
        // one in the viewer.
        app.on_key(KeyEvent::from(KeyCode::Char('O')));
        assert!(matches!(app.actions.last(),
            Some(Action::OpenImages { urls, current: 1 }) if urls == &["a", "b", "c", "d"]));
        // In the channel: just the selected message's.
        app.mode = Mode::Normal;
        app.viewer = None;
        app.focus = Pane::Timeline;
        app.selected.insert("ch".into(), "t".into());
        app.on_key(KeyEvent::from(KeyCode::Char('O')));
        assert!(
            matches!(app.actions.last(), Some(Action::OpenImages { urls, current: 0 }) if urls == &["top"])
        );
    }

    #[test]
    fn r_on_the_agents_view_asks_then_rotates_the_picked_agent() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["ch".into()];
        for (pk, name) in [("a1", "alpha"), ("b2", "beta")] {
            app.agents.own(pk, name);
            app.agents.presence(pk, "online");
        }
        app.agents_view = true;
        app.on_key(KeyEvent::from(KeyCode::Char('j')));
        app.on_key(KeyEvent::from(KeyCode::Char('r')));
        assert_eq!(app.mode, Mode::Reset);
        assert!(app.actions.is_empty());
        app.on_key(KeyEvent::from(KeyCode::Char('n')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.actions.is_empty());
        app.on_key(KeyEvent::from(KeyCode::Char('r')));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        assert!(matches!(app.actions.first(),
            Some(Action::Reset { channel, root: None, agent }) if channel == "ch" && agent == "b2"));
    }

    #[test]
    fn blank_insert_starts_with_last_bot() {
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["ch".into()];
        for (pk, name, agent) in [
            ("bot1", "judgebot", true),
            ("bot2", "taskbot", true),
            ("al", "Al", false),
        ] {
            let p = app.store.profiles.entry(pk.into()).or_default();
            p.name = name.into();
            p.is_agent = agent;
        }
        let msg = |id: &str, at: u64, mentions: &[&str]| crate::model::Message {
            id: id.into(),
            author: "me".into(),
            created_at: at,
            content: String::new(),
            edited: false,
            deleted: false,
            root: None,
            mentions: mentions.iter().map(|s| s.to_string()).collect(),
            media: vec![],
            system: false,
        };
        let ch = app.store.channel_mut("ch");
        ch.messages.insert("m1".into(), msg("m1", 10, &["bot1"]));
        ch.messages
            .insert("m2".into(), msg("m2", 20, &["al", "bot2"]));
        ch.messages.insert("m3".into(), msg("m3", 30, &["al"]));
        let key = |app: &mut App, c| app.on_key(KeyEvent::from(c));
        key(&mut app, KeyCode::Char('i'));
        assert_eq!(app.draft.text, "@taskbot ");
        assert_eq!(app.draft.mention_pubkeys(), vec!["bot2".to_string()]);
        // Esc with nothing typed leaves no draft behind.
        key(&mut app, KeyCode::Esc);
        assert!(app.draft.text.is_empty());
        // Sending to a bot remembers it, newer than history.
        key(&mut app, KeyCode::Char('i'));
        app.draft.clear();
        app.draft.insert("hi ");
        app.draft.complete_mention(3, "judgebot", "bot1");
        app.send();
        assert_eq!(app.last_bot.get("ch").map(|b| b.0.as_str()), Some("bot1"));
        app.draft.clear();
        key(&mut app, KeyCode::Char('i'));
        assert_eq!(app.draft.text, "@judgebot ");
        // Something typed after it stays on Esc; a non-blank draft is left alone.
        key(&mut app, KeyCode::Char('x'));
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('i'));
        assert_eq!(app.draft.text, "@judgebot x");
    }

    #[test]
    fn n_goes_to_the_newest_activity_and_stays() {
        let now = crate::sync::now();
        let mut app = App::new("me".into(), None);
        app.tabs = vec!["a".into(), "b".into(), "c".into()];
        let put = |app: &mut App, ch: &str, id: &str, at: u64, root: Option<&str>| {
            let c = app.store.channel_mut(ch);
            c.messages.insert(
                id.into(),
                crate::model::Message {
                    id: id.into(),
                    author: "them".into(),
                    created_at: at,
                    root: root.map(str::to_string),
                    ..Default::default()
                },
            );
            match root {
                Some(r) => c.replies.entry(r.into()).or_default().push(id.into()),
                None => c.timeline.push(id.into()),
            }
        };
        put(&mut app, "a", "a1", now - 50, None);
        put(&mut app, "a", "a2", now - 40, None);
        put(&mut app, "a", "a1r", now - 30, Some("a1"));
        app.store.channel_mut("b").unread.insert("x".into(), now);
        let key = |app: &mut App| app.on_key(KeyEvent::from(KeyCode::Char('n')));
        // The reply is newest: a1's thread opens on it.
        key(&mut app);
        assert_eq!(app.thread, Some(("a".into(), "a1".into())));
        assert_eq!(app.cursor_message().as_deref(), Some("a1r"));
        assert_eq!(app.anchor, Some((Pane::Thread, "a1r".into())));
        // Again: same place, same tab.
        key(&mut app);
        assert_eq!(app.current(), Some("a"));
        assert_eq!(app.cursor_message().as_deref(), Some("a1r"));
        // A newer top-level post takes over.
        put(&mut app, "a", "a3", now - 10, None);
        key(&mut app);
        assert_eq!(app.thread, None);
        assert_eq!(app.cursor_message().as_deref(), Some("a3"));
    }

    #[test]
    fn tabs_move_within_their_section_and_sync() {
        let mut app = App::new("me".into(), None);
        for id in ["a", "b", "c", "x"] {
            let c = app.store.channel_mut(id);
            c.name = id.into();
            c.is_member = true;
        }
        app.store.channel_mut("b").archived = true;
        app.sections = vec![Section {
            id: "s".into(),
            name: "S".into(),
            channels: vec!["a".into(), "b".into(), "c".into(), "x".into()],
        }];
        app.chosen = vec!["s".into()];
        app.rebuild_tabs();
        assert_eq!(app.tabs, vec!["a", "c", "x"]);
        let key = |app: &mut App, c: char| app.on_key(KeyEvent::from(KeyCode::Char(c)));
        // Not before the synced order has loaded: it would write over it.
        key(&mut app, '>');
        assert!(
            !app.actions
                .iter()
                .any(|a| matches!(a, Action::Order { .. }))
        );
        app.sections_loaded = true;
        // > on a: past the archived b too, so the tab visibly moves.
        key(&mut app, '>');
        assert_eq!(app.tabs, vec!["c", "a", "x"]);
        assert_eq!(app.current(), Some("a"));
        assert!(
            app.actions
                .iter()
                .any(|a| matches!(a, Action::Order { section, ids }
            if section == "s" && ids == &["b", "c", "a", "x"]))
        );
        key(&mut app, '<');
        key(&mut app, '<');
        assert_eq!(app.tabs, vec!["a", "c", "x"]);
        // T: J moves the picked tab down, the cursor going with it.
        key(&mut app, 'T');
        assert_eq!(app.mode, Mode::Order);
        key(&mut app, 'j');
        key(&mut app, 'J');
        assert_eq!(app.tabs, vec!["a", "x", "c"]);
        assert_eq!(app.order_cursor, 2);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.current(), Some("c"));
    }

    #[test]
    fn a_in_the_tab_menu_archives_after_a_yes() {
        let mut app = App::new("me".into(), None);
        for id in ["a", "b", "c"] {
            let c = app.store.channel_mut(id);
            c.name = id.into();
            c.is_member = true;
        }
        app.rebuild_tabs();
        let key = |app: &mut App, c: char| app.on_key(KeyEvent::from(KeyCode::Char(c)));
        key(&mut app, 'T');
        key(&mut app, 'j');
        // n keeps it.
        key(&mut app, 'a');
        assert_eq!(app.mode, Mode::Archive);
        key(&mut app, 'n');
        assert_eq!(app.mode, Mode::Order);
        assert_eq!(app.tabs, vec!["a", "b", "c"]);
        // y: gone now, the relay told, still in the menu.
        key(&mut app, 'a');
        key(&mut app, 'y');
        assert_eq!(app.mode, Mode::Order);
        assert_eq!(app.tabs, vec!["a", "c"]);
        assert!(
            app.actions
                .iter()
                .any(|a| matches!(a, Action::Archive(c) if c == "b"))
        );
        // The relay refusing brings it back.
        app.unarchive("b");
        assert_eq!(app.tabs, vec!["a", "b", "c"]);
    }
}
