//! The Control view (see `buzz_vim_core::control`): the #control channel's
//! daily ops notes and requests, as the terminal's Control tab shows them.
//! An overview (open requests with their vote buttons, a tile per topic,
//! the newest hooks note's gauges and seats), a topic's days, and a note.
//! Flat records with their labels already written, like the rest of the
//! door: Swift draws, the core decides.

use buzz_vim_core::control::{self, Board, Gauge, Health, Note, Tone, Unit};
use nostr::{Event, EventId};
use serde_json::Value;

use crate::{BuzzError, Client, Engine, Msg, now};

/// A topic's state: no note yet (dim), ok, warn, page.
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlHealth {
    NoNote,
    Ok,
    Warn,
    Page,
}

impl From<Health> for ControlHealth {
    fn from(h: Health) -> Self {
        match h {
            Health::NoNote => ControlHealth::NoNote,
            Health::Ok => ControlHealth::Ok,
            Health::Warn => ControlHealth::Warn,
            Health::Page => ControlHealth::Page,
        }
    }
}

/// How a delta reads: plain, better than its baseline, a flag, a page.
/// A flag is never green.
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeltaTone {
    Plain,
    Better,
    Flag,
    Page,
}

impl From<Tone> for DeltaTone {
    fn from(t: Tone) -> Self {
        match t {
            Tone::Plain => DeltaTone::Plain,
            Tone::Better => DeltaTone::Better,
            Tone::Flag => DeltaTone::Flag,
            Tone::Page => DeltaTone::Page,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlUnit {
    Tokens,
    Pct,
    Count,
}

impl From<Unit> for ControlUnit {
    fn from(u: Unit) -> Self {
        match u {
            Unit::Tokens => ControlUnit::Tokens,
            Unit::Pct => ControlUnit::Pct,
            Unit::Count => ControlUnit::Count,
        }
    }
}

impl From<ControlUnit> for Unit {
    fn from(u: ControlUnit) -> Self {
        match u {
            ControlUnit::Tokens => Unit::Tokens,
            ControlUnit::Pct => Unit::Pct,
            ControlUnit::Count => Unit::Count,
        }
    }
}

/// A value in its unit as the terminal writes it (767k, 42%, 3), for chart
/// axes.
#[uniffi::export]
pub fn control_value_label(value: f64, unit: ControlUnit) -> String {
    control::value_label(value, unit.into())
}

/// One declared action on a request: its button.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlAction {
    /// What `control_vote` takes back.
    pub index: u32,
    pub name: String,
    /// The reaction the button sends.
    pub emoji: String,
    /// Votes that count (from approvers when the note names any).
    pub votes: u32,
    /// Whether you are among them.
    pub mine: bool,
    /// Why you can't vote this, None when you can.
    pub blocked: Option<String>,
}

/// A request: a note that declares actions.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlRequest {
    pub id: String,
    pub topic: String,
    pub day: String,
    pub source: String,
    /// The summary's first line.
    pub line: String,
    /// "open", "closed", "superseded" (a newer note for its day and source
    /// replaced it; not votable), or "?" when it has none.
    pub status: String,
    pub outcome: Option<String>,
    pub actions: Vec<ControlAction>,
    /// The approvers by name; empty means anyone may vote.
    pub approvers: Vec<String>,
    pub at: u64,
    /// "2h", "3d".
    pub age: String,
}

/// One topic's tile on the overview.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlTile {
    pub topic: String,
    pub health: ControlHealth,
    /// ✓ ▲ ✗ ·, as in the terminal.
    pub glyph: String,
    /// The first line of the worst note on the newest day, or "no note".
    pub line: String,
    pub latest_day: Option<String>,
    pub sources: Vec<String>,
    /// What the sparkline draws ("cost / useful call", "held runs").
    pub metric: String,
    pub unit: ControlUnit,
    /// The sparkline's days, oldest first, ending today.
    pub days: Vec<String>,
    /// One value per day; None = no note that day.
    pub spark: Vec<Option<f64>>,
    /// The newest value, in its unit.
    pub latest: Option<String>,
    /// Requests still open in this topic.
    pub open: u32,
}

/// A hooks metric against its baseline (or a reset against its threshold).
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlGauge {
    pub title: String,
    pub value: f64,
    pub value_label: String,
    pub baseline: Option<f64>,
    pub baseline_label: Option<String>,
    /// "+25%", "+1pt"; None without a baseline.
    pub delta: Option<String>,
    pub tone: DeltaTone,
    pub unit: ControlUnit,
    /// The sample size, "n=12".
    pub aside: Option<String>,
    /// A reset against its threshold rather than a metric against its
    /// baseline: "vs" reads as a limit.
    pub threshold: bool,
    pub days: Vec<String>,
    pub spark: Vec<Option<f64>>,
}

/// A seat in the newest hooks note.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlSeat {
    pub name: String,
    pub health: ControlHealth,
}

/// The first screen.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlOverview {
    /// False until the first load answers.
    pub loaded: bool,
    pub error: Option<String>,
    /// Open requests, newest first, at most `STRIP_MAX`.
    pub requests: Vec<ControlRequest>,
    /// Open requests past those (they are on their topic tiles).
    pub more_requests: u32,
    pub tiles: Vec<ControlTile>,
    /// The newest hooks note's id, its gauges, seats and "hooks 10-21 taskbot".
    pub hooks: Option<String>,
    pub hooks_label: String,
    pub gauges: Vec<ControlGauge>,
    pub seats: Vec<ControlSeat>,
}

/// A row in a topic's list of days.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlDay {
    pub id: String,
    pub day: String,
    pub source: String,
    /// A request's status, else the details' status ("unreadable" when the
    /// details don't parse).
    pub status: Option<String>,
    pub health: ControlHealth,
    pub line: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlSeries {
    pub label: String,
    /// One per chart day; None = a gap.
    pub points: Vec<Option<f64>>,
    pub baseline: Option<f64>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlChart {
    pub title: String,
    pub unit: ControlUnit,
    pub series: Vec<ControlSeries>,
}

/// A hooks status for one day: "ok", "partial", "dead", None for a gap.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlStatusDay {
    pub day: String,
    pub status: Option<String>,
    pub health: ControlHealth,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlTable {
    pub title: String,
    pub head: Vec<String>,
    pub rows: Vec<ControlRow>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlRow {
    pub cells: Vec<String>,
    /// Highlighted: a dead seat (page), a flagged agent type (warn).
    pub hot: Option<ControlHealth>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlReaction {
    pub emoji: String,
    pub count: u32,
}

/// One note, everything the note page shows.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControlNote {
    pub id: String,
    pub topic: String,
    pub day: Option<String>,
    pub source: Option<String>,
    pub schema: Option<String>,
    pub author: String,
    pub at: u64,
    /// Markdown.
    pub summary: String,
    pub health: ControlHealth,
    pub reactions: Vec<ControlReaction>,
    pub replies: u32,
    pub request: Option<ControlRequest>,
    /// hooks/1: each day's status, the gauges, the trend charts over
    /// `chart_days`, and the seats / agent types / ramp tables.
    pub strip: Vec<ControlStatusDay>,
    pub gauges: Vec<ControlGauge>,
    pub chart_days: Vec<String>,
    pub charts: Vec<ControlChart>,
    pub tables: Vec<ControlTable>,
    /// Any other schema: the details as pretty JSON.
    pub details_json: Option<String>,
    pub details_error: Option<String>,
    /// Files on the note, by name.
    pub files: Vec<String>,
    /// `buzz://` link to the note in its channel.
    pub link: String,
}

fn glyph(h: Health) -> &'static str {
    match h {
        Health::Ok => "✓",
        Health::Warn => "▲",
        Health::Page => "✗",
        Health::NoNote => "·",
    }
}

/// "2h", "3d": how long before `now` `at` was.
fn age(at: u64, now: u64) -> String {
    match now.saturating_sub(at) {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The day the overview's sparklines end on, as the terminal's.
fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn seat_health(s: Option<&str>) -> Health {
    match s {
        Some("ok") => Health::Ok,
        Some("dead") => Health::Page,
        _ => Health::NoNote,
    }
}

fn status_health(s: Option<&str>) -> ControlHealth {
    match s {
        Some("ok") => ControlHealth::Ok,
        Some("dead") => ControlHealth::Page,
        None => ControlHealth::NoNote,
        Some(_) => ControlHealth::Warn,
    }
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.fract() == 0.0 && x.abs() < 1e4 => format!("{x:.0}"),
        Some(x) if x.abs() < 1e4 => format!("{x:.1}"),
        Some(x) => control::short_tokens(x),
        None => "-".into(),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

pub(crate) fn request(
    board: &Board,
    n: &Note,
    me: &str,
    name: &dyn Fn(&str) -> String,
    now: u64,
) -> ControlRequest {
    let st = board.status(n);
    ControlRequest {
        id: n.id.clone(),
        topic: n.topic.clone(),
        day: n.day.clone().unwrap_or_else(|| "?".into()),
        source: n.source.clone().unwrap_or_else(|| "?".into()),
        line: control::one_liner(&n.summary),
        status: st.status.unwrap_or_else(|| "?".into()),
        outcome: st.outcome,
        actions: board
            .votes(n)
            .into_iter()
            .enumerate()
            .map(|(i, t)| ControlAction {
                index: i as u32,
                name: t.action.name.clone(),
                emoji: t.action.emoji.clone(),
                votes: t.voters.len() as u32,
                mine: t.voters.iter().any(|v| v == me),
                blocked: board.can_vote(n, i, me).err(),
            })
            .collect(),
        approvers: n.approvers.iter().map(|p| name(p)).collect(),
        at: n.created_at,
        age: age(n.created_at, now),
    }
}

fn gauge(g: Gauge, days: &[String]) -> ControlGauge {
    let threshold = g.title.ends_with(" reset");
    ControlGauge {
        value_label: control::value_label(g.value, g.unit),
        baseline_label: g.baseline.map(|b| control::value_label(b, g.unit)),
        delta: g
            .baseline
            .map(|b| control::delta_label(g.value, b, g.unit))
            .filter(|d| !d.is_empty()),
        tone: g.tone.into(),
        unit: g.unit.into(),
        title: g.title,
        value: g.value,
        baseline: g.baseline,
        aside: g.aside,
        threshold,
        days: days.to_vec(),
        spark: g.spark,
    }
}

fn gauges(board: &Board, n: &Note) -> Vec<ControlGauge> {
    let days = control::days_ending(n.day.as_deref().unwrap_or(""), control::SPARK_DAYS);
    control::hooks_gauges(board, n)
        .into_iter()
        .map(|g| gauge(g, &days))
        .collect()
}

pub(crate) fn overview(
    board: &Board,
    me: &str,
    name: &dyn Fn(&str) -> String,
    today: &str,
    now: u64,
) -> ControlOverview {
    let open = board.open_requests();
    let days = control::days_ending(today, control::SPARK_DAYS);
    let hooks = board.newest_hooks();
    let seats = hooks
        .and_then(|n| n.details.as_ref().ok())
        .and_then(|d| d["seats"].as_array())
        .into_iter()
        .flatten()
        .map(|s| ControlSeat {
            name: text(&s["seat"]),
            health: seat_health(s["status"].as_str()).into(),
        })
        .collect();
    ControlOverview {
        loaded: board.loaded,
        error: board.error.clone(),
        requests: open
            .iter()
            .take(control::STRIP_MAX)
            .map(|n| request(board, n, me, name, now))
            .collect(),
        more_requests: open.len().saturating_sub(control::STRIP_MAX) as u32,
        tiles: board
            .tiles(today)
            .into_iter()
            .map(|t| ControlTile {
                glyph: glyph(t.health).into(),
                health: t.health.into(),
                latest: t
                    .spark
                    .iter()
                    .rev()
                    .find_map(|v| *v)
                    .map(|v| control::value_label(v, t.metric.unit)),
                metric: t.metric.label.into(),
                unit: t.metric.unit.into(),
                days: days.clone(),
                spark: t.spark,
                topic: t.topic,
                line: t.line,
                latest_day: t.latest_day,
                sources: t.sources,
                open: t.open as u32,
            })
            .collect(),
        hooks: hooks.map(|n| n.id.clone()),
        hooks_label: hooks
            .map(|n| {
                format!(
                    "hooks {} {}",
                    n.day.as_deref().and_then(|d| d.get(5..)).unwrap_or("?"),
                    n.source.as_deref().unwrap_or("?")
                )
            })
            .unwrap_or_default(),
        gauges: hooks.map(|n| gauges(board, n)).unwrap_or_default(),
        seats,
    }
}

/// What a row's status says: a request's status, else the details'.
fn note_status(board: &Board, n: &Note) -> Option<String> {
    if n.is_request() {
        return board.status(n).status;
    }
    match &n.details {
        Err(_) => Some("unreadable".into()),
        Ok(d) => d["status"].as_str().map(str::to_string),
    }
}

pub(crate) fn days(board: &Board, topic: &str) -> Vec<ControlDay> {
    board
        .days(topic)
        .into_iter()
        .map(|n| ControlDay {
            id: n.id.clone(),
            day: n.day.clone().unwrap_or_default(),
            source: n.source.clone().unwrap_or_default(),
            status: note_status(board, n),
            health: control::note_health(board, n).into(),
            line: control::one_liner(&n.summary),
        })
        .collect()
}

fn table(title: &str, head: &[&str], rows: Vec<ControlRow>) -> ControlTable {
    ControlTable {
        title: title.into(),
        head: head.iter().map(|s| s.to_string()).collect(),
        rows,
    }
}

/// The hooks/1 tables, as the terminal's note page has them.
fn hooks_tables(d: &Value) -> Vec<ControlTable> {
    let rows = |k: &str| d[k].as_array().cloned().unwrap_or_default();
    let seats = rows("seats")
        .iter()
        .map(|s| {
            let st = text(&s["status"]);
            ControlRow {
                hot: (st == "dead").then_some(ControlHealth::Page),
                cells: vec![
                    text(&s["seat"]),
                    num(&s["sessions"]),
                    num(&s["first_edits"]),
                    num(&s["ramp_lines"]),
                    format!("{} / {}", num(&s["mains"]), num(&s["standups"])),
                    num(&s["pre_hooks"]),
                    st,
                    text(&s["problem"]),
                ],
            }
        })
        .collect();
    let types = rows("agent_types")
        .iter()
        .map(|a| {
            let flagged = a["flagged"].as_bool() == Some(true);
            let delta = a["delta_pct"]
                .as_f64()
                .map(|x| format!("{x:+.0}%"))
                .unwrap_or_else(|| "-".into());
            ControlRow {
                hot: flagged.then_some(ControlHealth::Warn),
                cells: vec![
                    text(&a["agent_type"]),
                    text(&a["repo"]),
                    num(&a["n"]),
                    num(&a["recommended"]),
                    text(&a["threshold_label"]),
                    if flagged {
                        format!("{delta} ⚑")
                    } else {
                        delta
                    },
                ],
            }
        })
        .collect();
    let repos = rows("ramp_by_repo")
        .iter()
        .map(|r| ControlRow {
            hot: None,
            cells: vec![
                text(&r["repo"]),
                num(&r["running_n"]),
                num(&r["running_ramp_median_tokens"]),
                num(&r["running_calls_median"]),
                format!(
                    "{} / {}",
                    num(&r["ideating_n"]),
                    num(&r["ideating_ramp_median_tokens"])
                ),
                format!(
                    "{} / {}",
                    num(&r["quick_n"]),
                    num(&r["quick_ramp_median_tokens"])
                ),
            ],
        })
        .collect();
    vec![
        table(
            "seats",
            &[
                "seat",
                "sessions",
                "first edits",
                "ramp lines",
                "mains / stand-ups",
                "pre-hooks",
                "hooks",
                "problem",
            ],
            seats,
        ),
        table(
            "agent types (⚑ = reset more than 20% off the threshold)",
            &[
                "agent type",
                "repo",
                "n",
                "recommended",
                "threshold",
                "delta",
            ],
            types,
        ),
        table(
            "ramp by repo (context at first edit)",
            &[
                "repo",
                "running n",
                "ramp",
                "calls",
                "ideating n / ramp",
                "quick n / ramp",
            ],
            repos,
        ),
    ]
    .into_iter()
    .filter(|t| !t.rows.is_empty())
    .collect()
}

pub(crate) fn note(
    board: &Board,
    n: &Note,
    me: &str,
    name: &dyn Fn(&str) -> String,
    now: u64,
) -> ControlNote {
    let hooks = n.schema.as_deref() == Some("hooks/1") && n.details.is_ok();
    let source = n.source.clone().unwrap_or_default();
    let (chart_days, charts) = if hooks {
        control::hooks_charts(board, &source, n)
    } else {
        (vec![], vec![])
    };
    ControlNote {
        id: n.id.clone(),
        topic: n.topic.clone(),
        day: n.day.clone(),
        source: n.source.clone(),
        schema: n.schema.clone(),
        author: name(&n.author),
        at: n.created_at,
        summary: n.summary.clone(),
        health: control::note_health(board, n).into(),
        reactions: board
            .reaction_counts(n)
            .into_iter()
            .map(|(emoji, c)| ControlReaction {
                emoji,
                count: c as u32,
            })
            .collect(),
        replies: board.reply_count(n) as u32,
        request: n.is_request().then(|| request(board, n, me, name, now)),
        strip: if hooks {
            control::hooks_status_strip(board, &source, n)
                .into_iter()
                .map(|(day, status)| ControlStatusDay {
                    health: status_health(status.as_deref()),
                    day,
                    status,
                })
                .collect()
        } else {
            vec![]
        },
        gauges: if hooks { gauges(board, n) } else { vec![] },
        chart_days,
        charts: charts
            .into_iter()
            .map(|c| ControlChart {
                title: c.title,
                unit: c.unit.into(),
                series: c
                    .series
                    .into_iter()
                    .map(|s| ControlSeries {
                        label: s.label,
                        points: s.points,
                        baseline: s.baseline,
                    })
                    .collect(),
            })
            .collect(),
        tables: match (&n.details, hooks) {
            (Ok(d), true) => hooks_tables(d),
            _ => vec![],
        },
        details_json: match (&n.details, hooks) {
            (Ok(d), false) => serde_json::to_string_pretty(d).ok(),
            _ => None,
        },
        details_error: n.details.as_ref().err().cloned(),
        files: n
            .media
            .iter()
            .map(|m| {
                m.name
                    .clone()
                    .unwrap_or_else(|| m.url.rsplit('/').next().unwrap_or("file").to_string())
            })
            .collect(),
        link: format!("buzz://message?channel={}&id={}", board.channel, n.id),
    }
}

impl Engine {
    /// The control channel's history and its live tail.
    pub(crate) fn load_control(&mut self) {
        self.control_open = true;
        let channel = self.control.channel.clone();
        self.control_tail(&[]);
        let r = self.relay.clone();
        let at = now();
        self.spawn(async move {
            let result = control::fetch(&r, &channel, at).await;
            Msg::Control { channel, result }
        });
    }

    fn control_tail(&self, notes: &[String]) {
        self.sub(
            "control",
            control::tail_filters(&self.control.channel, notes, now()),
        );
    }

    /// The history answered: fold it into the board we have, so live
    /// events and votes that landed while it loaded are kept.
    pub(crate) fn on_control(
        &mut self,
        channel: String,
        result: anyhow::Result<(Vec<Event>, Vec<String>)>,
    ) {
        if channel != self.control.channel {
            return;
        }
        match result {
            Ok((evs, _)) => {
                let mut want = vec![];
                for ev in &evs {
                    if self.control.apply(ev) {
                        want.push(ev.pubkey.to_hex());
                    }
                }
                for id in self.control.note_ids() {
                    if let Some(n) = self.control.note(&id) {
                        want.extend(n.approvers.iter().cloned());
                    }
                }
                self.control.loaded = true;
                self.control.error = None;
                self.control_tail(&self.control.note_ids());
                self.want_profiles(want);
            }
            Err(e) => {
                self.control.loaded = true;
                self.control.error = Some(e.to_string());
            }
        }
    }

    /// A live event on the control tail.
    pub(crate) fn on_control_live(&mut self, ev: &Event) {
        if !self.control.apply(ev) {
            return;
        }
        if control::parse_note(ev).is_some() {
            // A new note: follow its reactions too.
            self.control_tail(&self.control.note_ids());
        }
        self.want_profiles([ev.pubkey.to_hex()]);
    }

    pub(crate) fn vote(&mut self, note: &str, action: u32) -> Result<(), BuzzError> {
        let me = self.me();
        let n = self
            .control
            .note(note)
            .ok_or_else(|| BuzzError::Failed("that note isn't loaded".into()))?;
        let i = action as usize;
        self.control
            .can_vote(n, i, &me)
            .map_err(BuzzError::Failed)?;
        let a = n.actions[i].clone();
        let id = EventId::from_hex(note).map_err(|e| BuzzError::Failed(e.to_string()))?;
        let ev = buzz_sdk::build_reaction(id, &a.emoji)
            .map_err(anyhow::Error::from)
            .and_then(|b| self.relay.sign(b))?;
        // Counted at once, so a second tap is refused (one vote per
        // request) and the buttons dim; taken back if the relay refuses.
        self.control.apply(&ev);
        let r = self.relay.clone();
        let label = a.name;
        self.spawn(async move {
            let result = r.publish(&ev).await;
            Msg::Voted {
                event: Box::new(ev),
                label,
                result,
            }
        });
        Ok(())
    }

    pub(crate) fn on_voted(&mut self, ev: &Event, label: String, result: anyhow::Result<()>) {
        match result {
            Ok(()) => {
                self.control.apply(ev);
                self.notices.push(format!("voted {label} {}", ev.content));
            }
            Err(e) => {
                self.control.forget_reaction(ev);
                self.notices.push(format!("vote not sent: {e}"))
            }
        }
    }
}

#[uniffi::export]
impl Client {
    /// The Control page is on screen: read the channel (again) and follow
    /// it live. The first call loads; later ones reload.
    pub fn open_control(&self) {
        self.engine.lock().unwrap().load_control();
    }

    pub fn control_overview(&self) -> ControlOverview {
        let e = self.engine.lock().unwrap();
        let me = e.me();
        overview(&e.control, &me, &|p| e.store.name_of(p), &today(), now())
    }

    /// A topic's notes, newest day first, then by source.
    pub fn control_days(&self, topic: String) -> Vec<ControlDay> {
        days(&self.engine.lock().unwrap().control, &topic)
    }

    /// A note, or None when it isn't (or is no longer) loaded.
    pub fn control_note(&self, id: String) -> Option<ControlNote> {
        let e = self.engine.lock().unwrap();
        let me = e.me();
        let n = e.control.note(&id)?;
        Some(note(&e.control, n, &me, &|p| e.store.name_of(p), now()))
    }

    /// Vote for a request's action: its declared emoji as a reaction. Errs
    /// at once when you can't (closed, not an approver, voted already);
    /// how the send went comes as a notice.
    pub fn control_vote(&self, note: String, action: u32) -> Result<(), BuzzError> {
        self.engine.lock().unwrap().vote(&note, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_vim_core::control::{
        DETAILS_FENCE, TAG_ACTION, TAG_APPROVER, TAG_DAY, TAG_SCHEMA, TAG_SOURCE, TAG_STATUS,
        TAG_TOPIC,
    };
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use serde_json::json;

    const CH: &str = ctl_channel();
    const fn ctl_channel() -> &'static str {
        control::DEFAULT_CHANNEL
    }
    const NOW: u64 = 1_792_540_800; // 2026-10-21T00:00:00Z
    const TODAY: &str = "2026-10-21";

    fn ev(keys: &Keys, kind: u16, content: &str, tags: &[Vec<&str>], at: u64) -> Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.iter().map(|t| Tag::parse(t.iter().copied()).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn note_ev(
        keys: &Keys,
        topic: &str,
        schema: &str,
        day: &str,
        source: &str,
        at: u64,
        d: &Value,
        extra: &[Vec<&str>],
    ) -> Event {
        let content = format!("**{topic}** for {day}.\n\n{DETAILS_FENCE}\n{d:#}\n```");
        let mut tags = vec![
            vec!["h", CH],
            vec![TAG_TOPIC, topic],
            vec![TAG_SCHEMA, schema],
            vec![TAG_DAY, day],
            vec![TAG_SOURCE, source],
        ];
        tags.extend(extra.iter().cloned());
        ev(keys, 9, &content, &tags, at)
    }

    fn react(keys: &Keys, target: &Event, emoji: &str, at: u64) -> Event {
        let id = target.id.to_hex();
        ev(keys, 7, emoji, &[vec!["e", id.as_str()]], at)
    }

    fn hooks(peak: u64) -> Value {
        json!({
            "status": "ok",
            "peak_context_subagent_tokens": peak,
            "peak_context_main_tokens": 604000,
            "share_spend_above_300k_pct": 42,
            "ramp_after_handoff_median_tokens": 108000,
            "ramp_after_handoff_n": 12,
            "cost_per_useful_call_tokens": 55100,
            "baseline": {"peak_context_subagent_tokens": 612000, "peak_context_main_tokens": 432000,
                "share_spend_above_300k_pct": 41, "ramp_after_handoff_median_tokens": 79000,
                "cost_per_useful_call_tokens": 75200},
            "agent_types": [
                {"agent_type": "builder", "repo": null, "n": 9, "recommended": 161000,
                 "threshold": 250000, "threshold_label": "250k", "delta_pct": -36, "flagged": true},
                {"agent_type": "builder", "repo": "aei", "n": 4, "recommended": 183000,
                 "threshold": 250000, "flagged": true}
            ],
            "seats": [
                {"seat": "chip", "status": "ok"},
                {"seat": "taskbot", "status": "dead", "problem": "no hooks"}
            ],
            "ramp_by_repo": [{"repo": "aei", "running_n": 3}]
        })
    }

    /// A board like the live one: hooks on two days, a converge request
    /// with one approver, a fleet-audit note, and a topic nobody knows.
    fn board(approver: &Keys, other: &Keys) -> (Board, Event) {
        let bot = Keys::generate();
        let mut b = Board::new(CH);
        b.apply(&note_ev(
            &bot,
            "hooks",
            "hooks/1",
            "2026-10-20",
            "taskbot",
            NOW - 90_000,
            &hooks(700_000),
            &[],
        ));
        b.apply(&note_ev(
            &bot,
            "hooks",
            "hooks/1",
            TODAY,
            "taskbot",
            NOW - 3_600,
            &hooks(767_000),
            &[],
        ));
        let ap = approver.public_key().to_hex();
        let req = note_ev(
            &bot,
            "converge",
            "converge/1",
            TODAY,
            "relay-box",
            NOW - 7_200,
            &json!({"status": "held", "held_n": 1}),
            &[
                vec![TAG_ACTION, "approve", "👍"],
                vec![TAG_ACTION, "hold", "⏳"],
                vec![TAG_APPROVER, ap.as_str()],
                vec![TAG_STATUS, "open"],
            ],
        );
        b.apply(&req);
        // The approver's vote counts; someone else's 👍 doesn't.
        b.apply(&react(approver, &req, "👍", NOW - 600));
        b.apply(&react(other, &req, "👍", NOW - 500));
        b.apply(&note_ev(
            &bot,
            "fleet-audit",
            "fleet-audit/1",
            TODAY,
            "taskbot",
            NOW - 100,
            &json!({"status": "ok", "problems_n": 4}),
            &[],
        ));
        b.apply(&note_ev(
            &bot,
            "routing",
            "routing/1",
            TODAY,
            "taskbot",
            NOW - 50,
            &json!({"status": "ok"}),
            &[],
        ));
        b.loaded = true;
        (b, req)
    }

    /// title, value, baseline, delta, tone, threshold.
    type GaugeRow<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        DeltaTone,
        bool,
    );

    fn name(p: &str) -> String {
        format!("n:{}", &p[..4])
    }

    #[test]
    fn overview_has_requests_tiles_gauges_and_seats() {
        let (approver, other) = (Keys::generate(), Keys::generate());
        let (b, req) = board(&approver, &other);
        let me = approver.public_key().to_hex();
        let o = overview(&b, &me, &name, TODAY, NOW);
        assert!(o.loaded);

        // The strip: the converge request, approve 1 (mine), hold 0.
        assert_eq!(o.requests.len(), 1);
        let r = &o.requests[0];
        assert_eq!(
            (r.id.as_str(), r.topic.as_str(), r.status.as_str()),
            (req.id.to_hex().as_str(), "converge", "open")
        );
        assert_eq!(r.line, "converge for 2026-10-21.");
        assert_eq!(r.age, "2h");
        assert_eq!(r.approvers, vec![name(&me)]);
        let a: Vec<(&str, &str, u32, bool)> = r
            .actions
            .iter()
            .map(|a| (a.name.as_str(), a.emoji.as_str(), a.votes, a.mine))
            .collect();
        assert_eq!(
            a,
            vec![("approve", "👍", 1, true), ("hold", "⏳", 0, false)]
        );
        assert_eq!(
            r.actions[0].blocked.as_deref(),
            Some("you already voted approve")
        );
        // One vote per request: the other button is off too.
        assert_eq!(
            r.actions[1].blocked.as_deref(),
            Some("you already voted approve")
        );
        assert_eq!(o.more_requests, 0);

        // Someone who isn't an approver sees why the buttons won't vote.
        let theirs = overview(&b, &other.public_key().to_hex(), &name, TODAY, NOW);
        assert_eq!(
            theirs.requests[0].actions[1].blocked.as_deref(),
            Some("only the note's approvers can vote on it")
        );

        // Tiles: the four known topics in order, then the others.
        let t: Vec<(&str, ControlHealth, &str)> = o
            .tiles
            .iter()
            .map(|t| (t.topic.as_str(), t.health, t.glyph.as_str()))
            .collect();
        assert_eq!(
            t,
            vec![
                ("hooks", ControlHealth::Ok, "✓"),
                ("upgrade", ControlHealth::NoNote, "·"),
                ("converge", ControlHealth::Warn, "▲"),
                ("fleet-audit", ControlHealth::Ok, "✓"),
                ("routing", ControlHealth::Ok, "✓"),
            ]
        );
        let hooks_tile = &o.tiles[0];
        assert_eq!(hooks_tile.days.len(), control::SPARK_DAYS);
        assert_eq!(hooks_tile.days.last().map(String::as_str), Some(TODAY));
        assert_eq!(hooks_tile.spark.len(), control::SPARK_DAYS);
        assert_eq!(hooks_tile.spark[control::SPARK_DAYS - 2], Some(55_100.0));
        assert_eq!(hooks_tile.spark[0], None);
        assert_eq!(hooks_tile.latest.as_deref(), Some("55k"));
        assert_eq!(o.tiles[2].open, 1);
        assert_eq!(o.tiles[1].line, "no note");
        assert_eq!(o.tiles[1].latest, None);

        // Gauges from the newest hooks note, with labels and tones.
        assert_eq!(o.hooks_label, "hooks 10-21 taskbot");
        let g: Vec<GaugeRow> = o
            .gauges
            .iter()
            .map(|g| {
                (
                    g.title.as_str(),
                    g.value_label.as_str(),
                    g.baseline_label.as_deref(),
                    g.delta.as_deref(),
                    g.tone,
                    g.threshold,
                )
            })
            .collect();
        assert_eq!(
            g,
            vec![
                (
                    "peak subagent",
                    "767k",
                    Some("612k"),
                    Some("+25%"),
                    DeltaTone::Flag,
                    false
                ),
                (
                    "peak main",
                    "604k",
                    Some("432k"),
                    Some("+40%"),
                    DeltaTone::Flag,
                    false
                ),
                (
                    "spend >300k",
                    "42%",
                    Some("41%"),
                    Some("+1pt"),
                    DeltaTone::Plain,
                    false
                ),
                (
                    "ramp after handoff",
                    "108k",
                    Some("79k"),
                    Some("+37%"),
                    DeltaTone::Flag,
                    false
                ),
                (
                    "cost / useful call",
                    "55k",
                    Some("75k"),
                    Some("-27%"),
                    DeltaTone::Better,
                    false
                ),
                // Flagged by the report: amber although it is lower.
                (
                    "builder reset",
                    "161k",
                    Some("250k"),
                    Some("-36%"),
                    DeltaTone::Flag,
                    true
                ),
            ]
        );
        assert_eq!(o.gauges[3].aside.as_deref(), Some("n=12"));
        assert_eq!(o.gauges[0].spark[control::SPARK_DAYS - 2], Some(700_000.0));
        let seats: Vec<(&str, ControlHealth)> = o
            .seats
            .iter()
            .map(|s| (s.name.as_str(), s.health))
            .collect();
        assert_eq!(
            seats,
            vec![
                ("chip", ControlHealth::Ok),
                ("taskbot", ControlHealth::Page)
            ]
        );
    }

    #[test]
    fn days_and_notes_drill_down() {
        let (approver, other) = (Keys::generate(), Keys::generate());
        let (b, req) = board(&approver, &other);
        let me = approver.public_key().to_hex();

        let d = days(&b, "hooks");
        let rows: Vec<(&str, &str, Option<&str>)> = d
            .iter()
            .map(|r| (r.day.as_str(), r.source.as_str(), r.status.as_deref()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (TODAY, "taskbot", Some("ok")),
                ("2026-10-20", "taskbot", Some("ok"))
            ]
        );
        assert_eq!(days(&b, "converge")[0].status.as_deref(), Some("open"));
        assert!(days(&b, "upgrade").is_empty());

        // A hooks note: strip, gauges, charts over its days, tables.
        let n = note(&b, b.note(&d[0].id).unwrap(), &me, &name, NOW);
        assert_eq!(n.request, None);
        assert_eq!(n.strip.len(), 2);
        assert_eq!(n.strip[1].health, ControlHealth::Ok);
        assert_eq!(n.gauges.len(), 6);
        assert_eq!(
            n.chart_days,
            vec!["2026-10-20".to_string(), TODAY.to_string()]
        );
        let titles: Vec<&str> = n.charts.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles[0], "Peak context");
        assert_eq!(titles.last().copied(), Some("Reset vs threshold"));
        assert_eq!(
            n.charts[0].series[0].points,
            vec![Some(700_000.0), Some(767_000.0)]
        );
        assert_eq!(n.charts[0].series[0].baseline, Some(612_000.0));
        let tables: Vec<&str> = n.tables.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(tables.len(), 3);
        assert_eq!(n.tables[0].rows[1].hot, Some(ControlHealth::Page));
        assert_eq!(n.tables[1].rows[0].cells[5], "-36% ⚑");
        assert_eq!(n.details_json, None);

        // A request note: its buttons, the raw reactions, JSON details.
        let r = note(&b, b.note(&req.id.to_hex()).unwrap(), &me, &name, NOW);
        assert_eq!(r.request.as_ref().map(|q| q.actions.len()), Some(2));
        assert_eq!(
            r.reactions,
            vec![ControlReaction {
                emoji: "👍".into(),
                count: 2
            }]
        );
        assert!(r.details_json.as_deref().unwrap().contains("\"held_n\": 1"));
        assert!(r.charts.is_empty() && r.gauges.is_empty() && r.tables.is_empty());
        assert_eq!(
            r.link,
            format!("buzz://message?channel={CH}&id={}", req.id.to_hex())
        );
    }

    #[test]
    fn votes_go_live_and_only_where_allowed() {
        let mut e = crate::tests::engine();
        let me = e.me();
        let bot = Keys::generate();
        let mine = note_ev(
            &bot,
            "upgrade",
            "upgrade/1",
            TODAY,
            "relay",
            NOW,
            &json!({"status": "offered"}),
            &[
                vec![TAG_ACTION, "stage", "🚀"],
                vec![TAG_APPROVER, me.as_str()],
                vec![TAG_STATUS, "open"],
            ],
        );
        let theirs = note_ev(
            &bot,
            "upgrade",
            "upgrade/1",
            TODAY,
            "other",
            NOW,
            &json!({"status": "offered"}),
            &[
                vec![TAG_ACTION, "stage", "🚀"],
                vec![TAG_APPROVER, &"ab".repeat(32)],
                vec![TAG_STATUS, "open"],
            ],
        );
        // Live notes land on the board.
        e.on_control_live(&mine);
        e.on_control_live(&theirs);
        assert_eq!(e.control.open_requests().len(), 2);

        let id = mine.id.to_hex();
        assert!(e.vote(&id, 0).is_ok());
        assert!(
            matches!(e.vote(&id, 3), Err(BuzzError::Failed(m)) if m == "this request has 1 actions")
        );
        assert!(matches!(e.vote(&theirs.id.to_hex(), 0),
            Err(BuzzError::Failed(m)) if m == "only the note's approvers can vote on it"));
        assert!(
            matches!(e.vote(&"cd".repeat(32), 0), Err(BuzzError::Failed(m)) if m == "that note isn't loaded")
        );

        // The relay took it: the vote counts and a second one is refused.
        let v = e
            .relay
            .sign(buzz_sdk::build_reaction(mine.id, "🚀").unwrap())
            .unwrap();
        e.on_voted(&v, "stage".into(), Ok(()));
        let o = overview(&e.control, &me, &name, TODAY, NOW);
        let r = o.requests.iter().find(|r| r.id == id).unwrap();
        assert_eq!((r.actions[0].votes, r.actions[0].mine), (1, true));
        assert!(e.vote(&id, 0).is_err());
        assert_eq!(e.notices.last().map(String::as_str), Some("voted stage 🚀"));
        // A refused one says so and counts nothing.
        e.on_voted(
            &v,
            "stage".into(),
            Err(anyhow::anyhow!("relay rejected: no")),
        );
        assert_eq!(
            e.notices.last().map(String::as_str),
            Some("vote not sent: relay rejected: no")
        );
    }

    #[test]
    fn a_second_tap_sends_nothing_and_a_refusal_frees_the_buttons() {
        let mut e = crate::tests::engine();
        let me = e.me();
        let bot = Keys::generate();
        let req = note_ev(
            &bot,
            "upgrade",
            "upgrade/1",
            TODAY,
            "build-box",
            NOW,
            &json!({"status": "offered"}),
            &[
                vec![TAG_ACTION, "approve", "👍"],
                vec![TAG_ACTION, "decline", "👎"],
                vec![TAG_APPROVER, me.as_str()],
                vec![TAG_STATUS, "open"],
            ],
        );
        e.on_control_live(&req);
        let id = req.id.to_hex();
        assert!(e.vote(&id, 0).is_ok());
        // Before the relay answers: a double tap, or the decline beside it,
        // is refused, and both buttons say so.
        assert!(
            matches!(e.vote(&id, 0), Err(BuzzError::Failed(m)) if m == "you already voted approve")
        );
        assert!(
            matches!(e.vote(&id, 1), Err(BuzzError::Failed(m)) if m == "you already voted approve")
        );
        let o = overview(&e.control, &me, &name, TODAY, NOW);
        let r = o.requests.iter().find(|r| r.id == id).unwrap();
        assert!(r.actions.iter().all(|a| a.blocked.is_some()));
        assert_eq!((r.actions[0].votes, r.actions[0].mine), (1, true));
        // The relay refused it: the count and the buttons come back.
        let v = e
            .relay
            .sign(buzz_sdk::build_reaction(req.id, "👍").unwrap())
            .unwrap();
        e.on_voted(
            &v,
            "approve".into(),
            Err(anyhow::anyhow!("relay rejected: no")),
        );
        let o = overview(&e.control, &me, &name, TODAY, NOW);
        let r = o.requests.iter().find(|r| r.id == id).unwrap();
        assert_eq!(r.actions[0].votes, 0);
        assert!(r.actions.iter().all(|a| a.blocked.is_none()));
    }

    #[test]
    fn value_labels_match_the_terminal() {
        assert_eq!(control_value_label(767_000.0, ControlUnit::Tokens), "767k");
        assert_eq!(
            control_value_label(1_250_000.0, ControlUnit::Tokens),
            "1.2M"
        );
        assert_eq!(control_value_label(42.0, ControlUnit::Pct), "42%");
        assert_eq!(control_value_label(3.0, ControlUnit::Count), "3");
        assert_eq!(age(NOW - 59, NOW), "0m");
        assert_eq!(age(NOW - 3 * 86_400, NOW), "3d");
    }
}
