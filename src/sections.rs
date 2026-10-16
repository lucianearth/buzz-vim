//! Desktop's sidebar, read from the encrypted kind 30078 lanes it syncs:
//! `channel-sections` and `channel-stars`. We only read; the picker's choice
//! of which sections are today's tabs is kept locally.
//!
//! The order matches Desktop's sidebar: Starred, each section, the
//! unsectioned "Channels" group, then DMs.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

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
}
