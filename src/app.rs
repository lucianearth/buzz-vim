//! App state and input handling. Rendering is in `ui.rs`, relay IO in `sync.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;

use crate::clip::Pending;
use crate::draft::Draft;
use crate::keys::Mode;
use crate::live::ConnState;
use crate::media::Media;
use crate::model::Store;
use crate::readstate::{self, ReadState};
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
    Upload,
    LoadOlder(String),
    ReloadSections,
    /// Move a channel into a section (`None` = the unsectioned group).
    MoveChannel {
        channel: String,
        section: Option<String>,
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
    /// Download images and open them in the OS viewer (Preview), all in
    /// one window.
    OpenImages(Vec<String>),
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
    /// channel -> where it was read up to when you came into it: newer
    /// top-level messages are what the jump key visits.
    pub entry: HashMap<String, u64>,
    /// Top-level messages the jump key has already been to.
    pub visited: HashSet<String>,
    /// Identities the relay has archived: left out of @ completion.
    pub archived: HashSet<String>,
    /// A message to keep at the top of its pane while it is the cursor
    /// (the jump key lands on the first unread with it at the top).
    pub anchor: Option<(Pane, String)>,
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
            entry: HashMap::new(),
            visited: HashSet::new(),
            archived: HashSet::new(),
            anchor: None,
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
            // Before OpenChannel marks it read. Until the startup load is
            // in, the position is unknown; that load fills it in.
            if let Some(at) = self.read.read_at(&id).filter(|_| self.read.tried) {
                self.entry.insert(id.clone(), at);
            }
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

    /// The jump key: the first unread thing in this tab, oldest first; a
    /// reply opens its thread with that reply at the top. With nothing
    /// unread here, on to the next tab that has some, mentions first.
    fn next_unread(&mut self) {
        let now = crate::sync::now();
        let here = |app: &App| {
            let c = app.current()?;
            unread::next_unread(
                app.store.channels.get(c)?,
                &app.store.me,
                &app.read,
                app.entry.get(c).copied(),
                &app.visited,
                now,
            )
        };
        if let Some(n) = here(self) {
            self.land(n);
            return;
        }
        let len = self.tabs.len();
        let order: Vec<usize> = (1..len).map(|k| (self.active + k) % len).collect();
        let count = |i: &usize, mentions: bool| {
            self.store.channels.get(&self.tabs[*i]).map_or(0, |c| {
                if mentions && !c.is_dm {
                    c.mentioned.len()
                } else {
                    c.unread.len()
                }
            })
        };
        let pick = order
            .iter()
            .find(|i| count(i, true) > 0)
            .or_else(|| order.iter().find(|i| count(i, false) > 0))
            .copied();
        match pick {
            Some(i) => {
                self.switch_tab(i);
                if let Some(n) = here(self) {
                    self.land(n);
                }
            }
            None => self.say("nothing unread"),
        }
    }

    fn land(&mut self, n: Next) {
        let Some(c) = self.current().map(str::to_string) else {
            return;
        };
        self.peek = None;
        match n {
            Next::Message(id) => {
                self.visited.insert(id.clone());
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
        match crate::links::target(url) {
            Target::Message {
                channel,
                id,
                thread,
            } => {
                // A reply opens its thread. So does a message older than
                // what's loaded: the thread fetch gets it by id.
                let known = self.store.channels.get(&channel);
                let in_timeline = |id: &String| known.is_some_and(|c| c.timeline.contains(id));
                let thread = thread.or_else(|| {
                    let id = id.as_ref()?;
                    match known.and_then(|c| c.messages.get(id)) {
                        Some(m) => m.root.clone(),
                        None if in_timeline(id) => None,
                        None => Some(id.clone()),
                    }
                });
                let jump = Jump {
                    selected: id.clone().filter(|i| thread.is_none() && in_timeline(i)),
                    thread: thread.map(|r| {
                        let pick = id.filter(|i| *i != r);
                        (r, pick)
                    }),
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
                    // Already here: just pick the reply, keep the timeline as is.
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

    /// Candidates for the `@query` being typed: channel members first.
    pub fn mention_candidates(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.draft_ref().mention_query() else {
            return vec![];
        };
        let q = q.to_lowercase();
        let ch = match (&self.thread, self.focus) {
            (Some((c, _)), Pane::Thread) => Some(c.as_str()),
            _ => self.current(),
        };
        let members: Vec<String> = ch
            .and_then(|c| self.store.channels.get(c))
            .map(|c| c.members.clone())
            .unwrap_or_default();
        let mut out: Vec<(String, String)> = members
            .iter()
            .filter(|pk| **pk != self.store.me && !self.archived.contains(*pk))
            .filter_map(|pk| {
                self.store
                    .profiles
                    .get(pk)
                    .map(|p| (p.name.clone(), pk.clone()))
            })
            .filter(|(n, _)| !n.is_empty() && n.to_lowercase().contains(&q))
            .collect();
        out.sort_by_key(|(n, _)| (!n.to_lowercase().starts_with(&q), n.to_lowercase()));
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
        let paths = crate::clip::dropped_paths(&s);
        if let Some(p) = paths.first() {
            self.say(format!("reading {}…", p.display()));
            self.actions.push(Action::ReadAttachment(Some(p.clone())));
            return;
        }
        if self.mode != Mode::Insert {
            self.mode = Mode::Insert;
        }
        self.draft_mut()
            .insert(&s.replace("\r\n", "\n").replace('\r', "\n"));
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
        if ctrl && k.code == KeyCode::Char('c') {
            if self.mode == Mode::Insert && !self.draft_ref().is_empty() {
                self.draft_mut().clear();
                self.mode = Mode::Normal;
            } else {
                self.actions.push(Action::Quit);
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('v') && matches!(self.mode, Mode::Normal | Mode::Insert)
        {
            self.paste_clipboard();
            return;
        }
        match self.mode {
            Mode::Insert => self.key_insert(k),
            Mode::Normal => self.key_normal(k),
            Mode::Sections => self.key_sections(k),
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
        match k.code {
            KeyCode::Char('q') => self.actions.push(Action::Quit),
            KeyCode::Char(c @ '0'..='9') => self.switch_tab(c as usize - '0' as usize),
            KeyCode::Char('L') | KeyCode::Tab => {
                if !self.tabs.is_empty() {
                    self.switch_tab((self.active + 1) % self.tabs.len());
                }
            }
            KeyCode::Char('H') | KeyCode::BackTab => {
                if !self.tabs.is_empty() {
                    self.switch_tab((self.active + self.tabs.len() - 1) % self.tabs.len());
                }
            }
            KeyCode::Char('d') if ctrl => self.move_cursor((self.page / 2).max(1) as isize),
            KeyCode::Char('u') if ctrl => self.move_cursor(-((self.page / 2).max(1) as isize)),
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
            KeyCode::Char('a') => self.agents_view = !self.agents_view,
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
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('o') if ctrl => self.back(),
            KeyCode::Backspace => self.back(),
            KeyCode::Char('f') => {
                if self.links.is_empty() {
                    self.say("no links on screen");
                } else {
                    self.hint_typed.clear();
                    self.mode = Mode::Hint;
                }
            }
            KeyCode::Char('o') => self.open_viewer(),
            KeyCode::Char('O') => self.open_in_os(),
            KeyCode::Char('n') => self.next_unread(),
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
        let urls = if self.focus == Pane::Thread {
            self.pane_images().0
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
            self.actions.push(Action::OpenImages(urls));
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
                Some(Hit::Tab(i)) => self.switch_tab(i),
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
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        app.follow("buzz://message?channel=a&id=m1");
        assert_eq!(
            (app.current(), app.selected.get("a")),
            (Some("a"), Some(&"m1".to_string()))
        );
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(app.current(), Some("b"));
        // Leaving the extra tab drops it.
        app.follow("buzz://channel/x");
        app.on_key(KeyEvent::from(KeyCode::Char('0')));
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
        app.on_key(KeyEvent::from(KeyCode::Char('0')));
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
        // O in a thread: every image in it, in thread order.
        app.on_key(KeyEvent::from(KeyCode::Char('O')));
        assert!(
            matches!(app.actions.last(), Some(Action::OpenImages(u)) if u == &["a", "b", "c", "d"])
        );
        // In the channel: just the selected message's.
        app.mode = Mode::Normal;
        app.viewer = None;
        app.focus = Pane::Timeline;
        app.selected.insert("ch".into(), "t".into());
        app.on_key(KeyEvent::from(KeyCode::Char('O')));
        assert!(matches!(app.actions.last(), Some(Action::OpenImages(u)) if u == &["top"]));
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
    fn n_walks_unread_then_moves_on_mentions_first() {
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
        put(&mut app, "a", "a1r", now - 40, Some("a1"));
        put(&mut app, "a", "a2", now - 30, None);
        app.entry.insert("a".into(), now - 45);
        let key = |app: &mut App| app.on_key(KeyEvent::from(KeyCode::Char('n')));
        // The reply in a1's thread is oldest: its thread opens on it.
        key(&mut app);
        assert_eq!(app.thread, Some(("a".into(), "a1".into())));
        assert_eq!(app.cursor_message().as_deref(), Some("a1r"));
        assert_eq!(app.anchor, Some((Pane::Thread, "a1r".into())));
        // Then the newer top-level message.
        key(&mut app);
        assert_eq!(app.thread, None);
        assert_eq!(app.cursor_message().as_deref(), Some("a2"));
        // Nothing left here: the tab that mentions you comes before the one
        // that is merely unread.
        app.store.channel_mut("b").unread.insert("x".into(), now);
        let c = app.store.channel_mut("c");
        c.unread.insert("y".into(), now);
        c.mentioned.insert("y".into());
        key(&mut app);
        assert_eq!(app.current(), Some("c"));
        assert!(app.store.channels["c"].mentioned.is_empty());
    }
}
