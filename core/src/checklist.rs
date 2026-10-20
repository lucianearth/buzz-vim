//! The operator loop's checklist records, read-only: a channel's header
//! (kind 30471), its items (30470) and the watcher's state (30472), all
//! addressable and channel-scoped by `h`. Agents write them with
//! `buzz checklist`; both apps only fold and show them.
//!
//! Ownership (NIP-CK reader rule v3): the creator is part of the address
//! (`buzz-checklist/header/<channel>/<creator-hex>`, items under
//! `.../item/<channel>/<creator-hex>/<item-id>`). Per address, the newest
//! version signed by that creator or by the owner counts; other signers are
//! ignored. The owner's newest header picks the driver; with none, one list
//! drives and two or more are contested. Watch records count only from the
//! watcher's keys. The vectors in `core/tests/nip-ck-vectors.json` pin it.

use std::collections::{BTreeMap, HashMap};

use nostr::Event;
use serde::Deserialize;
use serde_json::Value;

use crate::model::tag;

pub const KIND_ITEM: u16 = 30470;
pub const KIND_HEADER: u16 = 30471;
pub const KIND_WATCH: u16 = 30472;
pub const KINDS: &[u16] = &[KIND_ITEM, KIND_HEADER, KIND_WATCH];
/// The `t` tag every record carries.
pub const TAG: &str = "buzz-checklist";
/// lucian: accepted on every record, and he picks the driver (default,
/// overridable).
pub const DEFAULT_OWNER: &str = "5c017fc7a0525e163d44fae1b4e853ad236e690c4f607fd2c603088d854bc36a";
/// The watcher's key, the only one accepted on watch records by default:
/// the fleet-skills seat, which aei's buzz-watch and stall-judge sign with.
/// `watcher` in the config (or `BUZZ_VIM_WATCHER`) overrides it.
pub const DEFAULT_WATCHER: &str =
    "d84ddba92b7e936304ec9ea81c9b59594b9f974dd0ebeb0aa2227fe7d54d70d4";
pub const WATCHERS: &[&str] = &[DEFAULT_WATCHER];
/// What buzz-acp's notice says when it respawns a crashed agent process.
pub const CRASH_NEEDLE: &str = "agent process crashed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    Done,
    Parked,
}

impl Status {
    fn parse(s: &str) -> Self {
        match s {
            "done" => Status::Done,
            "parked" => Status::Parked,
            _ => Status::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Done => "done",
            Status::Parked => "parked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ideating,
    Running,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ideating => "ideating",
            Mode::Running => "running",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchState {
    Working,
    Stalled,
    WaitingOnLucian,
    WaitingOnAgent,
    WaitingOnRelease,
    Idle,
    /// A state this build doesn't know, shown as written.
    Other(String),
}

impl WatchState {
    fn parse(s: &str) -> Self {
        match s {
            "working" => WatchState::Working,
            "stalled" => WatchState::Stalled,
            "waiting_on_lucian" => WatchState::WaitingOnLucian,
            "waiting_on_agent" => WatchState::WaitingOnAgent,
            "waiting_on_release" => WatchState::WaitingOnRelease,
            "idle" => WatchState::Idle,
            o => WatchState::Other(o.to_string()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            WatchState::Working => "working".into(),
            WatchState::Stalled => "stalled".into(),
            WatchState::WaitingOnLucian => "waiting on lucian".into(),
            WatchState::WaitingOnAgent => "waiting on agent".into(),
            WatchState::WaitingOnRelease => "waiting on release".into(),
            WatchState::Idle => "idle".into(),
            WatchState::Other(s) => s.replace('_', " "),
        }
    }

    /// Needs the operator: stalled, or waiting on him.
    pub fn needs_you(&self) -> bool {
        matches!(
            self,
            WatchState::Stalled | WatchState::WaitingOnLucian | WatchState::WaitingOnRelease
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The stable 8-hex id from the `d` tag.
    pub id: String,
    pub text: String,
    pub details: String,
    pub status: Status,
    pub release: bool,
    pub order: f64,
    /// "first" / "last" for the two standing items.
    pub standing: Option<String>,
    pub evidence: Vec<String>,
    pub parked_question: Option<String>,
    pub creator: String,
    /// Who signed the version shown.
    pub signer: String,
    /// The version shown.
    pub event_id: String,
    pub updated_at: u64,
}

impl Item {
    /// Done with nothing to show for it: the spec requires evidence.
    pub fn missing_evidence(&self) -> bool {
        self.status == Status::Done && self.evidence.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resume {
    pub worktree: String,
    pub branch: String,
    pub worklog: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub title: String,
    pub mode: Mode,
    pub go: Option<String>,
    pub resume: Resume,
    /// The driver: the creator in the header's address.
    pub creator: String,
    /// The version shown.
    pub event_id: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub state: WatchState,
    pub since: Option<u64>,
    /// Root of the work's thread.
    pub thread: Option<String>,
    /// The ping to lucian, if one is out.
    pub ping: Option<String>,
    pub nudges: u32,
    pub reason: String,
    pub updated_at: u64,
    /// When this run of versions in the same state began, as far as we
    /// have seen: a nudge republish keeps it, a new stall starts it again.
    pub began: u64,
    /// The version shown.
    pub event_id: String,
}

/// Who a reader trusts.
#[derive(Debug, Clone)]
pub struct Trust {
    pub owner: String,
    /// The only keys whose watch records count.
    pub watchers: Vec<String>,
}

impl Default for Trust {
    fn default() -> Self {
        Trust {
            owner: DEFAULT_OWNER.to_string(),
            watchers: WATCHERS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// One signed version of a record, before ownership is applied.
#[derive(Debug, Clone)]
struct Version {
    signer: String,
    at: u64,
    id: String,
    content: Value,
    /// Watch records: when this signer's run of versions in the same
    /// state began (seen here; the relay keeps only the newest).
    began: u64,
}

impl Version {
    /// Newer first; same second: the lower id wins, as NIP-01 says.
    fn newer_than(&self, o: &Version) -> bool {
        (self.at, std::cmp::Reverse(&self.id)) > (o.at, std::cmp::Reverse(&o.id))
    }
}

/// The newer of two versions.
fn newest<'a>(a: Option<&'a Version>, b: Option<&'a Version>) -> Option<&'a Version> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.newer_than(a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// One address's versions: signer -> that signer's newest.
type Slot = BTreeMap<String, Version>;

/// The version a reader takes at an address: the newest signed by the
/// address's creator or by the owner. Every other signer is ignored.
fn accepted<'a>(slot: &'a Slot, creator: &str, owner: &str) -> Option<&'a Version> {
    newest(slot.get(creator), slot.get(owner))
}

/// 64 lowercase hex: a creator in a `d`.
fn is_key(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 8 lowercase hex: an item id.
fn is_item_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where a record lives, from its canonical `d`.
#[derive(Debug, PartialEq, Eq)]
enum Addr<'a> {
    Header { creator: &'a str },
    Item { creator: &'a str, id: &'a str },
    Watch,
}

/// A record's channel and address, or None unless the envelope is valid:
/// one of our kinds, `t` and `checklist-type` right for it, `h` ==
/// `channel` == the d's channel, and a canonical `d` (no case folding, no
/// npubs, no missing or extra segments).
fn address(ev: &Event) -> Option<(&str, Addr<'_>)> {
    let ty = match ev.kind.as_u16() {
        KIND_ITEM => "item",
        KIND_HEADER => "header",
        KIND_WATCH => "watch",
        _ => return None,
    };
    if tag(ev, "t") != Some(TAG) || tag(ev, "checklist-type") != Some(ty) {
        return None;
    }
    let h = tag(ev, "h").filter(|h| !h.is_empty())?;
    if tag(ev, "channel") != Some(h) {
        return None;
    }
    let p: Vec<&str> = tag(ev, "d")?.split('/').collect();
    if p.len() < 3 || p[0] != TAG || p[1] != ty || p[2] != h {
        return None;
    }
    let addr = match (ty, &p[3..]) {
        ("watch", []) => Addr::Watch,
        ("header", [c]) if is_key(c) => Addr::Header { creator: c },
        ("item", [c, id]) if is_key(c) && is_item_id(id) => Addr::Item { creator: c, id },
        _ => return None,
    };
    Some((h, addr))
}

/// Who drives a channel's list (NIP-CK rule v3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Driver {
    /// The list's creator, whose header and items show.
    pub driver: Option<String>,
    /// Two or more lists claim the channel and the owner hasn't picked:
    /// their creators, sorted. No header or items show then.
    pub contested: Vec<String>,
    /// Contested, and at least one of those lists is running.
    pub contested_running: bool,
    /// Contested: the newest of those lists' header versions (0 if not).
    pub since: u64,
}

#[derive(Debug, Default)]
struct ChannelRecords {
    /// Header addresses: creator -> signer -> newest.
    headers: BTreeMap<String, Slot>,
    /// Item addresses: (creator, item id) -> signer -> newest.
    items: BTreeMap<(String, String), Slot>,
    watch: Slot,
    /// Crash notices seen here: message id -> created_at.
    crashes: HashMap<String, u64>,
}

/// Every channel's checklist records, folded as they arrive.
#[derive(Debug, Default)]
pub struct Checklists {
    pub trust: Trust,
    channels: HashMap<String, ChannelRecords>,
    /// The judge's routing logs (30472, `checklist-type=routing`).
    routing: crate::routing::Logs,
}

/// What the status line shows for one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub title: Option<String>,
    pub mode: Option<Mode>,
    pub open: usize,
    pub done: usize,
    pub watch: Option<Watch>,
    /// `buzz://` link to the ping, when one is out.
    pub ping_link: Option<String>,
    pub crashes_today: usize,
    /// Two or more lists claim the channel: their creators.
    pub contested: Vec<String>,
    pub contested_running: bool,
}

impl Summary {
    /// Nothing to show: no list, no contest, no watcher, no crashes.
    pub fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.contested.is_empty()
            && self.open + self.done == 0
            && self.watch.is_none()
            && self.crashes_today == 0
    }
}

/// Read one content field leniently (NIP-CK content reading): a value of the
/// wrong type reads as the field's default instead of blanking the record.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(serde_json::from_value(Value::deserialize(d)?).unwrap_or_default())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ItemJson {
    #[serde(deserialize_with = "lenient")]
    text: String,
    #[serde(deserialize_with = "lenient")]
    details: String,
    #[serde(deserialize_with = "lenient")]
    status: String,
    #[serde(deserialize_with = "lenient")]
    release: bool,
    #[serde(deserialize_with = "lenient")]
    order: f64,
    #[serde(deserialize_with = "lenient")]
    standing: Option<String>,
    #[serde(deserialize_with = "lenient")]
    evidence: Vec<String>,
    #[serde(deserialize_with = "lenient")]
    parked_question: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HeaderJson {
    #[serde(deserialize_with = "lenient")]
    title: String,
    #[serde(deserialize_with = "lenient")]
    mode: String,
    #[serde(deserialize_with = "lenient")]
    go: Option<String>,
    #[serde(deserialize_with = "lenient")]
    resume: ResumeJson,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ResumeJson {
    #[serde(deserialize_with = "lenient")]
    worktree: String,
    #[serde(deserialize_with = "lenient")]
    branch: String,
    #[serde(deserialize_with = "lenient")]
    worklog: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WatchJson {
    state: String,
    since: Option<u64>,
    thread: Option<String>,
    ping: Option<String>,
    nudges: u32,
    reason: String,
}

/// The link to a message: in its thread when it is a reply there.
pub fn message_link(channel: &str, id: &str, thread: Option<&str>) -> String {
    match thread.filter(|t| *t != id) {
        Some(t) => format!("buzz://message?channel={channel}&id={id}&thread={t}"),
        None => format!("buzz://message?channel={channel}&id={id}"),
    }
}

/// Unix time of the last local midnight at or before `now`.
pub fn local_midnight(now: u64) -> u64 {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(now as i64, 0).single() else {
        return now - now % 86_400;
    };
    t.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|m| Local.from_local_datetime(&m).earliest())
        .map_or(now - now % 86_400, |m| m.timestamp().max(0) as u64)
}

pub fn is_crash_notice(content: &str) -> bool {
    content.to_lowercase().contains(CRASH_NEEDLE)
}

impl Checklists {
    pub fn new(trust: Trust) -> Self {
        Checklists {
            trust,
            ..Checklists::default()
        }
    }

    /// REQ filters for one channel's records.
    pub fn filters(channel: &str) -> Vec<Value> {
        vec![serde_json::json!({"kinds": KINDS, "#h": [channel], "limit": 500})]
    }

    /// Fold one record. Returns true when it is a valid record and the
    /// newest seen from its signer at its address.
    pub fn apply(&mut self, ev: &Event) -> bool {
        if crate::routing::channel_of(ev).is_some() {
            return self.routing.apply(ev);
        }
        let Some((channel, addr)) = address(ev) else {
            return false;
        };
        // Content never decides selection: anything that is not a JSON
        // object reads as `{}` (NIP-CK content reading).
        let content = serde_json::from_str::<Value>(&ev.content)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default()));
        let at = ev.created_at.as_secs();
        let mut v = Version {
            signer: ev.pubkey.to_hex(),
            at,
            id: ev.id.to_hex(),
            content,
            began: at,
        };
        let recs = self.channels.entry(channel.to_string()).or_default();
        let slot = match addr {
            Addr::Header { creator } => recs.headers.entry(creator.to_string()).or_default(),
            Addr::Item { creator, id } => recs
                .items
                .entry((creator.to_string(), id.to_string()))
                .or_default(),
            Addr::Watch => &mut recs.watch,
        };
        match slot.get(&v.signer) {
            Some(old) if !v.newer_than(old) => false,
            old => {
                if let Some(o) = old
                    && o.content.get("state") == v.content.get("state")
                {
                    v.began = o.began;
                }
                slot.insert(v.signer.clone(), v);
                true
            }
        }
    }

    /// Who drives the channel's list: the creator the owner's newest header
    /// names; else the one list with an accepted header; two or more and
    /// it is contested (no first-come).
    pub fn driver(&self, channel: &str) -> Driver {
        let Some(recs) = self.channels.get(channel) else {
            return Driver::default();
        };
        self.driver_in(recs)
    }

    fn driver_in(&self, recs: &ChannelRecords) -> Driver {
        let owner = self.trust.owner.as_str();
        let picked = recs
            .headers
            .iter()
            .filter_map(|(c, slot)| Some((c, slot.get(owner)?)))
            .fold(None::<(&String, &Version)>, |best, (c, v)| match best {
                Some((_, b)) if !v.newer_than(b) => best,
                _ => Some((c, v)),
            });
        if let Some((c, _)) = picked {
            return Driver {
                driver: Some(c.clone()),
                ..Driver::default()
            };
        }
        let heads: Vec<(&String, &Version)> = recs
            .headers
            .iter()
            .filter_map(|(c, slot)| Some((c, accepted(slot, c, owner)?)))
            .collect();
        match heads.as_slice() {
            [] => Driver::default(),
            [(c, _)] => Driver {
                driver: Some((*c).clone()),
                ..Driver::default()
            },
            _ => Driver {
                driver: None,
                // BTreeMap order: sorted.
                contested: heads.iter().map(|(c, _)| (*c).clone()).collect(),
                contested_running: heads
                    .iter()
                    .any(|(_, v)| v.content.get("mode").and_then(Value::as_str) == Some("running")),
                since: heads.iter().map(|(_, v)| v.at).max().unwrap_or(0),
            },
        }
    }

    /// Channels we hold any records for.
    pub fn channels(&self) -> impl Iterator<Item = &str> {
        self.channels.keys().map(String::as_str)
    }

    /// Count a message if it is a crash notice. Returns true when new.
    pub fn note_message(&mut self, channel: &str, id: &str, at: u64, content: &str) -> bool {
        if !is_crash_notice(content) {
            return false;
        }
        self.channels
            .entry(channel.to_string())
            .or_default()
            .crashes
            .insert(id.to_string(), at)
            .is_none()
    }

    pub fn header(&self, channel: &str) -> Option<Header> {
        let recs = self.channels.get(channel)?;
        let creator = self.driver_in(recs).driver?;
        let v = accepted(recs.headers.get(&creator)?, &creator, &self.trust.owner)?;
        let j: HeaderJson = serde_json::from_value(v.content.clone()).unwrap_or_default();
        Some(Header {
            title: j.title,
            mode: if j.mode == "running" {
                Mode::Running
            } else {
                Mode::Ideating
            },
            go: j.go.filter(|g| !g.is_empty()),
            resume: Resume {
                worktree: j.resume.worktree,
                branch: j.resume.branch,
                worklog: j.resume.worklog,
            },
            creator,
            event_id: v.id.clone(),
            updated_at: v.at,
        })
    }

    /// Items in list order: the standing first item, then by `order`, then
    /// the standing last item.
    pub fn items(&self, channel: &str) -> Vec<Item> {
        let Some(recs) = self.channels.get(channel) else {
            return vec![];
        };
        // Only a list with a header has items: the driver's, and the owner's.
        let Some(driver) = self.driver_in(recs).driver else {
            return vec![];
        };
        let owner = self.trust.owner.as_str();
        let mut out: Vec<Item> = recs
            .items
            .iter()
            .filter(|((creator, _), _)| *creator == driver || creator == owner)
            .filter_map(|((creator, id), slot)| {
                let v = accepted(slot, creator, owner)?;
                let j: ItemJson = serde_json::from_value(v.content.clone()).unwrap_or_default();
                Some(Item {
                    id: id.clone(),
                    text: j.text,
                    details: j.details,
                    status: Status::parse(&j.status),
                    release: j.release,
                    order: j.order,
                    standing: j.standing.filter(|s| s == "first" || s == "last"),
                    evidence: j.evidence,
                    parked_question: j.parked_question.filter(|q| !q.is_empty()),
                    creator: creator.clone(),
                    signer: v.signer.clone(),
                    event_id: v.id.clone(),
                    updated_at: v.at,
                })
            })
            .collect();
        let rank = |i: &Item| match i.standing.as_deref() {
            Some("first") => 0,
            Some("last") => 2,
            _ => 1,
        };
        out.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then(a.order.total_cmp(&b.order))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    pub fn watch(&self, channel: &str) -> Option<Watch> {
        let recs = self.channels.get(channel)?;
        let w = &self.trust.watchers;
        let v = recs.watch.values().filter(|v| w.contains(&v.signer)).fold(
            None::<&Version>,
            |best, v| match best {
                Some(b) if !v.newer_than(b) => Some(b),
                _ => Some(v),
            },
        )?;
        let j: WatchJson = serde_json::from_value(v.content.clone()).unwrap_or_default();
        Some(Watch {
            state: WatchState::parse(&j.state),
            since: j.since,
            thread: j.thread.filter(|t| !t.is_empty()),
            ping: j.ping.filter(|p| !p.is_empty()),
            nudges: j.nudges,
            reason: j.reason,
            updated_at: v.at,
            began: v.began,
            event_id: v.id.clone(),
        })
    }

    /// Every channel's routing decisions from the watcher's logs, each with
    /// its channel's watch record.
    pub fn decisions(&self) -> Vec<crate::routing::Decision> {
        let mut ds = self.routing.decisions(&self.trust.watchers);
        for d in &mut ds {
            d.watch = self.watch(&d.channel).map(|w| w.event_id);
        }
        ds
    }

    pub fn crashes_since(&self, channel: &str, since: u64) -> usize {
        self.channels
            .get(channel)
            .map_or(0, |r| r.crashes.values().filter(|at| **at >= since).count())
    }

    /// The status line for a channel, with crashes counted from `today`.
    pub fn summary(&self, channel: &str, today: u64) -> Summary {
        let items = self.items(channel);
        let count = |s: Status| items.iter().filter(|i| i.status == s).count();
        let header = self.header(channel);
        let watch = self.watch(channel);
        let driver = self.driver(channel);
        let ping_link = watch.as_ref().and_then(|w| {
            let p = w.ping.as_deref()?;
            Some(message_link(channel, p, w.thread.as_deref()))
        });
        Summary {
            title: header
                .as_ref()
                .map(|h| h.title.clone())
                .filter(|t| !t.is_empty()),
            mode: header.map(|h| h.mode),
            // No parking (lucian 2026-10-20): a legacy parked item is open work.
            open: count(Status::Open) + count(Status::Parked),
            done: count(Status::Done),
            watch,
            ping_link,
            crashes_today: self.crashes_since(channel, today),
            contested: driver.contested,
            contested_running: driver.contested_running,
        }
    }
}

/// An evidence entry as a link where it can be one: `msg:` in this
/// channel, `url:` as is. Commits and PR events stay text.
pub fn evidence_link(channel: &str, e: &str) -> Option<String> {
    if let Some(id) = e.strip_prefix("msg:") {
        return Some(message_link(channel, id, None));
    }
    e.strip_prefix("url:").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

    const CH: &str = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";

    fn hex(k: &Keys) -> String {
        k.public_key().to_hex()
    }

    /// A record with a valid envelope at `d`, signed by `keys`.
    fn rec(keys: &Keys, kind: u16, d: &str, at: u64, content: Value) -> Event {
        rec_raw(keys, kind, d, at, &content.to_string())
    }

    /// [`rec`] with the content as written, JSON or not.
    fn rec_raw(keys: &Keys, kind: u16, d: &str, at: u64, content: &str) -> Event {
        let ty = match kind {
            KIND_ITEM => "item",
            KIND_HEADER => "header",
            _ => "watch",
        };
        let t = [
            ["d", d],
            ["h", CH],
            ["t", TAG],
            ["channel", CH],
            ["checklist-type", ty],
        ];
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(t.into_iter().map(|t| Tag::parse(t).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn head_d(creator: &str) -> String {
        format!("{TAG}/header/{CH}/{creator}")
    }

    fn item_d(creator: &str, id: &str) -> String {
        format!("{TAG}/item/{CH}/{creator}/{id}")
    }

    fn head(title: &str, mode: &str) -> Value {
        serde_json::json!({"v": 1, "title": title, "mode": mode})
    }

    fn item(text: &str, status: &str, order: f64) -> Value {
        serde_json::json!({"v": 1, "text": text, "status": status, "order": order,
            "evidence": if status == "done" { vec!["commit:abc"] } else { vec![] }})
    }

    fn store(owner: &Keys) -> Checklists {
        Checklists::new(Trust {
            owner: hex(owner),
            watchers: vec![],
        })
    }

    fn title(s: &Checklists) -> Option<String> {
        s.header(CH).map(|h| h.title)
    }

    #[test]
    fn newest_version_from_creator_or_owner_wins() {
        let (owner, bot, other) = (Keys::generate(), Keys::generate(), Keys::generate());
        let bk = hex(&bot);
        let mut s = store(&owner);
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            1,
            head("t", "ideating"),
        ));
        let d = item_d(&bk, "aaaa0001");
        assert!(s.apply(&rec(&bot, KIND_ITEM, &d, 10, item("first", "open", 1.0))));
        // Someone else's later version at the bot's address is ignored.
        s.apply(&rec(&other, KIND_ITEM, &d, 20, item("hijack", "done", 1.0)));
        assert_eq!(s.items(CH)[0].text, "first");
        // The owner's edit counts.
        s.apply(&rec(
            &owner,
            KIND_ITEM,
            &d,
            30,
            item("owner edit", "parked", 1.0),
        ));
        let i = &s.items(CH)[0];
        assert_eq!((i.text.as_str(), i.status), ("owner edit", Status::Parked));
        assert_eq!(
            (i.creator.as_str(), i.signer.clone()),
            (bk.as_str(), hex(&owner))
        );
        // Then the creator's newer one.
        s.apply(&rec(&bot, KIND_ITEM, &d, 40, item("ticked", "done", 1.0)));
        let i = &s.items(CH)[0];
        assert_eq!((i.text.as_str(), i.status), ("ticked", Status::Done));
        // An older version from the same signer doesn't replace a newer one.
        assert!(!s.apply(&rec(&bot, KIND_ITEM, &d, 35, item("stale", "open", 1.0))));
        assert_eq!(s.items(CH)[0].text, "ticked");
    }

    /// judgebot's takeover repro: the bot inits at t=100, X signs a header
    /// at the bot's own address at t=200, the bot edits at t=300. X never
    /// shows, whichever versions the relay kept.
    #[test]
    fn a_member_cannot_take_a_list_over() {
        let (owner, bot, x) = (Keys::generate(), Keys::generate(), Keys::generate());
        let (bk, xk) = (hex(&bot), hex(&x));
        let mut s = store(&owner);
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            100,
            head("bot's", "running"),
        ));
        s.apply(&rec(
            &x,
            KIND_HEADER,
            &head_d(&bk),
            200,
            head("x's", "running"),
        ));
        assert_eq!(title(&s).as_deref(), Some("bot's"));
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            300,
            head("bot's, edited", "running"),
        ));
        let h = s.header(CH).unwrap();
        assert_eq!(
            (h.title.as_str(), h.creator.as_str()),
            ("bot's, edited", bk.as_str())
        );
        // Pruned to each signer's newest, seen fresh: the same.
        let mut s = store(&owner);
        s.apply(&rec(
            &x,
            KIND_HEADER,
            &head_d(&bk),
            200,
            head("x's", "running"),
        ));
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            300,
            head("bot's, edited", "running"),
        ));
        assert_eq!(title(&s).as_deref(), Some("bot's, edited"));
        // Nor its items: X's flip of the bot's item, and X's own items.
        let d = item_d(&bk, "aaaa0001");
        s.apply(&rec(&bot, KIND_ITEM, &d, 100, item("mine", "open", 1.0)));
        s.apply(&rec(&x, KIND_ITEM, &d, 400, item("x's", "done", 1.0)));
        s.apply(&rec(
            &x,
            KIND_ITEM,
            &item_d(&xk, "aaaa0002"),
            400,
            item("x's own", "parked", 2.0),
        ));
        let texts: Vec<String> = s.items(CH).into_iter().map(|i| i.text).collect();
        assert_eq!(texts, ["mine"]);
        // X's own list, even backdated, only contests: no header, no items.
        s.apply(&rec(
            &x,
            KIND_HEADER,
            &head_d(&xk),
            50,
            head("x's", "running"),
        ));
        assert_eq!(s.header(CH), None);
        assert!(s.items(CH).is_empty());
        let mut both = vec![bk.clone(), xk.clone()];
        both.sort();
        assert_eq!(
            s.driver(CH),
            Driver {
                driver: None,
                contested: both.clone(),
                contested_running: true,
                since: 300,
            }
        );
        let sum = s.summary(CH, 0);
        assert_eq!((sum.contested, sum.contested_running), (both, true));
        assert!(!s.summary(CH, 0).is_empty());
        // The owner picks the bot: it drives again.
        s.apply(&rec(
            &owner,
            KIND_HEADER,
            &head_d(&bk),
            500,
            head("bot's", "running"),
        ));
        assert_eq!(s.driver(CH).driver.as_deref(), Some(bk.as_str()));
        assert_eq!(s.items(CH).len(), 1);
    }

    #[test]
    fn items_sort_with_standing_ones_at_the_ends() {
        let (owner, bot) = (Keys::generate(), Keys::generate());
        let bk = hex(&bot);
        let mut s = store(&owner);
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            1,
            head("t", "running"),
        ));
        let put = |s: &mut Checklists, id: &str, v: Value| {
            s.apply(&rec(&bot, KIND_ITEM, &item_d(&bk, id), 10, v));
        };
        put(
            &mut s,
            "00000003",
            serde_json::json!({"text": "wrap-up", "standing": "last", "order": 0}),
        );
        put(&mut s, "00000002", item("b", "open", 2.0));
        put(&mut s, "00000001", item("a", "done", 3.0));
        put(
            &mut s,
            "00000004",
            serde_json::json!({"text": "reconcile", "standing": "first", "order": 99}),
        );
        put(
            &mut s,
            "00000005",
            serde_json::json!({"text": "no evidence", "status": "done", "release": true, "order": 4}),
        );
        let texts: Vec<String> = s.items(CH).into_iter().map(|i| i.text).collect();
        assert_eq!(texts, ["reconcile", "b", "a", "no evidence", "wrap-up"]);
        let last = &s.items(CH)[3];
        assert!(last.release && last.missing_evidence());
    }

    #[test]
    fn odd_content_reads_field_by_field() {
        let (owner, bot) = (Keys::generate(), Keys::generate());
        let bk = hex(&bot);
        // An array would otherwise fill the fields in order.
        for odd in [
            "",
            "not json",
            "[1]",
            r#"["t","running"]"#,
            "{}",
            r#"{"mode":"paused","title":5}"#,
        ] {
            let mut s = store(&owner);
            s.apply(&rec_raw(&bot, KIND_HEADER, &head_d(&bk), 1, odd));
            let h = s.header(CH).expect(odd);
            assert_eq!(
                (h.title.as_str(), h.mode, h.go),
                ("", Mode::Ideating, None),
                "{odd}"
            );
        }
        let mut s = store(&owner);
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &head_d(&bk),
            1,
            serde_json::json!({"title": "t", "mode": "running", "go": 5,
                "resume": {"worktree": "w", "branch": 1}}),
        ));
        let h = s.header(CH).unwrap();
        assert_eq!((h.title.as_str(), h.mode, h.go), ("t", Mode::Running, None));
        assert_eq!(
            h.resume,
            Resume {
                worktree: "w".into(),
                ..Resume::default()
            }
        );
        s.apply(&rec_raw(&bot, KIND_ITEM, &item_d(&bk, "aaaa0001"), 2, ""));
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &item_d(&bk, "aaaa0002"),
            2,
            serde_json::json!({"text": 5, "details": "d", "status": "wip", "release": "yes",
                "order": "3", "standing": "middle", "evidence": ["commit:a", 7],
                "parked_question": 7}),
        ));
        let items = s.items(CH);
        assert_eq!(items.len(), 2);
        for (i, details) in items.iter().zip(["", "d"]) {
            assert_eq!(
                (
                    i.text.as_str(),
                    i.details.as_str(),
                    i.status,
                    i.release,
                    i.order
                ),
                ("", details, Status::Open, false, 0.0)
            );
            assert_eq!(
                (&i.standing, &i.evidence, &i.parked_question),
                (&None, &vec![], &None)
            );
        }
    }

    #[test]
    fn malformed_and_misplaced_records_are_ignored() {
        let (owner, bot) = (Keys::generate(), Keys::generate());
        let bk = hex(&bot);
        let mut s = store(&owner);
        let bad = |d: String| rec(&bot, KIND_ITEM, &d, 1, item("x", "open", 1.0));
        // d names another channel; the wrong type; no creator; a bad item id;
        // an uppercase creator; an extra segment.
        for d in [
            format!("{TAG}/item/bcd11c3d-f78b-4934-8e48-c08b6baed1c5/{bk}/aaaa0003"),
            format!("{TAG}/header/{CH}/{bk}"),
            format!("{TAG}/item/{CH}/aaaa0003"),
            item_d(&bk, "AAAA0003"),
            item_d(&bk.to_uppercase(), "aaaa0003"),
            format!("{}/x", item_d(&bk, "aaaa0003")),
        ] {
            assert!(!s.apply(&bad(d.clone())), "{d}");
        }
        // No checklist-type or channel tag.
        let bare = EventBuilder::new(Kind::Custom(KIND_ITEM), "{}")
            .tags([
                Tag::parse(["d", &item_d(&bk, "aaaa0004")]).unwrap(),
                Tag::parse(["h", CH]).unwrap(),
                Tag::parse(["t", TAG]).unwrap(),
            ])
            .sign_with_keys(&bot)
            .unwrap();
        assert!(!s.apply(&bare));
        assert!(s.items(CH).is_empty());
    }

    #[test]
    fn header_watch_and_summary() {
        let (owner, bot, watcher, fake) = (
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
        );
        let bk = hex(&bot);
        let mut s = store(&owner);
        s.trust.watchers = vec![hex(&watcher)];
        let hd = head_d(&bk);
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &hd,
            10,
            serde_json::json!({
            "v": 1, "title": "operator loop", "mode": "ideating", "go": null,
            "resume": {"worktree": "/w", "branch": "agent/x", "worklog": "log.md"}}),
        ));
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &hd,
            20,
            serde_json::json!({
            "v": 1, "title": "operator loop", "mode": "running", "go": "abc"}),
        ));
        let h = s.header(CH).unwrap();
        assert_eq!((h.mode, h.go.as_deref()), (Mode::Running, Some("abc")));
        for (i, st) in ["open", "open", "done", "parked"].iter().enumerate() {
            s.apply(&rec(
                &bot,
                KIND_ITEM,
                &item_d(&bk, &format!("0000000{i}")),
                10,
                item("x", st, i as f64),
            ));
        }
        let wd = format!("{TAG}/watch/{CH}");
        s.apply(&rec(&watcher, KIND_WATCH, &wd, 30, serde_json::json!({
            "v": 1, "state": "waiting_on_lucian", "since": 100, "thread": "root1", "ping": "ping1", "nudges": 2})));
        // Not the watcher: ignored even though it's newer.
        s.apply(&rec(
            &fake,
            KIND_WATCH,
            &wd,
            40,
            serde_json::json!({"state": "working"}),
        ));
        let today = 1000;
        s.note_message(CH, "m1", 999, "agent process crashed (exit 1), respawning");
        s.note_message(CH, "m2", 1001, "⚠ Agent process crashed: signal 9");
        s.note_message(CH, "m2", 1001, "⚠ Agent process crashed: signal 9");
        s.note_message(CH, "m3", 1002, "all good");
        let sum = s.summary(CH, today);
        assert_eq!(
            (sum.open, sum.done),
            (3, 1),
            "a legacy parked item counts as open"
        );
        assert_eq!(sum.mode, Some(Mode::Running));
        assert_eq!(sum.title.as_deref(), Some("operator loop"));
        assert!(sum.contested.is_empty());
        let w = sum.watch.clone().unwrap();
        assert_eq!(w.state, WatchState::WaitingOnLucian);
        assert!(w.state.needs_you());
        assert_eq!(
            sum.ping_link.as_deref(),
            Some(format!("buzz://message?channel={CH}&id=ping1&thread=root1").as_str())
        );
        assert_eq!(sum.crashes_today, 1);
        assert!(!sum.is_empty());
        assert!(s.summary("elsewhere", today).is_empty());
        // Watch records count only from the watcher's keys, the owner's too.
        s.apply(&rec(
            &owner,
            KIND_WATCH,
            &wd,
            50,
            serde_json::json!({"state": "idle"}),
        ));
        assert_eq!(s.watch(CH).unwrap().state, WatchState::WaitingOnLucian);
        s.trust.watchers.clear();
        assert!(s.watch(CH).is_none());
    }

    #[test]
    fn evidence_links_and_midnight() {
        assert_eq!(
            evidence_link(CH, "msg:abc").as_deref(),
            Some(format!("buzz://message?channel={CH}&id=abc").as_str())
        );
        assert_eq!(
            evidence_link(CH, "url:https://x.y/z").as_deref(),
            Some("https://x.y/z")
        );
        assert_eq!(evidence_link(CH, "commit:abc"), None);
        let now = 1_792_300_000;
        let m = local_midnight(now);
        assert!(m <= now && now - m < 25 * 3600);
        assert_eq!(
            WatchState::parse("waiting_on_release").label(),
            "waiting on release"
        );
        assert_eq!(WatchState::parse("new_state").label(), "new state");
    }
}
