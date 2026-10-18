//! The operator loop's checklist records, read-only: a channel's header
//! (kind 30471), its items (30470) and the watcher's state (30472), all
//! addressable and channel-scoped by `h`. Agents write them with
//! `buzz checklist`; both apps only fold and show them.
//!
//! Ownership (spec v0): per `d`, the newest version signed by the record's
//! creator or by the owner. Other signers are ignored. Watch records count
//! only from the watcher's key.

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
/// lucian: accepted on every record (default, overridable).
pub const OWNER: &str = "5c017fc7a0525e163d44fae1b4e853ad236e690c4f607fd2c603088d854bc36a";
/// The watcher's keys, accepted on watch records by default: the fleet-skills
/// seat, which aei's buzz-watch and stall-judge sign with. `watcher` in the
/// config (or `BUZZ_VIM_WATCHER`) overrides it. The owner's watch records
/// always count.
pub const WATCHERS: &[&str] = &["d84ddba92b7e936304ec9ea81c9b59594b9f974dd0ebeb0aa2227fe7d54d70d4"];
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
    pub creator: String,
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
}

/// Who a reader trusts.
#[derive(Debug, Clone)]
pub struct Trust {
    pub owner: String,
    /// Keys whose watch records count, besides the owner's.
    pub watchers: Vec<String>,
}

impl Default for Trust {
    fn default() -> Self {
        Trust {
            owner: OWNER.to_string(),
            watchers: WATCHERS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// One signed version of a record, before ownership is applied.
#[derive(Debug, Clone)]
struct Version {
    signer: String,
    creator: Option<String>,
    at: u64,
    id: String,
    content: Value,
    /// The earliest (at, id) this signer claimed to be the creator, of all
    /// its versions seen: only the newest is kept, and its time is not the
    /// claim's.
    first_claim: Option<(u64, String)>,
}

impl Version {
    /// Newer first; same second: the lower id wins, as NIP-01 says.
    fn newer_than(&self, o: &Version) -> bool {
        (self.at, std::cmp::Reverse(&self.id)) > (o.at, std::cmp::Reverse(&o.id))
    }
}

/// The version a reader takes for one `d`: the newest signed by the
/// creator or the owner. The creator is what the owner's version says;
/// else `preferred` (the list's own creator, for items) when it claims it;
/// else the signer with the earliest claim. Relays keep only each signer's
/// newest version, so "earliest" is what we have seen, and a backdated
/// claim can still win it: hence the owner and the list's creator first.
fn pick<'a>(
    versions: &'a [Version],
    owner: &str,
    preferred: Option<&str>,
) -> Option<(&'a Version, String)> {
    let newest = |a: &&Version, b: &&Version| {
        if a.newer_than(b) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Less
        }
    };
    let creator = versions
        .iter()
        .filter(|v| v.signer == owner)
        .max_by(newest)
        .map(|v| v.creator.clone().unwrap_or_else(|| v.signer.clone()))
        .or_else(|| {
            let p = preferred?;
            versions
                .iter()
                .any(|v| v.signer == p && v.first_claim.is_some())
                .then(|| p.to_string())
        })
        .or_else(|| {
            versions
                .iter()
                .filter_map(|v| Some((v.first_claim.clone()?, &v.signer)))
                .min()
                .map(|(_, s)| s.clone())
        })?;
    let v = versions
        .iter()
        .filter(|v| v.signer == owner || v.signer == creator)
        .max_by(newest)?;
    Some((v, creator))
}

#[derive(Debug, Default)]
struct ChannelRecords {
    /// d -> signer -> newest version from that signer.
    items: HashMap<String, BTreeMap<String, Version>>,
    header: BTreeMap<String, Version>,
    watch: BTreeMap<String, Version>,
    /// Crash notices seen here: message id -> created_at.
    crashes: HashMap<String, u64>,
}

/// Every channel's checklist records, folded as they arrive.
#[derive(Debug, Default)]
pub struct Checklists {
    pub trust: Trust,
    channels: HashMap<String, ChannelRecords>,
}

/// What the status line shows for one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub title: Option<String>,
    pub mode: Option<Mode>,
    pub open: usize,
    pub done: usize,
    pub parked: usize,
    pub watch: Option<Watch>,
    /// `buzz://` link to the ping, when one is out.
    pub ping_link: Option<String>,
    pub crashes_today: usize,
}

impl Summary {
    /// Nothing to show: no list, no watcher, no crashes.
    pub fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.open + self.done + self.parked == 0
            && self.watch.is_none()
            && self.crashes_today == 0
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ItemJson {
    text: String,
    details: String,
    status: String,
    release: bool,
    order: Option<f64>,
    standing: Option<String>,
    evidence: Vec<String>,
    parked_question: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HeaderJson {
    title: String,
    mode: String,
    go: Option<String>,
    resume: ResumeJson,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ResumeJson {
    worktree: String,
    branch: String,
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
            channels: HashMap::new(),
        }
    }

    /// REQ filters for one channel's records.
    pub fn filters(channel: &str) -> Vec<Value> {
        vec![serde_json::json!({"kinds": KINDS, "#h": [channel], "limit": 500})]
    }

    /// Fold one record. Returns true when it was one of ours and well formed.
    pub fn apply(&mut self, ev: &Event) -> bool {
        let kind = ev.kind.as_u16();
        if !KINDS.contains(&kind) {
            return false;
        }
        let channel = tag(ev, "h").or_else(|| tag(ev, "channel"));
        let (Some(channel), Some(d)) = (channel, tag(ev, "d")) else {
            return false;
        };
        let mut parts = d.split('/');
        let ok = parts.next() == Some(TAG)
            && parts.next()
                == Some(match kind {
                    KIND_ITEM => "item",
                    KIND_HEADER => "header",
                    _ => "watch",
                })
            && parts.next() == Some(channel);
        let rest: Vec<&str> = parts.collect();
        let ok = ok
            && match kind {
                KIND_ITEM => rest.len() == 1 && !rest[0].is_empty(),
                _ => rest.is_empty(),
            };
        if !ok {
            return false;
        }
        let Ok(content) = serde_json::from_str::<Value>(&ev.content) else {
            return false;
        };
        let signer = ev.pubkey.to_hex();
        let creator = tag(ev, "creator").map(str::to_string);
        let at = ev.created_at.as_secs();
        let id = ev.id.to_hex();
        let claim = creator
            .as_deref()
            .is_none_or(|c| c == signer)
            .then(|| (at, id.clone()));
        let mut v = Version {
            signer,
            creator,
            at,
            id,
            content,
            first_claim: claim.clone(),
        };
        let recs = self.channels.entry(channel.to_string()).or_default();
        let slot = match kind {
            KIND_ITEM => recs.items.entry(d.to_string()).or_default(),
            KIND_HEADER => &mut recs.header,
            _ => &mut recs.watch,
        };
        let earliest = |a: Option<(u64, String)>, b: Option<(u64, String)>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match slot.get_mut(&v.signer) {
            Some(old) if !v.newer_than(old) => {
                // Older, but its claim still counts.
                old.first_claim = earliest(old.first_claim.take(), claim);
                false
            }
            old => {
                v.first_claim = earliest(old.and_then(|o| o.first_claim.clone()), claim);
                slot.insert(v.signer.clone(), v);
                true
            }
        }
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
        let versions: Vec<Version> = recs.header.values().cloned().collect();
        let (v, creator) = pick(&versions, &self.trust.owner, None)?;
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
            updated_at: v.at,
        })
    }

    /// Items in list order: the standing first item, then by `order`, then
    /// the standing last item.
    pub fn items(&self, channel: &str) -> Vec<Item> {
        let Some(recs) = self.channels.get(channel) else {
            return vec![];
        };
        let list_creator = self.header(channel).map(|h| h.creator);
        let mut out: Vec<Item> = recs
            .items
            .iter()
            .filter_map(|(d, by)| {
                let versions: Vec<Version> = by.values().cloned().collect();
                let (v, creator) = pick(&versions, &self.trust.owner, list_creator.as_deref())?;
                let j: ItemJson = serde_json::from_value(v.content.clone()).unwrap_or_default();
                Some(Item {
                    id: d.rsplit('/').next().unwrap_or_default().to_string(),
                    text: j.text,
                    details: j.details,
                    status: Status::parse(&j.status),
                    release: j.release,
                    order: j.order.unwrap_or(f64::MAX),
                    standing: j.standing.filter(|s| !s.is_empty()),
                    evidence: j.evidence,
                    parked_question: j.parked_question.filter(|q| !q.is_empty()),
                    creator,
                    signer: v.signer.clone(),
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
        let v = recs
            .watch
            .values()
            .filter(|v| v.signer == self.trust.owner || w.contains(&v.signer))
            .fold(None::<&Version>, |best, v| match best {
                Some(b) if !v.newer_than(b) => Some(b),
                _ => Some(v),
            })?;
        let j: WatchJson = serde_json::from_value(v.content.clone()).unwrap_or_default();
        Some(Watch {
            state: WatchState::parse(&j.state),
            since: j.since,
            thread: j.thread.filter(|t| !t.is_empty()),
            ping: j.ping.filter(|p| !p.is_empty()),
            nudges: j.nudges,
            reason: j.reason,
            updated_at: v.at,
        })
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
            open: count(Status::Open),
            done: count(Status::Done),
            parked: count(Status::Parked),
            watch,
            ping_link,
            crashes_today: self.crashes_since(channel, today),
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

    fn rec(
        keys: &Keys,
        kind: u16,
        d: &str,
        creator: Option<&str>,
        at: u64,
        content: Value,
    ) -> Event {
        let mut t = vec![
            vec!["d".to_string(), d.to_string()],
            vec!["h".into(), CH.into()],
            vec!["t".into(), TAG.into()],
            vec!["channel".into(), CH.into()],
        ];
        if let Some(c) = creator {
            t.push(vec!["creator".into(), c.into()]);
        }
        EventBuilder::new(Kind::Custom(kind), content.to_string())
            .tags(t.into_iter().map(|t| Tag::parse(t).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn item_d(id: &str) -> String {
        format!("{TAG}/item/{CH}/{id}")
    }

    fn item(text: &str, status: &str, order: f64) -> Value {
        serde_json::json!({"v": 1, "text": text, "status": status, "order": order,
            "evidence": if status == "done" { vec!["commit:abc"] } else { vec![] }})
    }

    fn store(owner: &Keys) -> Checklists {
        Checklists::new(Trust {
            owner: owner.public_key().to_hex(),
            watchers: vec![],
        })
    }

    #[test]
    fn newest_version_from_creator_or_owner_wins() {
        let (owner, bot, other) = (Keys::generate(), Keys::generate(), Keys::generate());
        let bk = bot.public_key().to_hex();
        let mut s = store(&owner);
        let d = item_d("aaaa0001");
        assert!(s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            10,
            item("first", "open", 1.0)
        )));
        // Someone else's later version is ignored, creator tag or not.
        s.apply(&rec(
            &other,
            KIND_ITEM,
            &d,
            Some(&bk),
            20,
            item("hijack", "done", 1.0),
        ));
        s.apply(&rec(
            &other,
            KIND_ITEM,
            &d,
            None,
            21,
            item("hijack2", "done", 1.0),
        ));
        assert_eq!(s.items(CH)[0].text, "first");
        // The owner's edit counts.
        s.apply(&rec(
            &owner,
            KIND_ITEM,
            &d,
            Some(&bk),
            30,
            item("owner edit", "parked", 1.0),
        ));
        let i = &s.items(CH)[0];
        assert_eq!((i.text.as_str(), i.status), ("owner edit", Status::Parked));
        assert_eq!(i.creator, bk);
        // Then the creator's newer one.
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            40,
            item("ticked", "done", 1.0),
        ));
        let i = &s.items(CH)[0];
        assert_eq!((i.text.as_str(), i.status), ("ticked", Status::Done));
        // An older version from the same signer doesn't replace a newer one.
        assert!(!s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            35,
            item("stale", "open", 1.0)
        )));
        assert_eq!(s.items(CH)[0].text, "ticked");
    }

    #[test]
    fn the_first_self_claimed_signer_is_the_creator() {
        let (owner, bot, late) = (Keys::generate(), Keys::generate(), Keys::generate());
        let mut s = store(&owner);
        let d = item_d("aaaa0002");
        let bk = bot.public_key().to_hex();
        let lk = late.public_key().to_hex();
        s.apply(&rec(
            &late,
            KIND_ITEM,
            &d,
            Some(&lk),
            50,
            item("late claim", "open", 1.0),
        ));
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            10,
            item("original", "open", 1.0),
        ));
        assert_eq!(s.items(CH)[0].text, "original");
        // The owner's creator tag settles it.
        s.apply(&rec(
            &owner,
            KIND_ITEM,
            &d,
            Some(&lk),
            5,
            item("by owner", "open", 1.0),
        ));
        s.apply(&rec(
            &late,
            KIND_ITEM,
            &d,
            Some(&lk),
            60,
            item("late again", "open", 1.0),
        ));
        assert_eq!(s.items(CH)[0].text, "late again");
    }

    /// A later self-claim doesn't take an item over when the creator edits
    /// it, and the list's creator wins a claim it shares.
    #[test]
    fn a_later_claim_does_not_take_an_item_over() {
        let (owner, bot, x) = (Keys::generate(), Keys::generate(), Keys::generate());
        let (bk, xk) = (bot.public_key().to_hex(), x.public_key().to_hex());
        let d = item_d("aaaa0009");
        let mut s = store(&owner);
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            10,
            item("mine", "open", 1.0),
        ));
        s.apply(&rec(
            &x,
            KIND_ITEM,
            &d,
            Some(&xk),
            20,
            item("x's", "open", 1.0),
        ));
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            40,
            item("mine, edited", "open", 1.0),
        ));
        assert_eq!(s.items(CH)[0].text, "mine, edited");
        // Seen fresh, only the newest of each survives: the list's creator
        // (who ran init) is preferred over an earlier-looking claim.
        let mut s = store(&owner);
        let hd = format!("{TAG}/header/{CH}");
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &hd,
            Some(&bk),
            5,
            serde_json::json!({"title": "t"}),
        ));
        s.apply(&rec(
            &x,
            KIND_ITEM,
            &d,
            Some(&xk),
            20,
            item("x's", "open", 1.0),
        ));
        s.apply(&rec(
            &bot,
            KIND_ITEM,
            &d,
            Some(&bk),
            40,
            item("mine, edited", "open", 1.0),
        ));
        assert_eq!(s.items(CH)[0].text, "mine, edited");
    }

    #[test]
    fn items_sort_with_standing_ones_at_the_ends() {
        let (owner, bot) = (Keys::generate(), Keys::generate());
        let bk = bot.public_key().to_hex();
        let mut s = store(&owner);
        let put = |s: &mut Checklists, id: &str, v: Value| {
            s.apply(&rec(&bot, KIND_ITEM, &item_d(id), Some(&bk), 10, v));
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
    fn malformed_and_misplaced_records_are_ignored() {
        let (owner, bot) = (Keys::generate(), Keys::generate());
        let mut s = store(&owner);
        // d names another channel.
        let other = format!("{TAG}/item/bcd11c3d-f78b-4934-8e48-c08b6baed1c5/aaaa0003");
        assert!(!s.apply(&rec(
            &bot,
            KIND_ITEM,
            &other,
            None,
            1,
            item("x", "open", 1.0)
        )));
        // Wrong record type in d, no item id, bad JSON.
        assert!(!s.apply(&rec(
            &bot,
            KIND_ITEM,
            &format!("{TAG}/header/{CH}"),
            None,
            1,
            item("x", "open", 1.0)
        )));
        assert!(!s.apply(&rec(
            &bot,
            KIND_ITEM,
            &format!("{TAG}/item/{CH}"),
            None,
            1,
            item("x", "open", 1.0)
        )));
        let bad = EventBuilder::new(Kind::Custom(KIND_ITEM), "not json")
            .tags([
                Tag::parse(["d", &item_d("aaaa0004")]).unwrap(),
                Tag::parse(["h", CH]).unwrap(),
            ])
            .sign_with_keys(&bot)
            .unwrap();
        assert!(!s.apply(&bad));
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
        let bk = bot.public_key().to_hex();
        let mut s = store(&owner);
        s.trust.watchers = vec![watcher.public_key().to_hex()];
        let hd = format!("{TAG}/header/{CH}");
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &hd,
            Some(&bk),
            10,
            serde_json::json!({
            "v": 1, "title": "operator loop", "mode": "ideating", "go": null,
            "resume": {"worktree": "/w", "branch": "agent/x", "worklog": "log.md"}}),
        ));
        s.apply(&rec(
            &bot,
            KIND_HEADER,
            &hd,
            Some(&bk),
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
                &item_d(&format!("0000000{i}")),
                Some(&bk),
                10,
                item("x", st, i as f64),
            ));
        }
        let wd = format!("{TAG}/watch/{CH}");
        s.apply(&rec(&watcher, KIND_WATCH, &wd, None, 30, serde_json::json!({
            "v": 1, "state": "waiting_on_lucian", "since": 100, "thread": "root1", "ping": "ping1", "nudges": 2})));
        // Not the watcher: ignored even though it's newer.
        s.apply(&rec(
            &fake,
            KIND_WATCH,
            &wd,
            None,
            40,
            serde_json::json!({"state": "working"}),
        ));
        let today = 1000;
        s.note_message(CH, "m1", 999, "agent process crashed (exit 1), respawning");
        s.note_message(CH, "m2", 1001, "⚠ Agent process crashed: signal 9");
        s.note_message(CH, "m2", 1001, "⚠ Agent process crashed: signal 9");
        s.note_message(CH, "m3", 1002, "all good");
        let sum = s.summary(CH, today);
        assert_eq!((sum.open, sum.done, sum.parked), (2, 1, 1));
        assert_eq!(sum.mode, Some(Mode::Running));
        assert_eq!(sum.title.as_deref(), Some("operator loop"));
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
        // No watcher configured: only the owner's count.
        s.trust.watchers.clear();
        assert!(s.watch(CH).is_none());
        s.apply(&rec(
            &owner,
            KIND_WATCH,
            &wd,
            None,
            50,
            serde_json::json!({"state": "idle"}),
        ));
        assert_eq!(s.watch(CH).unwrap().state, WatchState::Idle);
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
