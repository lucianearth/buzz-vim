//! Tab 0, "Needs you": what waits on the operator, derived from relay data
//! only (messages, profiles, checklist records), so both apps show the same
//! rows and nothing is a local flag.
//!
//! Needs an answer: channels the watcher says wait on him (stalled, waiting
//! on lucian, waiting on release), parked items with their question, and
//! people's @mentions and DMs he hasn't answered. FYI: bots' @mentions of
//! him the watcher didn't flag. A message row clears once he posts in its
//! thread (in a DM: anywhere in it) after the message; a watch or item row
//! once the record moves on.

use std::collections::HashMap;

use crate::checklist::{Checklists, Status, WatchState};
use crate::model::{Message, Store};
use crate::telemetry::Agents;

/// How far back an unanswered mention still counts.
pub const HORIZON: u64 = crate::unread::HORIZON;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// Waits on the operator.
    Answer,
    /// Bots mentioned him; nothing waits.
    Fyi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The watcher's state for the channel.
    Watch(WatchState),
    /// A parked checklist item.
    Parked,
    /// A person @mentioned him.
    Mention,
    /// A person wrote to him in a DM.
    Dm,
    /// A bot @mentioned him.
    Bot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub section: Section,
    pub reason: Reason,
    pub channel: String,
    /// Pubkey of whoever asks; "" when nobody is known (the watcher alone).
    pub who: String,
    /// One line.
    pub text: String,
    /// When it started waiting: the message's time, the watch's `since`,
    /// the item's last update.
    pub at: u64,
    /// The message to open at, if there is one.
    pub message: Option<String>,
    /// The thread a reply goes into (None: the channel itself).
    pub thread: Option<String>,
    /// Stable while the row stands for the same thing: notifications and
    /// the jump key go by it.
    pub key: String,
}

impl Row {
    /// Where opening the row goes: the message (in its thread), else the
    /// thread, else the channel.
    pub fn link(&self) -> String {
        let ch = &self.channel;
        match (&self.message, &self.thread) {
            (Some(id), t) => crate::checklist::message_link(ch, id, t.as_deref()),
            (None, Some(t)) => format!("buzz://message?channel={ch}&id={t}&thread={t}"),
            (None, None) => format!("buzz://message?channel={ch}"),
        }
    }

    /// Short tag for the row's kind.
    pub fn label(&self) -> String {
        match &self.reason {
            Reason::Watch(s) => s.label(),
            Reason::Parked => "parked".into(),
            Reason::Mention => "mention".into(),
            Reason::Dm => "dm".into(),
            Reason::Bot => "bot".into(),
        }
    }
}

/// A bot, as the apps already tell: a NIP-OA or channel `bot` role on its
/// profile, an agent we've seen telemetry or presence from, or the watcher.
pub fn is_bot(store: &Store, agents: &Agents, lists: &Checklists, pk: &str) -> bool {
    store.profiles.get(pk).is_some_and(|p| p.is_agent)
        || agents.by_pubkey.contains_key(pk)
        || lists.trust.watchers.iter().any(|w| w == pk)
}

/// The first non-empty line, trimmed.
pub fn one_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// What tab 0 reads, across `channels` (the ones we're in): mentions of
/// the operator, his own posts (they answer), every DM message (not every
/// client p-tags DMs) and the checklist records. `since` = None asks for
/// history over the horizon; Some(now) is the live tail. The relay only
/// tails channel events for a REQ with `#h`, so the ids go in every filter.
/// History asks for his posts and the records one channel at a time: the
/// relay caps each filter at 1000 events, which all channels together
/// could pass.
pub fn filters(
    channels: &[String],
    dms: &[String],
    operator: &str,
    now: u64,
    live: bool,
) -> Vec<serde_json::Value> {
    use serde_json::json;
    let kinds = crate::model::UNREAD_KINDS;
    let (since, limit) = if live {
        (now, 0)
    } else {
        (now.saturating_sub(HORIZON), 1000)
    };
    let chans: Vec<&String> = channels.iter().take(2000).collect();
    if chans.is_empty() {
        return vec![];
    }
    let checklist = crate::checklist::KINDS;
    let tag = crate::checklist::TAG;
    let mut f = vec![
        json!({"kinds": kinds, "#h": chans, "#p": [operator], "since": since, "limit": limit}),
    ];
    if live {
        f.push(
            json!({"kinds": kinds, "#h": chans, "authors": [operator], "since": since, "limit": 0}),
        );
        f.push(json!({"kinds": checklist, "#h": chans, "#t": [tag], "since": now, "limit": 0}));
    } else {
        for c in &chans {
            f.push(json!({"kinds": kinds, "#h": [c], "authors": [operator], "since": since, "limit": 1000}));
            f.push(json!({"kinds": checklist, "#h": [c], "#t": [tag], "since": 0, "limit": 500}));
        }
    }
    if !dms.is_empty() {
        f.push(json!({"kinds": kinds, "#h": dms, "since": since, "limit": limit}));
    }
    f
}

/// Every row, Needs an answer first (oldest first), then FYI (newest
/// first, the latest `FYI_MAX` only).
/// The operator is the checklist owner (config `owner`, default lucian).
pub fn rows(
    store: &Store,
    lists: &Checklists,
    is_bot: impl Fn(&str) -> bool,
    now: u64,
) -> Vec<Row> {
    let op = lists.trust.owner.as_str();
    let watchers = &lists.trust.watchers;
    // channel -> the operator's posts there.
    let mut said: HashMap<&str, Vec<&Message>> = HashMap::new();
    for (id, ch) in &store.channels {
        for m in ch.messages.values() {
            if m.author == op && !m.deleted {
                said.entry(id.as_str()).or_default().push(m);
            }
        }
    }
    // Answered: he posted in that thread (a DM, or a row with no thread:
    // anywhere in the channel) after `at`.
    let answered = |channel: &str, thread: Option<&str>, at: u64| {
        let dm = store.channels.get(channel).is_some_and(|c| c.is_dm);
        said.get(channel).is_some_and(|posts| {
            posts.iter().any(|m| {
                m.created_at > at
                    && (dm
                        || thread.is_none()
                        || m.root.as_deref() == thread
                        || Some(m.id.as_str()) == thread)
            })
        })
    };
    let mut out = vec![];
    // Threads the watcher has an ask out in: bots' mentions there are the
    // same ask, not FYI.
    let mut flagged: Vec<(String, Option<String>)> = vec![];
    let mut pings: Vec<String> = vec![];
    for channel in lists.channels() {
        let header = lists.header(channel);
        let watch = lists.watch(channel);
        let thread = watch
            .as_ref()
            .and_then(|w| w.thread.clone())
            .or_else(|| header.as_ref().and_then(|h| h.go.clone()));
        if let Some(w) = watch.as_ref().filter(|w| w.state.needs_you()) {
            let ping = w
                .ping
                .as_ref()
                .and_then(|p| store.channels.get(channel).and_then(|c| c.messages.get(p)));
            let at = w.since.unwrap_or(w.updated_at);
            let anchor = ping.map_or(at, |m| m.created_at);
            let thread = thread
                .clone()
                .or_else(|| ping.map(|m| m.root.clone().unwrap_or_else(|| m.id.clone())));
            if let Some(p) = &w.ping {
                pings.push(p.clone());
            }
            if !answered(channel, thread.as_deref(), anchor) {
                flagged.push((channel.to_string(), thread.clone()));
                let detail = if !w.reason.trim().is_empty() {
                    one_line(&w.reason)
                } else {
                    ping.map(|m| one_line(&m.content)).unwrap_or_default()
                };
                let label = w.state.label();
                out.push(Row {
                    section: Section::Answer,
                    reason: Reason::Watch(w.state.clone()),
                    channel: channel.to_string(),
                    who: header
                        .as_ref()
                        .map(|h| h.creator.clone())
                        .or_else(|| ping.map(|m| m.author.clone()))
                        .unwrap_or_default(),
                    text: if detail.is_empty() {
                        label.clone()
                    } else {
                        format!("{label}: {detail}")
                    },
                    at,
                    message: w.ping.clone(),
                    thread,
                    // Stable across republishes that only bump nudges; a
                    // new stall with neither ping nor since is a new key.
                    key: format!(
                        "watch:{channel}:{label}:{}",
                        w.ping
                            .clone()
                            .unwrap_or_else(|| w.since.unwrap_or(w.began).to_string())
                    ),
                });
            }
        }
        for it in lists.items(channel) {
            if it.status != Status::Parked {
                continue;
            }
            out.push(Row {
                section: Section::Answer,
                reason: Reason::Parked,
                channel: channel.to_string(),
                who: it.creator.clone(),
                text: it
                    .parked_question
                    .as_deref()
                    .map(one_line)
                    .unwrap_or_else(|| one_line(&it.text)),
                at: it.updated_at,
                message: None,
                thread: thread.clone(),
                // Stable across edits that leave the question as it was.
                key: format!(
                    "item:{channel}:{}:{}",
                    it.id,
                    it.parked_question.as_deref().unwrap_or_default()
                ),
            });
        }
    }
    let since = now.saturating_sub(HORIZON);
    for (id, ch) in &store.channels {
        let dm = ch.is_dm && ch.members.iter().any(|m| m == op);
        for m in ch.messages.values() {
            if m.deleted
                || m.system
                || m.author == op
                || m.created_at < since
                || !(dm || m.mentions.iter().any(|p| p == op))
                || watchers.contains(&m.author)
                || pings.contains(&m.id)
            {
                continue;
            }
            let thread = m.root.clone().unwrap_or_else(|| m.id.clone());
            if answered(id, Some(&thread), m.created_at) {
                continue;
            }
            let bot = is_bot(&m.author);
            if bot
                && flagged
                    .iter()
                    .any(|(c, t)| c == id && t.as_deref() == Some(thread.as_str()))
            {
                continue;
            }
            out.push(Row {
                section: if bot { Section::Fyi } else { Section::Answer },
                reason: match (bot, dm) {
                    (true, _) => Reason::Bot,
                    (false, true) => Reason::Dm,
                    (false, false) => Reason::Mention,
                },
                channel: id.clone(),
                who: m.author.clone(),
                text: one_line(&m.content),
                at: m.created_at,
                message: Some(m.id.clone()),
                thread: Some(thread),
                key: format!("msg:{}", m.id),
            });
        }
    }
    out.sort_by(|a, b| {
        a.section.cmp(&b.section).then_with(|| match a.section {
            Section::Answer => (a.at, &a.key).cmp(&(b.at, &b.key)),
            Section::Fyi => (b.at, &b.key).cmp(&(a.at, &a.key)),
        })
    });
    let answers = answer_count(&out);
    out.truncate(answers + FYI_MAX);
    out
}

/// FYI rows kept: the newest ones.
pub const FYI_MAX: usize = 20;

/// How many rows need an answer: the "⚑ N".
pub fn answer_count(rows: &[Row]) -> usize {
    rows.iter().filter(|r| r.section == Section::Answer).count()
}

/// The jump key's next stop: the Needs-an-answer row after `last` (by key),
/// oldest first, round again at the end.
pub fn next_answer<'a>(rows: &'a [Row], last: Option<&str>) -> Option<&'a Row> {
    let answer: Vec<&Row> = rows
        .iter()
        .filter(|r| r.section == Section::Answer)
        .collect();
    let at = last.and_then(|k| answer.iter().position(|r| r.key == k));
    match at {
        Some(i) => answer.get(i + 1).or(answer.first()).copied(),
        None => answer.first().copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::Trust;
    use crate::model::Profile;
    use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};

    const CH: &str = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
    const DM: &str = "bcd11c3d-f78b-4934-8e48-c08b6baed1c5";
    const NOW: u64 = 1_800_000_000;

    struct World {
        store: Store,
        lists: Checklists,
        op: Keys,
        human: Keys,
        bot: Keys,
        watcher: Keys,
    }

    impl World {
        fn new() -> Self {
            let (op, human, bot, watcher) = (
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
            );
            let mut store = Store::new(op.public_key().to_hex());
            store.profiles.insert(
                bot.public_key().to_hex(),
                Profile {
                    name: "taskbot".into(),
                    is_agent: true,
                    ..Default::default()
                },
            );
            let dm = store.channel_mut(DM);
            dm.is_dm = true;
            dm.members = vec![op.public_key().to_hex(), human.public_key().to_hex()];
            let lists = Checklists::new(Trust {
                owner: op.public_key().to_hex(),
                watchers: vec![watcher.public_key().to_hex()],
            });
            World {
                store,
                lists,
                op,
                human,
                bot,
                watcher,
            }
        }

        fn pk(k: &Keys) -> String {
            k.public_key().to_hex()
        }

        /// A message in `ch`, @mentioning `to`, replying under `root`.
        fn post(
            &mut self,
            who: &Keys,
            ch: &str,
            text: &str,
            to: Option<&Keys>,
            root: Option<&str>,
            at: u64,
        ) -> String {
            let mut tags = vec![Tag::parse(["h", ch]).unwrap()];
            if let Some(k) = to {
                tags.push(Tag::parse(["p", &Self::pk(k)]).unwrap());
            }
            if let Some(r) = root {
                tags.push(Tag::parse(["e", r, "", "reply"]).unwrap());
            }
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags(tags)
                .custom_created_at(Timestamp::from(at))
                .sign_with_keys(who)
                .unwrap();
            self.store.apply(&ev);
            ev.id.to_hex()
        }

        fn record(&mut self, who: &Keys, kind: u16, d: String, at: u64, v: serde_json::Value) {
            let ev: Event = EventBuilder::new(Kind::Custom(kind), v.to_string())
                .tags(
                    [
                        vec!["d".to_string(), d],
                        vec!["h".into(), CH.into()],
                        vec!["t".into(), "buzz-checklist".into()],
                    ]
                    .into_iter()
                    .map(|t| Tag::parse(t).unwrap()),
                )
                .custom_created_at(Timestamp::from(at))
                .sign_with_keys(who)
                .unwrap();
            assert!(self.lists.apply(&ev));
        }

        fn watch(&mut self, state: &str, at: u64, ping: Option<&str>, thread: Option<&str>) {
            let w = self.watcher.clone();
            self.record(
                &w,
                30472,
                format!("buzz-checklist/watch/{CH}"),
                at,
                serde_json::json!({"state": state, "since": at, "ping": ping,
                    "thread": thread, "reason": "which relay image?"}),
            );
        }

        fn rows(&self) -> Vec<Row> {
            let bots: Vec<String> = self
                .store
                .profiles
                .iter()
                .filter(|(_, p)| p.is_agent)
                .map(|(k, _)| k.clone())
                .chain(self.lists.trust.watchers.iter().cloned())
                .collect();
            rows(
                &self.store,
                &self.lists,
                |pk| bots.iter().any(|b| b == pk),
                NOW,
            )
        }
    }

    fn keys(rows: &[Row]) -> Vec<(Section, String)> {
        rows.iter().map(|r| (r.section, r.label())).collect()
    }

    #[test]
    fn filters_scope_every_request_to_our_channels() {
        let chans = vec![CH.to_string(), DM.to_string()];
        let f = filters(&chans, &[DM.to_string()], "op", NOW, false);
        assert_eq!(f.len(), 6);
        assert!(
            f.iter()
                .all(|f| f["#h"].as_array().is_some_and(|h| !h.is_empty()))
        );
        assert_eq!(f[0]["#p"][0], "op");
        assert_eq!(f[0]["since"], NOW - HORIZON);
        // History: his posts and the records one channel at a time.
        for (i, ch) in chans.iter().enumerate() {
            let (posts, records) = (&f[1 + 2 * i], &f[2 + 2 * i]);
            assert_eq!(posts["authors"][0], "op");
            assert_eq!(posts["#h"], serde_json::json!([ch]));
            assert_eq!(records["#h"], serde_json::json!([ch]));
            assert_eq!(records["kinds"], serde_json::json!(crate::checklist::KINDS));
        }
        let live = filters(&chans, &[], "op", NOW, true);
        assert_eq!(
            (live.len(), &live[2]["since"], &live[2]["limit"]),
            (3, &serde_json::json!(NOW), &serde_json::json!(0))
        );
        assert!(filters(&[], &[], "op", NOW, false).is_empty());
    }

    #[test]
    fn people_need_an_answer_bots_are_fyi() {
        let mut w = World::new();
        let (op, human, bot) = (w.op.clone(), w.human.clone(), w.bot.clone());
        w.post(
            &bot,
            CH,
            "@lucian built it, have a look",
            Some(&op),
            None,
            NOW - 50,
        );
        w.post(
            &human,
            CH,
            "@lucian can we ship today?",
            Some(&op),
            None,
            NOW - 300,
        );
        w.post(&human, DM, "lunch?", None, None, NOW - 100);
        // Not to him, his own, and too old: no rows.
        w.post(&human, CH, "unrelated", None, None, NOW - 10);
        w.post(&op, CH, "@lucian note to self", Some(&op), None, NOW - 10);
        w.post(
            &human,
            CH,
            "@lucian old",
            Some(&op),
            None,
            NOW - HORIZON - 5,
        );
        let r = w.rows();
        assert_eq!(
            keys(&r),
            [
                (Section::Answer, "mention".to_string()),
                (Section::Answer, "dm".into()),
                (Section::Fyi, "bot".into()),
            ]
        );
        assert_eq!(r[0].text, "@lucian can we ship today?");
        assert_eq!(r[0].who, World::pk(&human));
        assert_eq!(answer_count(&r), 2);
        // An unknown author counts as a person until a profile says bot.
        let stranger = Keys::generate();
        w.post(&stranger, CH, "@lucian hi", Some(&op), None, NOW - 20);
        assert_eq!(answer_count(&w.rows()), 3);
    }

    #[test]
    fn fyi_is_newest_first_and_capped() {
        let mut w = World::new();
        let (op, bot) = (w.op.clone(), w.bot.clone());
        for i in 0..(FYI_MAX as u64 + 5) {
            w.post(
                &bot,
                CH,
                &format!("@lucian fyi {i}"),
                Some(&op),
                None,
                NOW - 1000 + i,
            );
        }
        let r = w.rows();
        assert_eq!(r.len(), FYI_MAX);
        assert_eq!(r[0].text, format!("@lucian fyi {}", FYI_MAX + 4));
        assert!(r.windows(2).all(|p| p[0].at >= p[1].at));
    }

    #[test]
    fn a_reply_after_the_message_in_its_thread_clears_it() {
        let mut w = World::new();
        let (op, human) = (w.op.clone(), w.human.clone());
        let ask = w.post(&human, CH, "@lucian which one?", Some(&op), None, NOW - 300);
        let reply_ask = w.post(
            &human,
            CH,
            "@lucian and this?",
            Some(&op),
            Some(&ask),
            NOW - 200,
        );
        // A post elsewhere in the channel, or before it, doesn't answer.
        w.post(&op, CH, "something else", None, None, NOW - 100);
        assert_eq!(w.rows().len(), 2);
        let r = &w.rows()[1];
        assert_eq!(
            (r.message.as_deref(), r.thread.as_deref()),
            (Some(reply_ask.as_str()), Some(ask.as_str()))
        );
        assert!(r.link().ends_with(&format!("id={reply_ask}&thread={ask}")));
        // In the thread after both: both clear.
        w.post(&op, CH, "the first", None, Some(&ask), NOW - 50);
        assert!(w.rows().is_empty());
        // A DM clears with any later post in it.
        w.post(&human, DM, "you there?", None, None, NOW - 40);
        assert_eq!(w.rows().len(), 1);
        w.post(&op, DM, "yes", None, None, NOW - 30);
        assert!(w.rows().is_empty());
    }

    #[test]
    fn watcher_asks_and_parked_items_until_they_move() {
        let mut w = World::new();
        let (op, bot) = (w.op.clone(), w.bot.clone());
        let bk = World::pk(&bot);
        let root = w.post(&op, CH, "go", None, None, NOW - 4000);
        let wk = w.watcher.clone();
        let ping = w.post(
            &wk,
            CH,
            "@lucian taskbot asks: which relay image?",
            Some(&op),
            Some(&root),
            NOW - 600,
        );
        // The agent's own @mention of the same ask, in the same thread.
        w.post(
            &bot,
            CH,
            "@lucian which relay image?",
            Some(&op),
            Some(&root),
            NOW - 700,
        );
        w.record(
            &bot,
            30471,
            format!("buzz-checklist/header/{CH}"),
            NOW - 5000,
            serde_json::json!({"title": "t", "mode": "running", "go": root}),
        );
        w.record(&bot, 30470, format!("buzz-checklist/item/{CH}/a1"), NOW - 650,
            serde_json::json!({"text": "migrate relay", "status": "parked", "parked_question": "tonight or nightly?\nmore"}));
        w.record(
            &bot,
            30470,
            format!("buzz-checklist/item/{CH}/a2"),
            NOW - 640,
            serde_json::json!({"text": "open one", "status": "open"}),
        );
        w.watch("waiting_on_lucian", NOW - 600, Some(&ping), Some(&root));
        let r = w.rows();
        // Parked item (older) then the watch row; the ping and the agent's
        // mention in that thread aren't rows of their own.
        assert_eq!(
            keys(&r),
            [
                (Section::Answer, "parked".to_string()),
                (Section::Answer, "waiting on lucian".into()),
            ]
        );
        assert_eq!(r[0].text, "tonight or nightly?");
        assert_eq!(r[0].who, bk);
        assert_eq!(r[0].thread.as_deref(), Some(root.as_str()));
        assert_eq!(r[1].text, "waiting on lucian: which relay image?");
        assert_eq!(r[1].message.as_deref(), Some(ping.as_str()));
        assert_eq!(r[1].who, bk);
        // The jump goes oldest first and comes round.
        let first = next_answer(&r, None).unwrap();
        assert_eq!(first.key, r[0].key);
        assert_eq!(next_answer(&r, Some(&first.key)).unwrap().key, r[1].key);
        assert_eq!(next_answer(&r, Some(&r[1].key)).unwrap().key, r[0].key);
        // The item moves: its row goes. The watcher moves on: so does that.
        w.record(
            &bot,
            30470,
            format!("buzz-checklist/item/{CH}/a1"),
            NOW - 10,
            serde_json::json!({"text": "migrate relay", "status": "open"}),
        );
        assert_eq!(w.rows().len(), 1);
        w.watch("working", NOW - 5, None, Some(&root));
        let r = w.rows();
        // With no ask out, the agent's mention is plain FYI.
        assert_eq!(keys(&r), [(Section::Fyi, "bot".to_string())]);
    }

    #[test]
    fn republishing_keeps_row_keys_and_threadless_rows_clear() {
        let mut w = World::new();
        let (op, bot) = (w.op.clone(), w.bot.clone());
        w.record(&bot, 30470, format!("buzz-checklist/item/{CH}/a1"), NOW - 650,
            serde_json::json!({"text": "migrate relay", "status": "parked", "parked_question": "tonight?"}));
        let wk = w.watcher.clone();
        let watch = |w: &mut World, at: u64| {
            w.record(
                &wk,
                30472,
                format!("buzz-checklist/watch/{CH}"),
                at,
                serde_json::json!({"state": "stalled", "nudges": at}),
            );
        };
        watch(&mut w, NOW - 600);
        let before: Vec<String> = w.rows().iter().map(|r| r.key.clone()).collect();
        assert_eq!(before.len(), 2);
        // A nudge bump and a details edit: same asks, same keys.
        watch(&mut w, NOW - 300);
        w.record(&bot, 30470, format!("buzz-checklist/item/{CH}/a1"), NOW - 290,
            serde_json::json!({"text": "migrate relay", "details": "more", "status": "parked", "parked_question": "tonight?"}));
        let after: Vec<String> = w.rows().iter().map(|r| r.key.clone()).collect();
        assert_eq!(
            before.iter().collect::<std::collections::HashSet<_>>(),
            after.iter().collect::<std::collections::HashSet<_>>()
        );
        // No thread anywhere: a top-level answer in the channel clears the watch row.
        w.post(&op, CH, "nightly", None, None, NOW - 100);
        assert_eq!(keys(&w.rows()), [(Section::Answer, "parked".to_string())]);
    }

    #[test]
    fn a_new_stall_without_ping_or_since_gets_a_new_key() {
        let mut w = World::new();
        let wk = w.watcher.clone();
        let watch = |w: &mut World, state: &str, at: u64| {
            w.record(
                &wk,
                30472,
                format!("buzz-checklist/watch/{CH}"),
                at,
                serde_json::json!({"state": state, "nudges": at}),
            );
        };
        let key = |w: &World| w.rows().first().map(|r| r.key.clone());
        watch(&mut w, "stalled", NOW - 900);
        let first = key(&w).unwrap();
        // A nudge: the same stall.
        watch(&mut w, "stalled", NOW - 800);
        assert_eq!(key(&w).as_ref(), Some(&first));
        // Working again, then stalled again: a new row.
        watch(&mut w, "working", NOW - 700);
        assert_eq!(key(&w), None);
        watch(&mut w, "stalled", NOW - 600);
        let second = key(&w).unwrap();
        assert_ne!(second, first);
        watch(&mut w, "stalled", NOW - 500);
        assert_eq!(key(&w), Some(second));
    }

    #[test]
    fn answering_the_ping_in_its_thread_clears_the_watch_row() {
        let mut w = World::new();
        let op = w.op.clone();
        let root = w.post(&op, CH, "go", None, None, NOW - 4000);
        let wk = w.watcher.clone();
        let ping = w.post(
            &wk,
            CH,
            "@lucian stalled",
            Some(&op),
            Some(&root),
            NOW - 600,
        );
        w.watch("stalled", NOW - 900, Some(&ping), Some(&root));
        w.watch("waiting_on_release", NOW - 600, Some(&ping), Some(&root));
        let r = w.rows();
        assert_eq!(
            keys(&r),
            [(Section::Answer, "waiting on release".to_string())]
        );
        assert_eq!(r[0].at, NOW - 600);
        w.post(&op, CH, "released", None, Some(&root), NOW - 60);
        assert!(w.rows().is_empty());
    }
}
