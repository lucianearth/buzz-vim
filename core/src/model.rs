//! Everything the screen shows, folded from relay events. Pure data, no IO,
//! so it is unit-testable.

use std::collections::{BTreeMap, HashMap, HashSet};

use nostr::Event;

pub const KIND_PROFILE: u16 = 0;
pub const KIND_DELETION: u16 = 5;
pub const KIND_REACTION: u16 = 7;
pub const KIND_MESSAGE: u16 = 9;
pub const KIND_MESSAGE_V2: u16 = 40002;
pub const KIND_EDIT: u16 = 40003;
pub const KIND_DELETE_BUZZ: u16 = 9005;
pub const KIND_CHANNEL_META: u16 = 39000;
pub const KIND_CHANNEL_MEMBERS: u16 = 39002;
pub const KIND_THREAD_SUMMARY: u16 = 39005;
pub const KIND_SYSTEM: u16 = 40099;

/// Rows the main timeline shows (Desktop's channel window set, minus job
/// and huddle cards we don't render).
pub const TIMELINE_KINDS: &[u16] = &[KIND_MESSAGE, KIND_MESSAGE_V2, 40008, KIND_SYSTEM];
/// Everything the live tail of a channel needs.
pub const LIVE_KINDS: &[u16] = &[
    KIND_DELETION,
    KIND_REACTION,
    KIND_DELETE_BUZZ,
    KIND_MESSAGE,
    KIND_MESSAGE_V2,
    40008,
    KIND_EDIT,
    KIND_SYSTEM,
    KIND_THREAD_SUMMARY,
];
/// Kinds that count as unread (no system rows).
pub const UNREAD_KINDS: &[u16] = &[KIND_MESSAGE, KIND_MESSAGE_V2];

pub const THUMBS_UP: &str = "👍";

pub fn tag<'a>(ev: &'a Event, name: &str) -> Option<&'a str> {
    ev.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some(name))
            .then(|| s.get(1).map(String::as_str))
            .flatten()
    })
}

pub fn tags<'a>(ev: &'a Event, name: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
    ev.tags
        .iter()
        .map(|t| t.as_slice())
        .filter(move |s| s.first().map(String::as_str) == Some(name))
}

#[derive(Debug, Clone)]
pub struct Media {
    pub url: String,
    pub mime: String,
    pub dim: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Message {
    pub id: String,
    pub author: String,
    pub created_at: u64,
    pub content: String,
    pub edited: bool,
    pub deleted: bool,
    /// Thread root this message replies into, if any.
    pub root: Option<String>,
    pub mentions: Vec<String>,
    pub media: Vec<Media>,
    pub system: bool,
}

impl Message {
    pub fn from_event(ev: &Event) -> Self {
        let mut root = None;
        let mut reply = None;
        for t in tags(ev, "e") {
            let id = t.get(1).cloned();
            match t.get(3).map(String::as_str) {
                Some("root") => root = id,
                Some("reply") => reply = id,
                _ => {}
            }
        }
        // NIP-10 as Buzz writes it: a direct reply carries only `reply`.
        let root = root.or(reply);
        let media = tags(ev, "imeta")
            .filter_map(|t| {
                let mut url = None;
                let mut mime = String::new();
                let mut dim = None;
                for part in &t[1..] {
                    let (k, v) = part.split_once(' ')?;
                    match k {
                        "url" => url = Some(v.to_string()),
                        "m" => mime = v.to_string(),
                        "dim" => {
                            dim = v
                                .split_once('x')
                                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                        }
                        _ => {}
                    }
                }
                Some(Media {
                    url: url?,
                    mime,
                    dim,
                })
            })
            .collect();
        Message {
            id: ev.id.to_hex(),
            author: ev.pubkey.to_hex(),
            created_at: ev.created_at.as_secs(),
            content: ev.content.clone(),
            edited: false,
            deleted: false,
            root,
            // A notified mention carries both a `p` and a `mention` tag.
            mentions: tags(ev, "p")
                .chain(tags(ev, "mention"))
                .filter_map(|t| t.get(1).cloned())
                .fold(vec![], |mut out, pk| {
                    if !out.contains(&pk) {
                        out.push(pk);
                    }
                    out
                }),
            media,
            system: ev.kind.as_u16() == KIND_SYSTEM,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Reaction {
    pub emoji: String,
    /// reactor pubkey -> reaction event id (to undo our own)
    pub by: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub about: String,
    pub is_dm: bool,
    pub archived: bool,
    pub is_member: bool,
    /// First `t` tag of the 39000: stream, forum, dm, workflow, system.
    pub ctype: Option<String>,
    /// Relay-materialized reply counts (39005), root -> count.
    pub summary_replies: HashMap<String, (usize, u64)>,
    pub members: Vec<String>,
    /// Message id -> message, every message we have for the channel.
    pub messages: HashMap<String, Message>,
    /// Top-level timeline, oldest first.
    pub timeline: Vec<String>,
    /// root id -> reply ids, oldest first.
    pub replies: HashMap<String, Vec<String>>,
    pub loaded: bool,
    /// Unread top-level messages: id -> created_at.
    pub unread: HashMap<String, u64>,
    /// The unread ones that mention us.
    pub mentioned: HashSet<String>,
    pub oldest: Option<u64>,
    pub exhausted: bool,
}

impl Channel {
    pub fn reply_count(&self, root: &str) -> usize {
        let loaded = self.replies.get(root).map_or(0, Vec::len);
        self.summary_replies
            .get(root)
            .map_or(0, |s| s.0)
            .max(loaded)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub name: String,
    pub is_agent: bool,
    /// Avatar URL, "" when none.
    pub picture: String,
}

#[derive(Default)]
pub struct Store {
    pub me: String,
    pub channels: BTreeMap<String, Channel>,
    pub profiles: HashMap<String, Profile>,
    /// target event id -> emoji -> reaction
    pub reactions: HashMap<String, Vec<Reaction>>,
    seen: HashSet<String>,
    /// reaction event id -> (target, emoji, reactor)
    reaction_index: HashMap<String, (String, String, String)>,
    /// edits/deletes that arrived before their target
    pending_edits: HashMap<String, (u64, String, String)>,
    pending_deletes: HashSet<String>,
    edit_times: HashMap<String, u64>,
}

pub enum Applied {
    Nothing,
    /// A new message landed in this channel (id).
    Message {
        channel: String,
        id: String,
    },
    Changed,
}

impl Store {
    pub fn new(me: String) -> Self {
        Store {
            me,
            ..Default::default()
        }
    }

    /// The agents in `mentioned` that are `ours` and aren't in `channel`
    /// yet. A kind 0 bot flag is self-declared, so it doesn't count. DMs
    /// keep the people they were opened with.
    pub fn outsiders(
        &self,
        channel: &str,
        mentioned: &[String],
        ours: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let Some(ch) = self.channels.get(channel).filter(|c| !c.is_dm) else {
            return vec![];
        };
        mentioned
            .iter()
            .filter(|pk| !ch.members.contains(pk) && ours(pk))
            .cloned()
            .collect()
    }

    pub fn name_of(&self, pubkey: &str) -> String {
        match self.profiles.get(pubkey) {
            Some(p) if !p.name.is_empty() => p.name.clone(),
            _ => format!("{}…", pubkey.chars().take(8).collect::<String>()),
        }
    }

    pub fn channel_mut(&mut self, id: &str) -> &mut Channel {
        self.channels
            .entry(id.to_string())
            .or_insert_with(|| Channel {
                id: id.to_string(),
                ..Default::default()
            })
    }

    /// Fold one event. Idempotent: replays of the same event are ignored.
    pub fn apply(&mut self, ev: &Event) -> Applied {
        let kind = ev.kind.as_u16();
        // Replaceable kinds are folded every time (newest wins), the rest once.
        let replaceable = matches!(
            kind,
            KIND_PROFILE | KIND_CHANNEL_META | KIND_CHANNEL_MEMBERS
        );
        if !replaceable && !self.seen.insert(ev.id.to_hex()) {
            return Applied::Nothing;
        }
        match kind {
            KIND_MESSAGE | KIND_MESSAGE_V2 | 40008 | KIND_SYSTEM => self.apply_message(ev),
            KIND_THREAD_SUMMARY => {
                let Some(root) = tag(ev, "d").or_else(|| tag(ev, "e")).map(str::to_string) else {
                    return Applied::Nothing;
                };
                let ch = tag(ev, "h")
                    .map(str::to_string)
                    .or_else(|| self.find_message(&root).map(|(c, _)| c.id.clone()));
                let Some(ch) = ch else {
                    return Applied::Nothing;
                };
                let v: serde_json::Value = serde_json::from_str(&ev.content).unwrap_or_default();
                let n = v
                    .get("descendant_count")
                    .or_else(|| v.get("reply_count"))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0) as usize;
                let last = v.get("last_reply_at").and_then(|x| x.as_u64()).unwrap_or(0);
                self.channel_mut(&ch)
                    .summary_replies
                    .insert(root, (n, last));
                Applied::Changed
            }
            KIND_REACTION => self.apply_reaction(ev),
            KIND_EDIT => {
                let Some(target) = tag(ev, "e").map(str::to_string) else {
                    return Applied::Nothing;
                };
                let at = ev.created_at.as_secs();
                let editor = ev.pubkey.to_hex();
                if self.edit_times.get(&target).is_some_and(|t| *t > at) {
                    return Applied::Nothing;
                }
                if let Some(m) = self.find_message_mut(&target) {
                    // Only the author's edits count.
                    if m.author != editor {
                        return Applied::Nothing;
                    }
                    m.content = ev.content.clone();
                    m.edited = true;
                    self.edit_times.insert(target, at);
                    Applied::Changed
                } else {
                    self.pending_edits
                        .insert(target, (at, ev.content.clone(), editor));
                    Applied::Nothing
                }
            }
            KIND_DELETION | KIND_DELETE_BUZZ => {
                let mut changed = false;
                for t in tags(ev, "e") {
                    let Some(target) = t.get(1).cloned() else {
                        continue;
                    };
                    let own_reaction = self
                        .reaction_index
                        .get(&target)
                        .is_some_and(|(_, _, who)| *who == ev.pubkey.to_hex());
                    if let Some((tgt, emoji, who)) = own_reaction
                        .then(|| self.reaction_index.remove(&target))
                        .flatten()
                    {
                        if let Some(list) = self.reactions.get_mut(&tgt) {
                            for r in list.iter_mut().filter(|r| r.emoji == emoji) {
                                if r.by.get(&who) == Some(&target) {
                                    r.by.remove(&who);
                                }
                            }
                            list.retain(|r| !r.by.is_empty());
                        }
                        changed = true;
                    } else if let Some(m) = self.find_message_mut(&target) {
                        // Like Desktop: the relay already checked who may delete,
                        // and deleted messages simply disappear.
                        m.deleted = true;
                        self.unlist(&target);
                        changed = true;
                    } else {
                        self.pending_deletes.insert(target);
                    }
                }
                if changed {
                    Applied::Changed
                } else {
                    Applied::Nothing
                }
            }
            KIND_PROFILE => {
                let v: serde_json::Value = serde_json::from_str(&ev.content).unwrap_or_default();
                let name = ["display_name", "displayName", "name"]
                    .iter()
                    .find_map(|k| v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()))
                    .unwrap_or("")
                    .to_string();
                // NIP-OA: an agent's profile carries its owner's `auth` tag.
                let is_agent = tag(ev, "auth").is_some();
                let p = self.profiles.entry(ev.pubkey.to_hex()).or_default();
                p.name = name;
                p.is_agent |= is_agent;
                p.picture = v
                    .get("picture")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                Applied::Changed
            }
            KIND_CHANNEL_META => {
                let Some(d) = tag(ev, "d").map(str::to_string) else {
                    return Applied::Nothing;
                };
                let name = tag(ev, "name").unwrap_or("").to_string();
                let about = tag(ev, "about").unwrap_or("").to_string();
                let ctype = tag(ev, "t").map(str::to_string);
                let hidden = tags(ev, "hidden").next().is_some();
                let is_dm = ctype.as_deref() == Some("dm") || (ctype.is_none() && hidden);
                let archived = tag(ev, "archived") == Some("true");
                let ch = self.channel_mut(&d);
                ch.name = name;
                ch.about = about;
                ch.is_dm = is_dm;
                ch.ctype = ctype;
                ch.archived = archived;
                if is_dm {
                    ch.members = tags(ev, "p").filter_map(|t| t.get(1).cloned()).collect();
                }
                Applied::Changed
            }
            KIND_CHANNEL_MEMBERS => {
                let Some(d) = tag(ev, "d").map(str::to_string) else {
                    return Applied::Nothing;
                };
                let mut members = vec![];
                for t in tags(ev, "p") {
                    let Some(pk) = t.get(1) else { continue };
                    if t.get(3).map(String::as_str) == Some("bot") {
                        self.profiles.entry(pk.clone()).or_default().is_agent = true;
                    }
                    members.push(pk.clone());
                }
                let me = self.me.clone();
                let ch = self.channel_mut(&d);
                ch.is_member = members.contains(&me);
                ch.members = members;
                Applied::Changed
            }
            _ => Applied::Nothing,
        }
    }

    fn find_message_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.channels
            .values_mut()
            .find_map(|c| c.messages.get_mut(id))
    }

    pub fn find_message(&self, id: &str) -> Option<(&Channel, &Message)> {
        self.channels
            .values()
            .find_map(|c| c.messages.get(id).map(|m| (c, m)))
    }

    fn apply_message(&mut self, ev: &Event) -> Applied {
        let Some(ch_id) = tag(ev, "h").map(str::to_string) else {
            return Applied::Nothing;
        };
        let mut msg = Message::from_event(ev);
        if let Some((at, content, editor)) = self.pending_edits.remove(&msg.id) {
            if editor == msg.author {
                msg.content = content;
                msg.edited = true;
                self.edit_times.insert(msg.id.clone(), at);
            }
        }
        if self.pending_deletes.remove(&msg.id) {
            msg.deleted = true;
        }
        let id = msg.id.clone();
        let at = msg.created_at;
        let root = msg.root.clone();
        let deleted = msg.deleted;
        let ch = self.channel_mut(&ch_id);
        ch.oldest = Some(ch.oldest.map_or(at, |o| o.min(at)));
        ch.messages.insert(id.clone(), msg);
        if deleted {
            return Applied::Changed;
        }
        let list = match &root {
            Some(r) => ch.replies.entry(r.clone()).or_default(),
            None => &mut ch.timeline,
        };
        insert_sorted(list, &ch.messages, id.clone());
        Applied::Message { channel: ch_id, id }
    }

    fn apply_reaction(&mut self, ev: &Event) -> Applied {
        // NIP-25: the last `e` tag is the target.
        let Some(target) = tags(ev, "e").last().and_then(|t| t.get(1)).cloned() else {
            return Applied::Nothing;
        };
        let emoji = match ev.content.trim() {
            "" => "+".to_string(),
            s => s.to_string(),
        };
        let who = ev.pubkey.to_hex();
        let rid = ev.id.to_hex();
        if self.pending_deletes.remove(&rid) {
            return Applied::Nothing;
        }
        let list = self.reactions.entry(target.clone()).or_default();
        match list.iter_mut().find(|r| r.emoji == emoji) {
            Some(r) => {
                r.by.insert(who.clone(), rid.clone());
            }
            None => list.push(Reaction {
                emoji: emoji.clone(),
                by: BTreeMap::from([(who.clone(), rid.clone())]),
            }),
        }
        self.reaction_index.insert(rid, (target, emoji, who));
        Applied::Changed
    }

    /// Take a message out of the timeline/thread lists (it stays in `messages`).
    fn unlist(&mut self, id: &str) {
        for c in self.channels.values_mut() {
            c.timeline.retain(|x| x != id);
            for list in c.replies.values_mut() {
                list.retain(|x| x != id);
            }
        }
    }

    /// Drop an optimistic message the relay refused.
    pub fn remove_message(&mut self, id: &str) {
        for c in self.channels.values_mut() {
            if let Some(m) = c.messages.remove(id) {
                match &m.root {
                    Some(r) => {
                        if let Some(list) = c.replies.get_mut(r) {
                            list.retain(|x| x != id);
                        }
                    }
                    None => c.timeline.retain(|x| x != id),
                }
            }
        }
        self.seen.remove(id);
    }

    /// Our own reaction event id for `emoji` on `target`, if we've reacted.
    pub fn my_reaction(&self, target: &str, emoji: &str) -> Option<String> {
        self.reactions
            .get(target)?
            .iter()
            .find(|r| r.emoji == emoji)?
            .by
            .get(&self.me)
            .cloned()
    }
}

fn insert_sorted(list: &mut Vec<String>, msgs: &HashMap<String, Message>, id: String) {
    let key = |i: &String| msgs.get(i).map(|m| (m.created_at, m.id.clone()));
    let k = key(&id);
    let pos = list.partition_point(|x| key(x) < k);
    if list.get(pos) != Some(&id) {
        list.insert(pos, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn ev(keys: &Keys, kind: u16, content: &str, tags: Vec<Vec<&str>>, at: u64) -> Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.into_iter().map(|t| Tag::parse(t).unwrap()))
            .custom_created_at(nostr::Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    #[test]
    fn outsiders_are_mentioned_agents_not_yet_here() {
        let mut s = Store::new("me".into());
        let ch = s.channel_mut("c");
        ch.members = vec!["me".into(), "bot1".into()];
        s.profiles.entry("bot3".into()).or_default().is_agent = true;
        let said: Vec<String> = ["bot1", "bot2", "bot3", "human"].map(String::from).to_vec();
        let agent = |pk: &str| pk == "bot1" || pk == "bot2";
        assert_eq!(s.outsiders("c", &said, agent), vec!["bot2"]);
        s.channel_mut("d").is_dm = true;
        assert!(s.outsiders("d", &said, agent).is_empty());
        assert!(s.outsiders("nope", &said, agent).is_empty());
    }

    #[test]
    fn a_mention_with_p_and_mention_tags_counts_once() {
        let a = Keys::generate();
        let pk = "ab".repeat(32);
        let m = ev(
            &a,
            9,
            "hi",
            vec![vec!["h", "c"], vec!["p", &pk], vec!["mention", &pk]],
            1,
        );
        assert_eq!(Message::from_event(&m).mentions, vec![pk]);
    }

    #[test]
    fn unknown_names_cut_by_character() {
        let s = Store::new("me".into());
        // Mention tags are untrusted; a non-hex one mustn't panic the draw.
        assert_eq!(s.name_of("aaaaaaaé"), "aaaaaaaé…");
        assert_eq!(s.name_of("ééééééééé"), "éééééééé…");
        assert_eq!(s.name_of(&"ab".repeat(32)), "abababab…");
    }

    #[test]
    fn timeline_threads_reactions_edits_deletes() {
        let a = Keys::generate();
        let b = Keys::generate();
        let mut s = Store::new(a.public_key().to_hex());
        let ch = "c1";
        let m2 = ev(&a, 9, "second", vec![vec!["h", ch]], 20);
        let m1 = ev(&b, 9, "first", vec![vec!["h", ch]], 10);
        s.apply(&m2);
        s.apply(&m1);
        s.apply(&m1); // replay ignored
        let c = &s.channels[ch];
        assert_eq!(c.timeline, vec![m1.id.to_hex(), m2.id.to_hex()]);

        let r = ev(
            &b,
            9,
            "reply",
            vec![vec!["h", ch], vec!["e", &m1.id.to_hex(), "", "reply"]],
            30,
        );
        s.apply(&r);
        assert_eq!(s.channels[ch].timeline.len(), 2);
        assert_eq!(s.channels[ch].reply_count(&m1.id.to_hex()), 1);

        let up = ev(&a, 7, THUMBS_UP, vec![vec!["e", &m1.id.to_hex()]], 31);
        s.apply(&up);
        assert_eq!(
            s.my_reaction(&m1.id.to_hex(), THUMBS_UP),
            Some(up.id.to_hex())
        );
        s.apply(&ev(&a, 5, "", vec![vec!["e", &up.id.to_hex()]], 32));
        assert_eq!(s.my_reaction(&m1.id.to_hex(), THUMBS_UP), None);

        // Edit arriving before its target still applies.
        let late = ev(&a, 9, "typo", vec![vec!["h", ch]], 40);
        s.apply(&ev(
            &a,
            40003,
            "fixed",
            vec![vec!["h", ch], vec!["e", &late.id.to_hex()]],
            41,
        ));
        s.apply(&late);
        let m = &s.channels[ch].messages[&late.id.to_hex()];
        assert_eq!((m.content.as_str(), m.edited), ("fixed", true));

        // Someone else's edit is ignored.
        s.apply(&ev(
            &b,
            40003,
            "hijack",
            vec![vec!["h", ch], vec!["e", &late.id.to_hex()]],
            45,
        ));
        assert_eq!(s.channels[ch].messages[&late.id.to_hex()].content, "fixed");

        s.apply(&ev(&a, 5, "", vec![vec!["e", &m2.id.to_hex()]], 51));
        assert!(s.channels[ch].messages[&m2.id.to_hex()].deleted);
        assert!(!s.channels[ch].timeline.contains(&m2.id.to_hex()));
        // A delete that lands before its message keeps it out of the timeline.
        let gone = ev(&a, 9, "gone", vec![vec!["h", ch]], 55);
        s.apply(&ev(&a, 5, "", vec![vec!["e", &gone.id.to_hex()]], 56));
        s.apply(&gone);
        assert!(!s.channels[ch].timeline.contains(&gone.id.to_hex()));

        // A 39005 summary counts replies we haven't loaded.
        s.apply(&ev(
            &a,
            39005,
            r#"{"reply_count":7}"#,
            vec![vec!["d", &m2.id.to_hex()], vec!["h", ch]],
            60,
        ));
        assert_eq!(s.channels[ch].reply_count(&m2.id.to_hex()), 7);
    }
}
