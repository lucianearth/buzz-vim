//! The routing visualizer: how the watcher's stall-judge routed every stop
//! it judged, from its routing logs (kind 30472, `checklist-type=routing`,
//! `d` = `buzz-checklist/routing/<channel>`, one per channel, signed by the
//! watcher). Both apps draw the same layers from here:
//!
//! stops seen -> running -> Jev stream -> Claude check (asks only) ->
//! outcome (nudge the driver / ping you / no action).
//!
//! The watch reader never takes these records: it wants `checklist-type=watch`
//! and a three-segment `d`. Labels this build doesn't know go in "other".

use std::collections::{BTreeMap, HashMap};

use nostr::Event;
use serde::Deserialize;
use serde_json::Value;

use crate::checklist::{KIND_WATCH, TAG, lenient, message_link};
use crate::model::tag;

pub const TYPE: &str = "routing";

/// The view's time scrubber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Hour,
    Day,
    Week,
}

impl Window {
    pub const ALL: [Window; 3] = [Window::Hour, Window::Day, Window::Week];

    pub fn secs(self) -> u64 {
        match self {
            Window::Hour => 3600,
            Window::Day => 86_400,
            Window::Week => 7 * 86_400,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Window::Hour => "1h",
            Window::Day => "24h",
            Window::Week => "7d",
        }
    }
}

/// Jev's 7 fine labels, folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stream {
    KeepsGoing,
    WaitingOnBot,
    Finished,
    Asks,
    Other,
}

/// The Claude check, which only "asks something" reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Check {
    AgentDecides,
    NeedsOwner,
    NotAQuestion,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    NoAction,
    Nudge,
    Ping,
    Other,
}

/// A box in the diagram; layer order, then left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Node {
    Seen,
    Running,
    Jev(Stream),
    Claude(Check),
    Outcome(Outcome),
}

impl Node {
    /// Every node, in drawing order.
    pub const ALL: [Node; 15] = [
        Node::Seen,
        Node::Running,
        Node::Jev(Stream::KeepsGoing),
        Node::Jev(Stream::WaitingOnBot),
        Node::Jev(Stream::Finished),
        Node::Jev(Stream::Asks),
        Node::Jev(Stream::Other),
        Node::Claude(Check::AgentDecides),
        Node::Claude(Check::NeedsOwner),
        Node::Claude(Check::NotAQuestion),
        Node::Claude(Check::Other),
        Node::Outcome(Outcome::NoAction),
        Node::Outcome(Outcome::Nudge),
        Node::Outcome(Outcome::Ping),
        Node::Outcome(Outcome::Other),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Node::Seen => "stops seen",
            Node::Running => "running",
            Node::Jev(Stream::KeepsGoing) => "keeps going",
            Node::Jev(Stream::WaitingOnBot) => "waiting on a bot",
            Node::Jev(Stream::Finished) => "finished",
            Node::Jev(Stream::Asks) => "asks something",
            Node::Claude(Check::AgentDecides) => "agent decides",
            Node::Claude(Check::NeedsOwner) => "needs you",
            Node::Claude(Check::NotAQuestion) => "not a question",
            Node::Outcome(Outcome::NoAction) => "no action",
            Node::Outcome(Outcome::Nudge) => "nudge the driver",
            Node::Outcome(Outcome::Ping) => "ping you",
            Node::Jev(Stream::Other)
            | Node::Claude(Check::Other)
            | Node::Outcome(Outcome::Other) => "other",
        }
    }

    /// 0..5: stops seen, running, Jev, Claude check, outcome.
    pub fn layer(self) -> usize {
        match self {
            Node::Seen => 0,
            Node::Running => 1,
            Node::Jev(_) => 2,
            Node::Claude(_) => 3,
            Node::Outcome(_) => 4,
        }
    }

    /// A stable name for the apps to hand back ("jev:asks").
    pub fn key(self) -> String {
        let l = self.label().replace(' ', "_");
        match self {
            Node::Seen | Node::Running => l,
            Node::Jev(_) => format!("jev:{l}"),
            Node::Claude(_) => format!("claude:{l}"),
            Node::Outcome(_) => format!("out:{l}"),
        }
    }

    pub fn from_key(k: &str) -> Option<Node> {
        Node::ALL.into_iter().find(|n| n.key() == k)
    }
}

pub const LAYERS: [&str; 5] = ["stops seen", "running", "jev", "claude check", "outcome"];

fn stream(jev: Option<&str>) -> Stream {
    match jev {
        Some("asks_decision" | "needs_access" | "needs_human_action") => Stream::Asks,
        Some("will_continue" | "cut_off") => Stream::KeepsGoing,
        Some("waiting_on_agent") => Stream::WaitingOnBot,
        Some("reported_done") => Stream::Finished,
        _ => Stream::Other,
    }
}

fn check(model: Option<&str>) -> Check {
    match model {
        Some("agent_decides") => Check::AgentDecides,
        Some("needs_owner") => Check::NeedsOwner,
        Some("not_a_question") => Check::NotAQuestion,
        _ => Check::Other,
    }
}

fn outcome(o: &str) -> Outcome {
    match o {
        "nudge" => Outcome::Nudge,
        "ping" => Outcome::Ping,
        "none" => Outcome::NoAction,
        _ => Outcome::Other,
    }
}

/// One judge run, as the routing log has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub channel: String,
    /// The routing record (its newest version) that carries it.
    pub record: String,
    /// The channel's watch record, newest version, if we hold one.
    pub watch: Option<String>,
    pub at: u64,
    /// Root of the thread the watcher judged.
    pub thread: Option<String>,
    /// The message the watcher stopped on.
    pub stop: Option<String>,
    pub trigger: String,
    pub stage: String,
    /// Jev's fine label.
    pub jev: Option<String>,
    /// The Claude check's verdict, when it ran.
    pub model: Option<String>,
    pub outcome: String,
    /// Nudges and pings this run published.
    pub posts: Vec<String>,
    /// The watch state after.
    pub state: Option<String>,
    pub reason: String,
}

impl Decision {
    /// The boxes it passes through, top to bottom.
    pub fn path(&self) -> Vec<Node> {
        let mut p = vec![Node::Seen];
        if !matches!(self.stage.as_str(), "not_running" | "contested_idle") {
            p.push(Node::Running);
            // Judged routes that never asked Jev (owner or bot unanswered,
            // long-turn status, backoff, contested ping) go straight on.
            if let (true, Some(jev)) = (self.stage == "judged", self.jev.as_deref()) {
                let s = stream(Some(jev));
                p.push(Node::Jev(s));
                if s == Stream::Asks {
                    p.push(Node::Claude(check(self.model.as_deref())));
                }
            }
        }
        p.push(Node::Outcome(outcome(&self.outcome)));
        p
    }

    /// Where "open thread" goes: the stop message in its thread, else the
    /// thread.
    pub fn link(&self) -> Option<String> {
        match (&self.stop, &self.thread) {
            (Some(s), t) => Some(message_link(&self.channel, s, t.as_deref())),
            (None, Some(t)) => Some(message_link(&self.channel, t, None)),
            (None, None) => None,
        }
    }
}

/// One step of a decision's Why, pointing at the event behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub node: Node,
    /// "watcher saw it", "checklist", "jev", "claude check", "result".
    pub title: &'static str,
    /// The branch taken ("asks something").
    pub label: String,
    /// The fine print ("asks_decision", the judge's reason).
    pub detail: String,
    /// The event: the stop message, the watch record, the routing record,
    /// the posted nudge or ping.
    pub event: Option<String>,
    /// `buzz://` link when the event is a message.
    pub link: Option<String>,
}

fn words(s: &str) -> String {
    s.replace('_', " ")
}

fn stage_label(s: &str) -> String {
    match s {
        "not_running" => "list not running".into(),
        "contested_idle" => "contested, nothing runs".into(),
        "not_work_thread" => "not the work thread".into(),
        "judged" => "list running".into(),
        "turn_started" => "a turn started while judging".into(),
        "stopped_running" => "the list stopped running".into(),
        "thread_moved" => "the thread moved".into(),
        "" => "other".into(),
        o => words(o),
    }
}

fn trigger_label(t: &str) -> String {
    match t {
        "idle" => "agent stopped".into(),
        "" => "stop".into(),
        o => words(o),
    }
}

/// The chain for one decision: what the watcher saw, the checklist's
/// state, Jev's route, the Claude check (asks only) and the result.
pub fn why(d: &Decision) -> Vec<Step> {
    let msg = |id: &str| message_link(&d.channel, id, d.thread.as_deref());
    let path = d.path();
    let mut out = vec![Step {
        node: Node::Seen,
        title: "watcher saw it",
        label: trigger_label(&d.trigger),
        detail: String::new(),
        event: d.stop.clone(),
        link: d.stop.as_deref().map(msg),
    }];
    out.push(Step {
        node: Node::Running,
        title: "checklist",
        label: stage_label(&d.stage),
        detail: String::new(),
        event: d.watch.clone(),
        link: None,
    });
    let claude = path.iter().any(|n| matches!(n, Node::Claude(_)));
    for n in &path {
        match n {
            Node::Jev(_) => out.push(Step {
                node: *n,
                title: "jev",
                label: n.label().into(),
                detail: d.jev.clone().unwrap_or_default(),
                event: Some(d.record.clone()),
                link: None,
            }),
            Node::Claude(_) => out.push(Step {
                node: *n,
                title: "claude check",
                label: n.label().into(),
                detail: d.reason.clone(),
                event: Some(d.record.clone()),
                link: None,
            }),
            _ => {}
        }
    }
    let last = *path.last().unwrap_or(&Node::Seen);
    let mut detail = vec![];
    if let Some(s) = d.state.as_deref().filter(|s| !s.is_empty()) {
        detail.push(words(s));
    }
    if !claude && !d.reason.is_empty() {
        detail.push(d.reason.clone());
    }
    // A ping run with open items posts the ping, then the nudge: "ping you"
    // is the first post. Any other run's result is its last.
    let post = if d.outcome == "ping" {
        d.posts.first()
    } else {
        d.posts.last()
    };
    out.push(Step {
        node: last,
        title: "result",
        label: last.label().into(),
        detail: detail.join(" · "),
        event: Some(post.unwrap_or(&d.record).clone()),
        link: post.map(|p| msg(p)),
    });
    out
}

/// A layer's boxes with their counts. "other" only when something is in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: &'static str,
    pub nodes: Vec<(Node, usize)>,
}

impl Layer {
    pub fn total(&self) -> usize {
        self.nodes.iter().map(|(_, n)| n).sum()
    }
}

/// One band of the Sankey: how many decisions went from one box to the next
/// box on their path (a no-op skips straight to its outcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: Node,
    pub to: Node,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub layers: Vec<Layer>,
    /// In node order, from then to.
    pub edges: Vec<Edge>,
}

impl Flow {
    pub fn count(&self, n: Node) -> usize {
        self.layers
            .iter()
            .flat_map(|l| &l.nodes)
            .find(|(m, _)| *m == n)
            .map_or(0, |(_, c)| *c)
    }

    /// Every box drawn, in order: what Tab / j / k step through.
    pub fn nodes(&self) -> Vec<Node> {
        self.layers
            .iter()
            .flat_map(|l| l.nodes.iter().map(|(n, _)| *n))
            .collect()
    }
}

fn in_window(d: &Decision, now: u64, w: Window) -> bool {
    // `at` is whatever the record says: a huge one mustn't overflow.
    d.at.saturating_add(w.secs()) >= now
}

/// The diagram for the decisions within `w` of `now`.
pub fn flow(ds: &[Decision], now: u64, w: Window) -> Flow {
    let mut counts: HashMap<Node, usize> = HashMap::new();
    let mut edges: BTreeMap<(Node, Node), usize> = BTreeMap::new();
    for d in ds.iter().filter(|d| in_window(d, now, w)) {
        let p = d.path();
        for n in &p {
            *counts.entry(*n).or_default() += 1;
        }
        for e in p.windows(2) {
            *edges.entry((e[0], e[1])).or_default() += 1;
        }
    }
    let layers = LAYERS
        .iter()
        .enumerate()
        .map(|(i, name)| Layer {
            name,
            nodes: Node::ALL
                .into_iter()
                .filter(|n| n.layer() == i)
                .map(|n| (n, counts.get(&n).copied().unwrap_or(0)))
                .filter(|(n, c)| *c > 0 || n.label() != "other")
                .collect(),
        })
        .collect();
    Flow {
        layers,
        edges: edges
            .into_iter()
            .map(|((from, to), count)| Edge { from, to, count })
            .collect(),
    }
}

/// The `n`th newest decision through `node` within the window, round again
/// past the oldest: "sample another" asks for n + 1.
pub fn sample(ds: &[Decision], now: u64, w: Window, node: Node, n: usize) -> Option<&Decision> {
    let mut hits: Vec<&Decision> = ds
        .iter()
        .filter(|d| in_window(d, now, w) && d.path().contains(&node))
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.channel.cmp(&b.channel)));
    Some(hits[n % hits.len()])
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LogJson {
    #[serde(deserialize_with = "lenient")]
    decisions: Vec<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DecisionJson {
    #[serde(deserialize_with = "lenient")]
    at: u64,
    #[serde(deserialize_with = "lenient")]
    thread: Option<String>,
    #[serde(deserialize_with = "lenient")]
    stop: Option<String>,
    #[serde(deserialize_with = "lenient")]
    trigger: String,
    #[serde(deserialize_with = "lenient")]
    stage: String,
    #[serde(deserialize_with = "lenient")]
    jev: Option<String>,
    #[serde(deserialize_with = "lenient")]
    model: Option<String>,
    #[serde(deserialize_with = "lenient")]
    outcome: String,
    #[serde(deserialize_with = "lenient")]
    posts: Vec<Value>,
    #[serde(deserialize_with = "lenient")]
    state: Option<String>,
    #[serde(deserialize_with = "lenient")]
    reason: String,
}

/// The channel a routing record logs, or None unless its envelope is valid:
/// kind 30472, `t`, `checklist-type=routing`, `h` == `channel`, and `d`
/// exactly `buzz-checklist/routing/<h>`.
pub fn channel_of(ev: &Event) -> Option<&str> {
    if ev.kind.as_u16() != KIND_WATCH
        || tag(ev, "t") != Some(TAG)
        || tag(ev, "checklist-type") != Some(TYPE)
    {
        return None;
    }
    let h = tag(ev, "h").filter(|h| !h.is_empty())?;
    (tag(ev, "channel") == Some(h) && tag(ev, "d")? == format!("{TAG}/{TYPE}/{h}")).then_some(h)
}

#[derive(Debug, Clone)]
struct Log {
    at: u64,
    id: String,
    decisions: Vec<Decision>,
}

/// Every channel's routing log: channel -> signer -> newest version.
/// Only the watcher's count, decided when read.
#[derive(Debug, Default)]
pub struct Logs {
    channels: HashMap<String, BTreeMap<String, Log>>,
}

impl Logs {
    /// Fold a routing record. True when valid and the newest from its signer.
    pub fn apply(&mut self, ev: &Event) -> bool {
        let Some(channel) = channel_of(ev) else {
            return false;
        };
        let (at, id) = (ev.created_at.as_secs(), ev.id.to_hex());
        let slot = self.channels.entry(channel.to_string()).or_default();
        let signer = ev.pubkey.to_hex();
        // Newer first; same second: the lower id wins, as NIP-01 says.
        if slot
            .get(&signer)
            .is_some_and(|o| (o.at, std::cmp::Reverse(&o.id)) >= (at, std::cmp::Reverse(&id)))
        {
            return false;
        }
        let log: LogJson = serde_json::from_str(&ev.content).unwrap_or_default();
        let decisions = log
            .decisions
            .into_iter()
            .filter(Value::is_object)
            .map(|v| {
                let j: DecisionJson = serde_json::from_value(v).unwrap_or_default();
                Decision {
                    channel: channel.to_string(),
                    record: id.clone(),
                    watch: None,
                    at: j.at,
                    thread: j.thread.filter(|s| !s.is_empty()),
                    stop: j.stop.filter(|s| !s.is_empty()),
                    trigger: j.trigger,
                    stage: j.stage,
                    jev: j.jev.filter(|s| !s.is_empty()),
                    model: j.model.filter(|s| !s.is_empty()),
                    outcome: j.outcome,
                    // One bad entry drops itself, not the list.
                    posts: j
                        .posts
                        .into_iter()
                        .filter_map(|p| p.as_str().filter(|s| !s.is_empty()).map(String::from))
                        .collect(),
                    state: j.state.filter(|s| !s.is_empty()),
                    reason: j.reason,
                }
            })
            .collect();
        slot.insert(
            signer,
            Log {
                at,
                id: id.clone(),
                decisions,
            },
        );
        true
    }

    /// Every channel's decisions from the newest log a watcher signed,
    /// oldest first.
    pub fn decisions(&self, watchers: &[String]) -> Vec<Decision> {
        let mut out = vec![];
        for slot in self.channels.values() {
            let best = slot
                .iter()
                .filter(|(s, _)| watchers.contains(s))
                .map(|(_, l)| l)
                .max_by(|a, b| {
                    (a.at, std::cmp::Reverse(&a.id)).cmp(&(b.at, std::cmp::Reverse(&b.id)))
                });
            if let Some(l) = best {
                out.extend(l.decisions.iter().cloned());
            }
        }
        // Stable, so one log's same-second entries keep their order.
        out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.channel.cmp(&b.channel)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::{Checklists, Trust, WatchState};
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use serde_json::json;

    const CH: &str = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
    const NOW: u64 = 1_792_460_000;

    fn hex(k: &Keys) -> String {
        k.public_key().to_hex()
    }

    /// An event with these tags, as written.
    fn ev(keys: &Keys, kind: u16, tags: &[[&str; 2]], at: u64, content: &str) -> Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.iter().map(|t| Tag::parse(*t).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn envelope(ty: &str, d: &str) -> Vec<[String; 2]> {
        [
            ["d", d],
            ["h", CH],
            ["t", TAG],
            ["channel", CH],
            ["checklist-type", ty],
        ]
        .map(|[a, b]| [a.to_string(), b.to_string()])
        .to_vec()
    }

    fn with(tags: &[[String; 2]]) -> Vec<[&str; 2]> {
        tags.iter().map(|[a, b]| [a.as_str(), b.as_str()]).collect()
    }

    fn routing_d() -> String {
        format!("{TAG}/{TYPE}/{CH}")
    }

    /// A valid routing log in CH with these decisions.
    fn log(keys: &Keys, at: u64, decisions: Value) -> Event {
        let content = json!({"v": 1, "decisions": decisions}).to_string();
        ev(
            keys,
            KIND_WATCH,
            &with(&envelope(TYPE, &routing_d())),
            at,
            &content,
        )
    }

    fn dec(at: u64, stage: &str, jev: Option<&str>, model: Option<&str>, outcome: &str) -> Value {
        json!({"at": at, "thread": "root1", "stop": format!("stop{at}"), "trigger": "idle",
            "stage": stage, "jev": jev, "model": model, "outcome": outcome,
            "posts": if outcome == "none" { vec![] } else { vec![format!("post{at}")] },
            "state": "working", "reason": "r"})
    }

    fn logs_with(w: &Keys, ds: Value) -> Vec<Decision> {
        let mut l = Logs::default();
        assert!(l.apply(&log(w, NOW, ds)));
        l.decisions(&[hex(w)])
    }

    #[test]
    fn envelope_must_be_exact() {
        let w = Keys::generate();
        assert_eq!(channel_of(&log(&w, NOW, json!([]))), Some(CH));
        let good = envelope(TYPE, &routing_d());
        let bad = |i: usize, v: &str| {
            let mut t = good.clone();
            t[i][1] = v.to_string();
            ev(&w, KIND_WATCH, &with(&t), NOW, "{}")
        };
        // d: wrong prefix, type, channel, extra or missing segment, case.
        for d in [
            format!("x/{TYPE}/{CH}"),
            format!("{TAG}/watch/{CH}"),
            format!("{TAG}/{TYPE}/other"),
            format!("{TAG}/{TYPE}/{CH}/extra"),
            format!("{TAG}/{TYPE}"),
            format!("{TAG}/{TYPE}/{}", CH.to_uppercase()),
        ] {
            assert_eq!(channel_of(&bad(0, &d)), None, "{d}");
        }
        assert_eq!(channel_of(&bad(1, "")), None, "empty h");
        assert_eq!(channel_of(&bad(2, "other-tag")), None, "t");
        assert_eq!(channel_of(&bad(3, "elsewhere")), None, "channel != h");
        assert_eq!(channel_of(&bad(4, "watch")), None, "checklist-type");
        // Wrong kind, or a tag missing.
        let wrong_kind = ev(&w, 30471, &with(&good), NOW, "{}");
        assert_eq!(channel_of(&wrong_kind), None);
        for i in 0..good.len() {
            let mut t = good.clone();
            t.remove(i);
            assert_eq!(
                channel_of(&ev(&w, KIND_WATCH, &with(&t), NOW, "{}")),
                None,
                "without {}",
                good[i][0]
            );
        }
        let mut l = Logs::default();
        assert!(!l.apply(&bad(4, "watch")));
        assert!(l.decisions(&[hex(&w)]).is_empty());
    }

    /// A routing record never reads as a watch record, and a watch record
    /// never reads as routing, both kind 30472 in the same channel.
    #[test]
    fn routing_and_watch_records_never_cross() {
        let w = Keys::generate();
        let mut s = Checklists::new(Trust {
            owner: hex(&Keys::generate()),
            watchers: vec![hex(&w)],
        });
        // A routing log whose content even looks like a watch record.
        let content = json!({"v": 1, "state": "stalled", "decisions": [dec(NOW, "judged",
            Some("reported_done"), None, "nudge")]});
        let r = ev(
            &w,
            KIND_WATCH,
            &with(&envelope(TYPE, &routing_d())),
            NOW,
            &content.to_string(),
        );
        assert!(s.apply(&r));
        assert!(s.watch(CH).is_none(), "routing read as watch");
        assert_eq!(s.decisions().len(), 1);
        // A watch record whose content looks like a routing log.
        let wd = format!("{TAG}/watch/{CH}");
        let wc = json!({"v": 1, "state": "stalled", "decisions": [dec(NOW, "judged", None, None, "ping")]});
        let wr = ev(
            &w,
            KIND_WATCH,
            &with(&envelope("watch", &wd)),
            NOW + 1,
            &wc.to_string(),
        );
        assert_eq!(channel_of(&wr), None);
        assert!(s.apply(&wr));
        assert_eq!(s.watch(CH).unwrap().state, WatchState::Stalled);
        let ds = s.decisions();
        assert_eq!(ds.len(), 1, "watch read as routing");
        assert_eq!(ds[0].outcome, "nudge");
        // Each decision carries the channel's watch record.
        assert_eq!(ds[0].watch.as_deref(), Some(wr.id.to_hex().as_str()));
        // The mixes: routing type with the watch d, watch type with the
        // routing d. Neither reader takes them.
        for (ty, d) in [(TYPE, wd.clone()), ("watch", routing_d())] {
            let e = ev(&w, KIND_WATCH, &with(&envelope(ty, &d)), NOW + 5, "{}");
            assert!(!s.apply(&e), "{ty} {d}");
        }
        assert_eq!(s.watch(CH).unwrap().event_id, wr.id.to_hex());
        assert_eq!(s.decisions().len(), 1);
    }

    #[test]
    fn only_watcher_signed_logs_count() {
        let (w, fake) = (Keys::generate(), Keys::generate());
        let mut l = Logs::default();
        let one = json!([dec(NOW, "judged", Some("reported_done"), None, "nudge")]);
        assert!(l.apply(&log(&w, NOW, one)));
        // Newer, but not the watcher's: stored, never counted.
        let many = Value::Array(vec![dec(NOW, "not_running", None, None, "none"); 3]);
        assert!(l.apply(&log(&fake, NOW + 10, many)));
        assert_eq!(l.decisions(&[hex(&w)]).len(), 1);
        assert_eq!(l.decisions(&[hex(&fake)]).len(), 3);
        assert!(l.decisions(&[]).is_empty());
        // Two watcher keys: the newer log wins.
        assert_eq!(l.decisions(&[hex(&w), hex(&fake)]).len(), 3);
    }

    #[test]
    fn the_newest_version_replaces_the_log() {
        let w = Keys::generate();
        let mut l = Logs::default();
        let v1 = log(
            &w,
            NOW,
            json!([dec(NOW - 5, "not_running", None, None, "none")]),
        );
        let v2 = log(
            &w,
            NOW + 1,
            json!([
                dec(NOW - 5, "not_running", None, None, "none"),
                dec(NOW, "judged", Some("cut_off"), None, "nudge")
            ]),
        );
        assert!(l.apply(&v2));
        // Older arrives late, and a replay of the same version: ignored.
        assert!(!l.apply(&v1));
        assert!(!l.apply(&v2));
        let ds = l.decisions(&[hex(&w)]);
        assert_eq!(ds.len(), 2, "replaced, not appended");
        assert!(ds.iter().all(|d| d.record == v2.id.to_hex()));
        // Same second: the lower id wins (NIP-01).
        let a = log(&w, NOW + 2, json!([dec(NOW, "judged", None, None, "ping")]));
        let b = log(&w, NOW + 2, json!([dec(NOW, "judged", None, None, "none")]));
        let (lo, hi) = if a.id.to_hex() < b.id.to_hex() {
            (a, b)
        } else {
            (b, a)
        };
        assert!(l.apply(&hi));
        assert!(l.apply(&lo));
        assert!(!l.apply(&hi));
        assert_eq!(l.decisions(&[hex(&w)])[0].record, lo.id.to_hex());
    }

    #[test]
    fn parsing_is_lenient() {
        let w = Keys::generate();
        let mut l = Logs::default();
        // Not JSON, not an object, decisions not a list: an empty log that
        // still replaces the older one.
        for (i, c) in ["not json", "[1,2]", r#"{"decisions": "x"}"#]
            .iter()
            .enumerate()
        {
            let e = ev(
                &w,
                KIND_WATCH,
                &with(&envelope(TYPE, &routing_d())),
                NOW + i as u64,
                c,
            );
            assert!(l.apply(&e), "{c}");
            assert!(l.decisions(&[hex(&w)]).is_empty(), "{c}");
        }
        let ds = logs_with(
            &w,
            json!([
                "not an object",
                {"at": "soon", "stage": 7, "jev": ["x"], "outcome": "nudge",
                 "posts": ["p1", 5, "", null, "p2"], "thread": "", "stop": null,
                 "state": "", "reason": {"long": true}, "extra": 1},
                {"at": NOW, "stage": "judged", "jev": "asks_decision", "model": "needs_owner",
                 "outcome": "ping", "posts": "p3"}
            ]),
        );
        assert_eq!(ds.len(), 2, "a non-object entry drops itself only");
        let d = ds.iter().find(|d| d.outcome == "nudge").unwrap();
        assert_eq!((d.at, d.stage.as_str(), d.jev.as_deref()), (0, "", None));
        assert_eq!(d.posts, ["p1", "p2"]);
        assert_eq!((d.thread.as_deref(), d.stop.as_deref()), (None, None));
        assert_eq!((d.state.as_deref(), d.reason.as_str()), (None, ""));
        assert_eq!(d.channel, CH);
        let p = ds.iter().find(|d| d.outcome == "ping").unwrap();
        assert!(p.posts.is_empty());
        assert_eq!(p.model.as_deref(), Some("needs_owner"));
        // A wild `at` neither panics nor falls out of every window.
        let mut big = p.clone();
        big.at = u64::MAX;
        assert_eq!(flow(&[big], NOW, Window::Hour).count(Node::Seen), 1);
    }

    #[test]
    fn unknown_labels_go_to_other() {
        let d = |stage: &str, jev: Option<&str>, model: Option<&str>, out: &str| Decision {
            channel: CH.into(),
            record: "r".into(),
            watch: None,
            at: NOW,
            thread: None,
            stop: None,
            trigger: "idle".into(),
            stage: stage.into(),
            jev: jev.map(String::from),
            model: model.map(String::from),
            outcome: out.into(),
            posts: vec![],
            state: None,
            reason: String::new(),
        };
        use Node::*;
        assert_eq!(
            d("judged", Some("new_label"), None, "nudge").path(),
            [
                Seen,
                Running,
                Jev(Stream::Other),
                Outcome(super::Outcome::Nudge)
            ]
        );
        assert_eq!(
            d("judged", None, None, "nudge").path(),
            [Seen, Running, Outcome(super::Outcome::Nudge)],
            "judged without asking Jev skips its layer"
        );
        assert_eq!(
            d("judged", Some("needs_access"), Some("maybe"), "ping").path()[3],
            Claude(Check::Other)
        );
        assert_eq!(
            d("judged", Some("needs_access"), None, "ping").path()[3],
            Claude(Check::Other),
            "asks always reaches the check"
        );
        assert_eq!(
            *d("judged", Some("reported_done"), None, "escalate")
                .path()
                .last()
                .unwrap(),
            Outcome(super::Outcome::Other)
        );
        // "other" boxes show only when something is in them.
        let f = flow(
            &[d("judged", Some("reported_done"), None, "nudge")],
            NOW,
            Window::Day,
        );
        assert!(!f.nodes().iter().any(|n| n.label() == "other"));
        let f = flow(&[d("judged", Some("zzz"), None, "boom")], NOW, Window::Day);
        assert!(f.nodes().contains(&Jev(Stream::Other)));
        assert!(f.nodes().contains(&Outcome(super::Outcome::Other)));
        assert!(!f.nodes().contains(&Claude(Check::Other)));
        // Keys round-trip and are unique.
        for n in Node::ALL {
            assert_eq!(Node::from_key(&n.key()), Some(n), "{}", n.key());
        }
    }

    #[test]
    fn no_op_stages_skip_to_the_outcome() {
        let w = Keys::generate();
        use Node::*;
        for (stage, running) in [
            ("not_running", false),
            ("contested_idle", false),
            ("not_work_thread", true),
            ("turn_started", true),
            ("stopped_running", true),
            ("thread_moved", true),
        ] {
            let ds = logs_with(
                &w,
                json!([dec(
                    NOW,
                    stage,
                    Some("asks_decision"),
                    Some("needs_owner"),
                    "none"
                )]),
            );
            let want = if running {
                vec![Seen, Running, Outcome(super::Outcome::NoAction)]
            } else {
                vec![Seen, Outcome(super::Outcome::NoAction)]
            };
            assert_eq!(ds[0].path(), want, "{stage}");
        }
        let ds = logs_with(
            &w,
            json!([dec(
                NOW,
                "judged",
                Some("asks_decision"),
                Some("needs_owner"),
                "ping"
            )]),
        );
        assert_eq!(
            ds[0].path(),
            [
                Seen,
                Running,
                Jev(Stream::Asks),
                Claude(Check::NeedsOwner),
                Outcome(super::Outcome::Ping)
            ]
        );
        // Only "asks something" reaches the Claude check.
        for jev in [
            "will_continue",
            "cut_off",
            "waiting_on_agent",
            "reported_done",
        ] {
            let ds = logs_with(
                &w,
                json!([dec(NOW, "judged", Some(jev), Some("needs_owner"), "nudge")]),
            );
            assert!(
                !ds[0].path().iter().any(|n| matches!(n, Claude(_))),
                "{jev}"
            );
        }
    }

    /// The mock's mix: 6 not running, 2 asks (1 ping, 1 agent decides ->
    /// nudge), 3 keep going, 1 finished, 1 turn started.
    fn mix() -> Value {
        json!([
            dec(NOW - 100, "not_running", None, None, "none"),
            dec(NOW - 100, "not_running", None, None, "none"),
            dec(NOW - 100, "contested_idle", None, None, "none"),
            dec(NOW - 7200, "not_running", None, None, "none"),
            dec(NOW - 7200, "not_running", None, None, "none"),
            dec(NOW - 3 * 86_400, "not_running", None, None, "none"),
            dec(
                NOW - 60,
                "judged",
                Some("asks_decision"),
                Some("needs_owner"),
                "ping"
            ),
            dec(
                NOW - 50,
                "judged",
                Some("needs_access"),
                Some("agent_decides"),
                "nudge"
            ),
            dec(NOW - 40, "judged", Some("will_continue"), None, "nudge"),
            dec(NOW - 30, "judged", Some("cut_off"), None, "nudge"),
            dec(NOW - 9000, "judged", Some("cut_off"), None, "nudge"),
            dec(NOW - 20, "judged", Some("reported_done"), None, "none"),
            dec(NOW - 10, "turn_started", None, None, "none"),
        ])
    }

    #[test]
    fn the_result_step_opens_the_ping_and_no_label_skips_jev() {
        let d = |jev: Option<&str>, model: Option<&str>, out: &str, posts: &[&str]| Decision {
            channel: CH.into(),
            record: "r".into(),
            watch: None,
            at: NOW,
            thread: None,
            stop: None,
            trigger: "idle".into(),
            stage: "judged".into(),
            jev: jev.map(String::from),
            model: model.map(String::from),
            outcome: out.into(),
            posts: posts.iter().map(|p| p.to_string()).collect(),
            state: None,
            reason: String::new(),
        };
        // needs_owner with open items posts the ping, then the nudge.
        let ping = d(
            Some("asks_decision"),
            Some("needs_owner"),
            "ping",
            &["p1", "n1"],
        );
        assert_eq!(why(&ping).last().unwrap().event.as_deref(), Some("p1"));
        let nudge = d(Some("will_continue"), None, "nudge", &["s1", "n1"]);
        assert_eq!(why(&nudge).last().unwrap().event.as_deref(), Some("n1"));
        // A contested ping never asked Jev: no jev step, no "other" box.
        let contested = d(None, None, "ping", &["p2"]);
        assert!(why(&contested).iter().all(|s| s.title != "jev"));
        assert!(!contested.path().iter().any(|n| matches!(n, Node::Jev(_))));
    }

    #[test]
    fn flow_counts_and_edges() {
        let w = Keys::generate();
        let ds = logs_with(&w, mix());
        let f = flow(&ds, NOW, Window::Week);
        use super::Outcome as O;
        use Node::*;
        let c = |n| f.count(n);
        assert_eq!((c(Seen), c(Running)), (13, 7));
        assert_eq!(
            [
                c(Jev(Stream::KeepsGoing)),
                c(Jev(Stream::WaitingOnBot)),
                c(Jev(Stream::Finished)),
                c(Jev(Stream::Asks))
            ],
            [3, 0, 1, 2]
        );
        assert_eq!(
            [
                c(Claude(Check::AgentDecides)),
                c(Claude(Check::NeedsOwner)),
                c(Claude(Check::NotAQuestion))
            ],
            [1, 1, 0]
        );
        assert_eq!(
            [
                c(Outcome(O::NoAction)),
                c(Outcome(O::Nudge)),
                c(Outcome(O::Ping))
            ],
            [8, 4, 1]
        );
        // Every decision ends in exactly one outcome.
        assert_eq!(f.layers[4].total(), c(Seen));
        assert_eq!(f.layers.len(), 5);
        assert_eq!(f.layers[2].name, "jev");
        let e = |from, to| {
            f.edges
                .iter()
                .find(|e| e.from == from && e.to == to)
                .map_or(0, |e| e.count)
        };
        assert_eq!(e(Seen, Running), 7);
        assert_eq!(e(Seen, Outcome(O::NoAction)), 6, "no-ops skip the layers");
        assert_eq!(e(Running, Outcome(O::NoAction)), 1, "turn started");
        assert_eq!(e(Running, Jev(Stream::KeepsGoing)), 3);
        assert_eq!(e(Jev(Stream::KeepsGoing), Outcome(O::Nudge)), 3);
        assert_eq!(e(Jev(Stream::Asks), Claude(Check::NeedsOwner)), 1);
        assert_eq!(e(Claude(Check::NeedsOwner), Outcome(O::Ping)), 1);
        assert_eq!(e(Claude(Check::AgentDecides), Outcome(O::Nudge)), 1);
        assert_eq!(e(Jev(Stream::Finished), Outcome(O::NoAction)), 1);
        // Out of each box goes what came in (no box loses or gains).
        for n in f.nodes().into_iter().filter(|n| n.layer() < 4) {
            let out: usize = f
                .edges
                .iter()
                .filter(|e| e.from == n)
                .map(|e| e.count)
                .sum();
            assert_eq!(out, c(n), "{}", n.label());
        }
        // Edges come in node order.
        let keys: Vec<_> = f.edges.iter().map(|e| (e.from, e.to)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        // Tab order: every box drawn, top to bottom.
        assert_eq!(f.nodes()[0], Seen);
        assert_eq!(*f.nodes().last().unwrap(), Outcome(O::Ping));
    }

    #[test]
    fn the_window_filters() {
        let w = Keys::generate();
        let ds = logs_with(&w, mix());
        let seen = |win| flow(&ds, NOW, win).count(Node::Seen);
        assert_eq!(
            (seen(Window::Hour), seen(Window::Day), seen(Window::Week)),
            (9, 12, 13)
        );
        assert_eq!(
            flow(&ds, NOW + 8 * 86_400, Window::Week).count(Node::Seen),
            0
        );
        // The edge of the window counts.
        let at = |a| logs_with(&w, json!([dec(a, "not_running", None, None, "none")]));
        assert_eq!(
            flow(&at(NOW - 3600), NOW, Window::Hour).count(Node::Seen),
            1
        );
        assert_eq!(
            flow(&at(NOW - 3601), NOW, Window::Hour).count(Node::Seen),
            0
        );
        assert_eq!(Window::ALL.map(Window::label), ["1h", "24h", "7d"]);
    }

    #[test]
    fn sample_cycles_newest_first() {
        let w = Keys::generate();
        let ds = logs_with(&w, mix());
        let kg = Node::Jev(Stream::KeepsGoing);
        let ats: Vec<u64> = (0..4)
            .map(|n| sample(&ds, NOW, Window::Week, kg, n).unwrap().at)
            .collect();
        assert_eq!(
            ats,
            [NOW - 30, NOW - 40, NOW - 9000, NOW - 30],
            "round again"
        );
        // The window bounds what's sampled.
        let ats: Vec<u64> = (0..3)
            .map(|n| sample(&ds, NOW, Window::Hour, kg, n).unwrap().at)
            .collect();
        assert_eq!(ats, [NOW - 30, NOW - 40, NOW - 30]);
        assert!(sample(&ds, NOW, Window::Week, Node::Jev(Stream::WaitingOnBot), 0).is_none());
        let p = sample(
            &ds,
            NOW,
            Window::Hour,
            Node::Outcome(super::Outcome::Ping),
            7,
        )
        .unwrap();
        assert_eq!(p.jev.as_deref(), Some("asks_decision"));
        assert!(sample(&[], NOW, Window::Week, Node::Seen, 0).is_none());
    }

    #[test]
    fn why_walks_the_path_with_its_records() {
        let w = Keys::generate();
        let mut ds = logs_with(
            &w,
            json!([{"at": NOW, "thread": "root1", "stop": "stop1", "trigger": "idle",
                "stage": "judged", "jev": "asks_decision", "model": "needs_owner",
                "outcome": "ping", "posts": ["ping1"], "state": "waiting_on_lucian",
                "reason": "staging write, release step"}]),
        );
        ds[0].watch = Some("watch1".into());
        let d = &ds[0];
        let s = why(d);
        let titles: Vec<_> = s.iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            [
                "watcher saw it",
                "checklist",
                "jev",
                "claude check",
                "result"
            ]
        );
        let link = |id: &str| format!("buzz://message?channel={CH}&id={id}&thread=root1");
        assert_eq!(
            (s[0].label.as_str(), s[0].event.as_deref()),
            ("agent stopped", Some("stop1"))
        );
        assert_eq!(s[0].link.as_deref(), Some(link("stop1").as_str()));
        assert_eq!(
            (s[1].label.as_str(), s[1].event.as_deref()),
            ("list running", Some("watch1"))
        );
        assert_eq!(
            (s[2].label.as_str(), s[2].detail.as_str()),
            ("asks something", "asks_decision")
        );
        assert_eq!(s[2].event.as_deref(), Some(d.record.as_str()));
        assert_eq!(
            (s[3].label.as_str(), s[3].detail.as_str()),
            ("needs you", "staging write, release step")
        );
        assert_eq!(
            (s[4].label.as_str(), s[4].detail.as_str()),
            ("ping you", "waiting on lucian")
        );
        assert_eq!(s[4].event.as_deref(), Some("ping1"));
        assert_eq!(s[4].link.as_deref(), Some(link("ping1").as_str()));
        assert_eq!(d.link().as_deref(), Some(link("stop1").as_str()));
        // A no-op: what the watcher saw, why nothing ran, the result (the
        // routing record, nothing posted), with the reason.
        let n = logs_with(
            &w,
            json!([{"at": NOW, "thread": null, "stop": null, "trigger": "recheck",
                "stage": "not_running", "outcome": "none", "reason": "list ideating"}]),
        );
        let s = why(&n[0]);
        assert_eq!(
            s.iter().map(|s| s.title).collect::<Vec<_>>(),
            ["watcher saw it", "checklist", "result"]
        );
        assert_eq!(
            (s[0].label.as_str(), s[0].event.as_deref()),
            ("recheck", None)
        );
        assert_eq!(s[1].label, "list not running");
        assert_eq!(
            (s[2].label.as_str(), s[2].detail.as_str()),
            ("no action", "list ideating")
        );
        assert_eq!(
            (s[2].event.as_deref(), s[2].link.as_deref()),
            (Some(n[0].record.as_str()), None)
        );
        assert_eq!(n[0].link(), None);
    }
}
