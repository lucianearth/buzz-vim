//! Who an @ offers, in the order a person most likely means them.

use std::collections::HashSet;

use crate::model::Store;
use crate::telemetry::Agents;

/// People to offer for an @ in `channel` (inside thread `root` when given),
/// as (pubkey, name): whoever spoke in the thread, newest first; then the
/// channel's speakers, newest first; then its other members; then our
/// agents, the online ones first. Never `me`, never anyone in `skip`, never
/// someone with no name to type.
pub fn candidates(
    store: &Store,
    agents: &Agents,
    channel: &str,
    root: Option<&str>,
    skip: &HashSet<String>,
) -> Vec<(String, String)> {
    let ch = store.channels.get(channel);
    let mut keys: Vec<String> = Vec::new();
    if let (Some(c), Some(root)) = (ch, root) {
        let mut msgs: Vec<_> = c
            .replies
            .get(root)
            .into_iter()
            .flatten()
            .chain(std::iter::once(&root.to_string()))
            .filter_map(|id| c.messages.get(id))
            .collect();
        msgs.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        keys.extend(msgs.iter().map(|m| m.author.clone()));
    }
    if let Some(c) = ch {
        let mut msgs: Vec<_> = c.messages.values().filter(|m| !m.system).collect();
        msgs.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        keys.extend(msgs.iter().map(|m| m.author.clone()));
        keys.extend(c.members.iter().cloned());
    }
    // Agents only belong in channels: a DM is between the two of you.
    if !ch.is_some_and(|c| c.is_dm) {
        let mut ours: Vec<(&String, bool, String)> = agents
            .by_pubkey
            .iter()
            .filter(|(_, a)| a.owned)
            .map(|(k, a)| (k, a.online || a.is_busy(), name(store, agents, k)))
            .collect();
        ours.sort_by_key(|(_, online, n)| (!online, n.to_lowercase()));
        keys.extend(ours.into_iter().map(|(k, _, _)| k.clone()));
    }
    let mut seen = HashSet::new();
    keys.into_iter()
        .filter(|k| *k != store.me && !skip.contains(k) && seen.insert(k.clone()))
        .map(|k| {
            let n = name(store, agents, &k);
            (k, n)
        })
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

/// Profile name, else the name on our agent record, else "".
fn name(store: &Store, agents: &Agents, pk: &str) -> String {
    match store.profiles.get(pk) {
        Some(p) if !p.name.is_empty() => p.name.clone(),
        _ => agents
            .by_pubkey
            .get(pk)
            .map(|a| a.name.clone())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Message, Profile};

    fn msg(id: &str, author: &str, at: u64, root: Option<&str>) -> Message {
        Message {
            id: id.into(),
            author: author.into(),
            created_at: at,
            root: root.map(str::to_string),
            ..Default::default()
        }
    }

    fn named(store: &mut Store, pk: &str, n: &str) {
        store.profiles.insert(
            pk.into(),
            Profile {
                name: n.into(),
                ..Default::default()
            },
        );
    }

    #[test]
    fn thread_then_channel_then_members_then_online_agents() {
        let mut s = Store::new("me".into());
        for (pk, n) in [
            ("me", "me"),
            ("t", "threader"),
            ("c", "chatter"),
            ("m", "member"),
            ("on", "zed"),
            ("off", "abe"),
        ] {
            named(&mut s, pk, n);
        }
        let c = s.channel_mut("x");
        c.members = vec!["m".into(), "me".into(), "t".into()];
        for m in [
            msg("r", "me", 1, None),
            msg("r1", "t", 2, Some("r")),
            msg("o", "c", 9, None),
        ] {
            c.messages.insert(m.id.clone(), m);
        }
        c.replies.insert("r".into(), vec!["r1".into()]);
        let mut a = Agents::default();
        a.own("on", "zed");
        a.own("off", "abe");
        a.own("noname", "");
        a.known("on").online = true;
        let got: Vec<String> = candidates(&s, &a, "x", Some("r"), &HashSet::new())
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(got, ["threader", "chatter", "member", "zed", "abe"]);
        // Outside the thread the channel's newest speaker leads; skip drops.
        let skip = HashSet::from(["c".to_string()]);
        let got: Vec<String> = candidates(&s, &a, "x", None, &skip)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        assert_eq!(got, ["threader", "member", "zed", "abe"]);
    }
}
