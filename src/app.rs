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
    /// Section ids whose channels are the tabs.
    pub chosen: Vec<String>,
    pub picker_cursor: usize,
    pub picker_marks: HashSet<String>,
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
    pub show_agents: bool,
    pub agents: Agents,
    pub media: Media,
    pub picker: Option<Picker>,
    pub viewer: Option<String>,
    pub hits: Vec<(Rect, Hit)>,
    /// Timeline/thread page height from the last draw, for ^d/^u.
    pub page: usize,
    pub actions: Vec<Action>,
    /// Drafts of messages in flight, restored if the relay rejects them.
    pub pending_sends: HashMap<String, Draft>,
    pub relay_host: String,
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
            chosen: vec![],
            picker_cursor: 0,
            picker_marks: HashSet::new(),
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
            show_agents: true,
            agents: Agents::default(),
            media: Media::default(),
            picker,
            viewer: None,
            hits: vec![],
            page: 20,
            actions: vec![],
            pending_sends: HashMap::new(),
            relay_host: String::new(),
        }
    }

    /// Whether a tick should trigger a redraw (spinners, flashes, loading).
    pub fn animating(&self) -> bool {
        self.uploading
            || self.conn != ConnState::Online
            || self.agents.by_pubkey.values().any(|a| a.busy)
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
            self.store.channel_mut(&id).unread = 0;
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
            self.mode = Mode::Insert;
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
        self.actions.push(Action::Send {
            channel,
            root,
            parent,
        });
    }

    // ---------- mentions ----------

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
            Mode::Viewer => {
                if matches!(
                    k.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') | KeyCode::Enter
                ) {
                    self.viewer = None;
                    self.mode = Mode::Normal;
                }
            }
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
                self.mode = Mode::Normal;
            }
            KeyCode::Tab if !cands.is_empty() => {
                self.accept_mention();
            }
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
            KeyCode::Char(c @ '1'..='9') => self.switch_tab(c as usize - '1' as usize),
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
            KeyCode::PageDown => self.move_cursor(self.page.max(1) as isize),
            KeyCode::PageUp => self.move_cursor(-(self.page.max(1) as isize)),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(-100_000),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(100_000),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if self.focus == Pane::Timeline {
                    self.open_thread(false);
                }
            }
            KeyCode::Char('r') => self.open_thread(true),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                if self.thread.is_some() {
                    if self.focus == Pane::Thread || k.code != KeyCode::Esc {
                        self.close_thread();
                    } else {
                        self.focus = Pane::Thread;
                    }
                }
            }
            KeyCode::Char('i') | KeyCode::Char('a') => self.mode = Mode::Insert,
            KeyCode::Char('+') => self.thumbs_up(),
            KeyCode::Char('s') => {
                self.picker_marks = self.chosen.iter().cloned().collect();
                self.picker_cursor = 0;
                self.mode = Mode::Sections;
            }
            KeyCode::Char('R') => self.actions.push(Action::ReloadSections),
            KeyCode::Char('t') => self.show_agents = !self.show_agents,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('o') => self.open_viewer(),
            _ => {}
        }
    }

    fn open_viewer(&mut self) {
        let Some(id) = self.cursor_message() else {
            return;
        };
        let url = self.store.find_message(&id).and_then(|(_, m)| {
            m.media
                .iter()
                .find(|x| x.mime.starts_with("image/"))
                .map(|x| x.url.clone())
        });
        match url {
            Some(u) => {
                self.actions.push(Action::LoadImage(u.clone()));
                self.viewer = Some(u);
                self.mode = Mode::Viewer;
            }
            None => self.say("no image on this message"),
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
            if !self.picker_marks.remove(&s.id) {
                self.picker_marks.insert(s.id.clone());
            }
        }
    }

    fn apply_sections(&mut self) {
        if self.picker_marks.is_empty() {
            if let Some(s) = self.sections.get(self.picker_cursor) {
                self.picker_marks.insert(s.id.clone());
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
                        self.mode = Mode::Insert;
                    }
                }
                Some(Hit::Section(i)) => {
                    self.picker_cursor = i;
                    self.toggle_section(i);
                }
                Some(Hit::ConfirmYes) => self.on_key(KeyEvent::from(KeyCode::Char('y'))),
                Some(Hit::ConfirmNo) => self.on_key(KeyEvent::from(KeyCode::Char('n'))),
                Some(Hit::CloseThread) => self.close_thread(),
                None => {}
            },
            _ => {}
        }
    }
}
