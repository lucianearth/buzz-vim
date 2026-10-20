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
            Node::Jev(Stream::Other) | Node::Claude(Check::Other) | Node::Outcome(Outcome::Other) => {
                "other"
            }
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
            if self.stage == "judged" {
                let s = stream(self.jev.as_deref());
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
    let post = d.posts.last();
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
    d.at + w.secs() >= now
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
    posts: Vec<String>,
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
                    posts: j.posts,
                    state: j.state,
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

    /// Every channel's decisions from the newest log a watcher signed.
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
        out
    }
}
