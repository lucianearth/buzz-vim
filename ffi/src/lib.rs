//! The iPhone app's door into buzz-vim-core, through UniFFI.
//!
//! One `Client` per login. It owns the live socket and a background loop
//! that folds everything the relay says into one `Store`, then pokes the
//! `Listener`. Swift never sees events: it reads plain snapshots (tabs, a
//! timeline, a thread, agents) whenever it is poked, and calls verbs
//! (open, send, 👍) that return at once.

// Nested `if`s read better than let-chains here, as in the terminal app.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use buzz_vim_core::checklist::{self, Checklists};
use buzz_vim_core::links;
use buzz_vim_core::live::{self, Cmd, ConnState, Live};
use buzz_vim_core::mention;
use buzz_vim_core::model::{self, Applied, Store};
use buzz_vim_core::needs;
use buzz_vim_core::palette::{self, Role};
use buzz_vim_core::quick;
use buzz_vim_core::readstate::{self, ReadState};
use buzz_vim_core::relay::{BlobDescriptor, Relay};
use buzz_vim_core::sections::{self, Lanes, Section};
use buzz_vim_core::telemetry::Agents;
use buzz_vim_core::unread::{self, Next};
use nostr::nips::nip44;
use nostr::{Event, EventId, Keys, Tag};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedSender};

mod pairing;
mod push;
pub use pairing::{PairStep, Pairing};
pub use push::{PushChallenge, PushDelegation, PushNote, PushRelay};

uniffi::setup_scaffolding!();

/// reqwest and the websocket both link rustls with more than one crypto
/// backend, so rustls won't pick one itself: without this every TLS
/// connection panics inside its task and the app sits at "connecting".
/// The notification extension makes no `Client`, so push calls it too.
fn tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tls();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
});

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum BuzzError {
    #[error("{0}")]
    Failed(String),
    /// The push gateway no longer knows this installation: enroll again.
    #[error("{0}")]
    Gone(String),
}

impl From<anyhow::Error> for BuzzError {
    fn from(e: anyhow::Error) -> Self {
        BuzzError::Failed(e.to_string())
    }
}

/// Swift's side: called from a background thread whenever a snapshot may
/// have changed. Hop to the main thread and re-read.
#[uniffi::export(callback_interface)]
pub trait Listener: Send + Sync {
    fn changed(&self);
}

// ---------- what Swift reads ----------

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Online,
    Offline,
}

#[derive(uniffi::Record, Clone)]
pub struct ChannelTab {
    pub id: String,
    /// `#name` for channels, `@name` for DMs.
    pub label: String,
    pub unread: u32,
    /// Unread messages that mention us (every unread one, in a DM).
    pub mentions: u32,
    pub is_dm: bool,
    /// The section this tab came from; tabs reorder within their section.
    pub section: String,
}

/// Where a tapped `buzz://` message or channel link lands.
#[derive(uniffi::Record, Clone)]
pub struct Follow {
    pub channel: String,
    /// The thread to open, if any.
    pub root: Option<String>,
    /// The reply to open the thread on, or the message to show in the
    /// channel; None: the bottom.
    pub id: Option<String>,
    /// The section to show when the channel isn't a tab now; None when it
    /// is one, or is in no section.
    pub section: Option<String>,
}

#[derive(uniffi::Record, Clone)]
pub struct SectionInfo {
    pub id: String,
    pub name: String,
    pub channels: u32,
    pub chosen: bool,
    /// The channel on screen can be moved into this section.
    pub can_move_here: bool,
}

#[derive(uniffi::Enum, Clone)]
pub enum Segment {
    /// Markdown text (inline markup only: links, bold, italics, code spans).
    Text { text: String },
    /// A fenced code block.
    Code { lang: String, body: String },
}

#[derive(uniffi::Record, Clone)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct Media {
    pub url: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    /// The uploader's file name, when it gave one.
    pub name: Option<String>,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    Sent,
    Sending,
    /// The relay never confirmed it. Tap to send the same event again.
    Failed,
}

#[derive(uniffi::Record, Clone)]
pub struct Message {
    pub id: String,
    pub author: String,
    pub author_name: String,
    pub is_agent: bool,
    pub at: u64,
    pub segments: Vec<Segment>,
    /// Display names of the people this message mentions, for `@Name` chips.
    pub mention_names: Vec<String>,
    pub media: Vec<Media>,
    pub reactions: Vec<Reaction>,
    pub reply_count: u32,
    /// Who replied, newest first, at most three.
    pub repliers: Vec<Person>,
    /// When the last reply landed, 0 if unknown.
    pub last_reply_at: u64,
    /// Who wrote the last reply, empty until it has loaded.
    pub last_reply_by: String,
    /// Replies from others you haven't seen in the thread.
    pub new_replies: u32,
    pub edited: bool,
    pub deleted: bool,
    pub system: bool,
    pub state: SendState,
}

#[derive(uniffi::Record, Clone)]
pub struct Person {
    pub pubkey: String,
    pub name: String,
    /// Avatar URL, "" when none.
    pub picture: String,
    pub is_agent: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct Timeline {
    pub messages: Vec<Message>,
    pub loaded: bool,
    /// There is older history to page in.
    pub has_more: bool,
    /// Agents mid-turn here right now.
    pub working: Vec<String>,
    /// The message with the newest activity ("" if none): in a channel the
    /// one whose own time or latest reply is newest, in a thread the last.
    pub latest: String,
}

/// Where the bell lands: a top-level message, or a reply (`root` set)
/// whose thread opens with it at the top.
#[derive(uniffi::Record, Clone)]
pub struct Jump {
    pub root: Option<String>,
    pub id: String,
}

#[derive(uniffi::Record, Clone)]
pub struct Agent {
    pub pubkey: String,
    pub name: String,
    pub busy: bool,
    /// `#channel` it is working in, if busy anywhere.
    pub place: Option<String>,
    pub secs: Option<u64>,
    /// Last tool calls, newest last.
    pub recent: Vec<String>,
    pub thought: String,
    pub last_turn: Option<String>,
    /// Tokens in its context in the open channel, e.g. "142k".
    pub context: Option<String>,
}

#[derive(uniffi::Record, Clone)]
pub struct Attachment {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub dim: Option<String>,
}

impl From<BlobDescriptor> for Attachment {
    fn from(d: BlobDescriptor) -> Self {
        Attachment {
            url: d.url,
            sha256: d.sha256,
            size: d.size,
            mime: d.mime_type,
            dim: d.dim,
        }
    }
}

impl From<&Attachment> for BlobDescriptor {
    fn from(a: &Attachment) -> Self {
        BlobDescriptor {
            url: a.url.clone(),
            sha256: a.sha256.clone(),
            size: a.size,
            mime_type: a.mime.clone(),
            dim: a.dim.clone(),
            blurhash: None,
            thumb: None,
        }
    }
}

/// One color as 0xRRGGBB.
#[derive(uniffi::Record, Clone, Copy)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<[u8; 3]> for Rgb {
    fn from([r, g, b]: [u8; 3]) -> Self {
        Rgb { r, g, b }
    }
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum Tone {
    Surface,
    Text,
    Accent,
    Muted,
    Code,
}

/// Step `level` (0 canvas … 23 strongest) of a role's ramp.
#[uniffi::export]
pub fn tone(role: Tone, level: u8, light: bool) -> Rgb {
    let role = match role {
        Tone::Surface => Role::Surface,
        Tone::Text => Role::Text,
        Tone::Accent => Role::Accent,
        Tone::Muted => Role::Muted,
        Tone::Code => Role::Code,
    };
    palette::tone(role, level, light).into()
}

/// Step `level` in the stable hue of `key` (a pubkey or channel id).
#[uniffi::export]
pub fn ink(key: String, level: u8, light: bool) -> Rgb {
    palette::ink(palette::hue(&key), level, light).into()
}

/// The pubkey (hex) for an nsec or hex secret, or why it isn't one.
#[uniffi::export]
pub fn pubkey_for(secret: String) -> Result<String, BuzzError> {
    Keys::parse(secret.trim())
        .map(|k| k.public_key().to_hex())
        .map_err(|e| BuzzError::Failed(format!("that isn't a private key: {e}")))
}

/// A fresh id for a read-state slot or client, to save and pass back in.
#[uniffi::export]
pub fn new_read_id() -> String {
    readstate::new_id()
}

// ---------- operator loop ----------

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Open,
    Done,
}

/// One evidence entry; `link` when it can open (a message, a URL).
#[derive(uniffi::Record, Clone)]
pub struct Evidence {
    pub text: String,
    pub link: Option<String>,
}

#[derive(uniffi::Record, Clone)]
pub struct ChecklistItem {
    pub id: String,
    pub text: String,
    /// Markdown, any length.
    pub details: String,
    pub status: ItemStatus,
    /// Needs lucian to release it.
    pub release: bool,
    /// "first" / "last" for the two standing items.
    pub standing: Option<String>,
    pub evidence: Vec<Evidence>,
    /// Done with no evidence: the spec requires some.
    pub missing_evidence: bool,
    pub creator: String,
    pub updated_at: u64,
}

/// A channel's checklist, read-only (agents write it with `buzz checklist`).
#[derive(uniffi::Record, Clone)]
pub struct Checklist {
    pub title: String,
    /// "ideating" / "running", None without a header.
    pub mode: Option<String>,
    /// Branch, worktree and work log, joined.
    pub resume: String,
    pub items: Vec<ChecklistItem>,
}

/// The status line for a channel in the operator loop.
#[derive(uniffi::Record, Clone)]
pub struct LoopStatus {
    pub mode: Option<String>,
    pub running: bool,
    /// The watcher's state ("waiting on lucian"), if it has said.
    pub watch: Option<String>,
    /// Stalled or waiting on lucian: worth a look.
    pub needs_you: bool,
    pub since: Option<u64>,
    /// `buzz://` link to the watcher's ping.
    pub ping_link: Option<String>,
    pub open: u32,
    pub done: u32,
    pub crashes_today: u32,
    /// Two or more lists claim the channel and no list shows until the
    /// owner picks a driver: "contested: name, name". None otherwise.
    pub contested: Option<String>,
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum NeedSection {
    /// Waits on the operator.
    Answer,
    /// Bots mentioned him; nothing waits.
    Fyi,
}

/// A row on tab 0, Needs you (see `buzz_vim_core::needs`).
#[derive(uniffi::Record, Clone)]
pub struct NeedRow {
    /// Stable while the row stands for the same thing: notifications go by it.
    pub key: String,
    pub section: NeedSection,
    /// "waiting on lucian", "stalled", "contested", "mention",
    /// "dm", "bot".
    pub kind: String,
    /// The watcher flagged it: the only rows that notify (push and local).
    pub watcher: bool,
    /// A watcher row with its ping out: the push lease wakes the phone for
    /// that message, so a live lease covers it. Other watcher rows notify
    /// locally even then.
    pub pushed: bool,
    pub channel: String,
    /// "#name", or "@name" for a DM.
    pub channel_label: String,
    /// Pubkey of whoever asks, "" for the watcher alone.
    pub who: String,
    pub who_name: String,
    /// One line.
    pub text: String,
    pub at: u64,
    /// Opens the row (`follow`): its message in its thread, else the
    /// thread, else the channel.
    pub link: String,
    /// The thread a reply goes into (`send`'s root); None: the channel.
    pub thread: Option<String>,
}

/// The Q box as it opens: where the question goes, and what a second tap
/// adds.
#[derive(uniffi::Record, Clone)]
pub struct QuickBox {
    /// #quick's id.
    pub channel: String,
    /// "#quick".
    pub label: String,
    /// The agent asked, by name ("taskbot").
    pub agent: String,
    /// The channel you're in as the second tap adds it ("#buzz-vim"), with
    /// its latest messages; None when there's nothing to add (no channel
    /// open, or you're in #quick).
    pub here: Option<String>,
}

// ---------- the client ----------

enum Msg {
    Channels(anyhow::Result<Vec<Event>>),
    Lanes(anyhow::Result<Vec<Event>>),
    ReadState(anyhow::Result<Vec<Event>>),
    Window {
        channel: String,
        events: anyhow::Result<Vec<Event>>,
        older: bool,
    },
    Events(anyhow::Result<Vec<Event>>),
    Archived(anyhow::Result<Vec<String>>),
    Unread {
        channel: String,
        events: anyhow::Result<Vec<Event>>,
    },
    Sent {
        id: String,
        result: anyhow::Result<()>,
    },
    Moved(anyhow::Result<(Event, String)>),
    Ordered(anyhow::Result<Event>),
    /// The relay's answer to archiving a channel.
    ChannelArchived {
        channel: String,
        result: anyhow::Result<()>,
    },
    Notice(String),
    /// A channel's checklist records, and its messages since midnight.
    Checklist {
        channel: String,
        records: anyhow::Result<Vec<Event>>,
        today: anyhow::Result<Vec<Event>>,
    },
}

struct Engine {
    relay: Relay,
    live: UnboundedSender<Cmd>,
    tx: UnboundedSender<Msg>,
    store: Store,
    lanes: Lanes,
    sections: Vec<Section>,
    chosen: Vec<String>,
    tabs: Vec<String>,
    /// tab -> the section it came from ("" for the all-channels fallback).
    tab_section: HashMap<String, String>,
    channels_loaded: bool,
    sections_loaded: bool,
    /// Tab orders set before the synced one loaded (the phone's old local
    /// order at launch): folded in under it once it has, then written.
    order_pending: HashMap<String, Vec<String>>,
    agents: Agents,
    read: ReadState,
    conn: Conn,
    /// The channel on screen, if any: its live messages count as read.
    current: Option<String>,
    /// False while the app is in the background: nothing counts as seen.
    visible: bool,
    cursors: HashMap<String, Option<(u64, String)>>,
    requested_profiles: HashSet<String>,
    opened: HashSet<String>,
    subscribed: HashSet<String>,
    loading_older: HashSet<String>,
    unread_checked: HashSet<String>,
    started: u64,
    /// channel -> the bell's last place there (see [`unread::bell`]).
    bell: HashMap<String, (u64, Next)>,
    /// Identities the relay has archived: left out of @ completion.
    archived: HashSet<String>,
    /// Our sends the relay hasn't confirmed: id -> event.
    sending: HashMap<String, Event>,
    failed: HashMap<String, Event>,
    notices: Vec<String>,
    checklists: Checklists,
    /// The channel whose checklist records are tailed live.
    checklist_for: Option<String>,
}

fn now() -> u64 {
    nostr::Timestamp::now().as_secs()
}

#[derive(uniffi::Object)]
pub struct Client {
    engine: Arc<Mutex<Engine>>,
    stop: Arc<tokio::sync::Notify>,
}

/// Logging out drops the client: its loop ends, which drops the engine,
/// its keys and the live socket.
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

#[uniffi::export]
impl Client {
    /// Log in and start syncing. `chosen` = section ids picked as tabs;
    /// the read ids come from `new_read_id` the first time, then from
    /// `read_slot`/`read_client`, so Desktop sees one device, not many.
    /// `auth_tag` is a NIP-OA tag (JSON) for agent keys; people pass none.
    #[uniffi::constructor]
    pub fn new(
        relay_url: String,
        secret: String,
        chosen: Vec<String>,
        read_slot: String,
        read_client: String,
        auth_tag: Option<String>,
        listener: Box<dyn Listener>,
    ) -> Result<Arc<Self>, BuzzError> {
        let keys = Keys::parse(secret.trim())
            .map_err(|e| BuzzError::Failed(format!("that isn't a private key: {e}")))?;
        // A managed agent's key also needs its NIP-OA tag (simulator runs).
        let auth = match auth_tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(json) => {
                buzz_sdk::nip_oa::verify_auth_tag(json, &keys.public_key())
                    .map_err(|e| BuzzError::Failed(format!("bad auth tag: {e}")))?;
                Some(
                    buzz_sdk::nip_oa::parse_auth_tag(json)
                        .map_err(|e| BuzzError::Failed(format!("bad auth tag: {e}")))?,
                )
            }
            None => None,
        };
        let _guard = RT.enter();
        let relay = Relay::new(&relay_url, keys.clone(), auth.clone())?;
        let (live_tx, live_rx) = mpsc::unbounded_channel::<Live>();
        let live_cmd = live::spawn(relay.ws_url.clone(), keys.clone(), auth, live_tx);
        let (tx, rx) = mpsc::unbounded_channel::<Msg>();
        let me = keys.public_key().to_hex();
        let engine = Engine {
            relay,
            live: live_cmd,
            tx,
            store: Store::new(me.clone()),
            lanes: Lanes::default(),
            sections: vec![],
            chosen,
            tabs: vec![],
            tab_section: HashMap::new(),
            channels_loaded: false,
            sections_loaded: false,
            order_pending: HashMap::new(),
            agents: Agents::default(),
            read: ReadState::new(read_slot, read_client),
            conn: Conn::Connecting,
            current: None,
            visible: true,
            cursors: HashMap::new(),
            requested_profiles: HashSet::new(),
            opened: HashSet::new(),
            subscribed: HashSet::new(),
            loading_older: HashSet::new(),
            unread_checked: HashSet::new(),
            started: now(),
            bell: HashMap::new(),
            archived: HashSet::new(),
            sending: HashMap::new(),
            failed: HashMap::new(),
            notices: vec![],
            checklists: Checklists::default(),
            checklist_for: None,
        };
        let engine = Arc::new(Mutex::new(engine));
        {
            let mut e = engine.lock().unwrap();
            e.start();
            e.want_profiles([me]);
        }
        let stop = Arc::new(tokio::sync::Notify::new());
        RT.spawn(run(engine.clone(), rx, live_rx, listener, stop.clone()));
        Ok(Arc::new(Client { engine, stop }))
    }

    /// A link to a file on our relay: tapping it downloads with our auth
    /// (`save_file`) instead of opening a browser that has none.
    pub fn is_file(&self, url: String) -> bool {
        self.engine.lock().unwrap().relay.is_file(&url)
    }

    pub fn me(&self) -> String {
        self.engine.lock().unwrap().store.me.clone()
    }

    /// The name of the channel on screen, or "" when there is none.
    pub fn current_name(&self) -> String {
        let e = self.engine.lock().unwrap();
        e.current
            .as_ref()
            .and_then(|c| e.store.channels.get(c))
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    pub fn my_name(&self) -> String {
        let e = self.engine.lock().unwrap();
        e.store.name_of(&e.store.me)
    }

    pub fn conn(&self) -> Conn {
        self.engine.lock().unwrap().conn
    }

    /// True once channels and sections have both arrived.
    pub fn ready(&self) -> bool {
        let e = self.engine.lock().unwrap();
        e.channels_loaded && e.sections_loaded
    }

    pub fn tabs(&self) -> Vec<ChannelTab> {
        let e = self.engine.lock().unwrap();
        e.tabs
            .iter()
            .map(|id| e.tab(id, e.tab_section.get(id).cloned().unwrap_or_default()))
            .collect()
    }

    /// Any section's channels as tabs, whether or not it is the one showing.
    pub fn section_tabs(&self, section: String) -> Vec<ChannelTab> {
        let e = self.engine.lock().unwrap();
        let Some(s) = e.sections.iter().find(|s| s.id == section) else {
            return vec![];
        };
        s.channels
            .iter()
            .filter(|c| e.store.channels.get(*c).is_none_or(|ch| !ch.archived))
            .map(|c| e.tab(c, section.clone()))
            .collect()
    }

    /// A tapped link that we show ourselves (a channel, a message in one),
    /// or None for the system to open.
    pub fn follow(&self, url: String) -> Option<Follow> {
        let links::Target::Message {
            channel,
            id,
            thread,
        } = links::target(&url)
        else {
            return None;
        };
        let e = self.engine.lock().unwrap();
        let l = links::land(e.store.channels.get(&channel), id, thread);
        let section = if e.tabs.contains(&channel) {
            None
        } else {
            e.sections
                .iter()
                .find(|s| s.channels.contains(&channel))
                .map(|s| s.id.clone())
        };
        Some(match l.thread {
            Some((root, pick)) => Follow {
                channel,
                root: Some(root),
                id: pick,
                section,
            },
            None => Follow {
                channel,
                root: None,
                id: l.selected,
                section,
            },
        })
    }

    pub fn sections(&self) -> Vec<SectionInfo> {
        let e = self.engine.lock().unwrap();
        let first = e.chosen.is_empty();
        let cur = e
            .current
            .as_ref()
            .filter(|c| e.store.channels.get(*c).is_some_and(|ch| !ch.is_dm));
        e.sections
            .iter()
            .enumerate()
            .map(|(i, s)| SectionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                channels: s.channels.len() as u32,
                chosen: e.chosen.contains(&s.id) || (first && i == 0),
                can_move_here: cur.is_some_and(|c| {
                    !matches!(s.id.as_str(), "★" | "dms") && !s.channels.contains(c)
                }),
            })
            .collect()
    }

    /// Pick which sections become tabs. Save it and pass it back next launch.
    pub fn set_chosen(&self, ids: Vec<String>) {
        let mut e = self.engine.lock().unwrap();
        e.chosen = ids;
        e.rebuild_tabs();
    }

    /// A section's tab order, as dragged. Channels it doesn't list (new
    /// ones) follow by name. Synced: the terminal app shows it too.
    pub fn set_order(&self, section: String, ids: Vec<String>) {
        let mut e = self.engine.lock().unwrap();
        if !e.sections_loaded {
            e.order_pending.insert(section, ids);
            return;
        }
        e.lanes.order.insert(section, ids);
        e.refresh_sections();
        e.publish_order();
    }

    /// Archive a channel for everyone. Its tab goes now; it comes back
    /// with a notice if the relay says no.
    pub fn archive_channel(&self, channel: String) -> Result<(), BuzzError> {
        let mut e = self.engine.lock().unwrap();
        let uuid = uuid::Uuid::parse_str(&channel)
            .map_err(|_| BuzzError::Failed("bad channel id".into()))?;
        let builder =
            buzz_sdk::build_archive(uuid).map_err(|e| BuzzError::Failed(e.to_string()))?;
        let ev = e.relay.sign(builder)?;
        e.store.channel_mut(&channel).archived = true;
        e.rebuild_tabs();
        let r = e.relay.clone();
        e.spawn(async move {
            let result = r.publish(&ev).await;
            Msg::ChannelArchived { channel, result }
        });
        Ok(())
    }

    /// Move the channel on screen into `section` ("channels" = out of its
    /// section), in Desktop's sidebar record so Desktop sees it too.
    pub fn move_here(&self, section: String) {
        let e = self.engine.lock().unwrap();
        let Some(ch) = e.current.clone() else { return };
        let label = format!(
            "moved #{} to {}",
            e.store.channels.get(&ch).map_or("?", |c| c.name.as_str()),
            e.sections
                .iter()
                .find(|s| s.id == section)
                .map_or(section.as_str(), |s| s.name.as_str())
        );
        let target = (section != "channels").then_some(section);
        let r = e.relay.clone();
        e.spawn(async move {
            Msg::Moved(
                sections::publish_move(&r, &ch, target.as_deref())
                    .await
                    .map(|ev| (ev, label)),
            )
        });
    }

    /// Fetch channels and sections again: new channels appear, archived
    /// ones drop. Live updates do this too; this covers what they miss.
    pub fn refresh(&self) {
        let e = self.engine.lock().unwrap();
        e.load_channels();
        e.load_lanes();
    }

    /// The channel now on screen (or none): loads it and marks it read.
    pub fn open_channel(&self, channel: Option<String>) {
        let mut e = self.engine.lock().unwrap();
        e.current = channel.clone();
        if let Some(id) = &channel {
            if e.checklist_for.as_ref() != Some(id) {
                e.load_checklist(id);
            }
        }
        if let Some(id) = channel {
            e.mark_read(&id);
            e.open(&id);
        }
    }

    /// The app went to the background (false) or came back (true). Back on
    /// screen, the open channel counts as read again.
    pub fn set_visible(&self, visible: bool) {
        let mut e = self.engine.lock().unwrap();
        e.visible = visible;
        if let (true, Some(id)) = (visible, e.current.clone()) {
            e.mark_read(&id);
        }
        if visible {
            e.load_channels();
            e.load_lanes();
        }
    }

    /// A channel's checklist: header and items in list order.
    pub fn checklist(&self, channel: String) -> Checklist {
        let e = self.engine.lock().unwrap();
        let h = e.checklists.header(&channel);
        let items = e
            .checklists
            .items(&channel)
            .into_iter()
            .map(|i| ChecklistItem {
                missing_evidence: i.missing_evidence(),
                status: match i.status {
                    // No parking (lucian 2026-10-20): a legacy parked item is open.
                    checklist::Status::Open | checklist::Status::Parked => ItemStatus::Open,
                    checklist::Status::Done => ItemStatus::Done,
                },
                evidence: i
                    .evidence
                    .iter()
                    .map(|t| Evidence {
                        link: checklist::evidence_link(&channel, t),
                        text: t.clone(),
                    })
                    .collect(),
                creator: e.store.name_of(&i.creator),
                id: i.id,
                text: i.text,
                details: i.details,
                release: i.release,
                standing: i.standing,
                updated_at: i.updated_at,
            })
            .collect();
        Checklist {
            title: h.as_ref().map(|h| h.title.clone()).unwrap_or_default(),
            mode: h.as_ref().map(|h| h.mode.label().to_string()),
            resume: h
                .map(|h| {
                    [h.resume.branch, h.resume.worktree, h.resume.worklog]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .unwrap_or_default(),
            items,
        }
    }

    /// The channel's status line, or None when it has no list, no watcher
    /// and no crashes today.
    pub fn loop_status(&self, channel: String) -> Option<LoopStatus> {
        let e = self.engine.lock().unwrap();
        let s = e
            .checklists
            .summary(&channel, checklist::local_midnight(now()));
        if s.is_empty() {
            return None;
        }
        Some(LoopStatus {
            running: s.mode == Some(checklist::Mode::Running),
            mode: s.mode.map(|m| m.label().to_string()),
            needs_you: s.watch.as_ref().is_some_and(|w| w.state.needs_you()),
            since: s.watch.as_ref().and_then(|w| w.since),
            watch: s.watch.map(|w| w.state.label()),
            ping_link: s.ping_link,
            open: s.open as u32,
            done: s.done as u32,
            crashes_today: s.crashes_today as u32,
            contested: (!s.contested.is_empty()).then(|| {
                let names: Vec<String> = s.contested.iter().map(|c| e.store.name_of(c)).collect();
                format!("contested: {}", names.join(", "))
            }),
        })
    }

    /// Tab 0: what needs an answer (oldest first), then FYI (newest first).
    pub fn needs_you(&self) -> Vec<NeedRow> {
        let e = self.engine.lock().unwrap();
        e.needs_rows()
            .into_iter()
            .map(|r| NeedRow {
                section: match r.section {
                    needs::Section::Answer => NeedSection::Answer,
                    needs::Section::Fyi => NeedSection::Fyi,
                },
                kind: r.label(),
                watcher: matches!(r.reason, needs::Reason::Watch(_)),
                pushed: matches!(r.reason, needs::Reason::Watch(_)) && r.message.is_some(),
                channel_label: e.label(&r.channel),
                who_name: if let needs::Reason::Contested(c) = &r.reason {
                    let names: Vec<String> = c.iter().map(|c| e.store.name_of(c)).collect();
                    names.join(", ")
                } else if r.who.is_empty() {
                    "watcher".into()
                } else {
                    e.store.name_of(&r.who)
                },
                link: r.link(),
                key: r.key,
                channel: r.channel,
                who: r.who,
                text: r.text,
                at: r.at,
                thread: r.thread,
            })
            .collect()
    }

    /// How many rows need an answer: tab 0's badge, the status line's ⚑ N.
    pub fn needs_count(&self) -> u32 {
        needs::answer_count(&self.engine.lock().unwrap().needs_rows()) as u32
    }

    /// Ids of our messages the relay hasn't confirmed yet (sending or failed).
    pub fn unconfirmed(&self) -> Vec<String> {
        let e = self.engine.lock().unwrap();
        e.sending.keys().chain(e.failed.keys()).cloned().collect()
    }

    pub fn load_older(&self, channel: String) {
        let mut e = self.engine.lock().unwrap();
        if let Some(Some(cursor)) = e.cursors.get(&channel).cloned() {
            if e.loading_older.insert(channel.clone()) {
                e.window(&channel, Some(cursor));
            }
        }
    }

    pub fn timeline(&self, channel: String) -> Timeline {
        let e = self.engine.lock().unwrap();
        let Some(ch) = e.store.channels.get(&channel) else {
            return Timeline {
                messages: vec![],
                loaded: false,
                has_more: false,
                working: vec![],
                latest: String::new(),
            };
        };
        Timeline {
            messages: ch
                .timeline
                .iter()
                .filter_map(|id| ch.messages.get(id))
                .filter(|m| !m.deleted)
                .map(|m| e.view(ch, m))
                .collect(),
            loaded: ch.loaded,
            has_more: matches!(e.cursors.get(&channel), Some(Some(_))) && !ch.exhausted,
            working: e.agents.working(&e.store, &channel, None),
            latest: unread::latest(ch).unwrap_or_default(),
        }
    }

    /// A thread on screen: loads it and marks its replies seen.
    pub fn open_thread(&self, channel: String, root: String) {
        let mut e = self.engine.lock().unwrap();
        e.read.mark(&unread::thread_key(&root), now());
        e.thread(&channel, &root);
    }

    /// Leaving a thread: what arrived while it was open is seen too.
    pub fn close_thread(&self, root: String) {
        let mut e = self.engine.lock().unwrap();
        e.read.mark(&unread::thread_key(&root), now());
    }

    /// The bell in `channel`: the newest activity there, read or not; a
    /// thread opens on its first new reply. The same place again until
    /// something newer arrives.
    pub fn bell(&self, channel: String) -> Option<Jump> {
        let mut e = self.engine.lock().unwrap();
        let ch = e.store.channels.get(&channel)?;
        let b = unread::bell(ch, &e.store.me, &e.read, now(), e.bell.get(&channel))?;
        e.bell.insert(channel, b.clone());
        Some(match b.1 {
            Next::Message(id) => Jump { root: None, id },
            Next::Reply { root, id } => Jump {
                root: Some(root),
                id,
            },
        })
    }

    /// The root first, then its replies, oldest first.
    pub fn thread(&self, channel: String, root: String) -> Timeline {
        let e = self.engine.lock().unwrap();
        let Some(ch) = e.store.channels.get(&channel) else {
            return Timeline {
                messages: vec![],
                loaded: false,
                has_more: false,
                working: vec![],
                latest: String::new(),
            };
        };
        let mut messages: Vec<Message> = ch
            .messages
            .get(&root)
            .map(|m| e.view(ch, m))
            .into_iter()
            .collect();
        if let Some(ids) = ch.replies.get(&root) {
            messages.extend(
                ids.iter()
                    .filter_map(|id| ch.messages.get(id))
                    .filter(|m| !m.deleted)
                    .map(|m| e.view(ch, m)),
            );
        }
        Timeline {
            loaded: !messages.is_empty(),
            latest: messages.last().map(|m| m.id.clone()).unwrap_or_default(),
            messages,
            has_more: false,
            working: e.agents.working(&e.store, &channel, Some(&root)),
        }
    }

    /// Post `text` (and uploaded attachments) to a channel, or into the
    /// thread under `root`. Shows at once; the relay confirms later.
    /// Returns the message id ("" when there was nothing to send).
    pub fn send(
        &self,
        channel: String,
        root: Option<String>,
        text: String,
        attachments: Vec<Attachment>,
        to: Vec<String>,
    ) -> Result<String, BuzzError> {
        let mut e = self.engine.lock().unwrap();
        e.send(channel, root, text, &attachments, &to)
    }

    /// Open the Q box, as `Q` does in the terminal: fails with why when
    /// there's no #quick (buzz-vim never makes it). `here` is the channel
    /// on screen; #quick's members' names load for the mention.
    pub fn open_quick(&self, here: Option<String>) -> Result<QuickBox, BuzzError> {
        let mut e = self.engine.lock().unwrap();
        if let Some(why) = quick::unavailable(&e.store, quick::CHANNEL, e.channels_loaded) {
            return Err(BuzzError::Failed(why));
        }
        let q = quick::find_channel(&e.store, quick::CHANNEL)
            .ok_or_else(|| BuzzError::Failed(format!("no #{} channel", quick::CHANNEL)))?;
        let (id, members) = (q.id.clone(), q.members.clone());
        let here = quick::can_add(&e.store, quick::CHANNEL, here.as_deref())
            .ok()
            .and(here)
            .map(|h| e.label(&h));
        e.want_profiles(members);
        Ok(QuickBox {
            label: e.label(&id),
            channel: id,
            agent: quick::AGENT.to_string(),
            here,
        })
    }

    /// Send a Q: a new top-level message in #quick, `[Q]` first, that
    /// @mentions taskbot. `here` (with its open `thread`) adds that channel
    /// and its latest messages; None sends the question alone. Returns the
    /// message id: the answer comes in its thread.
    pub fn ask_quick(
        &self,
        text: String,
        here: Option<String>,
        thread: Option<String>,
    ) -> Result<String, BuzzError> {
        let mut e = self.engine.lock().unwrap();
        e.ask_quick(&text, here.as_deref(), thread.as_deref())
    }

    /// Agents `text` @mentions, by pubkey: they become the sticky audience.
    pub fn mentioned_agents(&self, channel: String, text: String) -> Vec<String> {
        let e = self.engine.lock().unwrap();
        e.mentions_in(&channel, &text)
            .into_iter()
            .filter(|pk| e.is_agent(pk))
            .collect()
    }

    /// Agents a thread's root addressed: a reply goes to them by default.
    pub fn root_agents(&self, channel: String, root: String) -> Vec<String> {
        let e = self.engine.lock().unwrap();
        e.store
            .channels
            .get(&channel)
            .and_then(|c| c.messages.get(&root))
            .map(|m| {
                m.mentions
                    .iter()
                    .filter(|pk| e.is_agent(pk))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn person(&self, pubkey: String) -> Person {
        self.engine.lock().unwrap().person(&pubkey)
    }

    /// Send a failed message's event again, unchanged.
    pub fn retry(&self, id: String) {
        let mut e = self.engine.lock().unwrap();
        if let Some(ev) = e.failed.remove(&id) {
            let channel = model::tag(&ev, "h").unwrap_or_default().to_string();
            e.publish_message(ev, channel, vec![]);
        }
    }

    /// Drop a failed message and hand its text back for the compose box.
    pub fn discard(&self, id: String) -> Option<String> {
        let mut e = self.engine.lock().unwrap();
        let ev = e.failed.remove(&id)?;
        e.store.remove_message(&id);
        Some(ev.content.clone())
    }

    /// 👍 a message, or take your 👍 back.
    pub fn thumbs_up(&self, target: String) {
        let mut e = self.engine.lock().unwrap();
        e.thumbs_up(&target);
    }

    /// Your online agents, busiest first.
    pub fn agents(&self) -> Vec<Agent> {
        let e = self.engine.lock().unwrap();
        let ctx = |pk: &str| {
            let a = e.agents.by_pubkey.get(pk)?;
            let c = a.context.get(e.current.as_deref()?)?;
            Some(buzz_vim_core::telemetry::tokens(c.used))
        };
        e.agents
            .rows(&e.store, None)
            .into_iter()
            .map(|r| Agent {
                context: ctx(&r.pubkey),
                place: r
                    .channel
                    .as_deref()
                    .or(r.elsewhere.as_deref())
                    .filter(|c| !c.is_empty())
                    .map(|c| e.label(c)),
                pubkey: r.pubkey,
                name: r.name,
                busy: r.busy || r.elsewhere.is_some(),
                secs: r.secs,
                recent: r.recent,
                thought: r.thought,
                last_turn: r.last_turn,
            })
            .collect()
    }

    /// Reset `agent`'s session in `channel` (the thread under `root` for
    /// thread-scoped sessions): the owner `!rotate` buzz-acp acts on, from
    /// you, p-tagging the agent.
    pub fn reset_agent(
        &self,
        channel: String,
        root: Option<String>,
        agent: String,
    ) -> Result<(), BuzzError> {
        let mut e = self.engine.lock().unwrap();
        // Built here, not through send: in a DM that p-tags every member,
        // and each agent tagged would rotate.
        let thread = root
            .as_deref()
            .map(EventId::from_hex)
            .transpose()
            .map_err(|e| BuzzError::Failed(e.to_string()))?
            .map(|r| buzz_sdk::ThreadRef {
                root_event_id: r,
                parent_event_id: r,
            });
        let uuid = uuid::Uuid::parse_str(&channel)
            .map_err(|_| BuzzError::Failed("bad channel id".into()))?;
        let builder = buzz_sdk::build_message(
            uuid,
            "!rotate",
            thread.as_ref(),
            &[agent.as_str()],
            false,
            &[],
            &[],
        )
        .map_err(|e| BuzzError::Failed(e.to_string()))?;
        let ev = e.relay.sign(builder)?;
        e.store.apply(&ev);
        e.publish_message(ev, channel.clone(), vec![]);
        e.agents.rotated(&agent, &channel);
        Ok(())
    }

    /// Agents' context counts as JSON, for the app to keep across launches.
    pub fn saved_context(&self) -> String {
        self.engine
            .lock()
            .unwrap()
            .agents
            .saved_context()
            .to_string()
    }

    pub fn restore_context(&self, json: String) {
        if let Ok(v) = serde_json::from_str(&json) {
            self.engine.lock().unwrap().agents.restore_context(&v);
        }
    }

    /// Agents mid-turn anywhere, by name.
    pub fn working_anywhere(&self) -> Vec<String> {
        self.agents()
            .into_iter()
            .filter(|a| a.busy)
            .map(|a| a.name)
            .collect()
    }

    /// Who an @ offers here, best guess first: the thread's people, the
    /// channel's speakers and members, then our agents (online first).
    pub fn people(&self, channel: String, root: Option<String>) -> Vec<Person> {
        let e = self.engine.lock().unwrap();
        mention::candidates(&e.store, &e.agents, &channel, root.as_deref(), &e.archived)
            .into_iter()
            .map(|(k, name)| Person {
                name,
                ..e.person(&k)
            })
            .collect()
    }

    /// Short messages for the person: failures, mostly. Taking empties it.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.engine.lock().unwrap().notices)
    }

    /// Publish our read positions now (the app is going to the background).
    pub fn flush(&self) {
        let (relay, ev) = {
            let mut e = self.engine.lock().unwrap();
            let relay = e.relay.clone();
            let ev = e.read.take_publish(&relay.keys, true);
            (relay, ev)
        };
        if let Some(ev) = ev {
            RT.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(3), relay.publish(&ev)).await;
            });
        }
    }

    /// The app came back: reload what may have moved while iOS froze us.
    pub fn resume(&self) {
        let mut e = self.engine.lock().unwrap();
        if let Some(c) = e.current.clone() {
            e.load_checklist(&c);
        }
        e.load_channels();
        let open: Vec<String> = e.opened.iter().cloned().collect();
        for id in open {
            e.window(&id, None);
        }
        e.heartbeat();
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Upload to the relay's Blossom store. Nothing is sent until `send`.
    pub async fn upload(&self, bytes: Vec<u8>, mime: String) -> Result<Attachment, BuzzError> {
        let relay = self.engine.lock().unwrap().relay.clone();
        Ok(relay.upload(bytes, &mime).await?.into())
    }

    /// Image bytes, with our Blossom auth when the image is on our relay.
    pub async fn image(&self, url: String) -> Result<Vec<u8>, BuzzError> {
        let relay = self.engine.lock().unwrap().relay.clone();
        if !relay.is_ours(&url) {
            return Err(BuzzError::Failed("external image".into()));
        }
        Ok(relay.download(&url).await?)
    }

    /// Download a file on our relay into `dir` under the name it was sent
    /// with, for the share sheet. Returns its path.
    pub async fn save_file(&self, url: String, dir: String) -> Result<String, BuzzError> {
        let (relay, name) = {
            let e = self.engine.lock().unwrap();
            (e.relay.clone(), e.store.media_name(&url))
        };
        if !relay.is_file(&url) {
            return Err(BuzzError::Failed("not a file on this relay".into()));
        }
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).map_err(|e| BuzzError::Failed(e.to_string()))?;
        let path = links::free_path(dir, &links::file_name(&url, name.as_deref()))
            .map_err(|e| BuzzError::Failed(e.to_string()))?;
        relay.download_to(&url, &path).await?;
        Ok(path.to_string_lossy().into_owned())
    }
}

async fn run(
    engine: Arc<Mutex<Engine>>,
    mut rx: mpsc::UnboundedReceiver<Msg>,
    mut live_rx: mpsc::UnboundedReceiver<Live>,
    listener: Box<dyn Listener>,
    stop: Arc<tokio::sync::Notify>,
) {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    let mut channels_poll = tokio::time::interval(Duration::from_secs(300));
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    channels_poll.tick().await;
    let mut was_online = false;
    loop {
        let mut changed = true;
        tokio::select! {
            _ = stop.notified() => break,
            Some(msg) = rx.recv() => {
                let mut e = engine.lock().unwrap();
                e.handle(msg);
                while let Ok(m) = rx.try_recv() {
                    e.handle(m);
                }
            }
            Some(l) = live_rx.recv() => {
                let mut e = engine.lock().unwrap();
                e.on_live(l, &mut was_online);
                while let Ok(l) = live_rx.try_recv() {
                    e.on_live(l, &mut was_online);
                }
            }
            _ = heartbeat.tick() => {
                engine.lock().unwrap().heartbeat();
                changed = false;
            }
            _ = channels_poll.tick() => {
                engine.lock().unwrap().load_channels();
                changed = false;
            }
            // Agent rows count seconds, and read positions settle.
            _ = tick.tick() => {
                let e = engine.lock().unwrap();
                changed = e.agents.by_pubkey.values().any(|a| a.is_busy());
            }
            else => break,
        }
        {
            let mut e = engine.lock().unwrap();
            let tabs = e.tabs.clone();
            e.check_unread(&tabs);
            e.flush_read();
        }
        if changed {
            listener.changed();
        }
    }
}

impl Engine {
    fn me(&self) -> String {
        self.store.me.clone()
    }

    fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        RT.spawn(async move {
            let _ = tx.send(f.await);
        });
    }

    fn sub(&self, id: &str, filters: Vec<Value>) {
        let _ = self.live.send(Cmd::Sub {
            id: id.to_string(),
            filters,
        });
    }

    fn tab(&self, id: &str, section: String) -> ChannelTab {
        let ch = self.store.channels.get(id);
        ChannelTab {
            id: id.to_string(),
            label: self.label(id),
            unread: ch.map_or(0, |c| c.unread.len() as u32),
            mentions: ch.map_or(0, |c| {
                (if c.is_dm {
                    c.unread.len()
                } else {
                    c.mentioned.len()
                }) as u32
            }),
            is_dm: ch.is_some_and(|c| c.is_dm),
            section,
        }
    }

    fn label(&self, id: &str) -> String {
        self.store.label(id)
    }

    /// Startup: channels, sidebar lanes, read state and the global live
    /// subscriptions, as the terminal app does.
    fn start(&mut self) {
        let me = self.me();
        self.load_channels();
        self.load_lanes();
        let now = now();
        self.sub(
            "obs",
            vec![json!({"kinds": [24200], "#p": [me], "since": now - 300, "limit": 1000})],
        );
        self.sub(
            "metrics",
            vec![json!({"kinds": [44200], "#p": [me], "limit": 30})],
        );
        self.sub("lanes", vec![json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_SECTIONS, sections::LANE_STARS, sections::LANE_ORDER], "limit": 0})]);
        self.sub(
            "readstate",
            vec![json!({"kinds": [30078], "authors": [me], "#t": ["read-state"], "since": now, "limit": 0})],
        );
        // Channel renames and archiving, and channels we're added to.
        self.sub(
            "channels",
            vec![json!({"kinds": [39000], "since": now, "limit": 0})],
        );
        self.sub(
            "joined",
            vec![json!({"kinds": [39002], "#p": [me], "since": now, "limit": 0})],
        );
        self.sub(
            "membership",
            vec![json!({"kinds": [44100, 44101], "#p": [me], "since": now, "limit": 50})],
        );
        let r = self.relay.clone();
        let who = me.clone();
        self.spawn(async move {
            Msg::ReadState(
                r.query(&[json!({"kinds": [30078], "authors": [who], "limit": 500})])
                    .await,
            )
        });
        let r = self.relay.clone();
        self.spawn(async move {
            Msg::Events(
                r.query(&[json!({"kinds": [30177], "authors": [me], "limit": 200})])
                    .await,
            )
        });
        let r = self.relay.clone();
        self.spawn(async move { Msg::Archived(r.archived().await) });
    }

    fn load_channels(&self) {
        let r = self.relay.clone();
        let me = self.me();
        self.spawn(async move {
            let res = async {
                let membership = r
                    .query(&[json!({"kinds": [39002], "#p": [me], "limit": 500})])
                    .await?;
                let ids: Vec<String> = membership
                    .iter()
                    .filter_map(|e| model::tag(e, "d").map(str::to_string))
                    .collect();
                let mut all = membership;
                for chunk in ids.chunks(200) {
                    all.extend(
                        r.query(&[json!({"kinds": [39000], "#d": chunk, "limit": chunk.len()})])
                            .await?,
                    );
                }
                Ok(all)
            }
            .await;
            Msg::Channels(res)
        });
    }

    fn load_lanes(&self) {
        let r = self.relay.clone();
        let me = self.me();
        self.spawn(async move {
            Msg::Lanes(
                r.query(&[
                    json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_SECTIONS], "limit": 1}),
                    json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_STARS], "limit": 1}),
                    json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_ORDER], "limit": 1}),
                ])
                .await,
            )
        });
    }

    fn heartbeat(&self) {
        if let Ok(b) = buzz_sdk::build_presence_update("online") {
            if let Ok(ev) = self.relay.sign(b) {
                let _ = self.live.send(Cmd::Publish(Box::new(ev)));
            }
        }
    }

    fn decrypt_for_me(&self, ev: &Event) -> Option<Value> {
        let plain = nip44::decrypt(self.relay.keys.secret_key(), &ev.pubkey, &ev.content).ok()?;
        serde_json::from_str(&plain).ok()
    }

    fn apply_lanes(&mut self, evs: &[Event]) -> bool {
        let mut changed = false;
        for ev in evs {
            if ev.pubkey != self.relay.keys.public_key() {
                continue;
            }
            let Some(doc) = self.decrypt_for_me(ev) else {
                continue;
            };
            match model::tag(ev, "d") {
                Some(sections::LANE_SECTIONS) => sections::parse_sections(&doc, &mut self.lanes),
                Some(sections::LANE_STARS) => sections::parse_stars(&doc, &mut self.lanes),
                Some(sections::LANE_ORDER) => {
                    sections::parse_order(&doc, ev.created_at.as_secs(), &mut self.lanes)
                }
                _ => continue,
            }
            changed = true;
        }
        changed
    }

    /// Write the whole tab order (see sections::LANE_ORDER).
    fn publish_order(&mut self) {
        let at = sections::next_order_at(&self.lanes);
        self.lanes.order_at = at;
        let order = self.lanes.order.clone();
        let r = self.relay.clone();
        self.spawn(async move { Msg::Ordered(sections::publish_order(&r, &order, at).await) });
    }

    fn refresh_sections(&mut self) {
        self.sections = sections::build(&self.lanes, &self.store);
        if self.sections_loaded && self.channels_loaded {
            let sections = &self.sections;
            self.chosen.retain(|c| sections.iter().any(|s| s.id == *c));
        }
        self.rebuild_tabs();
    }

    /// Tabs = the chosen sections' channels, in sidebar order; the first
    /// section when nothing is chosen; every channel when that is empty.
    fn rebuild_tabs(&mut self) {
        let mut tabs: Vec<String> = vec![];
        let mut tab_section = HashMap::new();
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
                    tab_section.insert(c.clone(), s.id.clone());
                }
            }
        }
        if tabs.is_empty() {
            let mut all: Vec<&model::Channel> = self
                .store
                .channels
                .values()
                .filter(|c| c.is_member && !c.archived && !c.name.is_empty())
                .collect();
            all.sort_by_key(|c| (c.is_dm, c.name.to_lowercase()));
            tabs = all.into_iter().map(|c| c.id.clone()).collect();
        }
        self.tabs = tabs;
        self.tab_section = tab_section;
        for t in self.tabs.clone() {
            self.subscribe_channel(&t);
        }
        let keep = self.tabs.clone();
        let drop: Vec<String> = self
            .subscribed
            .iter()
            .filter(|s| !keep.contains(s) && self.current.as_ref() != Some(*s))
            .cloned()
            .collect();
        for id in drop {
            self.subscribed.remove(&id);
            let _ = self.live.send(Cmd::Close {
                id: format!("ch:{id}"),
            });
        }
    }

    fn subscribe_channel(&mut self, id: &str) {
        if !self.subscribed.insert(id.to_string()) {
            return;
        }
        let since = now().min(self.started);
        self.sub(
            &format!("ch:{id}"),
            vec![json!({"kinds": model::LIVE_KINDS, "#h": [id], "since": since, "limit": 1000})],
        );
    }

    fn window(&self, channel: &str, older: Option<(u64, String)>) {
        let r = self.relay.clone();
        let ch = channel.to_string();
        let mut f = json!({"#h": [channel], "kinds": model::TIMELINE_KINDS, "limit": 60,
            "top_level": true, "include_summaries": true, "include_aux": true});
        let is_older = older.is_some();
        if let Some((until, id)) = older {
            f["until"] = json!(until);
            f["before_id"] = json!(id);
        }
        self.spawn(async move {
            Msg::Window {
                channel: ch,
                events: r.query(&[f]).await,
                older: is_older,
            }
        });
    }

    /// The channel's checklist records and today's crash notices, and a
    /// live tail of its records while it's on screen.
    fn load_checklist(&mut self, id: &str) {
        self.checklist_for = Some(id.to_string());
        self.sub(
            "checklist",
            vec![json!({"kinds": checklist::KINDS, "#h": [id], "since": now(), "limit": 0})],
        );
        let r = self.relay.clone();
        let channel = id.to_string();
        let today = checklist::local_midnight(now());
        self.spawn(async move {
            let records = r.query(&Checklists::filters(&channel)).await;
            let today = r
                .query(&[
                    json!({"kinds": model::UNREAD_KINDS, "#h": [channel], "since": today,
                    "limit": 500}),
                ])
                .await;
            Msg::Checklist {
                channel,
                records,
                today,
            }
        });
    }

    fn open(&mut self, id: &str) {
        self.subscribe_channel(id);
        if self.opened.insert(id.to_string()) {
            self.window(id, None);
        }
    }

    fn thread(&self, channel: &str, root: &str) {
        let r = self.relay.clone();
        let filters = [
            json!({"ids": [root]}),
            json!({"#e": [root], "#h": [channel], "kinds": model::TIMELINE_KINDS, "depth_limit": 64, "limit": 200, "include_aux": true}),
        ];
        self.spawn(async move {
            // The relay answers one extension filter per request.
            let res = async {
                let mut evs = r.query(&filters[..1]).await?;
                evs.extend(r.query(&filters[1..]).await?);
                Ok(evs)
            }
            .await;
            Msg::Events(res)
        });
    }

    fn want_profiles(&mut self, pubkeys: impl IntoIterator<Item = String>) {
        let fresh: Vec<String> = pubkeys
            .into_iter()
            .filter(|p| self.requested_profiles.insert(p.clone()))
            .collect();
        for chunk in fresh.chunks(100) {
            let r = self.relay.clone();
            let chunk = chunk.to_vec();
            self.spawn(async move {
                Msg::Events(
                    r.query(&[json!({"kinds": [0], "authors": chunk, "limit": 100})])
                        .await,
                )
            });
        }
    }

    fn want_presence(&self, agents: Vec<String>) {
        if agents.is_empty() {
            return;
        }
        self.sub(
            "presence",
            vec![json!({"kinds": [20001], "authors": agents, "limit": 0})],
        );
        let r = self.relay.clone();
        self.spawn(async move {
            Msg::Events(
                r.query(&[json!({"kinds": [20001], "authors": agents, "limit": 200})])
                    .await,
            )
        });
    }

    fn mark_read(&mut self, channel: &str) {
        self.read.mark(channel, now());
        if let Some(ch) = self.store.channels.get_mut(channel) {
            ch.unread.clear();
            ch.mentioned.clear();
        }
    }

    fn needs_rows(&self) -> Vec<needs::Row> {
        let bot = |pk: &str| needs::is_bot(&self.store, &self.agents, &self.checklists, pk);
        needs::rows(&self.store, &self.checklists, bot, now())
    }

    /// Tab 0's data across every channel we're in (see the terminal app's
    /// `load_needs`): the last week as history, and a live tail.
    fn load_needs(&mut self) {
        let mine: Vec<&model::Channel> = self
            .store
            .channels
            .values()
            .filter(|c| c.is_member && !c.archived)
            .collect();
        let ids: Vec<String> = mine.iter().map(|c| c.id.clone()).collect();
        let dms: Vec<String> = mine
            .iter()
            .filter(|c| c.is_dm)
            .map(|c| c.id.clone())
            .collect();
        let op = self.checklists.trust.owner.clone();
        let now = now();
        let live = needs::filters(&ids, &dms, &op, now, true);
        if live.is_empty() {
            return;
        }
        self.sub("needs", live);
        let r = self.relay.clone();
        let history = needs::filters(&ids, &dms, &op, now, false);
        self.spawn(async move { Msg::Events(Ok(r.query(&history).await.unwrap_or_default())) });
    }

    fn is_agent(&self, pk: &str) -> bool {
        self.store.profiles.get(pk).is_some_and(|p| p.is_agent)
            || self.agents.by_pubkey.contains_key(pk)
    }

    fn person(&self, pk: &str) -> Person {
        Person {
            pubkey: pk.to_string(),
            name: self.store.name_of(pk),
            picture: self
                .store
                .profiles
                .get(pk)
                .map(|p| p.picture.clone())
                .unwrap_or_default(),
            is_agent: self.is_agent(pk),
        }
    }

    fn flush_read(&mut self) {
        if let Some(ev) = self.read.take_publish(&self.relay.keys, false) {
            let r = self.relay.clone();
            RT.spawn(async move {
                let _ = r.publish(&ev).await;
            });
        }
    }

    fn check_unread(&mut self, tabs: &[String]) {
        if !self.read.tried {
            return;
        }
        for id in tabs {
            if !self.unread_checked.insert(id.clone()) {
                continue;
            }
            // A week back: what's after the read position is unread, and
            // the replies feed "(N new)" and the bell.
            let since = now().saturating_sub(unread::HORIZON);
            let r = self.relay.clone();
            let channel = id.clone();
            self.spawn(async move {
                let f = json!({"kinds": model::UNREAD_KINDS, "#h": [channel], "since": since,
                    "limit": 500});
                Msg::Unread {
                    events: r.query(&[f]).await,
                    channel,
                }
            });
        }
    }

    fn prune_unread(&mut self) {
        for (id, ch) in self.store.channels.iter_mut() {
            if let Some(r) = self.read.read_at(id) {
                ch.unread.retain(|_, at| *at > r);
                let left = &ch.unread;
                ch.mentioned.retain(|m| left.contains_key(m));
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::ReadState(r) => {
                if let Ok(evs) = r {
                    for ev in &evs {
                        self.read.apply(&self.relay.keys, ev);
                    }
                    self.read.loaded = true;
                }
                // Even on failure, so unread still works from the last week.
                self.read.tried = true;
                self.prune_unread();
            }
            Msg::Archived(r) => {
                if let Ok(pks) = r {
                    self.archived = pks.into_iter().collect();
                }
            }
            Msg::Unread { channel, events } => {
                let Ok(evs) = events else { return };
                let mut want = vec![];
                for ev in evs
                    .iter()
                    .filter(|e| model::Message::from_event(e).root.is_some())
                {
                    want.extend(self.fold(ev, false));
                }
                self.want_profiles(want);
                if self.visible && self.current.as_deref() == Some(channel.as_str()) {
                    return;
                }
                let me = self.me();
                let after = self.read.read_at(&channel).unwrap_or(0);
                let ch = self.store.channel_mut(&channel);
                for ev in &evs {
                    let at = ev.created_at.as_secs();
                    if model::UNREAD_KINDS.contains(&ev.kind.as_u16())
                        && ev.pubkey.to_hex() != me
                        && at > after
                    {
                        ch.unread.insert(ev.id.to_hex(), at);
                        let tagged = ev
                            .tags
                            .iter()
                            .any(|t| matches!(t.as_slice(), [k, v, ..] if k == "p" && *v == me));
                        if tagged {
                            ch.mentioned.insert(ev.id.to_hex());
                        }
                    }
                }
            }
            Msg::Channels(Ok(evs)) => {
                self.channels_loaded = true;
                for ev in &evs {
                    self.store.apply(ev);
                }
                let people: Vec<String> = self
                    .store
                    .channels
                    .values()
                    .filter(|c| c.is_dm)
                    .flat_map(|c| c.members.clone())
                    .collect();
                self.want_profiles(people);
                self.refresh_sections();
                self.load_needs();
            }
            Msg::Channels(Err(e)) => self.notices.push(format!("couldn't load channels: {e}")),
            Msg::Lanes(Ok(evs)) => {
                self.apply_lanes(&evs);
                self.sections_loaded = true;
                if !self.order_pending.is_empty() {
                    let mut changed = false;
                    for (k, v) in std::mem::take(&mut self.order_pending) {
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            self.lanes.order.entry(k)
                        {
                            slot.insert(v);
                            changed = true;
                        }
                    }
                    if changed {
                        self.publish_order();
                    }
                }
                self.refresh_sections();
            }
            Msg::Lanes(Err(e)) => self.notices.push(format!("couldn't load sections: {e}")),
            Msg::Moved(Ok((ev, label))) => {
                self.apply_lanes(&[ev]);
                self.refresh_sections();
                self.notices.push(label);
            }
            Msg::Moved(Err(e)) => self.notices.push(format!("couldn't move: {e}")),
            Msg::Ordered(Ok(_)) => {}
            Msg::Ordered(Err(e)) => self.notices.push(format!("tab order not saved: {e}")),
            Msg::ChannelArchived { channel, result } => {
                let name = self.label(&channel);
                match result {
                    Ok(()) => self.notices.push(format!("archived {name}")),
                    Err(e) => {
                        self.store.channel_mut(&channel).archived = false;
                        self.rebuild_tabs();
                        self.notices.push(format!("{name} not archived: {e}"));
                    }
                }
            }
            Msg::Window {
                channel,
                events,
                older,
            } => {
                self.loading_older.remove(&channel);
                match events {
                    Ok(evs) => {
                        let mut want = vec![];
                        for ev in &evs {
                            if ev.kind.as_u16() == 39006 {
                                let v: Value =
                                    serde_json::from_str(&ev.content).unwrap_or_default();
                                let more =
                                    v.get("has_more").and_then(Value::as_bool).unwrap_or(false);
                                let cur = v.get("next_cursor").and_then(|c| {
                                    Some((
                                        c.get("created_at")?.as_u64()?,
                                        c.get("id")?.as_str()?.to_string(),
                                    ))
                                });
                                self.cursors
                                    .insert(channel.clone(), if more { cur } else { None });
                                continue;
                            }
                            want.extend(self.fold(ev, false));
                        }
                        let ch = self.store.channel_mut(&channel);
                        ch.loaded = true;
                        if older && evs.len() <= 1 {
                            ch.exhausted = true;
                        }
                        self.want_profiles(want);
                    }
                    Err(e) => {
                        // Opening it again retries.
                        if !older {
                            self.opened.remove(&channel);
                        }
                        self.notices.push(format!("couldn't load messages: {e}"));
                    }
                }
            }
            Msg::Events(Ok(evs)) => {
                let mut want = vec![];
                let mut agents = false;
                for ev in &evs {
                    agents |= ev.kind.as_u16() == 30177;
                    want.extend(self.fold(ev, false));
                }
                self.want_profiles(want);
                if agents {
                    self.want_presence(self.agents.by_pubkey.keys().cloned().collect());
                }
            }
            Msg::Events(Err(e)) => self.notices.push(format!("couldn't load: {e}")),
            Msg::Notice(n) => self.notices.push(n),
            // Errors are expected while the relay lacks the kinds: no notice.
            Msg::Checklist {
                channel,
                records,
                today,
            } => {
                let mut want = vec![];
                for ev in records.iter().flatten() {
                    if self.checklists.apply(ev) {
                        want.push(ev.pubkey.to_hex());
                    }
                }
                for ev in today.iter().flatten() {
                    let id = ev.id.to_hex();
                    self.checklists.note_message(
                        &channel,
                        &id,
                        ev.created_at.as_secs(),
                        &ev.content,
                    );
                }
                self.want_profiles(want);
            }
            Msg::Sent { id, result } => {
                let Some(ev) = self.sending.remove(&id) else {
                    return;
                };
                // It stays on screen marked failed: retry sends the same event,
                // discard hands the text back.
                if let Err(e) = result {
                    if e.to_string().starts_with("relay rejected") {
                        self.notices.push(format!("not sent: {e}"));
                    }
                    self.failed.insert(id, ev);
                }
            }
        }
    }

    fn on_live(&mut self, l: Live, was_online: &mut bool) {
        match l {
            Live::Event { sub, event } => {
                // Tab 0's tail carries tabs' messages too: count them as the tab's.
                let tab = sub == "needs"
                    && model::tag(&event, "h").is_some_and(|h| self.subscribed.contains(h));
                let want = self.fold(&event, sub.starts_with("ch:") || tab);
                self.want_profiles(want);
            }
            Live::Status(s) => {
                self.conn = match s {
                    ConnState::Connecting => Conn::Connecting,
                    ConnState::Online => Conn::Online,
                    ConnState::Offline => Conn::Offline,
                };
                if s == ConnState::Online {
                    self.heartbeat();
                    if *was_online {
                        // The live tail replays from `since`; open windows refetch.
                        for id in self.opened.clone() {
                            self.window(&id, None);
                        }
                        if let Some(c) = self.current.clone() {
                            self.load_checklist(&c);
                        }
                        self.load_needs();
                    }
                    *was_online = true;
                }
            }
            Live::Closed { .. } | Live::Eose { .. } => {}
        }
    }

    /// Fold one event. Returns pubkeys to look up profiles for.
    fn fold(&mut self, ev: &Event, live_channel: bool) -> Vec<String> {
        let kind = ev.kind.as_u16();
        match kind {
            24200 => {
                if model::tag(ev, "agent") == Some(&ev.pubkey.to_hex()) {
                    if let Some(frame) = self.decrypt_for_me(ev) {
                        self.agents.frame(&ev.pubkey.to_hex(), &frame);
                        return vec![ev.pubkey.to_hex()];
                    }
                }
                return vec![];
            }
            44200 => {
                let agent = ev.pubkey.to_hex();
                if model::tag(ev, "agent").is_none_or(|a| a == agent) {
                    if let Some(m) = self.decrypt_for_me(ev) {
                        self.agents.metric(&agent, &m);
                        return vec![agent];
                    }
                }
                return vec![];
            }
            30177 => {
                let me = self.store.me.clone();
                return self.agents.own_record(&me, ev).into_iter().collect();
            }
            20001 => {
                self.agents.presence(&ev.pubkey.to_hex(), &ev.content);
                return vec![];
            }
            30078 if readstate::is_read_state(ev) => {
                if self.read.apply(&self.relay.keys, ev) {
                    self.prune_unread();
                }
                return vec![];
            }
            30078 => {
                if self.apply_lanes(std::slice::from_ref(ev)) {
                    self.refresh_sections();
                }
                return vec![];
            }
            44100 | 44101 => {
                self.load_channels();
                return vec![];
            }
            k if checklist::KINDS.contains(&k) => {
                self.checklists.apply(ev);
                return vec![ev.pubkey.to_hex()];
            }
            39000 => {
                // Only channels we know: the relay sends every channel's.
                let known =
                    model::tag(ev, "d").is_some_and(|d| self.store.channels.contains_key(d));
                if known {
                    self.store.apply(ev);
                    self.refresh_sections();
                }
                return vec![];
            }
            39002 => {
                let d = model::tag(ev, "d").unwrap_or_default();
                if self
                    .store
                    .channels
                    .get(d)
                    .is_some_and(|c| c.is_member && !c.name.is_empty())
                {
                    self.store.apply(ev);
                } else {
                    // New to us: fetch its name and the rest.
                    self.load_channels();
                }
                return vec![];
            }
            _ => {}
        }
        let applied = self.store.apply(ev);
        let mut want = vec![ev.pubkey.to_hex()];
        if let Applied::Message { channel, id } = applied {
            self.checklists
                .note_message(&channel, &id, ev.created_at.as_secs(), &ev.content);
            let active = self.visible && self.current.as_deref() == Some(channel.as_str());
            let at = ev.created_at.as_secs();
            let author = ev.pubkey.to_hex();
            let seen = self.read.read_at(&channel).is_some_and(|r| at <= r);
            let counts = model::UNREAD_KINDS.contains(&kind) && author != self.store.me;
            if live_channel && !seen && counts && !active {
                let me = self.store.me.clone();
                let ch = self.store.channel_mut(&channel);
                ch.unread.insert(id.clone(), at);
                if ch
                    .messages
                    .get(&id)
                    .is_some_and(|m| m.mentions.contains(&me))
                {
                    ch.mentioned.insert(id.clone());
                }
            } else if live_channel && active && author != self.store.me {
                self.read.mark(&channel, now());
            }
            if let Some(m) = self
                .store
                .channels
                .get(&channel)
                .and_then(|c| c.messages.get(&id))
            {
                want.extend(m.mentions.iter().cloned());
            }
        }
        want
    }

    fn send(
        &mut self,
        channel: String,
        root: Option<String>,
        text: String,
        attachments: &[Attachment],
        to: &[String],
    ) -> Result<String, BuzzError> {
        let mut content = text.trim_end().to_string();
        for a in attachments {
            let label = if a.mime.starts_with("video/") {
                "video"
            } else {
                "image"
            };
            content.push_str(&format!("\n![{label}]({})", a.url));
        }
        let content = content.trim_start_matches('\n').to_string();
        if content.is_empty() {
            return Ok(String::new());
        }
        let mentioned = self.mentions_in(&channel, &content);
        let mut mentions = mentioned.clone();
        // The sticky agents: notified without an @ in the text, as Desktop
        // sends its pinned audience.
        for pk in to {
            if *pk != self.store.me && !mentions.contains(pk) {
                mentions.push(pk.clone());
            }
        }
        // In a DM everyone else is notified, as Desktop does.
        if let Some(ch) = self.store.channels.get(&channel).filter(|c| c.is_dm) {
            for m in &ch.members {
                if *m != self.store.me && !mentions.contains(m) {
                    mentions.push(m.clone());
                }
            }
        }
        // Replies go under the root, as the terminal app sends them.
        let thread = match &root {
            Some(r) => {
                let r = EventId::from_hex(r).map_err(|e| BuzzError::Failed(e.to_string()))?;
                Some(buzz_sdk::ThreadRef {
                    root_event_id: r,
                    parent_event_id: r,
                })
            }
            None => None,
        };
        let uuid = uuid::Uuid::parse_str(&channel)
            .map_err(|_| BuzzError::Failed("bad channel id".into()))?;
        let refs: Vec<&str> = mentions.iter().map(String::as_str).collect();
        let media: Vec<Vec<String>> = attachments
            .iter()
            .map(|a| BlobDescriptor::from(a).imeta_tag())
            .collect();
        let builder =
            buzz_sdk::build_message(uuid, &content, thread.as_ref(), &refs, false, &media, &[])
                .map_err(|e| BuzzError::Failed(e.to_string()))?
                // Non-notifying label refs so Desktop renders mention chips.
                .tags(
                    mentioned
                        .iter()
                        .filter_map(|pk| Tag::parse(["mention", pk.as_str()]).ok()),
                );
        let ev = self.relay.sign(builder)?;
        let id = ev.id.to_hex();
        self.store.apply(&ev);
        // Agents it @mentions who aren't here yet join first, so they see it.
        let ours = &self.agents;
        let invite = self
            .store
            .outsiders(&channel, &mentioned, |pk| ours.is_owned(pk));
        self.publish_message(ev, channel, invite);
        Ok(id)
    }

    /// A Q, signed as the terminal app signs it: p-tag and mention tag
    /// for the agent, no thread.
    fn ask_quick(
        &mut self,
        text: &str,
        here: Option<&str>,
        thread: Option<&str>,
    ) -> Result<String, BuzzError> {
        let here = match here {
            Some(h) => {
                quick::can_add(&self.store, quick::CHANNEL, Some(h)).map_err(BuzzError::Failed)?;
                Some((h, thread))
            }
            None => None,
        };
        let a = quick::ask(&self.store, quick::CHANNEL, quick::AGENT, text, here)
            .map_err(BuzzError::Failed)?;
        let uuid = uuid::Uuid::parse_str(&a.channel)
            .map_err(|_| BuzzError::Failed("bad #quick channel id".into()))?;
        let builder =
            buzz_sdk::build_message(uuid, &a.content, None, &[a.agent.as_str()], false, &[], &[])
                .map_err(|e| BuzzError::Failed(e.to_string()))?
                .tags(Tag::parse(["mention", a.agent.as_str()]).ok());
        let ev = self.relay.sign(builder)?;
        let id = ev.id.to_hex();
        self.store.apply(&ev);
        self.publish_message(ev, a.channel, vec![]);
        Ok(id)
    }

    fn publish_message(&mut self, ev: Event, channel: String, invite: Vec<String>) {
        let id = ev.id.to_hex();
        self.sending.insert(id.clone(), ev.clone());
        let r = self.relay.clone();
        let tx = self.tx.clone();
        self.spawn(async move {
            if !invite.is_empty() {
                if let Err(e) = r.add_members(&channel, &invite).await {
                    let _ = tx.send(Msg::Notice(format!("couldn't add to channel: {e}")));
                }
            }
            let result = r.publish(&ev).await;
            Msg::Sent { id, result }
        });
    }

    /// Pubkeys of channel members whose `@Name` appears in `text`. Longest
    /// names first, so `@Ann Lee` wins over `@Ann`.
    fn mentions_in(&self, channel: &str, text: &str) -> Vec<String> {
        let mut people: Vec<(String, String)> = self
            .store
            .channels
            .get(channel)
            .map(|c| c.members.clone())
            .unwrap_or_default()
            .into_iter()
            .chain(self.agents.owned_keys().cloned())
            .filter_map(|pk| {
                let p = self.store.profiles.get(&pk)?;
                (!p.name.is_empty()).then(|| (p.name.clone(), pk))
            })
            .collect();
        people.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
        let mut rest = text.to_string();
        let mut out = vec![];
        for (name, pk) in people {
            let at = format!("@{name}");
            if rest.contains(&at) {
                rest = rest.replace(&at, "");
                if !out.contains(&pk) {
                    out.push(pk);
                }
            }
        }
        out
    }

    fn thumbs_up(&mut self, target: &str) {
        let Ok(id) = EventId::from_hex(target) else {
            return;
        };
        let mine = self.store.my_reaction(target, model::THUMBS_UP);
        let built = match &mine {
            Some(rid) => EventId::from_hex(rid)
                .map_err(anyhow::Error::from)
                .and_then(|r| Ok(buzz_sdk::build_remove_reaction(r)?)),
            None => buzz_sdk::build_reaction(id, model::THUMBS_UP).map_err(anyhow::Error::from),
        };
        let ev = match built.and_then(|b| self.relay.sign(b)) {
            Ok(ev) => ev,
            Err(e) => return self.notices.push(format!("couldn't react: {e}")),
        };
        self.store.apply(&ev);
        let r = self.relay.clone();
        let tx = self.tx.clone();
        RT.spawn(async move {
            if let Err(e) = r.publish(&ev).await {
                let _ = tx.send(Msg::Events(Err(e.context("👍 didn't go through"))));
            }
        });
    }

    fn view(&self, ch: &model::Channel, m: &model::Message) -> Message {
        let me = &self.store.me;
        let body: String = m
            .content
            .lines()
            .filter(|l| {
                !m.media.iter().any(|x| {
                    l.trim().starts_with("![") && l.trim().ends_with(&format!("({})", x.url))
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = if m.system { system_text(&body) } else { body };
        let reactions = self
            .store
            .reactions
            .get(&m.id)
            .map(|rs| {
                rs.iter()
                    .filter(|r| !r.by.is_empty())
                    .map(|r| Reaction {
                        emoji: if r.emoji == "+" {
                            model::THUMBS_UP.into()
                        } else {
                            r.emoji.clone()
                        },
                        count: r.by.len() as u32,
                        mine: r.by.contains_key(me),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut repliers: Vec<Person> = vec![];
        let mut last_reply_at = ch.summary_replies.get(&m.id).map_or(0, |s| s.1);
        let mut last_reply_by = String::new();
        let new_replies = unread::new_replies(ch, &m.id, me, &self.read, now()).len() as u32;
        if let Some(ids) = ch.replies.get(&m.id) {
            for id in ids.iter().rev() {
                let Some(r) = ch.messages.get(id) else {
                    continue;
                };
                if r.deleted {
                    continue;
                }
                if last_reply_by.is_empty() && r.created_at >= last_reply_at {
                    last_reply_by = self.store.name_of(&r.author);
                }
                last_reply_at = last_reply_at.max(r.created_at);
                if repliers.len() < 3 && !repliers.iter().any(|p| p.pubkey == r.author) {
                    repliers.push(self.person(&r.author));
                }
            }
        }
        let state = if self.sending.contains_key(&m.id) {
            SendState::Sending
        } else if self.failed.contains_key(&m.id) {
            SendState::Failed
        } else {
            SendState::Sent
        };
        Message {
            id: m.id.clone(),
            author: m.author.clone(),
            author_name: self.store.name_of(&m.author),
            is_agent: self.is_agent(&m.author),
            at: m.created_at,
            segments: segments(body.trim_end()),
            mention_names: m
                .mentions
                .iter()
                .filter_map(|pk| self.store.profiles.get(pk))
                .map(|p| p.name.clone())
                .filter(|n| !n.is_empty())
                .collect(),
            media: m
                .media
                .iter()
                .map(|x| Media {
                    url: x.url.clone(),
                    mime: x.mime.clone(),
                    width: x.dim.map_or(0, |d| d.0),
                    height: x.dim.map_or(0, |d| d.1),
                    name: x.name.clone(),
                })
                .collect(),
            reactions,
            reply_count: ch.reply_count(&m.id) as u32,
            repliers,
            last_reply_at,
            last_reply_by,
            new_replies,
            edited: m.edited,
            deleted: m.deleted,
            system: m.system,
            state,
        }
    }
}

fn system_text(raw: &str) -> String {
    let v: Value = serde_json::from_str(raw).unwrap_or_default();
    match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => format!("_{}_", t.replace('_', " ")),
        None => raw.to_string(),
    }
}

/// Split markdown into text and fenced code blocks.
fn segments(src: &str) -> Vec<Segment> {
    let mut out = vec![];
    let mut text: Vec<&str> = vec![];
    let mut code: Option<(String, Vec<&str>)> = None;
    for line in src.lines() {
        let fence = line.trim_start().strip_prefix("```");
        match (&mut code, fence) {
            (None, Some(lang)) => {
                if !text.is_empty() {
                    out.push(Segment::Text {
                        text: text.join("\n").trim_matches('\n').to_string(),
                    });
                    text.clear();
                }
                code = Some((lang.trim().to_string(), vec![]));
            }
            (Some(_), Some(_)) => {
                let (lang, body) = code.take().unwrap();
                out.push(Segment::Code {
                    lang,
                    body: body.join("\n"),
                });
            }
            (Some((_, body)), None) => body.push(line),
            (None, None) => text.push(line),
        }
    }
    // An unclosed fence still shows as code.
    if let Some((lang, body)) = code {
        out.push(Segment::Code {
            lang,
            body: body.join("\n"),
        });
    }
    if !text.is_empty() {
        let t = text.join("\n").trim_matches('\n').to_string();
        if !t.is_empty() {
            out.push(Segment::Text { text: t });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_fences_split_out() {
        let s = segments("look:\n```rust\nlet a = 1;\n```\nok");
        assert_eq!(s.len(), 3);
        assert!(matches!(&s[0], Segment::Text { text } if text == "look:"));
        assert!(
            matches!(&s[1], Segment::Code { lang, body } if lang == "rust" && body == "let a = 1;")
        );
        assert!(matches!(&s[2], Segment::Text { text } if text == "ok"));
        let open = segments("```\nunclosed");
        assert!(matches!(&open[0], Segment::Code { body, .. } if body == "unclosed"));
    }

    #[test]
    fn colors_match_the_terminal() {
        // Same ramp step as the terminal's canvas and body text.
        let c = tone(Tone::Surface, 0, false);
        assert_eq!((c.r, c.g, c.b), (0x06, 0x08, 0x0f));
        let light = tone(Tone::Surface, 0, true);
        assert_eq!((light.r, light.g, light.b), (0xf8, 0xfa, 0xff));
    }

    fn engine() -> Engine {
        let keys = Keys::generate();
        let _g = RT.enter();
        let relay = Relay::new("wss://example.invalid", keys.clone(), None).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (live, _lrx) = mpsc::unbounded_channel();
        Engine {
            relay,
            live,
            tx,
            store: Store::new(keys.public_key().to_hex()),
            lanes: Lanes::default(),
            sections: vec![],
            chosen: vec![],
            tabs: vec![],
            tab_section: HashMap::new(),
            channels_loaded: false,
            sections_loaded: false,
            order_pending: HashMap::new(),
            agents: Agents::default(),
            read: ReadState::new(new_read_id(), new_read_id()),
            conn: Conn::Connecting,
            current: None,
            visible: true,
            cursors: HashMap::new(),
            requested_profiles: HashSet::new(),
            opened: HashSet::new(),
            subscribed: HashSet::new(),
            loading_older: HashSet::new(),
            unread_checked: HashSet::new(),
            started: 0,
            bell: HashMap::new(),
            archived: HashSet::new(),
            sending: HashMap::new(),
            failed: HashMap::new(),
            notices: vec![],
            checklists: Checklists::default(),
            checklist_for: None,
        }
    }

    /// Tab 0's rows come from folded events, with what Swift shows, and a
    /// reply in the thread clears them.
    #[test]
    fn needs_you_rows_from_folded_events() {
        use nostr::{EventBuilder, Kind, Timestamp};
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let mut e = engine();
        let (op, alice) = (Keys::generate(), Keys::generate());
        let opk = op.public_key().to_hex();
        e.checklists.trust.owner = opk.clone();
        e.store.channel_mut(ch).name = "beacon".into();
        let ask = EventBuilder::new(Kind::Custom(9), "@lucian ship it?\nmore")
            .tags([
                Tag::parse(["h", ch]).unwrap(),
                Tag::parse(["p", &opk]).unwrap(),
            ])
            .custom_created_at(Timestamp::from(now() - 60))
            .sign_with_keys(&alice)
            .unwrap();
        e.fold(&ask, true);
        let c = Client {
            engine: Arc::new(Mutex::new(e)),
            stop: Arc::new(tokio::sync::Notify::new()),
        };
        let rows = c.needs_you();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert!(r.section == NeedSection::Answer);
        assert_eq!(
            (r.kind.as_str(), r.channel_label.as_str()),
            ("mention", "#beacon")
        );
        assert_eq!(r.text, "@lucian ship it?");
        assert_eq!(r.thread.as_deref(), Some(ask.id.to_hex().as_str()));
        assert!(c.follow(r.link.clone()).is_some_and(|f| f.channel == ch));
        assert_eq!(c.needs_count(), 1);
        // His answer in its thread clears it.
        let reply = EventBuilder::new(Kind::Custom(9), "yes")
            .tags([
                Tag::parse(["h", ch]).unwrap(),
                Tag::parse(["e", &ask.id.to_hex(), "", "reply"]).unwrap(),
            ])
            .sign_with_keys(&op)
            .unwrap();
        c.engine.lock().unwrap().fold(&reply, true);
        assert_eq!(c.needs_count(), 0);
    }

    /// Q from the phone: the same core message as the terminal's, the
    /// question alone first, then with the channel on screen.
    #[test]
    fn q_asks_taskbot_in_quick_alone_or_with_this_channel() {
        let here = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let q = quick::CHANNEL_ID;
        let tb = Keys::generate().public_key().to_hex();
        let mut e = engine();
        let ch = e.store.channel_mut(here);
        ch.name = "buzz-vim".into();
        ch.timeline = vec!["m1".into(), "m2".into()];
        let c = Client {
            engine: Arc::new(Mutex::new(e)),
            stop: Arc::new(tokio::sync::Notify::new()),
        };
        let err = |r: Result<QuickBox, BuzzError>| match r {
            Err(BuzzError::Failed(m)) => m,
            _ => String::new(),
        };
        assert!(err(c.open_quick(Some(here.into()))).contains("still loading"));
        c.engine.lock().unwrap().channels_loaded = true;
        assert!(err(c.open_quick(Some(here.into()))).starts_with("no #quick channel"));
        {
            let mut e = c.engine.lock().unwrap();
            let me = e.store.me.clone();
            let qc = e.store.channel_mut(q);
            qc.name = "quick".into();
            qc.members = vec![me, tb.clone()];
            e.store.profiles.entry(tb.clone()).or_default().name = "taskbot".into();
        }
        let b = c.open_quick(Some(here.into())).ok().unwrap();
        assert_eq!((b.channel.as_str(), b.label.as_str()), (q, "#quick"));
        assert_eq!(
            (b.agent.as_str(), b.here.as_deref()),
            ("taskbot", Some("#buzz-vim"))
        );
        assert!(c.engine.lock().unwrap().requested_profiles.contains(&tb));
        // In #quick, or nowhere: nothing for a second tap to add.
        assert!(c.open_quick(Some(q.into())).ok().unwrap().here.is_none());
        assert!(c.open_quick(None).ok().unwrap().here.is_none());
        let sent = |id: &str| c.engine.lock().unwrap().sending.get(id).cloned().unwrap();
        let tags = |ev: &Event| -> Vec<Vec<String>> {
            ev.tags.iter().map(|t| t.as_slice().to_vec()).collect()
        };
        // First Q: the question alone, top level, to taskbot.
        let id = c
            .ask_quick(" how do we ship? ".into(), None, None)
            .ok()
            .unwrap();
        let ev = sent(&id);
        assert_eq!(ev.content, "[Q]\n@taskbot how do we ship?");
        let tg = tags(&ev);
        assert!(tg.contains(&vec!["h".into(), q.into()]));
        assert!(tg.iter().any(|t| t[0] == "p" && t[1] == tb));
        assert!(tg.contains(&vec!["mention".into(), tb.clone()]));
        assert!(!tg.iter().any(|t| t[0] == "e"));
        assert!(
            c.engine.lock().unwrap().store.channels[q]
                .messages
                .contains_key(&id)
        );
        // Q again: + this channel, its open thread and latest messages.
        let id = c
            .ask_quick("why?".into(), Some(here.into()), Some("m1".into()))
            .ok()
            .unwrap();
        let ev = sent(&id);
        assert!(
            ev.content
                .starts_with("[Q]\nContext: #buzz-vim (channel 2130646b")
        );
        assert!(
            ev.content
                .contains("Open thread: buzz://message?channel=2130646b")
        );
        assert!(ev.content.contains("Last 2 messages (oldest first): m1 m2"));
        assert!(ev.content.ends_with("\n\n@taskbot why?"));
        // The landing link opens that thread.
        let f = c
            .follow(format!("buzz://message?channel={q}&id={id}&thread={id}"))
            .unwrap();
        assert_eq!(
            (f.channel.as_str(), f.root.as_deref()),
            (q, Some(id.as_str()))
        );
        // Nothing typed, or #quick as the context: says why, sends nothing.
        assert!(matches!(c.ask_quick(" ".into(), None, None),
            Err(BuzzError::Failed(m)) if m == "type a question first"));
        assert!(matches!(c.ask_quick("x".into(), Some(q.into()), None),
            Err(BuzzError::Failed(m)) if m.contains("already")));
    }

    /// Records and crash notices folded live show up in the checklist and
    /// the status line the app reads.
    #[test]
    fn checklist_and_loop_status_from_live_events() {
        use nostr::{EventBuilder, Kind, Timestamp};
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let mut e = engine();
        let bot = Keys::generate();
        let bk = bot.public_key().to_hex();
        // The bot is the watcher here too.
        e.checklists.trust.watchers = vec![bk.clone()];
        let rec = |kind: u16, d: String, v: Value| {
            EventBuilder::new(Kind::Custom(kind), v.to_string())
                .tags(
                    [
                        ["d", d.as_str()],
                        ["h", ch],
                        ["t", "buzz-checklist"],
                        ["channel", ch],
                        ["checklist-type", d.split('/').nth(1).unwrap()],
                    ]
                    .map(|t| Tag::parse(t).unwrap()),
                )
                .custom_created_at(Timestamp::from(now() - 10))
                .sign_with_keys(&bot)
                .unwrap()
        };
        e.fold(
            &rec(
                30471,
                format!("buzz-checklist/header/{ch}/{bk}"),
                json!({"title": "loop", "mode": "running", "resume": {"branch": "agent/x"}}),
            ),
            true,
        );
        e.fold(
            &rec(30470, format!("buzz-checklist/item/{ch}/{bk}/000000a1"),
                json!({"text": "ship it", "status": "done", "order": 1, "evidence": ["msg:abc", "commit:123"]})),
            true,
        );
        e.fold(
            &rec(
                30472,
                format!("buzz-checklist/watch/{ch}"),
                json!({"state": "stalled", "since": now() - 60}),
            ),
            true,
        );
        let crash = EventBuilder::new(Kind::Custom(9), "agent process crashed, respawning")
            .tags([Tag::parse(["h", ch]).unwrap()])
            .sign_with_keys(&bot)
            .unwrap();
        e.fold(&crash, true);
        let c = Client {
            engine: Arc::new(Mutex::new(e)),
            stop: Arc::new(tokio::sync::Notify::new()),
        };
        let list = c.checklist(ch.into());
        assert_eq!(
            (list.title.as_str(), list.mode.as_deref()),
            ("loop", Some("running"))
        );
        assert_eq!(list.resume, "agent/x");
        let it = &list.items[0];
        assert!(it.status == ItemStatus::Done && !it.missing_evidence);
        assert!(
            it.evidence[0]
                .link
                .as_deref()
                .is_some_and(|l| l.ends_with("id=abc"))
        );
        assert!(it.evidence[1].link.is_none());
        let s = c.loop_status(ch.into()).unwrap();
        assert!(s.running && s.needs_you);
        assert_eq!(s.watch.as_deref(), Some("stalled"));
        // A stall with no ping: no push covers it, so it notifies locally.
        let rows = c.needs_you();
        assert!(
            rows.iter()
                .any(|r| r.watcher && !r.pushed && r.kind == "stalled")
        );
        assert_eq!((s.open, s.done, s.crashes_today), (0, 1, 1));
        assert!(c.loop_status("other".into()).is_none());
    }

    #[test]
    fn mentions_need_the_whole_name() {
        let mut e = engine();
        let ch = e.store.channel_mut("c");
        ch.members = vec!["a".into(), "b".into()];
        for (pk, name) in [("a", "Ann"), ("b", "Ann Lee")] {
            e.store.profiles.insert(
                pk.into(),
                model::Profile {
                    name: name.into(),
                    is_agent: false,
                    ..Default::default()
                },
            );
        }
        assert_eq!(e.mentions_in("c", "hi @Ann Lee"), vec!["b".to_string()]);
        assert_eq!(e.mentions_in("c", "hi @Ann"), vec!["a".to_string()]);
        assert!(e.mentions_in("c", "hi Ann").is_empty());
    }

    #[test]
    fn reply_line_counts_what_you_have_not_seen() {
        let mut e = engine();
        let me = e.store.me.clone();
        let t = now() - 1000;
        let ch = e.store.channel_mut("c");
        for (id, author, at) in [("root", "a"), ("r1", "a"), ("r2", "b"), ("r3", me.as_str())]
            .into_iter()
            .zip([
                t - unread::HORIZON - 50,
                t - unread::HORIZON - 10,
                t + 150,
                t + 160,
            ])
            .map(|((id, au), at)| (id, au.to_string(), at))
        {
            ch.messages.insert(
                id.into(),
                model::Message {
                    id: id.into(),
                    author,
                    created_at: at,
                    root: (id != "root").then(|| "root".to_string()),
                    ..Default::default()
                },
            );
        }
        ch.replies
            .insert("root".into(), vec!["r1".into(), "r2".into(), "r3".into()]);
        let view = |e: &Engine| {
            let ch = &e.store.channels["c"];
            e.view(ch, &ch.messages["root"])
        };
        // r1 is older than a week and the thread has no read position; r3 is ours.
        let m = view(&e);
        assert_eq!(
            (m.new_replies, m.last_reply_at, m.repliers.len()),
            (1, t + 160, 3)
        );
        assert_eq!(m.repliers[0].pubkey, me);
        assert_eq!(m.last_reply_by, e.store.name_of(&me));
        // A summary newer than the replies we hold: who wrote it is unknown.
        e.store
            .channel_mut("c")
            .summary_replies
            .insert("root".into(), (4, t + 200));
        assert_eq!((view(&e).last_reply_at, view(&e).last_reply_by), (t + 200, String::new()));
        e.store.channel_mut("c").summary_replies.remove("root");
        e.read.mark("thread:root", t + 155);
        assert_eq!(view(&e).new_replies, 0);
    }

    #[test]
    fn dragged_order_holds_and_new_channels_follow() {
        let mut e = engine();
        for (id, name) in [("a", "alpha"), ("b", "beta"), ("c", "gamma"), ("x", "old")] {
            let ch = e.store.channel_mut(id);
            ch.name = name.into();
            ch.is_member = true;
        }
        e.store.channel_mut("x").archived = true;
        e.sections = vec![Section {
            id: "s".into(),
            name: "S".into(),
            channels: vec!["c".into(), "a".into(), "b".into(), "x".into()],
        }];
        e.chosen = vec!["s".into()];
        e.rebuild_tabs();
        assert_eq!(e.tabs, vec!["c", "a", "b"]);
        assert_eq!(e.tab_section["b"], "s");
    }
}
