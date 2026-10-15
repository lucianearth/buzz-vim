//! App state and input handling. Rendering is in `ui.rs`, relay IO in `sync.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;

use crate::clip::Pending;
use crate::draft::Draft;
use crate::keys::Mode;
use crate::live::ConnState;
use crate::media::Media;
use crate::model::Store;
use crate::sections::Section;
use crate::telemetry::Agents;

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
    LoadImage(String),
    EncodeImage((String, u16, u16)),
    /// Decode the pending upload so the confirm box can show it.
    PreviewPending,
    OpenThread {
        channel: String,
        root: String,
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
        self.tabs = tabs;
        self.active = keep
            .and_then(|k| self.tabs.iter().position(|t| *t == k))
            .unwrap_or(0);
        self.open_current();
    }

    pub fn open_current(&mut self) {
        if let Some(id) = self.current().map(str::to_string) {
            self.store.channel_mut(&id).unread.clear();
            self.actions.push(Action::OpenChannel(id));
        }
    }

    fn switch_tab(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.thread = None;
            self.focus = Pane::Timeline;
            self.open_current();
            self.actions.push(Action::SaveState);
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
        // A message taller than the pane pages through its lines first,
        // like a pager, and only then moves on.
        if let Some((id, bh, h, off)) = self.tall.clone() {
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
        if next == cur {
            return;
        }
        let off = if delta < 0 { usize::MAX } else { 0 };
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
        self.thread = Some((ch, root));
        self.thread_selected = None;
        self.focus = Pane::Thread;
        if insert {
            self.enter_insert();
        }
    }

    fn close_thread(&mut self) {
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
            .filter(|pk| **pk != self.store.me)
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
            match crate::clip::from_file(p, "dropped file") {
                Ok(pending) => {
                    self.pending = Some(pending);
                    self.mode = Mode::Confirm;
                    self.actions.push(Action::PreviewPending);
                }
                Err(e) => self.say(format!("can't attach {}: {e}", p.display())),
            }
            return;
        }
        if self.mode != Mode::Insert {
            self.mode = Mode::Insert;
        }
        self.draft_mut()
            .insert(&s.replace("\r\n", "\n").replace('\r', "\n"));
    }

    fn paste_clipboard(&mut self) {
        match crate::clip::from_clipboard() {
            Ok(p) => {
                self.pending = Some(p);
                self.mode = Mode::Confirm;
                self.actions.push(Action::PreviewPending);
            }
            Err(e) => self.say(e.to_string()),
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
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
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('j') | KeyCode::Down => {
                    self.step_viewer(1)
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
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(-100_000),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(100_000),
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
            KeyCode::Char('o') => self.open_viewer(),
            KeyCode::Char('[') => self.nudge_width(-4),
            KeyCode::Char(']') => self.nudge_width(4),
            _ => {}
        }
    }

    /// Full-screen image viewer. Opens on the selected message's image (or
    /// the nearest one above it) and pages through every image in the pane.
    fn open_viewer(&mut self) {
        let Some(id) = self.cursor_message() else {
            return;
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
        match start {
            Some(i) => {
                self.viewer_list = list;
                self.mode = Mode::Viewer;
                self.show_image(i);
            }
            None => self.say("no images here"),
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
            KeyCode::Char('R') => self.actions.push(Action::ReloadSections),
            _ => {}
        }
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
}
