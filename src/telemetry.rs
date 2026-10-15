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
    pub busy: bool,
    pub online: bool,
    pub channel: Option<String>,
    /// Root of the thread the current turn answers (the triggering message
    /// when it was top-level).
    pub thread: Option<String>,
    pub turn_started: Option<Instant>,
    /// newest first
    pub recent: Vec<String>,
    pub thought: String,
    pub last_turn: Option<String>,
    pub last_seen: Option<Instant>,
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
}

impl Agent {
    /// A busy flag with no news for 15 minutes is a lost turn_completed.
    pub fn is_busy(&self) -> bool {
        self.busy
            && self
                .last_seen
                .is_some_and(|t| t.elapsed() <= Duration::from_secs(900))
    }

    fn display_name(&self, pk: &str, store: &Store) -> String {
        match store.profiles.get(pk) {
            Some(p) if !p.name.is_empty() => p.name.clone(),
            _ if !self.name.is_empty() => self.name.clone(),
            _ => store.name_of(pk),
        }
    }
}

fn push_recent(a: &mut Agent, s: String) {
    if a.recent.first() == Some(&s) {
        return;
    }
    a.recent.insert(0, s);
    a.recent.truncate(6);
}

impl Agents {
    pub fn known(&mut self, pubkey: &str) -> &mut Agent {
        self.by_pubkey.entry(pubkey.to_string()).or_default()
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
        a.last_seen = Some(Instant::now());
        if let Some(c) = frame.get("channelId").and_then(Value::as_str) {
            a.channel = Some(c.to_string());
        }
        match frame.get("kind").and_then(Value::as_str).unwrap_or("") {
            "turn_started" => {
                a.busy = true;
                a.turn_started = Some(Instant::now());
                a.thought.clear();
                a.thread = frame
                    .pointer("/payload/threadRootEventId")
                    .or_else(|| frame.pointer("/payload/triggeringEventIds/0"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                push_recent(a, "turn started".into());
            }
            // Ticks while a turn runs: catches turns that began before we
            // connected.
            "turn_liveness" => {
                a.busy = true;
                a.turn_started.get_or_insert_with(Instant::now);
                if let Some(r) = frame
                    .pointer("/payload/threadRootEventId")
                    .and_then(Value::as_str)
                {
                    a.thread = Some(r.to_string());
                }
            }
            "turn_completed" => {
                a.busy = false;
                a.thread = None;
                let took = a
                    .turn_started
                    .take()
                    .map(|t| format!(" in {}s", t.elapsed().as_secs()))
                    .unwrap_or_default();
                push_recent(a, format!("turn done{took}"));
            }
            "turn_error" | "agent_panic" => {
                a.busy = false;
                a.thread = None;
                let msg = frame
                    .pointer("/payload/error")
                    .and_then(Value::as_str)
                    .unwrap_or("turn failed");
                push_recent(a, format!("✗ {msg}"));
            }
            "acp_read" => {
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
                        push_recent(a, title.lines().next().unwrap_or(title).to_string());
                    }
                    "tool_call_update" => {
                        if update.get("status").and_then(Value::as_str) == Some("failed") {
                            push_recent(a, "✗ tool failed".into());
                        }
                    }
                    "agent_thought_chunk" | "agent_message_chunk" => {
                        if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                            a.thought.push_str(t);
                            let keep = a.thought.len().saturating_sub(400);
                            let cut = a.thought.ceil_char_boundary(keep);
                            a.thought.drain(..cut);
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
            .filter(|(_, a)| a.is_busy() && a.channel.as_deref() == Some(channel))
            .filter(|(_, a)| root.is_none_or(|r| a.thread.as_deref() == Some(r)))
            .map(|(pk, a)| a.display_name(pk, store))
            .collect();
        names.sort();
        names
    }

    pub fn rows(&self, store: &Store) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .by_pubkey
            .iter()
            .filter(|(_, a)| a.owned)
            .filter_map(|(pk, a)| {
                let busy = a.is_busy();
                // Offline agents stay off the list; you only see who can answer.
                if !a.online && !busy {
                    return None;
                }
                Some(Row {
                    pubkey: pk.clone(),
                    name: a.display_name(pk, store),
                    busy,
                    online: true,
                    recent: a
                        .recent
                        .iter()
                        .filter(|r| *r != "turn started")
                        .cloned()
                        .collect(),
                    channel: a.channel.clone(),
                    thread: a.thread.clone().filter(|_| busy),
                    secs: a
                        .turn_started
                        .filter(|_| busy)
                        .map(|t| t.elapsed().as_secs()),
                    last_turn: a.last_turn.clone(),
                    thought: if busy {
                        let w: Vec<&str> = a.thought.split_whitespace().collect();
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
    fn turn_lifecycle() {
        let mut ag = Agents::default();
        ag.own("a1", "bot");
        ag.frame("a1", &json!({"kind": "batch", "payload": {"events": [
            {"kind": "turn_started", "channelId": "c", "payload": {}},
            {"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "tool_call", "title": "Bash: cargo test"}}}},
            {"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "agent_thought_chunk", "content": {"text": "checking the gate"}}}}}
        ]}}));
        let s = Store::new("me".into());
        let r = &ag.rows(&s)[0];
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
        let r = &ag.rows(&s)[0];
        assert!(!r.busy);
        assert_eq!(
            r.last_turn.as_deref(),
            Some("last turn 12k tok $0.50 · opus")
        );
        // Offline, or not one of yours: not listed.
        ag.presence("a1", "offline");
        ag.presence("x9", "online");
        assert!(ag.rows(&s).is_empty());
    }
}
