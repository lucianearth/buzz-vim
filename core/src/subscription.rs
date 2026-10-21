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
    map: Option<Rec<Map>>,
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
    let channels = o
        .get("channels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(c, p)| {
                    let p = p.as_str()?;
                    (is_uuid(c) && is_person(p)).then(|| (c.clone(), p.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Map { default, channels }
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

    /// Fold one event. True when it is now the record shown.
    pub fn apply(&mut self, ev: &Event) -> bool {
        if !Self::is_record(ev) {
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
            if replaces(self.map.as_ref().map(|r| (r.at, r.id.as_str()))) {
                self.map = Some(Rec {
                    at,
                    id,
                    value: read_map(ev),
                });
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
        let mut m = self.map().cloned().unwrap_or_default();
        match person {
            Some(p) => {
                m.channels.insert(channel.to_string(), p.to_string());
            }
            None => {
                m.channels.remove(channel);
            }
        }
        m
    }

    /// The current map with a new default (None: each agent's owner).
    pub fn with_default(&self, person: Option<&str>) -> Map {
        let mut m = self.map().cloned().unwrap_or_default();
        m.default = person.map(str::to_string);
        m
    }
}

/// The record for a map, ready to sign.
pub fn builder(m: &Map) -> anyhow::Result<EventBuilder> {
    let content = json!({
        "v": 1,
        "default": m.default.clone().unwrap_or_default(),
        "channels": m.channels,
    });
    Ok(
        EventBuilder::new(Kind::from(KIND), content.to_string()).tags([
            Tag::parse(["d", D_MAP])?,
            Tag::parse(["t", "buzz-subscription"])?,
            Tag::parse(["alt", "Which Claude subscription each channel's agents use"])?,
        ]),
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
        let ev = builder(&s.with_channel(OPS, Some("carol")))
            .unwrap()
            .custom_created_at(Timestamp::from(200))
            .sign_with_keys(&op)
            .unwrap();
        assert_eq!(tag(&ev, "d"), Some(D_MAP));
        assert!(s.apply(&ev));
        assert_eq!(s.status(OPS).unwrap(), "sub carol");
        assert_eq!(s.status(DEV).unwrap(), "sub alice");
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
