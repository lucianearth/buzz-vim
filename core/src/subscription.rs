//! Which Claude subscription a channel's agent sessions spend (aei C5).
//!
//! An operator sets it with `B` in a channel. That signs one NIP-78
//! app-data record, kind 30078 with `d` = [`D_MAP`], holding the whole map:
//! `{"v":1,"default":"<person>"|"","channels":{"<uuid>":"<person>"}}`.
//! A person is an agentauth user id; `""` / absent means "follow the
//! default" for a channel and "each agent's owner" for the default.
//!
//! The box's reconciler reads the newest map an operator signed and
//! writes it into every Claude seat (agents/channel-subscriptions.py in
//! aei). It publishes whom to offer as [`D_PEOPLE`], signed by its poster
//! key. Both readers apply the same rule: only operator signers count for
//! the map, only the publisher for the people, newest wins (created_at,
//! then the lower id), and an event whose signature does not verify is
//! ignored. One record holds the whole map because the relay narrows a
//! 30078 query by `#d` in SQL only; per-channel records could not be listed.

use std::collections::BTreeMap;

use nostr::{Event, EventBuilder, Kind, Tag};
use serde_json::{Value, json};

use crate::model::tag;

pub const KIND: u16 = 30078;
pub const D_MAP: &str = "buzz-subscription";
pub const D_PEOPLE: &str = "buzz-subscription/people";
/// chip / opsbot: the key the aei reconciler publishes the people with.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How far ahead of this clock a record may be dated (the reconciler's rule).
pub const MAX_SKEW: u64 = 600;
pub const PUBLISHER: &str = "f708a2af2c0b6f64abb2dffbdc9b1f1e43a7d166a583bce5ce19f83afdd95e6a";

/// Whose records count.
#[derive(Clone, Debug)]
pub struct Trust {
    /// Signers of the map (default: the checklist owner, lucian).
    pub operators: Vec<String>,
    /// Signers of the people list.
    pub publishers: Vec<String>,
}

impl Default for Trust {
    fn default() -> Self {
        Trust {
            operators: vec![crate::checklist::DEFAULT_OWNER.to_string()],
            publishers: vec![PUBLISHER.to_string()],
        }
    }
}

/// The map: a default and per-channel picks, by agentauth person id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Map {
    pub default: Option<String>,
    pub channels: BTreeMap<String, String>,
    /// What this client could not read (top-level fields it doesn't know,
    /// channel entries it can't parse), written back as found, so an edit
    /// here never deletes another writer's data.
    pub kept: serde_json::Map<String, Value>,
    /// A `default` this client can't read, written back unless replaced.
    pub kept_default: Option<Value>,
    pub kept_channels: serde_json::Map<String, Value>,
}

/// One change from the picker, applied to the newest map at publish time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A channel's own pick (None: follow the default).
    Channel(String, Option<String>),
    /// The default for every channel (None: each agent's owner).
    Default(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub label: String,
}

/// What a channel spends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// The person, or None for each agent's owner.
    pub person: Option<String>,
    /// True when it comes from the default, not a pick of its own.
    pub inherited: bool,
}

#[derive(Clone, Debug)]
struct Rec<T> {
    at: u64,
    id: String,
    value: T,
}

#[derive(Clone, Debug, Default)]
pub struct Subscriptions {
    pub trust: Trust,
    /// The map shown: the newest known, our unacked records included.
    map: Option<Rec<Map>>,
    /// The newest map the relay is known to hold (read from it, or acked).
    confirmed: Option<Rec<Map>>,
    /// Our records signed but not yet acked, oldest first.
    pending: Vec<Rec<Map>>,
    people: Option<Rec<Vec<Person>>>,
}

/// agentauth's user id rule: `^[a-z0-9][a-z0-9-]{0,30}$`.
pub fn is_person(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 31
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A lowercase hyphenated uuid.
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => matches!(c, b'0'..=b'9' | b'a'..=b'f'),
        })
}

fn object(ev: &Event) -> serde_json::Map<String, Value> {
    match serde_json::from_str::<Value>(&ev.content) {
        Ok(Value::Object(o)) => o,
        _ => Default::default(),
    }
}

/// The record's map, read leniently: a bad field takes its default, a bad
/// entry is dropped, the rest stands.
fn read_map(ev: &Event) -> Map {
    let o = object(ev);
    let default = o
        .get("default")
        .and_then(Value::as_str)
        .filter(|s| is_person(s))
        .map(str::to_string);
    let mut channels = BTreeMap::new();
    let mut kept_channels = serde_json::Map::new();
    if let Some(m) = o.get("channels").and_then(Value::as_object) {
        for (c, p) in m {
            match p.as_str() {
                Some(p) if is_uuid(c) && is_person(p) => {
                    channels.insert(c.clone(), p.to_string());
                }
                _ => {
                    kept_channels.insert(c.clone(), p.clone());
                }
            }
        }
    }
    let kept_default = match o.get("default") {
        None | Some(Value::Null) => None,
        Some(v) if default.is_none() && v != &json!("") => Some(v.clone()),
        Some(_) => None,
    };
    let kept = o
        .into_iter()
        .filter(|(k, _)| !matches!(k.as_str(), "v" | "default" | "channels"))
        .collect();
    Map {
        default,
        channels,
        kept,
        kept_default,
        kept_channels,
    }
}

fn read_people(ev: &Event) -> Vec<Person> {
    object(ev)
        .get("people")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let id = p.get("id")?.as_str().filter(|s| is_person(s))?;
                    let label = p
                        .get("label")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(id);
                    Some(Person {
                        id: id.to_string(),
                        label: label.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Subscriptions {
    pub fn new(trust: Trust) -> Self {
        Subscriptions {
            trust,
            ..Default::default()
        }
    }

    /// Both records, each narrowed to its trusted signers and its `d`.
    pub fn filters(&self) -> Vec<Value> {
        vec![
            json!({"kinds": [KIND], "authors": self.trust.operators, "#d": [D_MAP], "limit": 50}),
            json!({"kinds": [KIND], "authors": self.trust.publishers, "#d": [D_PEOPLE], "limit": 5}),
        ]
    }

    /// One of ours, before trust: kind 30078 with our `d`.
    pub fn is_record(ev: &Event) -> bool {
        ev.kind.as_u16() == KIND && matches!(tag(ev, "d"), Some(D_MAP) | Some(D_PEOPLE))
    }

    pub fn is_operator(&self, pubkey: &str) -> bool {
        self.trust.operators.iter().any(|o| o == pubkey)
    }

    /// Fold one event from the relay. True when it is now the record shown.
    pub fn apply(&mut self, ev: &Event) -> bool {
        self.fold(ev, false)
    }

    fn fold(&mut self, ev: &Event, ours: bool) -> bool {
        if !Self::is_record(ev) {
            return false;
        }
        // Dated more than MAX_SKEW ahead: skipped, as the aei reconciler
        // does, so one bad clock can't outrank (and ratchet) every later pick.
        if ev.created_at.as_secs() > unix_now() + MAX_SKEW {
            return false;
        }
        let signer = ev.pubkey.to_hex();
        let (at, id) = (ev.created_at.as_secs(), ev.id.to_hex());
        let map = tag(ev, "d") == Some(D_MAP);
        let trusted = if map {
            self.is_operator(&signer)
        } else {
            self.trust.publishers.contains(&signer)
        };
        if !trusted || ev.verify().is_err() {
            return false;
        }
        // Newer than what's shown (or the first); the same event again is no change.
        let replaces = |r: Option<(u64, &str)>| {
            r.is_none_or(|(a, i)| {
                i != id && (at, std::cmp::Reverse(id.as_str())) > (a, std::cmp::Reverse(i))
            })
        };
        if map {
            let rec = Rec {
                at,
                id: id.clone(),
                value: read_map(ev),
            };
            if ours {
                self.pending.push(rec.clone());
            } else if replaces(self.confirmed.as_ref().map(|r| (r.at, r.id.as_str()))) {
                self.confirmed = Some(rec.clone());
            }
            if replaces(self.map.as_ref().map(|r| (r.at, r.id.as_str()))) {
                self.map = Some(rec);
                return true;
            }
        } else if replaces(self.people.as_ref().map(|r| (r.at, r.id.as_str()))) {
            self.people = Some(Rec {
                at,
                id,
                value: read_people(ev),
            });
            return true;
        }
        false
    }

    /// The map, once an operator has published one.
    pub fn map(&self) -> Option<&Map> {
        self.map.as_ref().map(|r| &r.value)
    }

    pub fn people(&self) -> &[Person] {
        self.people.as_ref().map_or(&[], |r| r.value.as_slice())
    }

    /// A person's label, or their id.
    pub fn label(&self, id: &str) -> String {
        self.people()
            .iter()
            .find(|p| p.id == id)
            .map_or_else(|| id.to_string(), |p| p.label.clone())
    }

    /// What a channel spends. None until the map is known: with no relay
    /// record the box uses the dashboard's map, which isn't visible here.
    pub fn for_channel(&self, channel: &str) -> Option<Shown> {
        let m = self.map()?;
        Some(match m.channels.get(channel) {
            Some(p) => Shown {
                person: Some(p.clone()),
                inherited: false,
            },
            None => Shown {
                person: m.default.clone(),
                inherited: true,
            },
        })
    }

    /// The status-line text: `sub alice`, `sub kevin (default)`,
    /// `sub owner (default)`.
    pub fn status(&self, channel: &str) -> Option<String> {
        let s = self.for_channel(channel)?;
        let who = s
            .person
            .as_deref()
            .map_or_else(|| "owner".to_string(), |p| self.label(p));
        Some(if s.inherited {
            format!("sub {who} (default)")
        } else {
            format!("sub {who}")
        })
    }

    /// The current map with one channel's pick changed (None: follow the
    /// default). Starts from the newest known map, so other picks stay.
    pub fn with_channel(&self, channel: &str, person: Option<&str>) -> Map {
        self.edited(&Edit::Channel(
            channel.to_string(),
            person.map(str::to_string),
        ))
    }

    /// The current map with a new default (None: each agent's owner).
    pub fn with_default(&self, person: Option<&str>) -> Map {
        self.edited(&Edit::Default(person.map(str::to_string)))
    }

    /// The newest known map with one edit applied.
    pub fn edited(&self, edit: &Edit) -> Map {
        let mut m = self.map().cloned().unwrap_or_default();
        match edit {
            Edit::Channel(c, Some(p)) => {
                m.kept_channels.remove(c);
                m.channels.insert(c.clone(), p.clone());
            }
            Edit::Channel(c, None) => {
                m.kept_channels.remove(c);
                m.channels.remove(c);
            }
            Edit::Default(p) => {
                m.kept_default = None;
                m.default = p.clone();
            }
        }
        m
    }

    /// The created_at for our next record: now, but always after the map
    /// held, so it wins even against a clock ahead of ours or a second
    /// edit within the same second.
    pub fn next_at(&self, now: u64) -> u64 {
        self.map.as_ref().map_or(now, |r| now.max(r.at + 1))
    }

    /// Our own record, just signed: shown at once so the next edit builds
    /// on it, and held as pending until `ack` or `retract`.
    pub fn apply_ours(&mut self, ev: &Event) -> bool {
        self.fold(ev, true)
    }

    /// The relay took our record.
    pub fn ack(&mut self, id: &str) {
        if let Some(i) = self.pending.iter().position(|r| r.id == id) {
            let rec = self.pending.remove(i);
            let newer = self.confirmed.as_ref().is_none_or(|c| {
                (rec.at, std::cmp::Reverse(&rec.id)) > (c.at, std::cmp::Reverse(&c.id))
            });
            if newer {
                self.confirmed = Some(rec);
            }
        }
    }

    /// The relay refused our record. If it is the one shown, show the
    /// newest of our other unacked records, else what the relay holds:
    /// never a record the relay has refused.
    pub fn retract(&mut self, id: &str) {
        self.pending.retain(|r| r.id != id);
        if self.map.as_ref().is_some_and(|r| r.id == id) {
            self.map = self
                .pending
                .last()
                .cloned()
                .or_else(|| self.confirmed.clone());
        }
    }
}

/// The record for a map, dated `at` (see `Subscriptions::next_at`), ready
/// to sign. What this client could not read goes back as it was found.
pub fn builder(m: &Map, at: u64) -> anyhow::Result<EventBuilder> {
    let mut channels = m.kept_channels.clone();
    for (c, p) in &m.channels {
        channels.insert(c.clone(), json!(p));
    }
    let mut content = m.kept.clone();
    content.insert("v".into(), json!(1));
    content.insert(
        "default".into(),
        match (&m.default, &m.kept_default) {
            (Some(p), _) => json!(p),
            (None, Some(raw)) => raw.clone(),
            (None, None) => json!(""),
        },
    );
    content.insert("channels".into(), Value::Object(channels));
    Ok(
        EventBuilder::new(Kind::from(KIND), Value::Object(content).to_string())
            .tags([
                Tag::parse(["d", D_MAP])?,
                Tag::parse(["t", "buzz-subscription"])?,
                Tag::parse(["alt", "Which Claude subscription each channel's agents use"])?,
            ])
            .custom_created_at(nostr::Timestamp::from(at)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{Keys, Timestamp};

    const DEV: &str = "5e8e98d5-b696-49e2-8c6c-c0bc8faa831a";
    const OPS: &str = "11111111-2222-4333-8444-555555555555";

    fn rec(keys: &Keys, d: &str, content: Value, at: u64) -> Event {
        EventBuilder::new(Kind::from(KIND), content.to_string())
            .tags([Tag::parse(["d", d]).unwrap()])
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn setup() -> (Keys, Keys, Keys, Subscriptions) {
        let (op, other, publ) = (Keys::generate(), Keys::generate(), Keys::generate());
        let s = Subscriptions::new(Trust {
            operators: vec![op.public_key().to_hex()],
            publishers: vec![publ.public_key().to_hex()],
        });
        (op, other, publ, s)
    }

    #[test]
    fn an_operators_map_shows_per_channel_and_by_default() {
        let (op, _, publ, mut s) = setup();
        assert_eq!(s.status(DEV), None, "no record: nothing shown");
        assert!(s.apply(&rec(
            &op,
            D_MAP,
            json!({"default": "kevin", "channels": {DEV: "alice"}}),
            100
        )));
        assert_eq!(s.status(DEV).unwrap(), "sub alice");
        assert_eq!(s.status(OPS).unwrap(), "sub kevin (default)");
        s.apply(&rec(
            &publ,
            D_PEOPLE,
            json!({"people": [{"id": "alice", "label": "Alice A"}]}),
            100,
        ));
        assert_eq!(s.status(DEV).unwrap(), "sub Alice A");
        s.apply(&rec(&op, D_MAP, json!({"default": ""}), 101));
        assert_eq!(s.status(OPS).unwrap(), "sub owner (default)");
    }

    #[test]
    fn a_non_operators_record_is_ignored_even_when_newer() {
        let (op, other, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"channels": {DEV: "alice"}}), 100));
        assert!(!s.apply(&rec(
            &other,
            D_MAP,
            json!({"channels": {DEV: "mallory"}}),
            200
        )));
        assert_eq!(s.status(DEV).unwrap(), "sub alice");
        let (_, other2, _, mut fresh) = setup();
        fresh.apply(&rec(&other2, D_MAP, json!({"default": "mallory"}), 1));
        assert!(fresh.map().is_none());
    }

    #[test]
    fn a_bad_signature_and_strangers_people_are_ignored() {
        let (op, other, _, mut s) = setup();
        let good = rec(&op, D_MAP, json!({"default": "kevin"}), 100);
        let mut forged = rec(&op, D_MAP, json!({"default": "mallory"}), 200);
        forged.content = json!({"default": "mallory2"}).to_string();
        assert!(!s.apply(&forged));
        s.apply(&good);
        assert!(!s.apply(&rec(
            &other,
            D_PEOPLE,
            json!({"people": [{"id": "x"}]}),
            100
        )));
        assert!(s.people().is_empty());
    }

    #[test]
    fn newest_wins_and_same_second_the_lower_id() {
        let (op, _, _, mut s) = setup();
        let new = rec(&op, D_MAP, json!({"default": "b"}), 101);
        let old = rec(&op, D_MAP, json!({"default": "a"}), 100);
        s.apply(&new);
        assert!(!s.apply(&old));
        assert_eq!(s.map().unwrap().default.as_deref(), Some("b"));
        let x = rec(&op, D_MAP, json!({"default": "x"}), 500);
        let y = rec(&op, D_MAP, json!({"default": "y"}), 500);
        let lower = if x.id.to_hex() < y.id.to_hex() {
            "x"
        } else {
            "y"
        };
        for order in [[&x, &y], [&y, &x]] {
            let (_, _, _, mut t) = setup();
            t.trust.operators = vec![op.public_key().to_hex()];
            for e in order {
                t.apply(e);
            }
            assert_eq!(t.map().unwrap().default.as_deref(), Some(lower));
        }
    }

    #[test]
    fn content_is_read_leniently() {
        let (op, _, _, mut s) = setup();
        s.apply(&rec(
            &op,
            D_MAP,
            json!({"default": 7, "channels": {DEV: "alice", "NOT": "bob", OPS: "Bad Id"}}),
            100,
        ));
        let m = s.map().unwrap();
        assert_eq!(m.default, None);
        assert_eq!(m.channels.len(), 1);
        let (op2, _, _, mut t) = setup();
        t.apply(&rec(&op2, D_MAP, json!("not an object"), 100));
        assert_eq!(t.map(), Some(&Map::default()));
    }

    #[test]
    fn edits_keep_the_other_picks_and_round_trip() {
        let (op, _, _, mut s) = setup();
        s.apply(&rec(
            &op,
            D_MAP,
            json!({"default": "kevin", "channels": {DEV: "alice", OPS: "bob"}}),
            100,
        ));
        let m = s.with_channel(DEV, None);
        assert_eq!(m.channels.len(), 1);
        assert_eq!(m.default.as_deref(), Some("kevin"));
        let m = s.with_default(None);
        assert_eq!(m.channels.len(), 2);
        let ev = builder(&s.with_channel(OPS, Some("carol")), 200)
            .unwrap()
            .sign_with_keys(&op)
            .unwrap();
        assert_eq!(tag(&ev, "d"), Some(D_MAP));
        assert!(s.apply(&ev));
        assert_eq!(s.status(OPS).unwrap(), "sub carol");
        assert_eq!(s.status(DEV).unwrap(), "sub alice");
    }

    /// Two picks before the first is acked: the second builds on the
    /// first (shown at sign time), and is dated after it even in the same
    /// second, so neither is lost.
    #[test]
    fn a_second_edit_before_the_ack_keeps_the_first() {
        let (op, _, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), 100));
        let at1 = s.next_at(100);
        assert_eq!(at1, 101, "after the held record even at the same second");
        let e1 = builder(
            &s.edited(&Edit::Channel(DEV.into(), Some("alice".into()))),
            at1,
        )
        .unwrap()
        .sign_with_keys(&op)
        .unwrap();
        assert!(s.apply_ours(&e1));
        let at2 = s.next_at(101);
        assert_eq!(at2, 102);
        let m2 = s.edited(&Edit::Channel(OPS.into(), Some("bob".into())));
        assert_eq!(m2.channels.get(DEV).map(String::as_str), Some("alice"));
        assert_eq!(m2.channels.get(OPS).map(String::as_str), Some("bob"));
        assert_eq!(m2.default.as_deref(), Some("kevin"));
        // A clock behind the held record still dates ours after it.
        assert_eq!(s.next_at(50), 102);
    }

    /// The relay refuses ours: back to the map before it.
    #[test]
    fn a_refused_record_is_retracted() {
        let (op, _, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), 100));
        let ev = builder(&s.edited(&Edit::Default(Some("alice".into()))), 101)
            .unwrap()
            .sign_with_keys(&op)
            .unwrap();
        assert!(s.apply_ours(&ev));
        assert_eq!(s.map().unwrap().default.as_deref(), Some("alice"));
        s.retract(&ev.id.to_hex());
        assert_eq!(s.map().unwrap().default.as_deref(), Some("kevin"));
        // A first-ever record that fails leaves no map at all.
        let (op, _, _, mut s) = setup();
        let ev = builder(&s.edited(&Edit::Default(Some("alice".into()))), 5)
            .unwrap()
            .sign_with_keys(&op)
            .unwrap();
        s.apply_ours(&ev);
        s.retract(&ev.id.to_hex());
        assert!(s.map().is_none());
    }

    /// Two picks in flight: whichever the relay refuses, the shown map is
    /// never one the relay refused.
    #[test]
    fn two_in_flight_never_leave_a_refused_record_shown() {
        let sign = |s: &Subscriptions, op: &Keys, e: Edit, at| {
            builder(&s.edited(&e), at)
                .unwrap()
                .sign_with_keys(op)
                .unwrap()
        };
        // Both refused, the first refusal arriving after the second is signed.
        let (op, _, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), 100));
        let e1 = sign(
            &s,
            &op,
            Edit::Channel(DEV.into(), Some("alice".into())),
            101,
        );
        s.apply_ours(&e1);
        let e2 = sign(&s, &op, Edit::Channel(OPS.into(), Some("bob".into())), 102);
        s.apply_ours(&e2);
        s.retract(&e1.id.to_hex());
        s.retract(&e2.id.to_hex());
        let m = s.map().unwrap();
        assert!(m.channels.is_empty(), "{m:?}");
        assert_eq!(m.default.as_deref(), Some("kevin"));
        assert_eq!(
            s.next_at(100),
            101,
            "dated after what the relay holds again"
        );
        // The second refused, the first acked: the first stays.
        let (op, _, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), 100));
        let e1 = sign(
            &s,
            &op,
            Edit::Channel(DEV.into(), Some("alice".into())),
            101,
        );
        s.apply_ours(&e1);
        let e2 = sign(&s, &op, Edit::Channel(OPS.into(), Some("bob".into())), 102);
        s.apply_ours(&e2);
        s.ack(&e1.id.to_hex());
        s.retract(&e2.id.to_hex());
        assert_eq!(
            s.map().unwrap().channels.get(DEV).map(String::as_str),
            Some("alice")
        );
        assert!(!s.map().unwrap().channels.contains_key(OPS));
        // The second refused while the first is still unacked: the first shows.
        let (op, _, _, mut s) = setup();
        s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), 100));
        let e1 = sign(
            &s,
            &op,
            Edit::Channel(DEV.into(), Some("alice".into())),
            101,
        );
        s.apply_ours(&e1);
        let e2 = sign(&s, &op, Edit::Channel(OPS.into(), Some("bob".into())), 102);
        s.apply_ours(&e2);
        s.retract(&e2.id.to_hex());
        assert_eq!(
            s.map().unwrap().channels.get(DEV).map(String::as_str),
            Some("alice")
        );
        assert!(!s.map().unwrap().channels.contains_key(OPS));
    }

    /// A record dated far ahead is skipped (the reconciler's rule), so it
    /// can't ratchet every later pick into the future.
    #[test]
    fn a_far_future_record_is_skipped() {
        let (op, _, _, mut s) = setup();
        let now = unix_now();
        assert!(!s.apply(&rec(&op, D_MAP, json!({"default": "mallory"}), now + 3600)));
        assert!(s.map().is_none());
        assert!(s.apply(&rec(&op, D_MAP, json!({"default": "kevin"}), now + 60)));
    }

    /// An unreadable default is written back unless the edit sets one.
    #[test]
    fn an_unreadable_default_is_kept() {
        let (op, _, _, mut s) = setup();
        s.apply(&rec(
            &op,
            D_MAP,
            json!({"default": "Kevin X", "channels": {}}),
            100,
        ));
        assert_eq!(s.map().unwrap().default, None);
        let ev = builder(
            &s.edited(&Edit::Channel(DEV.into(), Some("alice".into()))),
            101,
        )
        .unwrap()
        .sign_with_keys(&op)
        .unwrap();
        let c: Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(c["default"], json!("Kevin X"));
        let ev = builder(&s.edited(&Edit::Default(None)), 102)
            .unwrap()
            .sign_with_keys(&op)
            .unwrap();
        let c: Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(c["default"], json!(""));
    }

    /// What this client can't read is written back as found.
    #[test]
    fn an_edit_keeps_what_it_could_not_read() {
        let (op, _, _, mut s) = setup();
        let upper = "5E8E98D5-B696-49E2-8C6C-C0BC8FAA831A";
        s.apply(&rec(
            &op,
            D_MAP,
            json!({"v": 1, "default": "kevin", "note": {"x": 1},
                   "channels": {DEV: "alice", upper: "bob", OPS: 7}}),
            100,
        ));
        assert_eq!(s.map().unwrap().channels.len(), 1, "shown leniently");
        let m = s.edited(&Edit::Default(None));
        let ev = builder(&m, 101).unwrap().sign_with_keys(&op).unwrap();
        let c: Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(c["note"], json!({"x": 1}));
        assert_eq!(c["channels"][upper], json!("bob"));
        assert_eq!(c["channels"][OPS], json!(7));
        assert_eq!(c["channels"][DEV], json!("alice"));
        assert_eq!(c["default"], json!(""));
        assert_eq!(c["v"], json!(1));
        // Setting a channel this client couldn't read replaces the raw entry.
        let m = s.edited(&Edit::Channel(OPS.into(), Some("carol".into())));
        let ev = builder(&m, 102).unwrap().sign_with_keys(&op).unwrap();
        let c: Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(c["channels"][OPS], json!("carol"));
    }

    #[test]
    fn filters_narrow_by_signer_and_d() {
        let s = Subscriptions::new(Trust::default());
        let f = s.filters();
        assert_eq!(f[0]["#d"], json!([D_MAP]));
        assert_eq!(f[0]["authors"], json!([crate::checklist::DEFAULT_OWNER]));
        assert_eq!(f[1]["authors"], json!([PUBLISHER]));
    }

    #[test]
    fn person_ids_follow_agentauth() {
        assert!(is_person("kevin"));
        assert!(is_person("a-1"));
        assert!(!is_person("-a"));
        assert!(!is_person("Kevin"));
        assert!(!is_person(&"a".repeat(32)));
    }
}
