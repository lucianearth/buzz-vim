//! Read positions shared with Buzz Desktop (NIP-RS): encrypted kind 30078
//! events, `d = read-state:<slot>`, holding `{context: unix-seconds}`. A
//! channel's context is its UUID. Every device publishes its own slot; the
//! position for a channel is the max across all slots.

use nostr::{Event, EventBuilder, Keys, Kind, Tag, nips::nip44};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const D_PREFIX: &str = "read-state:";
/// Desktop debounces its publishes by the same amount.
const DEBOUNCE: Duration = Duration::from_secs(5);

pub struct ReadState {
    slot: String,
    client_id: String,
    /// Max position per context across every slot, ours included.
    merged: HashMap<String, u64>,
    /// What our slot holds; republished whole each time.
    mine: HashMap<String, u64>,
    /// When our slot changed and hasn't been published yet.
    pending: Option<Instant>,
    pub loaded: bool,
}

/// Is this one of anyone's read-state slots (the shape NIP-RS requires)?
pub fn is_read_state(ev: &Event) -> bool {
    let ds: Vec<&str> = ev
        .tags
        .iter()
        .filter_map(|t| match t.as_slice() {
            [k, v, ..] if k == "d" => Some(v.as_str()),
            _ => None,
        })
        .collect();
    let rs_tags = ev
        .tags
        .iter()
        .filter(|t| matches!(t.as_slice(), [k, v, ..] if k == "t" && v == "read-state"))
        .count();
    ds.len() == 1
        && rs_tags == 1
        && ds[0].strip_prefix(D_PREFIX).is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

impl ReadState {
    pub fn new(slot: String, client_id: String) -> Self {
        ReadState {
            slot,
            client_id,
            merged: HashMap::new(),
            mine: HashMap::new(),
            pending: None,
            loaded: false,
        }
    }

    pub fn slot(&self) -> &str {
        &self.slot
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn read_at(&self, context: &str) -> Option<u64> {
        self.merged.get(context).copied()
    }

    /// Fold one of our own read-state events. Returns true if any position
    /// moved forward.
    pub fn apply(&mut self, keys: &Keys, ev: &Event) -> bool {
        if ev.pubkey != keys.public_key() || !is_read_state(ev) {
            return false;
        }
        let Ok(plain) = nip44::decrypt(keys.secret_key(), &ev.pubkey, &ev.content) else {
            return false;
        };
        let Some((client_id, contexts)) = parse(&plain) else {
            return false;
        };
        let ours = model_d(ev).is_some_and(|d| d == format!("{D_PREFIX}{}", self.slot));
        if ours {
            if client_id != self.client_id {
                // Someone else writes this slot: move ours somewhere new.
                self.slot = new_id();
                self.pending.get_or_insert_with(Instant::now);
            } else {
                for (k, v) in &contexts {
                    bump(&mut self.mine, k, *v);
                }
            }
        }
        let mut moved = false;
        for (k, v) in contexts {
            moved |= bump(&mut self.merged, &k, v);
        }
        moved
    }

    /// Mark a context read through `at`.
    pub fn mark(&mut self, context: &str, at: u64) {
        bump(&mut self.merged, context, at);
        if bump(&mut self.mine, context, at) {
            self.pending.get_or_insert_with(Instant::now);
        }
    }

    /// Our slot as a signed event, once it has waited out the debounce (or
    /// right away with `now`). Only after the startup load, so a fresh slot
    /// never overwrites positions we haven't seen yet.
    pub fn take_publish(&mut self, keys: &Keys, now: bool) -> Option<Event> {
        let since = self.pending?;
        if !self.loaded || (!now && since.elapsed() < DEBOUNCE) {
            return None;
        }
        self.pending = None;
        let blob = json!({"v": 1, "client_id": self.client_id, "contexts": self.mine});
        let content = nip44::encrypt(
            keys.secret_key(),
            &keys.public_key(),
            blob.to_string(),
            nip44::Version::V2,
        )
        .ok()?;
        EventBuilder::new(Kind::Custom(30078), content)
            .tags([
                Tag::parse(["d", &format!("{D_PREFIX}{}", self.slot)]).ok()?,
                Tag::parse(["t", "read-state"]).ok()?,
            ])
            .sign_with_keys(keys)
            .ok()
    }
}

fn model_d(ev: &Event) -> Option<&str> {
    crate::model::tag(ev, "d")
}

fn bump(map: &mut HashMap<String, u64>, k: &str, v: u64) -> bool {
    match map.get(k) {
        Some(&old) if old >= v => false,
        _ => {
            map.insert(k.to_string(), v);
            true
        }
    }
}

/// `{v: 1, client_id, contexts}`, keeping only plain channel/thread
/// positions (not the `ov_*` manual-unread entries).
fn parse(plain: &str) -> Option<(String, HashMap<String, u64>)> {
    let v: Value = serde_json::from_str(plain).ok()?;
    if v.get("v")?.as_u64()? != 1 {
        return None;
    }
    let client_id = v.get("client_id")?.as_str()?.to_string();
    if client_id.is_empty() || client_id.chars().count() > 64 {
        return None;
    }
    let contexts = v
        .get("contexts")?
        .as_object()?
        .iter()
        .filter(|(k, _)| k.len() <= 256 && !k.starts_with("ov_") && !k.starts_with("esc:"))
        .filter_map(|(k, v)| Some((k.clone(), v.as_u64().filter(|&t| t <= u32::MAX as u64)?)))
        .collect();
    Some((client_id, contexts))
}

/// 32 lowercase hex characters.
pub fn new_id() -> String {
    hex::encode(Keys::generate().secret_key().as_secret_bytes())[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_event(keys: &Keys, slot: &str, client: &str, contexts: Value) -> Event {
        let blob = json!({"v": 1, "client_id": client, "contexts": contexts});
        let content = nip44::encrypt(
            keys.secret_key(),
            &keys.public_key(),
            blob.to_string(),
            nip44::Version::V2,
        )
        .unwrap();
        EventBuilder::new(Kind::Custom(30078), content)
            .tags([
                Tag::parse(["d", &format!("{D_PREFIX}{slot}")]).unwrap(),
                Tag::parse(["t", "read-state"]).unwrap(),
            ])
            .sign_with_keys(keys)
            .unwrap()
    }

    #[test]
    fn merges_slots_by_max_and_round_trips_ours() {
        let keys = Keys::generate();
        let mut rs = ReadState::new(new_id(), "buzz-vim-test".into());
        let desktop = slot_event(&keys, &new_id(), "desktop", json!({"c1": 100, "c2": 50}));
        assert!(rs.apply(&keys, &desktop));
        assert_eq!(rs.read_at("c1"), Some(100));
        rs.loaded = true;
        rs.mark("c2", 80);
        rs.mark("c1", 90);
        assert_eq!(rs.read_at("c1"), Some(100));
        let ev = rs.take_publish(&keys, true).unwrap();
        assert!(is_read_state(&ev));
        let mut other = ReadState::new(new_id(), "x".into());
        other.apply(&keys, &ev);
        assert_eq!(other.read_at("c2"), Some(80));
        assert_eq!(other.read_at("c1"), Some(90));
        assert!(rs.take_publish(&keys, true).is_none());
    }

    #[test]
    fn someone_elses_slot_or_key_is_ignored_or_moved() {
        let keys = Keys::generate();
        let slot = new_id();
        let mut rs = ReadState::new(slot.clone(), "me".into());
        let stranger = Keys::generate();
        assert!(!rs.apply(
            &keys,
            &slot_event(&stranger, &new_id(), "x", json!({"c1": 5}))
        ));
        rs.apply(
            &keys,
            &slot_event(&keys, &slot, "other-client", json!({"c1": 5})),
        );
        assert_ne!(rs.slot(), slot);
    }
}
