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

use buzz_vim_core::live::{self, Cmd, ConnState, Live};
use buzz_vim_core::model::{self, Applied, Store};
use buzz_vim_core::palette::{self, Role};
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
pub use pairing::{PairStep, Pairing};

uniffi::setup_scaffolding!();

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    // reqwest and the websocket both link rustls with more than one crypto
    // backend, so rustls won't pick one itself: without this every TLS
    // connection panics inside its task and the app sits at "connecting".
    let _ = rustls::crypto::ring::default_provider().install_default();
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

/// Where the jump key lands: a top-level message, or a reply (`root` set)
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
    Notice(String),
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
    /// Section -> the user's own tab order, set by dragging tabs.
    order: HashMap<String, Vec<String>>,
    channels_loaded: bool,
    sections_loaded: bool,
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
    /// channel -> where it was read up to when it last came on screen:
    /// newer top-level messages are what the jump key visits.
    entry: HashMap<String, u64>,
    /// Top-level messages the jump key has already been to.
    visited: HashSet<String>,
    /// Identities the relay has archived: left out of @ completion.
    archived: HashSet<String>,
    /// Our sends the relay hasn't confirmed: id -> event.
    sending: HashMap<String, Event>,
    failed: HashMap<String, Event>,
    notices: Vec<String>,
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
            order: HashMap::new(),
            channels_loaded: false,
            sections_loaded: false,
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
            entry: HashMap::new(),
            visited: HashSet::new(),
            archived: HashSet::new(),
            sending: HashMap::new(),
            failed: HashMap::new(),
            notices: vec![],
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
            .map(|id| {
                let ch = e.store.channels.get(id);
                ChannelTab {
                    id: id.clone(),
                    label: e.label(id),
                    unread: ch.map_or(0, |c| c.unread.len() as u32),
                    mentions: ch.map_or(0, |c| {
                        (if c.is_dm {
                            c.unread.len()
                        } else {
                            c.mentioned.len()
                        }) as u32
                    }),
                    is_dm: ch.is_some_and(|c| c.is_dm),
                    section: e.tab_section.get(id).cloned().unwrap_or_default(),
                }
            })
            .collect()
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
    /// ones) follow in sidebar order. Save it and pass it back next launch.
    pub fn set_order(&self, section: String, ids: Vec<String>) {
        let mut e = self.engine.lock().unwrap();
        e.order.insert(section, ids);
        e.rebuild_tabs();
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
        let fresh = e.current != channel;
        e.current = channel.clone();
        if let Some(id) = channel {
            // Coming onto it: remember where it was read up to first. Until
            // the startup load is in that is unknown; the load fills it in.
            if fresh && e.read.tried {
                match e.read.read_at(&id) {
                    Some(at) => e.entry.insert(id.clone(), at),
                    None => e.entry.remove(&id),
                };
            }
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

    /// The jump key in `channel`: the oldest unread thing there, or none
    /// (then the app moves on to the next tab with something unread). A
    /// top-level message counts as visited once returned; a reply's thread
    /// is marked read when the app opens it.
    pub fn next_unread(&self, channel: String) -> Option<Jump> {
        let mut e = self.engine.lock().unwrap();
        let ch = e.store.channels.get(&channel)?;
        let next = unread::next_unread(
            ch,
            &e.store.me,
            &e.read,
            e.entry.get(&channel).copied(),
            &e.visited,
            now(),
        )?;
        Some(match next {
            Next::Message(id) => {
                e.visited.insert(id.clone());
                Jump { root: None, id }
            }
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
        e.agents
            .rows(&e.store, None)
            .into_iter()
            .map(|r| Agent {
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

    /// Agents mid-turn anywhere, by name.
    pub fn working_anywhere(&self) -> Vec<String> {
        self.agents()
            .into_iter()
            .filter(|a| a.busy)
            .map(|a| a.name)
            .collect()
    }

    /// Channel members and our agents for @ completion, agents first.
    pub fn people(&self, channel: String) -> Vec<Person> {
        let e = self.engine.lock().unwrap();
        let mut keys: Vec<String> = e
            .store
            .channels
            .get(&channel)
            .map(|c| c.members.clone())
            .unwrap_or_default();
        keys.extend(e.agents.owned_keys().cloned());
        keys.sort();
        keys.dedup();
        keys.retain(|k| *k != e.store.me && !e.archived.contains(k));
        let mut out: Vec<Person> = keys.iter().map(|k| e.person(k)).collect();
        out.retain(|p| !p.name.is_empty());
        out.sort_by_key(|p| (!p.is_agent, p.name.to_lowercase()));
        out
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
        let e = self.engine.lock().unwrap();
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

    fn label(&self, id: &str) -> String {
        let Some(c) = self.store.channels.get(id) else {
            return format!("#{}", id.chars().take(8).collect::<String>());
        };
        if c.is_dm {
            let others: Vec<String> = c
                .members
                .iter()
                .filter(|m| **m != self.store.me)
                .map(|m| self.store.name_of(m))
                .collect();
            if !others.is_empty() {
                return format!("@{}", others.join(", "));
            }
        }
        let n = if c.name.is_empty() {
            c.id.chars().take(8).collect()
        } else {
            c.name.clone()
        };
        if c.is_dm {
            format!("@{n}")
        } else {
            format!("#{n}")
        }
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
        self.sub("lanes", vec![json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_SECTIONS, sections::LANE_STARS], "limit": 0})]);
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
                _ => continue,
            }
            changed = true;
        }
        changed
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
            let mut chans: Vec<&String> = s.channels.iter().collect();
            if let Some(o) = self.order.get(&s.id) {
                chans.sort_by_key(|c| o.iter().position(|x| x == *c).unwrap_or(usize::MAX));
            }
            for c in chans {
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
            // the replies feed "(N new)" and the jump key.
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
                    // Where the open channel was read up to before this
                    // run marked it, for the jump key.
                    let mut before = ReadState::new(String::new(), String::new());
                    for ev in &evs {
                        before.apply(&self.relay.keys, ev);
                        self.read.apply(&self.relay.keys, ev);
                    }
                    if let Some(c) = self.current.clone() {
                        if let Some(at) = before.read_at(&c) {
                            self.entry.insert(c, at);
                        }
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
            }
            Msg::Channels(Err(e)) => self.notices.push(format!("couldn't load channels: {e}")),
            Msg::Lanes(Ok(evs)) => {
                self.apply_lanes(&evs);
                self.sections_loaded = true;
                self.refresh_sections();
            }
            Msg::Lanes(Err(e)) => self.notices.push(format!("couldn't load sections: {e}")),
            Msg::Moved(Ok((ev, label))) => {
                self.apply_lanes(&[ev]);
                self.refresh_sections();
                self.notices.push(label);
            }
            Msg::Moved(Err(e)) => self.notices.push(format!("couldn't move: {e}")),
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
                let want = self.fold(&event, sub.starts_with("ch:"));
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
        let new_replies = unread::new_replies(ch, &m.id, me, &self.read, now()).len() as u32;
        if let Some(ids) = ch.replies.get(&m.id) {
            for id in ids.iter().rev() {
                let Some(r) = ch.messages.get(id) else {
                    continue;
                };
                if r.deleted {
                    continue;
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
                })
                .collect(),
            reactions,
            reply_count: ch.reply_count(&m.id) as u32,
            repliers,
            last_reply_at,
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
            order: HashMap::new(),
            channels_loaded: false,
            sections_loaded: false,
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
            entry: HashMap::new(),
            visited: HashSet::new(),
            archived: HashSet::new(),
            sending: HashMap::new(),
            failed: HashMap::new(),
            notices: vec![],
        }
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
            channels: vec!["a".into(), "b".into(), "c".into(), "x".into()],
        }];
        e.chosen = vec!["s".into()];
        e.order.insert("s".into(), vec!["c".into(), "a".into()]);
        e.rebuild_tabs();
        assert_eq!(e.tabs, vec!["c", "a", "b"]);
        assert_eq!(e.tab_section["b"], "s");
    }
}
