//! Desktop's sidebar, read from the encrypted kind 30078 lanes it syncs:
//! `channel-sections` and `channel-stars`. We write only a channel move; the picker's choice
//! of which sections are today's tabs is kept locally.
//!
//! The order matches Desktop's sidebar: Starred, each section, the
//! unsectioned "Channels" group, then DMs.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::model::Store;

pub const LANE_SECTIONS: &str = "channel-sections";
pub const LANE_STARS: &str = "channel-stars";

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub channels: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Lanes {
    /// (id, name, order) for live sections
    pub sections: Vec<(String, String, f64)>,
    /// channel -> section id
    pub assignments: HashMap<String, String>,
    pub starred: HashSet<String>,
}

/// LWW register `[v_ms, dev, value]` -> value.
fn reg(v: &Value) -> Option<&Value> {
    v.as_array().and_then(|a| a.get(2))
}

pub fn parse_sections(doc: &Value, lanes: &mut Lanes) {
    lanes.sections.clear();
    lanes.assignments.clear();
    if let Some(meta) = doc.get("meta").filter(|m| m.is_object()) {
        // Authoritative register form.
        if let Some(s) = meta.get("s").and_then(Value::as_object) {
            for (id, fields) in s {
                let live = fields
                    .get("live")
                    .and_then(reg)
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if !live {
                    continue;
                }
                let name = fields
                    .get("name")
                    .and_then(reg)
                    .and_then(Value::as_str)
                    .unwrap_or("untitled");
                let order = fields
                    .get("order")
                    .and_then(reg)
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::MAX);
                lanes.sections.push((id.clone(), name.to_string(), order));
            }
        }
        if let Some(a) = meta.get("a").and_then(Value::as_object) {
            for (ch, r) in a {
                if let Some(sid) = reg(r).and_then(Value::as_str) {
                    lanes.assignments.insert(ch.clone(), sid.to_string());
                }
            }
        }
    } else {
        // Legacy projection.
        for s in doc
            .get("sections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(id), Some(name)) = (
                s.get("id").and_then(Value::as_str),
                s.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let order = s.get("order").and_then(Value::as_f64).unwrap_or(f64::MAX);
            lanes
                .sections
                .push((id.to_string(), name.to_string(), order));
        }
        for (ch, sid) in doc
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(sid) = sid.as_str() {
                lanes.assignments.insert(ch.clone(), sid.to_string());
            }
        }
    }
    lanes
        .sections
        .sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let live: HashSet<&String> = lanes.sections.iter().map(|s| &s.0).collect();
    lanes.assignments.retain(|_, sid| live.contains(sid));
}

pub fn parse_stars(doc: &Value, lanes: &mut Lanes) {
    lanes.starred = doc
        .get("channels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, v)| v.get("starred").and_then(Value::as_bool) == Some(true))
        .map(|(k, _)| k.clone())
        .collect();
}

/// The picker list, in sidebar order. Only channels you belong to, not archived.
pub fn build(lanes: &Lanes, store: &Store) -> Vec<Section> {
    let mut chans: Vec<&crate::model::Channel> = store
        .channels
        .values()
        .filter(|c| c.is_member && !c.archived)
        .collect();
    chans.sort_by_key(|c| c.name.to_lowercase());
    let stream =
        |c: &&&crate::model::Channel| !c.is_dm && c.ctype.as_deref().is_none_or(|t| t == "stream");
    let mut out = vec![];
    let starred: Vec<String> = chans
        .iter()
        .filter(|c| lanes.starred.contains(&c.id))
        .map(|c| c.id.clone())
        .collect();
    if !starred.is_empty() {
        out.push(Section {
            id: "★".into(),
            name: "★ Starred".into(),
            channels: starred,
        });
    }
    for (sid, name, _) in &lanes.sections {
        let channels: Vec<String> = chans
            .iter()
            .filter(stream)
            .filter(|c| lanes.assignments.get(&c.id) == Some(sid) && !lanes.starred.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        out.push(Section {
            id: sid.clone(),
            name: name.clone(),
            channels,
        });
    }
    let rest: Vec<String> = chans
        .iter()
        .filter(stream)
        .filter(|c| !lanes.assignments.contains_key(&c.id) && !lanes.starred.contains(&c.id))
        .map(|c| c.id.clone())
        .collect();
    if !rest.is_empty() {
        out.push(Section {
            id: "channels".into(),
            name: "Channels".into(),
            channels: rest,
        });
    }
    let dms: Vec<String> = chans
        .iter()
        .filter(|c| c.is_dm)
        .map(|c| c.id.clone())
        .collect();
    if !dms.is_empty() {
        out.push(Section {
            id: "dms".into(),
            name: "DMs".into(),
            channels: dms,
        });
    }
    out
}

/// The register `dev` this app stamps: 16 hex chars, stable per identity.
pub fn dev_id(pubkey_hex: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(format!("buzz-vim:{pubkey_hex}"))[..8])
}

fn max_version(node: &Value) -> i64 {
    match node {
        Value::Array(a) => a.first().and_then(Value::as_i64).unwrap_or(0),
        Value::Object(o) => o.values().map(max_version).max().unwrap_or(0),
        _ => 0,
    }
}

/// Desktop's register tree for a sections doc: `meta` without `v`, or a
/// legacy doc stamped at `legacy_v` the way Desktop imports it.
fn tree_of(doc: &Value, legacy_v: i64) -> serde_json::Map<String, Value> {
    if let Some(meta) = doc.get("meta").and_then(Value::as_object) {
        let mut t = meta.clone();
        t.remove("v");
        return t;
    }
    let dev = "0".repeat(16);
    let stamp = |v: Value| json!([legacy_v, dev, v]);
    let mut s = serde_json::Map::new();
    for sec in doc
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(name), Some(order)) = (
            sec.get("id").and_then(Value::as_str),
            sec.get("name").and_then(Value::as_str),
            sec.get("order").and_then(Value::as_f64),
        ) else {
            continue;
        };
        let icon = sec.get("icon").cloned().unwrap_or(Value::Null);
        s.insert(
            id.into(),
            json!({"name": stamp(name.into()), "icon": stamp(icon), "order": stamp(json!(order.round() as i64)), "live": stamp(true.into())}),
        );
    }
    let mut a = serde_json::Map::new();
    for (ch, sid) in doc
        .get("assignments")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if sid.as_str().is_some_and(|sid| s.contains_key(sid)) {
            a.insert(ch.clone(), stamp(sid.clone()));
        }
    }
    serde_json::Map::from_iter([
        ("s".to_string(), Value::Object(s)),
        ("a".to_string(), Value::Object(a)),
    ])
}

/// Desktop's `encodeDoc`: the legacy projection older clients read, plus the registers.
fn encode(tree: serde_json::Map<String, Value>) -> Value {
    let mut ranked: Vec<(String, String, Option<String>, f64)> = tree
        .get("s")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, n)| n.get("live").and_then(reg) == Some(&Value::Bool(true)))
        .filter_map(|(id, n)| {
            let name = n.get("name").and_then(reg)?.as_str()?.to_string();
            let icon = n
                .get("icon")
                .and_then(reg)
                .and_then(Value::as_str)
                .filter(|i| !i.is_empty())
                .map(String::from);
            let order = n
                .get("order")
                .and_then(reg)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            Some((id.clone(), name, icon, order))
        })
        .collect();
    ranked.sort_by(|a, b| a.3.total_cmp(&b.3).then_with(|| a.0.cmp(&b.0)));
    let live: HashSet<&str> = ranked.iter().map(|s| s.0.as_str()).collect();
    let mut assignments = serde_json::Map::new();
    for (ch, r) in tree
        .get("a")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if let Some(sid) = reg(r).and_then(Value::as_str).filter(|s| live.contains(s)) {
            assignments.insert(ch.clone(), sid.into());
        }
    }
    let sections: Vec<Value> = ranked
        .iter()
        .enumerate()
        .map(|(order, (id, name, icon, _))| match icon {
            Some(icon) => json!({"id": id, "name": name, "icon": icon, "order": order}),
            None => json!({"id": id, "name": name, "order": order}),
        })
        .collect();
    let mut meta = serde_json::Map::from_iter([("v".to_string(), json!(1))]);
    meta.extend(tree);
    json!({"version": 1, "sections": sections, "assignments": assignments, "meta": meta})
}

/// Desktop's sections doc with `channel` moved into section `target`, or
/// back to the unsectioned group with `None`. `head` is the newest relay
/// doc and its `created_at`; every other register is kept as it is, so
/// Desktop's per-field merge loses nothing.
pub fn move_channel(
    head: Option<(&Value, u64)>,
    channel: &str,
    target: Option<&str>,
    dev: &str,
    now_ms: i64,
) -> Value {
    let mut tree = head
        .map(|(doc, at)| tree_of(doc, at as i64 * 1000))
        .unwrap_or_default();
    let v = now_ms.max(max_version(&Value::Object(tree.clone())) + 1);
    let a = tree.entry("a").or_insert_with(|| json!({}));
    if let Some(a) = a.as_object_mut() {
        a.insert(channel.into(), json!([v, dev, target]));
    }
    encode(tree)
}

/// Read the newest sections doc, move `channel`, publish. Returns the
/// published event so the caller can fold it in straight away.
pub async fn publish_move(
    relay: &crate::relay::Relay,
    channel: &str,
    target: Option<&str>,
) -> anyhow::Result<nostr::Event> {
    use nostr::nips::nip44;
    let keys = &relay.keys;
    let me = keys.public_key();
    let heads = relay
        .query(&[
            json!({"kinds": [30078], "authors": [me.to_hex()], "#d": [LANE_SECTIONS], "limit": 1}),
        ])
        .await?;
    let head = heads
        .iter()
        .filter(|e| e.pubkey == me)
        .max_by_key(|e| (e.created_at, std::cmp::Reverse(e.id)));
    let doc = match head {
        Some(e) => Some((
            serde_json::from_str::<Value>(&nip44::decrypt(keys.secret_key(), &me, &e.content)?)?,
            e.created_at.as_secs(),
        )),
        None => None,
    };
    // Never write over a doc we can't read: Desktop would lose it.
    if let Some((d, _)) = &doc {
        if d.get("version") != Some(&json!(1))
            || d.get("meta").is_some_and(|m| m.get("v") != Some(&json!(1)))
        {
            anyhow::bail!("sections doc is a newer format; move it in Desktop");
        }
    }
    let now = chrono::Utc::now();
    let next = move_channel(
        doc.as_ref().map(|(d, at)| (d, *at)),
        channel,
        target,
        &dev_id(&me.to_hex()),
        now.timestamp_millis(),
    );
    let ct = nip44::encrypt(keys.secret_key(), &me, next.to_string(), nip44::Version::V2)?;
    let created = (now.timestamp() as u64).max(doc.as_ref().map_or(0, |d| d.1 + 1));
    let ev = relay.sign(
        nostr::EventBuilder::new(nostr::Kind::Custom(30078), ct)
            .tags([
                nostr::Tag::parse(["d", LANE_SECTIONS])?,
                nostr::Tag::parse(["t", LANE_SECTIONS])?,
            ])
            .custom_created_at(nostr::Timestamp::from(created)),
    )?;
    relay.publish(&ev).await?;
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registers_win_and_dead_sections_drop() {
        let doc = json!({
            "version": 1,
            "sections": [{"id": "stale", "name": "Stale", "order": 0}],
            "meta": {"v": 1,
                "s": {
                    "b": {"name": [1, "d", "Beta"], "order": [1, "d", 1], "live": [1, "d", true]},
                    "a": {"name": [1, "d", "Alpha"], "order": [1, "d", 0], "live": [1, "d", true]},
                    "x": {"name": [1, "d", "Gone"], "order": [1, "d", 2], "live": [2, "d", false]}
                },
                "a": {"c1": [1, "d", "a"], "c2": [1, "d", "x"], "c3": [1, "d", null]}}
        });
        let mut l = Lanes::default();
        parse_sections(&doc, &mut l);
        let names: Vec<&str> = l.sections.iter().map(|s| s.1.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
        assert_eq!(l.assignments.len(), 1);
        assert_eq!(l.assignments["c1"], "a");

        let legacy = json!({"sections": [{"id": "s", "name": "Old", "order": 0}], "assignments": {"c9": "s"}});
        parse_sections(&legacy, &mut l);
        assert_eq!(l.sections[0].1, "Old");
        assert_eq!(l.assignments["c9"], "s");

        parse_stars(
            &json!({"version": 1, "channels": {"c1": {"starred": true, "updatedAt": 1}, "c2": {"starred": false}}}),
            &mut l,
        );
        assert_eq!(l.starred, HashSet::from(["c1".to_string()]));
    }

    #[test]
    fn move_keeps_every_other_register() {
        let head = json!({"version": 1, "sections": [], "assignments": {},
            "meta": {"v": 1,
                "s": {
                    "b": {"name": [5, "aaaaaaaaaaaaaaaa", "Beta"], "icon": [5, "aaaaaaaaaaaaaaaa", "🐝"], "order": [5, "aaaaaaaaaaaaaaaa", 1], "live": [5, "aaaaaaaaaaaaaaaa", true]},
                    "a": {"name": [5, "aaaaaaaaaaaaaaaa", "Alpha"], "icon": [5, "aaaaaaaaaaaaaaaa", null], "order": [5, "aaaaaaaaaaaaaaaa", 0], "live": [5, "aaaaaaaaaaaaaaaa", true]},
                    "x": {"name": [5, "aaaaaaaaaaaaaaaa", "Gone"], "order": [5, "aaaaaaaaaaaaaaaa", 2], "live": [9, "aaaaaaaaaaaaaaaa", false]}
                },
                "a": {"c1": [5, "aaaaaaaaaaaaaaaa", "a"], "c2": [7, "aaaaaaaaaaaaaaaa", null]}}});
        let dev = dev_id("ab");
        assert_eq!(dev.len(), 16);
        let out = move_channel(Some((&head, 1)), "c1", Some("b"), &dev, 3);
        // Stamp beats everything seen, even with a slow clock.
        assert_eq!(out["meta"]["a"]["c1"], json!([10, dev, "b"]));
        let mut want = head["meta"].clone();
        want["a"]["c1"] = json!([10, dev, "b"]);
        assert_eq!(out["meta"], want);
        assert_eq!(out["assignments"], json!({"c1": "b"}));
        assert_eq!(
            out["sections"],
            json!([{"id": "a", "name": "Alpha", "order": 0}, {"id": "b", "name": "Beta", "icon": "🐝", "order": 1}])
        );
        // Back to the unsectioned group is a null register.
        let out = move_channel(Some((&out, 1)), "c1", None, &dev, 1_000);
        assert_eq!(out["meta"]["a"]["c1"], json!([1_000, dev, null]));
        assert_eq!(out["assignments"], json!({}));
    }

    #[test]
    fn move_imports_a_legacy_doc() {
        let legacy = json!({"version": 1, "sections": [{"id": "s", "name": "Old", "order": 0}], "assignments": {"c9": "s", "c8": "gone"}});
        let out = move_channel(
            Some((&legacy, 2)),
            "c1",
            Some("s"),
            "bbbbbbbbbbbbbbbb",
            5_000,
        );
        let z = "0".repeat(16);
        assert_eq!(out["meta"]["s"]["s"]["name"], json!([2_000, z, "Old"]));
        assert_eq!(out["meta"]["a"]["c9"], json!([2_000, z, "s"]));
        assert!(out["meta"]["a"].get("c8").is_none());
        assert_eq!(out["assignments"], json!({"c9": "s", "c1": "s"}));
        let mut l = Lanes::default();
        parse_sections(&out, &mut l);
        assert_eq!(l.assignments["c1"], "s");
    }
}
