//! Agent activity: the encrypted observer frames (kind 24200) your agents
//! send you while they work, plus per-turn metrics (kind 44200) and presence.
//! Frames are live only; the relay doesn't store them.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::model::Store;

#[derive(Default)]
pub struct Agent {
    /// One of your managed agents (kind 30177). Only these are listed.
    pub owned: bool,
    pub name: String,
    pub online: bool,
    /// Turns in flight, by turnId. One agent can run turns in several
    /// channels at once, so these are tracked one by one.
    pub turns: BTreeMap<String, Turn>,
    /// What the agent did and said, kept per channel ("" when a frame
    /// names none) so each channel's agents screen shows only its own turns.
    pub activity: BTreeMap<String, Activity>,
    pub last_turn: Option<String>,
}

#[derive(Default)]
pub struct Activity {
    /// newest first
    pub recent: Vec<String>,
    pub thought: String,
}

pub struct Turn {
    pub channel: Option<String>,
    /// Root of the thread the turn answers (the triggering message when it
    /// was top-level).
    pub thread: Option<String>,
    pub started: Instant,
    pub seen: Instant,
}

#[derive(Default)]
pub struct Agents {
    pub by_pubkey: BTreeMap<String, Agent>,
}

pub struct Row {
    pub pubkey: String,
    pub name: String,
    pub busy: bool,
    pub online: bool,
    pub recent: Vec<String>,
    pub channel: Option<String>,
    pub thread: Option<String>,
    /// Seconds into the current turn.
    pub secs: Option<u64>,
    pub last_turn: Option<String>,
    /// Tail of what the agent is saying or thinking right now.
    pub thought: String,
    /// Busy in another channel (not the one asked about), by channel id.
    pub elsewhere: Option<String>,
}

/// The harness sends turn_liveness every ~10s, so a turn quiet for this
/// long lost its turn_completed.
const TURN_STALE: Duration = Duration::from_secs(60);

impl Agent {
    /// Live turns, newest first.
    fn live(&self) -> impl Iterator<Item = &Turn> {
        let mut t: Vec<&Turn> = self
            .turns
            .values()
            .filter(|t| t.seen.elapsed() <= TURN_STALE)
            .collect();
        t.sort_by_key(|t| std::cmp::Reverse(t.started));
        t.into_iter()
    }

    pub fn is_busy(&self) -> bool {
        self.live().next().is_some()
    }

    fn display_name(&self, pk: &str, store: &Store) -> String {
        match store.profiles.get(pk) {
            Some(p) if !p.name.is_empty() => p.name.clone(),
            _ if !self.name.is_empty() => self.name.clone(),
            _ => store.name_of(pk),
        }
    }
}

fn push_recent(a: &mut Agent, channel: &str, s: String) {
    let r = &mut a.activity.entry(channel.to_string()).or_default().recent;
    if r.first() == Some(&s) {
        return;
    }
    r.insert(0, s);
    r.truncate(6);
}

impl Agents {
    pub fn known(&mut self, pubkey: &str) -> &mut Agent {
        self.by_pubkey.entry(pubkey.to_string()).or_default()
    }

    /// Only agents you own: anyone can send a telemetry frame.
    pub fn is_owned(&self, pubkey: &str) -> bool {
        self.by_pubkey.get(pubkey).is_some_and(|a| a.owned)
    }

    pub fn owned_keys(&self) -> impl Iterator<Item = &String> {
        self.by_pubkey
            .iter()
            .filter(|(_, a)| a.owned)
            .map(|(k, _)| k)
    }

    /// A managed agent you own, from its kind 30177 record.
    pub fn own(&mut self, pubkey: &str, name: &str) {
        let a = self.known(pubkey);
        a.owned = true;
        a.name = name.to_string();
    }

    /// One decrypted frame from `agent`. Batches are unwrapped.
    pub fn frame(&mut self, agent: &str, frame: &Value) {
        if frame.get("kind").and_then(Value::as_str) == Some("batch") {
            for inner in frame
                .pointer("/payload/events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.frame(agent, inner);
            }
            return;
        }
        let a = self.known(agent);
        a.online = true;
        let now = Instant::now();
        let channel = frame
            .get("channelId")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Older frames carry no turnId; one turn per channel then.
        let key = frame
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("ch:{}", channel.as_deref().unwrap_or("")));
        // Where this frame's activity is filed: its channel, else its turn's,
        // else the only turn running.
        let here = channel
            .clone()
            .or_else(|| a.turns.get(&key).and_then(|t| t.channel.clone()))
            .or_else(|| {
                let mut live = a.live();
                match (live.next(), live.next()) {
                    (Some(t), None) => t.channel.clone(),
                    _ => None,
                }
            })
            .unwrap_or_default();
        let root = frame
            .pointer("/payload/threadRootEventId")
            .and_then(Value::as_str)
            .map(str::to_string);
        match frame.get("kind").and_then(Value::as_str).unwrap_or("") {
            "turn_started" => {
                a.activity.entry(here.clone()).or_default().thought.clear();
                let thread = root.or_else(|| {
                    frame
                        .pointer("/payload/triggeringEventIds/0")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
                a.turns.insert(
                    key,
                    Turn {
                        channel,
                        thread,
                        started: now,
                        seen: now,
                    },
                );
                push_recent(a, &here, "turn started".into());
            }
            // Ticks while a turn runs: catches turns that began before we
            // connected.
            "turn_liveness" => {
                let t = a.turns.entry(key).or_insert_with(|| Turn {
                    channel: channel.clone(),
                    thread: None,
                    started: now,
                    seen: now,
                });
                t.seen = now;
                if channel.is_some() {
                    t.channel = channel;
                }
                if root.is_some() {
                    t.thread = root;
                }
            }
            "turn_completed" | "turn_error" | "agent_panic" => {
                let ended: Vec<String> = if a.turns.contains_key(&key) {
                    vec![key]
                } else {
                    // No turnId to match: end what ran in that channel, or
                    // everything when the frame names no channel either.
                    a.turns
                        .iter()
                        .filter(|(_, t)| channel.is_none() || t.channel == channel)
                        .map(|(k, _)| k.clone())
                        .collect()
                };
                let done = frame.get("kind").and_then(Value::as_str) == Some("turn_completed");
                let msg = frame
                    .pointer("/payload/error")
                    .and_then(Value::as_str)
                    .unwrap_or("turn failed");
                let mut chans = vec![];
                for k in ended {
                    if let Some(t) = a.turns.remove(&k) {
                        let took = format!(" in {}s", t.started.elapsed().as_secs());
                        chans.push((t.channel.unwrap_or_default(), took));
                    }
                }
                if chans.is_empty() {
                    chans.push((here, String::new()));
                }
                for (ch, took) in chans {
                    let line = if done {
                        format!("turn done{took}")
                    } else {
                        format!("✗ {msg}")
                    };
                    push_recent(a, &ch, line);
                }
            }
            "acp_read" => {
                if let Some(t) = a.turns.get_mut(&key) {
                    t.seen = now;
                }
                let Some(update) = frame.pointer("/payload/params/update") else {
                    return;
                };
                match update
                    .get("sessionUpdate")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                {
                    "tool_call" => {
                        let title = update
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let line = title.lines().next().unwrap_or(title).to_string();
                        push_recent(a, &here, line);
                    }
                    "tool_call_update" => {
                        if update.get("status").and_then(Value::as_str) == Some("failed") {
                            push_recent(a, &here, "✗ tool failed".into());
                        }
                    }
                    "agent_thought_chunk" | "agent_message_chunk" => {
                        if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                            let th = &mut a.activity.entry(here).or_default().thought;
                            th.push_str(t);
                            let keep = th.len().saturating_sub(400);
                            let cut = th.ceil_char_boundary(keep);
                            th.drain(..cut);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// A kind 44200 turn metric payload.
    pub fn metric(&mut self, agent: &str, m: &Value) {
        let tokens = m.pointer("/turn/totalTokens").and_then(Value::as_u64);
        let cost = m.pointer("/turn/costUsd").and_then(Value::as_f64);
        let model = m.get("model").and_then(Value::as_str).unwrap_or("");
        let mut s = String::from("last turn");
        if let Some(t) = tokens {
            s.push_str(&format!(" {}k tok", (t + 500) / 1000));
        }
        if let Some(c) = cost {
            s.push_str(&format!(" ${c:.2}"));
        }
        if !model.is_empty() {
            s.push_str(&format!(" · {model}"));
        }
        self.known(agent).last_turn = Some(s);
    }

    pub fn presence(&mut self, agent: &str, status: &str) {
        self.known(agent).online = status == "online" || status == "away";
    }

    /// Agents mid-turn in `channel`, and in thread `root` if given.
    pub fn working(&self, store: &Store, channel: &str, root: Option<&str>) -> Vec<String> {
        let mut names: Vec<String> = self
            .by_pubkey
            .iter()
            .filter(|(_, a)| {
                a.owned
                    && a.live().any(|t| {
                        t.channel.as_deref() == Some(channel)
                            && root.is_none_or(|r| t.thread.as_deref() == Some(r))
                    })
            })
            .map(|(pk, a)| a.display_name(pk, store))
            .collect();
        names.sort();
        names
    }

    /// Your online agents as seen from channel `here`: busy means a turn in
    /// that channel, and steps and thoughts are only that channel's. An
    /// agent busy somewhere else is idle here with `elsewhere` set. `None`
    /// looks across every channel.
    pub fn rows(&self, store: &Store, here: Option<&str>) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .by_pubkey
            .iter()
            .filter(|(_, a)| a.owned)
            .filter_map(|(pk, a)| {
                let any = a.live().next();
                let turn = a
                    .live()
                    .find(|t| here.is_none_or(|h| t.channel.as_deref() == Some(h)));
                let busy = turn.is_some();
                // Offline agents stay off the list; you only see who can answer.
                if !a.online && any.is_none() {
                    return None;
                }
                let elsewhere = if busy {
                    None
                } else {
                    any.map(|t| t.channel.clone().unwrap_or_default())
                };
                let act = here
                    .or(any.and_then(|t| t.channel.as_deref()))
                    .and_then(|c| a.activity.get(c));
                let (recent, thought) = act.map_or((&[][..], ""), |x| (&x.recent[..], &x.thought));
                Some(Row {
                    pubkey: pk.clone(),
                    name: a.display_name(pk, store),
                    busy,
                    online: true,
                    elsewhere,
                    recent: recent
                        .iter()
                        .filter(|r| *r != "turn started")
                        .cloned()
                        .collect(),
                    channel: turn.and_then(|t| t.channel.clone()),
                    thread: turn.and_then(|t| t.thread.clone()),
                    secs: turn.map(|t| t.started.elapsed().as_secs()),
                    last_turn: a.last_turn.clone(),
                    thought: if busy {
                        let w: Vec<&str> = thought.split_whitespace().collect();
                        w[w.len().saturating_sub(16)..].join(" ")
                    } else {
                        String::new()
                    },
                })
            })
            .collect();
        rows.sort_by_key(|r| (!r.busy, !r.online, r.name.to_lowercase()));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_frame_sender_is_not_ours() {
        let mut ag = Agents::default();
        ag.own("mine", "bot");
        ag.frame(
            "stranger",
            &json!({"kind": "turn_started", "channelId": "c", "payload": {}}),
        );
        assert!(ag.is_owned("mine"));
        assert!(!ag.is_owned("stranger"));
        assert_eq!(ag.owned_keys().collect::<Vec<_>>(), vec!["mine"]);
        let mut s = Store::new("me".into());
        s.channel_mut("c").members = vec!["me".into()];
        s.profiles.entry("stranger".into()).or_default().is_agent = true;
        let said = vec!["mine".to_string(), "stranger".to_string()];
        assert_eq!(s.outsiders("c", &said, |pk| ag.is_owned(pk)), vec!["mine"]);
    }

    #[test]
    fn turn_lifecycle() {
        let mut ag = Agents::default();
        ag.own("a1", "bot");
        ag.frame("a1", &json!({"kind": "batch", "payload": {"events": [
            {"kind": "turn_started", "channelId": "c", "payload": {}},
            {"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "tool_call", "title": "Bash: cargo test"}}}},
            {"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "agent_thought_chunk", "content": {"text": "checking the gate"}}}}}
        ]}}));
        let s = Store::new("me".into());
        let r = &ag.rows(&s, Some("c"))[0];
        assert!(r.busy);
        assert_eq!(ag.working(&s, "c", None), vec!["bot"]);
        assert!(ag.working(&s, "c", Some("t1")).is_empty());
        ag.frame("a1", &json!({"kind": "turn_liveness", "channelId": "c", "payload": {"threadRootEventId": "t1"}}));
        assert_eq!(ag.working(&s, "c", Some("t1")), vec!["bot"]);
        assert_eq!(r.recent[0], "Bash: cargo test");
        assert!(r.thought.ends_with("checking the gate"), "{}", r.thought);
        ag.frame("a1", &json!({"kind": "turn_completed", "payload": {}}));
        ag.metric(
            "a1",
            &json!({"model": "opus", "turn": {"totalTokens": 12400, "costUsd": 0.5}}),
        );
        ag.presence("a1", "online");
        assert!(ag.working(&s, "c", None).is_empty());
        let r = &ag.rows(&s, Some("c"))[0];
        assert!(!r.busy);
        assert_eq!(
            r.last_turn.as_deref(),
            Some("last turn 12k tok $0.50 · opus")
        );
        // Offline, or not one of yours: not listed.
        ag.presence("a1", "offline");
        ag.presence("x9", "online");
        assert!(ag.rows(&s, Some("c")).is_empty());
        // Frames from someone who isn't your agent don't show as thinking.
        ag.frame(
            "x9",
            &json!({"kind": "turn_started", "channelId": "c", "payload": {}}),
        );
        assert!(ag.working(&s, "c", None).is_empty());
    }

    #[test]
    fn concurrent_turns_end_separately() {
        let mut ag = Agents::default();
        ag.own("a1", "bot");
        let s = Store::new("me".into());
        for (t, c) in [("t1", "c1"), ("t2", "c2")] {
            ag.frame(
                "a1",
                &json!({"kind": "turn_started", "turnId": t, "channelId": c, "payload": {}}),
            );
        }
        ag.frame(
            "a1",
            &json!({"kind": "turn_completed", "turnId": "t1", "channelId": "c1", "payload": {}}),
        );
        assert!(ag.working(&s, "c1", None).is_empty());
        assert_eq!(ag.working(&s, "c2", None), vec!["bot"]);
        assert!(ag.rows(&s, None)[0].busy);
        // Seen from c1 it is idle here and working in c2.
        let r = &ag.rows(&s, Some("c1"))[0];
        assert!(!r.busy);
        assert_eq!(r.elsewhere.as_deref(), Some("c2"));
        assert!(ag.rows(&s, Some("c2"))[0].busy);
    }

    #[test]
    fn steps_and_thoughts_stay_in_their_channel() {
        let mut ag = Agents::default();
        ag.own("a1", "bot");
        ag.presence("a1", "online");
        let s = Store::new("me".into());
        for (t, c) in [("t1", "c1"), ("t2", "c2")] {
            ag.frame(
                "a1",
                &json!({"kind": "turn_started", "turnId": t, "channelId": c, "payload": {}}),
            );
            ag.frame("a1", &json!({"kind": "acp_read", "turnId": t, "payload": {"params": {"update": {"sessionUpdate": "tool_call", "title": format!("step in {c}")}}}}));
            ag.frame("a1", &json!({"kind": "acp_read", "turnId": t, "payload": {"params": {"update": {"sessionUpdate": "agent_thought_chunk", "content": {"text": format!("thinking about {c}")}}}}}));
        }
        for c in ["c1", "c2"] {
            let r = &ag.rows(&s, Some(c))[0];
            assert_eq!(r.recent, vec![format!("step in {c}")]);
            assert_eq!(r.thought, format!("thinking about {c}"));
        }
        ag.frame(
            "a1",
            &json!({"kind": "turn_completed", "turnId": "t1", "payload": {}}),
        );
        assert!(ag.rows(&s, Some("c1"))[0].recent[0].starts_with("turn done"));
        assert_eq!(ag.rows(&s, Some("c2"))[0].recent[0], "step in c2");
        assert!(ag.rows(&s, Some("c3"))[0].recent.is_empty());
    }
}
