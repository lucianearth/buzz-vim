//! The routing view (see `buzz_vim_core::routing`): the watcher's
//! stall-judge decisions as layered boxes, a sampled decision per box and
//! the Why for one decision. Flat records, like the rest of the door.

use buzz_vim_core::needs::one_line;
use buzz_vim_core::routing::{self, Decision, Node, Window};

use crate::{Client, Engine, now};

/// The time scrubber: 1h / 24h / 7d.
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq)]
pub enum RouteWindow {
    Hour,
    Day,
    Week,
}

impl From<RouteWindow> for Window {
    fn from(w: RouteWindow) -> Self {
        match w {
            RouteWindow::Hour => Window::Hour,
            RouteWindow::Day => Window::Day,
            RouteWindow::Week => Window::Week,
        }
    }
}

/// One box. `key` is what `route_sample` takes back ("jev:asks_something").
#[derive(uniffi::Record, Clone)]
pub struct RouteNode {
    pub key: String,
    pub label: String,
    /// 0..5: stops seen, running, Jev, Claude check, outcome.
    pub layer: u32,
    pub count: u32,
}

/// A band between two boxes: how many decisions went from one to the next.
#[derive(uniffi::Record, Clone)]
pub struct RouteEdge {
    pub from: String,
    pub to: String,
    pub count: u32,
}

/// The diagram for one window.
#[derive(uniffi::Record, Clone)]
pub struct RouteFlow {
    /// Layer names, top to bottom; `RouteNode.layer` indexes them.
    pub layers: Vec<String>,
    /// Every box drawn, layer by layer, left to right. "other" only when
    /// something is in it.
    pub nodes: Vec<RouteNode>,
    pub edges: Vec<RouteEdge>,
}

/// One judge run, with what the app shows for it. Hand it back to
/// `route_why` for its steps.
#[derive(uniffi::Record, Clone)]
pub struct RouteDecision {
    pub channel: String,
    /// "#name".
    pub channel_label: String,
    /// The routing record that carries it.
    pub record: String,
    /// The channel's watch record, if we hold one.
    pub watch: Option<String>,
    pub at: u64,
    pub thread: Option<String>,
    /// The message the watcher stopped on.
    pub stop: Option<String>,
    /// Who wrote the stop message, "" when we don't hold it.
    pub who_name: String,
    /// Its first line, "" when we don't hold it.
    pub text: String,
    pub trigger: String,
    pub stage: String,
    /// Jev's fine label.
    pub jev: Option<String>,
    /// The Claude check's verdict, when it ran.
    pub model: Option<String>,
    pub outcome: String,
    /// Nudges and pings it published.
    pub posts: Vec<String>,
    pub state: Option<String>,
    pub reason: String,
    /// Keys of the boxes it went through, top to bottom.
    pub path: Vec<String>,
    /// "Open thread": the stop message in its thread, else the thread.
    pub link: Option<String>,
}

/// One step of a decision's Why.
#[derive(uniffi::Record, Clone)]
pub struct RouteStep {
    /// The box it stands for.
    pub key: String,
    /// "watcher saw it", "checklist", "jev", "claude check", "result".
    pub title: String,
    /// The branch taken ("asks something").
    pub label: String,
    /// The fine print ("asks_decision", the judge's reason).
    pub detail: String,
    /// The event behind it: the stop message, the watch record, the
    /// routing record, the posted nudge or ping.
    pub event_id: Option<String>,
    /// `buzz://` link when the event is a message.
    pub link: Option<String>,
}

fn decision(e: &Engine, d: &Decision) -> RouteDecision {
    let held = d.stop.as_deref().and_then(|s| e.store.find_message(s));
    RouteDecision {
        channel_label: e.label(&d.channel),
        who_name: held
            .map(|(_, m)| e.store.name_of(&m.author))
            .unwrap_or_default(),
        text: held.map(|(_, m)| one_line(&m.content)).unwrap_or_default(),
        path: d.path().into_iter().map(Node::key).collect(),
        link: d.link(),
        channel: d.channel.clone(),
        record: d.record.clone(),
        watch: d.watch.clone(),
        at: d.at,
        thread: d.thread.clone(),
        stop: d.stop.clone(),
        trigger: d.trigger.clone(),
        stage: d.stage.clone(),
        jev: d.jev.clone(),
        model: d.model.clone(),
        outcome: d.outcome.clone(),
        posts: d.posts.clone(),
        state: d.state.clone(),
        reason: d.reason.clone(),
    }
}

/// The core decision a `RouteDecision` came from.
fn core_decision(d: RouteDecision) -> Decision {
    Decision {
        channel: d.channel,
        record: d.record,
        watch: d.watch,
        at: d.at,
        thread: d.thread,
        stop: d.stop,
        trigger: d.trigger,
        stage: d.stage,
        jev: d.jev,
        model: d.model,
        outcome: d.outcome,
        posts: d.posts,
        state: d.state,
        reason: d.reason,
    }
}

pub(crate) fn flow(ds: &[Decision], now: u64, w: Window) -> RouteFlow {
    let f = routing::flow(ds, now, w);
    RouteFlow {
        layers: f.layers.iter().map(|l| l.name.to_string()).collect(),
        nodes: f
            .layers
            .iter()
            .flat_map(|l| &l.nodes)
            .map(|(n, c)| RouteNode {
                key: n.key(),
                label: n.label().to_string(),
                layer: n.layer() as u32,
                count: *c as u32,
            })
            .collect(),
        edges: f
            .edges
            .iter()
            .map(|e| RouteEdge {
                from: e.from.key(),
                to: e.to.key(),
                count: e.count as u32,
            })
            .collect(),
    }
}

/// A decision's Why: the chain of steps, each pointing at its record.
#[uniffi::export]
pub fn route_why(decision: RouteDecision) -> Vec<RouteStep> {
    routing::why(&core_decision(decision))
        .into_iter()
        .map(|s| RouteStep {
            key: s.node.key(),
            title: s.title.to_string(),
            label: s.label,
            detail: s.detail,
            event_id: s.event,
            link: s.link,
        })
        .collect()
}

#[uniffi::export]
impl Client {
    /// The routing diagram for the decisions within `window` of now, from
    /// every channel we read.
    pub fn route_flow(&self, window: RouteWindow) -> RouteFlow {
        let e = self.engine.lock().unwrap();
        flow(&e.checklists.decisions(), now(), window.into())
    }

    /// The `n`th newest decision through the box `key` within `window`,
    /// wrapping: "sample another" asks for n + 1. None for an unknown key
    /// or an empty box.
    pub fn route_sample(&self, window: RouteWindow, key: String, n: u32) -> Option<RouteDecision> {
        let node = Node::from_key(&key)?;
        let e = self.engine.lock().unwrap();
        let ds = e.checklists.decisions();
        let d = routing::sample(&ds, now(), window.into(), node, n as usize)?;
        Some(decision(&e, d))
    }
}
