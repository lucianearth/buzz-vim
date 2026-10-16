//! Relay IO: turns `Action`s into requests and folds the answers back into
//! the app. Every request runs as its own task and reports on `Msg`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use nostr::nips::nip44;
use nostr::{Event, EventId, Tag};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{Action, App};
use crate::live::Cmd;
use crate::model::{self, Applied};
use crate::readstate::{self, ReadState};
use crate::relay::{BlobDescriptor, Relay};
use crate::sections::{self, Lanes};
use crate::unread;

pub enum Msg {
    Channels(anyhow::Result<Vec<Event>>),
    /// A page of channel history. `older` = it was a "load more" request.
    Window {
        channel: String,
        events: anyhow::Result<Vec<Event>>,
        older: bool,
    },
    Thread {
        events: anyhow::Result<Vec<Event>>,
    },
    Events(Vec<Event>),
    Lanes(anyhow::Result<Vec<Event>>),
    /// Our read-state slots at startup (NIP-RS full-state load).
    ReadState(anyhow::Result<Vec<Event>>),
    /// Recent top-level messages in a tab, to find what's unread.
    Unread {
        channel: String,
        events: anyhow::Result<Vec<Event>>,
    },
    /// Identities the relay has archived (NIP-IA).
    Archived(anyhow::Result<Vec<String>>),
    Sent {
        id: String,
        result: anyhow::Result<()>,
    },
    Did(anyhow::Result<String>),
    /// Our sections doc after a move, and the status line to show.
    Moved(anyhow::Result<(Event, String)>),
    /// The upload, and the (channel, thread root) it was meant for.
    Uploaded(anyhow::Result<BlobDescriptor>, (String, Option<String>)),
    Image {
        url: String,
        result: anyhow::Result<image::DynamicImage>,
    },
    /// An image download moved along: (bytes so far, total if known).
    Progress {
        url: String,
        got: u64,
        total: Option<u64>,
    },
    /// An image file arrived, before it is decoded.
    Bytes {
        url: String,
        bytes: Arc<Vec<u8>>,
    },
    Encoded {
        key: (String, u16, u16),
        result: anyhow::Result<ratatui_image::protocol::Protocol>,
    },
    /// A dropped file (`label` is its path) or the clipboard, read.
    Attachment {
        label: Option<String>,
        result: anyhow::Result<crate::clip::Pending>,
    },
}

pub struct Sync {
    pub relay: Relay,
    pub live: UnboundedSender<Cmd>,
    pub tx: UnboundedSender<Msg>,
    pub lanes: Lanes,
    /// channel -> (has_more, cursor) from the 39006 page marker
    pub cursors: std::collections::HashMap<String, Option<(u64, String)>>,
    requested_profiles: HashSet<String>,
    opened: HashSet<String>,
    subscribed: HashSet<String>,
    loading_older: HashSet<String>,
    started: u64,
    /// Tabs whose unread messages we've looked up since startup.
    unread_checked: HashSet<String>,
    /// Image downloads at a time. The viewer queues the picture on screen
    /// first, so it isn't stuck behind the rest of the thread.
    image_slots: Arc<tokio::sync::Semaphore>,
}

pub fn now() -> u64 {
    nostr::Timestamp::now().as_secs()
}

impl Sync {
    pub fn new(relay: Relay, live: UnboundedSender<Cmd>, tx: UnboundedSender<Msg>) -> Self {
        Sync {
            relay,
            live,
            tx,
            lanes: Lanes::default(),
            cursors: Default::default(),
            requested_profiles: HashSet::new(),
            opened: HashSet::new(),
            subscribed: HashSet::new(),
            loading_older: HashSet::new(),
            started: now(),
            unread_checked: HashSet::new(),
            image_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }

    fn me(&self) -> String {
        self.relay.keys.public_key().to_hex()
    }

    fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(f.await);
        });
    }

    /// Startup: channels, sidebar lanes, and the global live subscriptions.
    pub fn start(&mut self) {
        let me = self.me();
        self.load_channels();
        self.load_lanes();
        let r = self.relay.clone();
        self.spawn(async move { Msg::Archived(r.archived().await) });
        let now = now();
        let sub = |id: &str, filters: Vec<Value>| Cmd::Sub {
            id: id.to_string(),
            filters,
        };
        // Agent telemetry (never stored) and turn metrics; both p-gated to exactly [me].
        let _ = self.live.send(sub(
            "obs",
            vec![json!({"kinds": [24200], "#p": [me], "since": now - 300, "limit": 1000})],
        ));
        let _ = self.live.send(sub(
            "metrics",
            vec![json!({"kinds": [44200], "#p": [me], "limit": 30})],
        ));
        let _ = self.live.send(sub("lanes", vec![json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_SECTIONS, sections::LANE_STARS], "limit": 0})]));
        // Read positions from Desktop and our other devices, as they move.
        let _ = self.live.send(sub(
            "readstate",
            vec![json!({"kinds": [30078], "authors": [me], "#t": ["read-state"], "since": now, "limit": 0})],
        ));
        let r = self.relay.clone();
        let who = me.clone();
        self.spawn(async move {
            // No `t` filter: NIP-RS says a full load must not rely on it.
            Msg::ReadState(
                r.query(&[json!({"kinds": [30078], "authors": [who], "limit": 500})])
                    .await,
            )
        });
        let _ = self.live.send(sub(
            "membership",
            vec![json!({"kinds": [44100, 44101], "#p": [me], "since": now, "limit": 50})],
        ));
        // Managed agents you own (Desktop's agent list).
        let r = self.relay.clone();
        self.spawn(async move {
            Msg::Events(
                r.query(&[json!({"kinds": [30177], "authors": [me], "limit": 200})])
                    .await
                    .unwrap_or_default(),
            )
        });
    }

    pub fn load_channels(&self) {
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

    pub fn load_lanes(&self) {
        let r = self.relay.clone();
        let me = self.me();
        self.spawn(async move {
            let ev = r
                .query(&[
                    json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_SECTIONS], "limit": 1}),
                    json!({"kinds": [30078], "authors": [me], "#d": [sections::LANE_STARS], "limit": 1}),
                ])
                .await;
            Msg::Lanes(ev)
        });
    }

    /// Decrypt and fold sidebar lane events. Returns true if anything changed.
    pub fn apply_lanes(&mut self, evs: &[Event]) -> bool {
        let mut changed = false;
        for ev in evs {
            if ev.pubkey != self.relay.keys.public_key() {
                continue;
            }
            let Ok(plain) = nip44::decrypt(self.relay.keys.secret_key(), &ev.pubkey, &ev.content)
            else {
                continue;
            };
            let Ok(doc) = serde_json::from_str::<Value>(&plain) else {
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

    /// Fold read-state events. Returns true if a position moved.
    pub fn apply_read(&self, read: &mut ReadState, evs: &[Event]) -> bool {
        let mut moved = false;
        for ev in evs {
            moved |= read.apply(&self.relay.keys, ev);
        }
        moved
    }

    /// Publish our read-state slot once it has settled (or now, on quit).
    pub fn flush_read(&mut self, read: &mut ReadState) {
        if let Some(ev) = read.take_publish(&self.relay.keys, false) {
            let r = self.relay.clone();
            tokio::spawn(async move {
                if let Err(e) = r.publish(&ev).await {
                    crate::log::line(format!("read state: {e}"));
                }
            });
        }
    }

    pub async fn flush_read_now(&mut self, read: &mut ReadState) {
        if let Some(ev) = read.take_publish(&self.relay.keys, true) {
            let _ = tokio::time::timeout(Duration::from_secs(3), self.relay.publish(&ev)).await;
        }
    }

    /// Look up the last week of messages and thread replies in each tab we
    /// haven't checked yet: what's after its read position is unread, and
    /// the replies feed "(N new)" and the jump key.
    pub fn check_unread(&mut self, read: &ReadState, tabs: &[String]) {
        if !read.tried {
            return;
        }
        for id in tabs {
            if !self.unread_checked.insert(id.clone()) {
                continue;
            }
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

    /// Live tail for a channel. Kept open for every tab so unread dots work.
    pub fn subscribe_channel(&mut self, id: &str) {
        if !self.subscribed.insert(id.to_string()) {
            return;
        }
        let since = now().min(self.started);
        let _ = self.live.send(Cmd::Sub {
            id: format!("ch:{id}"),
            filters: vec![
                json!({"kinds": model::LIVE_KINDS, "#h": [id], "since": since, "limit": 1000}),
            ],
        });
    }

    pub fn unsubscribe_others(&mut self, keep: &[String]) {
        let drop: Vec<String> = self
            .subscribed
            .iter()
            .filter(|s| !keep.contains(s))
            .cloned()
            .collect();
        for id in drop {
            self.subscribed.remove(&id);
            let _ = self.live.send(Cmd::Close {
                id: format!("ch:{id}"),
            });
        }
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

    pub fn open_channel(&mut self, id: &str) {
        self.subscribe_channel(id);
        if self.opened.insert(id.to_string()) {
            self.window(id, None);
        }
    }

    pub fn load_older(&mut self, id: &str) {
        if let Some(Some(cursor)) = self.cursors.get(id) {
            if self.loading_older.insert(id.to_string()) {
                self.window(id, Some(cursor.clone()));
            }
        }
    }

    pub fn window_done(&mut self, id: &str) {
        self.loading_older.remove(id);
    }

    /// After a reconnect the live tail replays from `since`, but a channel
    /// opened before the drop also refetches its window, as Desktop does.
    pub fn refetch_open(&mut self) {
        for id in self.opened.clone() {
            self.window(&id, None);
        }
    }

    pub fn open_thread(&self, channel: &str, root: &str) {
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
            Msg::Thread { events: res }
        });
    }

    /// Fetch kind 0 for any pubkeys we haven't asked about yet.
    pub fn want_profiles(&mut self, pubkeys: impl IntoIterator<Item = String>) {
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
                        .await
                        .unwrap_or_default(),
                )
            });
        }
    }

    pub fn want_presence(&self, agents: Vec<String>) {
        if agents.is_empty() {
            return;
        }
        let r = self.relay.clone();
        let _ = self.live.send(Cmd::Sub {
            id: "presence".into(),
            filters: vec![json!({"kinds": [20001], "authors": agents, "limit": 0})],
        });
        self.spawn(async move {
            Msg::Events(
                r.query(&[json!({"kinds": [20001], "authors": agents, "limit": 200})])
                    .await
                    .unwrap_or_default(),
            )
        });
    }

    pub fn heartbeat(&self) {
        if let Ok(b) = buzz_sdk::build_presence_update("online") {
            if let Ok(ev) = self.relay.sign(b) {
                let _ = self.live.send(Cmd::Publish(Box::new(ev)));
            }
        }
    }

    /// Decrypt an observer frame or turn metric addressed to us.
    pub fn decrypt_for_me(&self, ev: &Event) -> Option<Value> {
        let plain = nip44::decrypt(self.relay.keys.secret_key(), &ev.pubkey, &ev.content).ok()?;
        serde_json::from_str(&plain).ok()
    }

    // ---------- writes ----------

    pub fn run(&mut self, app: &mut App, action: Action) {
        match action {
            Action::OpenChannel(id) => {
                app.read.mark(&id, now());
                self.open_channel(&id);
            }
            Action::LoadOlder(id) => self.load_older(&id),
            Action::ReloadSections => {
                app.say("reloading sections…");
                self.load_channels();
                self.load_lanes();
            }
            Action::MoveChannel { channel, section } => {
                let name = |id: &str| {
                    app.sections
                        .iter()
                        .find(|s| s.id == id)
                        .map_or(id.to_string(), |s| s.name.clone())
                };
                let label = format!(
                    "moved #{} to {}",
                    app.store
                        .channels
                        .get(&channel)
                        .map_or("?", |c| c.name.as_str()),
                    name(section.as_deref().unwrap_or("channels"))
                );
                app.say("moving…");
                let r = self.relay.clone();
                self.spawn(async move {
                    Msg::Moved(
                        sections::publish_move(&r, &channel, section.as_deref())
                            .await
                            .map(|ev| (ev, label)),
                    )
                });
            }
            Action::Send {
                channel,
                root,
                parent,
            } => self.send(app, channel, root, parent),
            Action::ThumbsUp { target } => self.thumbs_up(app, target),
            Action::Upload => {
                let Some(p) = app.pending.take() else { return };
                let Some(target) = app.draft_target() else {
                    return;
                };
                app.uploading = true;
                let r = self.relay.clone();
                self.spawn(async move { Msg::Uploaded(r.upload(p.bytes, &p.mime).await, target) });
            }
            Action::LoadImage(url) => self.load_image(app, url),
            Action::OpenThread { channel, root } => self.open_thread(&channel, &root),
            Action::OpenImages(urls) => self.open_images(app, urls),
            Action::OpenUrl(url) => {
                if let Err(e) = crate::links::open(&url) {
                    app.say(format!("couldn't open {url}: {e}"));
                }
            }
            Action::ReadAttachment(path) => self.spawn(async move {
                let label = path.as_ref().map(|p| p.display().to_string());
                let result = tokio::task::spawn_blocking(move || match path {
                    Some(p) => crate::clip::from_file(&p, "dropped file"),
                    None => crate::clip::from_clipboard(),
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r);
                Msg::Attachment { label, result }
            }),
            Action::PreviewPending => {
                let Some(p) = &app.pending else { return };
                if !p.mime.starts_with("image/") || app.picker.is_none() {
                    return;
                }
                let url = p.preview_key();
                let bytes = p.bytes.clone();
                app.media
                    .images
                    .insert(url.clone(), crate::media::Slot::Loading);
                self.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || crate::media::decode(&bytes))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|r| r);
                    Msg::Image { url, result }
                });
            }
            Action::EncodeImage(key) => self.encode(app, key),
            Action::SaveState | Action::Quit => {}
        }
    }

    fn send(
        &mut self,
        app: &mut App,
        channel: String,
        root: Option<String>,
        parent: Option<String>,
    ) {
        let draft = std::mem::take(app.draft_mut());
        let mut content = draft.text.trim_end().to_string();
        for a in &draft.attachments {
            let label = if a.mime_type.starts_with("video/") {
                "video"
            } else {
                "image"
            };
            content.push_str(&format!("\n![{label}]({})", a.url));
        }
        let content = content.trim_start_matches('\n').to_string();
        let mut mentions = draft.mention_pubkeys();
        // In a DM everyone else is notified, as Desktop does.
        if let Some(ch) = app.store.channels.get(&channel).filter(|c| c.is_dm) {
            for m in &ch.members {
                if *m != app.store.me && !mentions.contains(m) {
                    mentions.push(m.clone());
                }
            }
        }
        let thread = match (root.clone(), parent) {
            (Some(r), Some(p)) => match (EventId::from_hex(&r), EventId::from_hex(&p)) {
                (Ok(r), Ok(p)) => Some(buzz_sdk::ThreadRef {
                    root_event_id: r,
                    parent_event_id: p,
                }),
                _ => None,
            },
            _ => None,
        };
        let Ok(uuid) = uuid::Uuid::parse_str(&channel) else {
            *app.draft_mut() = draft;
            app.say("bad channel id");
            return;
        };
        let refs: Vec<&str> = mentions.iter().map(String::as_str).collect();
        let media: Vec<Vec<String>> = draft
            .attachments
            .iter()
            .map(BlobDescriptor::imeta_tag)
            .collect();
        // Unchanged since a send that failed: the same event again.
        let again = draft
            .sent_as
            .as_ref()
            .filter(|s| s.channel == channel && s.root == root && s.content == content)
            .map(|s| Ok(*s.event.clone()));
        let built =
            buzz_sdk::build_message(uuid, &content, thread.as_ref(), &refs, false, &media, &[])
                .map(|b| {
                    // Non-notifying label refs so Desktop renders mention chips.
                    b.tags(
                        draft
                            .mention_pubkeys()
                            .iter()
                            .filter_map(|pk| Tag::parse(["mention", pk.as_str()]).ok()),
                    )
                });
        let ev = match again.unwrap_or_else(|| {
            built
                .map_err(anyhow::Error::from)
                .and_then(|b| self.relay.sign(b))
        }) {
            Ok(ev) => ev,
            Err(e) => {
                app.say(format!("couldn't build message: {e}"));
                *app.draft_mut() = draft;
                return;
            }
        };
        // Show it right away; the live tail dedupes the echo.
        app.store.apply(&ev);
        let id = ev.id.to_hex();
        let mut draft = draft;
        draft.sent_as = Some(crate::draft::SentAs {
            channel: channel.clone(),
            root: root.clone(),
            content,
            event: Box::new(ev.clone()),
        });
        app.pending_sends
            .insert(id.clone(), (channel.clone(), root.clone(), draft));
        let r = self.relay.clone();
        self.spawn(async move {
            let result = r.publish(&ev).await.map(|_| ());
            Msg::Sent { id, result }
        });
    }

    fn thumbs_up(&mut self, app: &mut App, target: String) {
        let Ok(id) = EventId::from_hex(&target) else {
            return;
        };
        let mine = app.store.my_reaction(&target, model::THUMBS_UP);
        let built = match &mine {
            Some(rid) => EventId::from_hex(rid)
                .map_err(anyhow::Error::from)
                .and_then(|r| Ok(buzz_sdk::build_remove_reaction(r)?)),
            None => buzz_sdk::build_reaction(id, model::THUMBS_UP).map_err(anyhow::Error::from),
        };
        let ev = match built.and_then(|b| self.relay.sign(b)) {
            Ok(ev) => ev,
            Err(e) => return app.say(format!("couldn't react: {e}")),
        };
        app.store.apply(&ev);
        let label = if mine.is_some() {
            "removed 👍"
        } else {
            "👍"
        };
        let r = self.relay.clone();
        self.spawn(async move { Msg::Did(r.publish(&ev).await.map(|_| label.to_string())) });
    }

    /// Save images to temp files and hand them to the OS in one go, so a
    /// Mac shows them in one Preview window. Reuses what the viewer already
    /// downloaded.
    fn open_images(&mut self, app: &mut App, urls: Vec<String>) {
        let (ours, theirs): (Vec<_>, Vec<_>) =
            urls.into_iter().partition(|u| self.relay.is_ours(u));
        for u in theirs {
            if let Err(e) = crate::links::open(&u) {
                app.say(format!("couldn't open {u}: {e}"));
            }
        }
        if ours.is_empty() {
            return;
        }
        let n = ours.len();
        app.say(if n == 1 {
            "opening image…".to_string()
        } else {
            format!("opening {n} images…")
        });
        let have: Vec<Option<Arc<Vec<u8>>>> = ours
            .iter()
            .map(|u| app.media.bytes.get(u).cloned())
            .collect();
        let r = self.relay.clone();
        self.spawn(async move {
            let res = async {
                let files = futures::future::join_all(ours.iter().zip(have).map(|(u, b)| {
                    let r = r.clone();
                    async move {
                        match b {
                            Some(b) => Ok(b),
                            None => r.download(u).await.map(Arc::new),
                        }
                    }
                }))
                .await;
                // A fresh folder each time, numbered in thread order, so
                // Preview's sidebar reads top to bottom like the thread.
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let dir = std::env::temp_dir()
                    .join("buzz-vim")
                    .join(stamp.to_string());
                std::fs::create_dir_all(&dir)?;
                let mut paths = vec![];
                let mut failed = 0;
                for (i, (url, bytes)) in ours.iter().zip(files).enumerate() {
                    let Ok(bytes) = bytes else {
                        failed += 1;
                        continue;
                    };
                    let name = url.rsplit('/').next().unwrap_or("image");
                    let stem = name.split('.').next().unwrap_or(name);
                    let short = &stem[stem.len().saturating_sub(8)..];
                    let ext = image::guess_format(&bytes)
                        .ok()
                        .and_then(|f| f.extensions_str().first().copied())
                        .unwrap_or("png");
                    let path = dir.join(format!("{:02}-{short}.{ext}", i + 1));
                    std::fs::write(&path, &*bytes)?;
                    paths.push(path);
                }
                if paths.is_empty() {
                    anyhow::bail!("couldn't download any of them");
                }
                crate::links::open_files(&paths)?;
                Ok(match (paths.len(), failed) {
                    (1, 0) => format!("opened {}", paths[0].display()),
                    (k, 0) => format!("opened {k} images in Preview"),
                    (k, f) => format!("opened {k} images in Preview; {f} didn't download"),
                })
            }
            .await;
            Msg::Did(res)
        });
    }

    fn load_image(&mut self, app: &mut App, url: String) {
        if app.media.images.contains_key(&url) || app.picker.is_none() {
            return;
        }
        // Only send our Blossom auth to our own relay.
        if !self.relay.is_ours(&url) {
            app.media
                .images
                .insert(url, crate::media::Slot::Failed("external image".into()));
            return;
        }
        app.media
            .images
            .insert(url.clone(), crate::media::Slot::Loading);
        let r = self.relay.clone();
        let tx = self.tx.clone();
        let slots = Arc::clone(&self.image_slots);
        self.spawn(async move {
            let result = async {
                let _slot = slots.acquire_owned().await?;
                // Report every 128 KB or so; each one redraws the viewer.
                let mut shown = 0;
                let bytes = r
                    .download_progress(&url, |got, total| {
                        if got >= shown + (128 << 10) || Some(got) == total {
                            shown = got;
                            let _ = tx.send(Msg::Progress {
                                url: url.clone(),
                                got,
                                total,
                            });
                        }
                    })
                    .await?;
                drop(_slot);
                let bytes = Arc::new(bytes);
                let _ = tx.send(Msg::Bytes {
                    url: url.clone(),
                    bytes: Arc::clone(&bytes),
                });
                tokio::task::spawn_blocking(move || crate::media::decode(&bytes)).await?
            }
            .await;
            Msg::Image { url, result }
        });
    }

    pub fn encode(&mut self, app: &mut App, key: (String, u16, u16)) {
        let Some(picker) = app.picker.clone() else {
            return;
        };
        let img = match app.media.images.get(&key.0) {
            Some(crate::media::Slot::Ready(img)) => Arc::clone(img),
            Some(_) => return,
            None => return self.load_image(app, key.0.clone()),
        };
        if !app.media.encoding.insert(key.clone()) {
            return;
        }
        self.spawn(async move {
            let k = key.clone();
            let result =
                tokio::task::spawn_blocking(move || crate::media::encode(&picker, &img, k.1, k.2))
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|r| r);
            Msg::Encoded { key, result }
        });
    }
}

/// Fold one live or queried event into the app. Returns pubkeys we should
/// look up profiles for.
pub fn fold(app: &mut App, sync: &mut Sync, ev: &Event, live_channel: bool) -> Vec<String> {
    let kind = ev.kind.as_u16();
    match kind {
        24200 => {
            if model::tag(ev, "agent") == Some(&ev.pubkey.to_hex()) {
                if let Some(frame) = sync.decrypt_for_me(ev) {
                    app.agents.frame(&ev.pubkey.to_hex(), &frame);
                    return vec![ev.pubkey.to_hex()];
                }
            }
            return vec![];
        }
        44200 => {
            // The agent signs its own metrics (buzz-acp pool.rs), like 24200.
            let agent = ev.pubkey.to_hex();
            if model::tag(ev, "agent").is_none_or(|a| a == agent) {
                if let Some(m) = sync.decrypt_for_me(ev) {
                    app.agents.metric(&agent, &m);
                    return vec![agent];
                }
            }
            return vec![];
        }
        30177 => {
            // Managed agent you own: d = agent pubkey.
            if let Some(pk) = model::tag(ev, "d").filter(|d| d.len() == 64) {
                let name = serde_json::from_str::<serde_json::Value>(&ev.content)
                    .ok()
                    .and_then(|v| v.get("name")?.as_str().map(str::to_string))
                    .unwrap_or_default();
                app.agents.own(pk, &name);
                return vec![pk.to_string()];
            }
            return vec![];
        }
        20001 => {
            app.agents.presence(&ev.pubkey.to_hex(), &ev.content);
            return vec![];
        }
        30078 if readstate::is_read_state(ev) => {
            if sync.apply_read(&mut app.read, std::slice::from_ref(ev)) {
                prune_unread(app);
            }
            return vec![];
        }
        30078 => {
            if sync.apply_lanes(std::slice::from_ref(ev)) {
                app.sections = sections::build(&sync.lanes, &app.store);
                app.rebuild_tabs();
            }
            return vec![];
        }
        44100 | 44101 => {
            sync.load_channels();
            return vec![];
        }
        _ => {}
    }
    let applied = app.store.apply(ev);
    let mut want = vec![ev.pubkey.to_hex()];
    if let Applied::Message { channel, id } = applied {
        let active = app.current() == Some(channel.as_str());
        let at = ev.created_at.as_secs();
        let author = ev.pubkey.to_hex();
        let seen = app.read.read_at(&channel).is_some_and(|r| at <= r);
        if live_channel && !seen && counts_unread(kind, &author, &app.store.me, active) {
            let me = app.store.me.clone();
            let ch = app.store.channel_mut(&channel);
            ch.unread.insert(id.clone(), at);
            if ch
                .messages
                .get(&id)
                .is_some_and(|m| m.mentions.contains(&me))
            {
                ch.mentioned.insert(id.clone());
            }
        } else if live_channel && active && author != app.store.me {
            // You're looking at it, so it's read.
            app.read.mark(&channel, now());
        }
        // A reply landing in the thread on screen is seen too.
        let root = app
            .store
            .channels
            .get(&channel)
            .and_then(|c| c.messages.get(&id))
            .and_then(|m| m.root.clone());
        if live_channel
            && root.is_some()
            && app.thread.as_ref().map(|t| (&t.0, Some(&t.1))) == Some((&channel, root.as_ref()))
        {
            app.mark_thread(root.as_deref().unwrap_or_default());
        }
        if let Some(m) = app
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

/// Drop unread messages at or before each channel's read position, e.g.
/// after you read the channel in Desktop.
pub fn prune_unread(app: &mut App) {
    let read = &app.read;
    for (id, ch) in app.store.channels.iter_mut() {
        if let Some(r) = read.read_at(id) {
            ch.unread.retain(|_, at| *at > r);
            let left = &ch.unread;
            ch.mentioned.retain(|m| left.contains_key(m));
        }
    }
}

/// Unread messages from a lookup: someone else's, after the read
/// position, in a tab you aren't looking at.
pub fn fold_unread(app: &mut App, channel: &str, evs: &[Event]) {
    if app.current() == Some(channel) {
        return;
    }
    let me = app.store.me.clone();
    let after = app.read.read_at(channel).unwrap_or(0);
    let ch = app.store.channel_mut(channel);
    for ev in evs {
        let at = ev.created_at.as_secs();
        if model::UNREAD_KINDS.contains(&ev.kind.as_u16()) && ev.pubkey.to_hex() != me && at > after
        {
            ch.unread.insert(ev.id.to_hex(), at);
            if ch.is_dm || model::Message::from_event(ev).mentions.contains(&me) {
                ch.mentioned.insert(ev.id.to_hex());
            }
        }
    }
}

/// A live message marks its tab unread when it's a real message from
/// someone else in a tab you aren't looking at. Thread replies count, as in
/// Desktop: most agent answers land in threads.
fn counts_unread(kind: u16, author: &str, me: &str, active: bool) -> bool {
    model::UNREAD_KINDS.contains(&kind) && author != me && !active
}

#[cfg(test)]
mod tests {
    use super::counts_unread;

    #[test]
    fn unread_rules() {
        assert!(counts_unread(9, "them", "me", false));
        assert!(!counts_unread(9, "me", "me", false));
        assert!(!counts_unread(9, "them", "me", true));
        assert!(!counts_unread(40099, "them", "me", false));
    }
}
