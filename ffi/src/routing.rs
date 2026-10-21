//! The routing view (see `buzz_vim_core::routing`): the watcher's
//! stall-judge decisions as a pipeline of checks and cards, a sampled
//! decision per card or outcome line and the Why for one decision. Flat records, like the rest of the door.

use buzz_vim_core::needs::one_line;
use buzz_vim_core::routing::{self, Decision, Lane, Node, Outcome, Pick, Window};

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

/// One outcome line on a card: coloured dot, glyph, count, sub-lines.
#[derive(uniffi::Record, Clone)]
pub struct RouteOutcome {
    /// What `route_sample` takes back for this line ("needs_you/ping").
    pub key: String,
    /// "none", "nudge", "ping", "other".
    pub outcome: String,
    /// "—", "↻", "🔔".
    pub glyph: String,
    /// "no action", "nudged agent", "pinged you" (on a card); "nudged" under
    /// the totals bar.
    pub word: String,
    /// The reserved outcome colour, "#rrggbb".
    pub color: String,
    pub count: u32,
    /// Plain sub-lines, each its own line.
    pub notes: Vec<String>,
}

/// One exit stream: its strip on the trunk and its card.
#[derive(uniffi::Record, Clone)]
pub struct RouteCard {
    /// What `route_sample` takes back for the whole card ("needs_you").
    pub key: String,
    pub title: String,
    /// The plain line under the title.
    pub why: String,
    pub count: u32,
    /// Strip, border and accent, "#rrggbb".
    pub color: String,
    /// The same hue darkened to read as text on the light page: the "N →".
    pub ink: String,
    pub outcomes: Vec<RouteOutcome>,
}

/// One check down the trunk with the cards that peel off at it.
#[derive(uniffi::Record, Clone)]
pub struct RouteGate {
    /// "46 running. Is the agent mid-turn?"
    pub caption: String,
    /// "waiting on a bot 0 · other 0 (not drawn)".
    pub zeros: Option<String>,
    pub cards: Vec<RouteCard>,
}

/// The Routing page for one window.
#[derive(uniffi::Record, Clone)]
pub struct RoutePipeline {
    /// Stops counted (the retired rule's are not).
    pub total: u32,
    /// Top to bottom; only checks some stop still reaches.
    pub gates: Vec<RouteGate>,
    /// "All N stops ended as": no action, nudged, pinged you (then "other"
    /// when something is in it). Counts sum to `total`.
    pub outcomes: Vec<RouteOutcome>,
    /// "Not counted: N decisions from a retired rule …", when N > 0.
    pub retired_note: Option<String>,
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

fn outcome(o: Outcome, lane: Option<Lane>, count: usize, notes: Vec<String>) -> RouteOutcome {
    RouteOutcome {
        key: lane.map_or(String::new(), |lane| {
            Pick {
                lane,
                outcome: Some(o),
            }
            .key()
        }),
        outcome: o.key().to_string(),
        glyph: o.glyph().to_string(),
        word: if lane.is_some() {
            o.word()
        } else {
            o.total_word()
        }
        .to_string(),
        color: o.color().to_string(),
        count: count as u32,
        notes,
    }
}

pub(crate) fn pipeline(ds: &[Decision], now: u64, w: Window) -> RoutePipeline {
    let p = routing::pipeline(ds, now, w);
    RoutePipeline {
        total: p.total as u32,
        gates: p
            .gates
            .iter()
            .map(|g| RouteGate {
                caption: g.caption.clone(),
                zeros: g.zeros.clone(),
                cards: g
                    .cards
                    .iter()
                    .map(|c| RouteCard {
                        key: c.lane.key().to_string(),
                        title: c.lane.title().to_string(),
                        why: c.lane.why().to_string(),
                        count: c.count as u32,
                        color: c.color.to_string(),
                        ink: c.ink.clone(),
                        outcomes: c
                            .lines
                            .iter()
                            .map(|l| outcome(l.outcome, Some(c.lane), l.count, l.notes.clone()))
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
        outcomes: p
            .outcomes
            .iter()
            .map(|(o, n)| outcome(*o, None, *n, vec![]))
            .collect(),
        retired_note: p.retired_note(),
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
    /// The Routing page for the decisions within `window` of now, from
    /// every channel we read.
    pub fn route_pipeline(&self, window: RouteWindow) -> RoutePipeline {
        let e = self.engine.lock().unwrap();
        pipeline(&e.checklists.decisions(), now(), window.into())
    }

    /// The `n`th newest decision a card or outcome line holds (its `key`)
    /// within `window`, wrapping: "sample another" asks for n + 1. None
    /// for an unknown key or an empty one.
    pub fn route_sample(&self, window: RouteWindow, key: String, n: u32) -> Option<RouteDecision> {
        let pick = Pick::from_key(&key)?;
        let e = self.engine.lock().unwrap();
        let ds = e.checklists.decisions();
        let d = routing::sample_pick(&ds, now(), window.into(), pick, n as usize)?;
        Some(decision(&e, d))
    }
}
