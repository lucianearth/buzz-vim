//! Control notes: the daily ops reports and requests in #control, as
//! `buzz control publish` writes them (aei `wiki/architecture/control-notes.md`).
//!
//! A note is a top-level kind 9 in the control channel with `control-topic`,
//! `control-day`, `control-source` and `control-schema` tags, a markdown
//! summary, and its details as JSON in the last ```` ```json control-details ````
//! fence. A note that declares `control-action name emoji` tags is a request:
//! a kind-7 reaction with that emoji is a vote (only from `control-approver`
//! pubkeys when it names any), and the newest thread reply carrying
//! `control-status`, signed by the author or an approver, sets its status.
//!
//! The relay can't filter on the multi-letter tags, so the client reads
//! `kinds:[9]` + `#h` + `since` and sorts it out here. One note per topic,
//! day and source counts: the newest. A day without a note is a gap.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nostr::Event;
use serde_json::{Value, json};

use crate::model::{Media, Message};

/// The #control channel (created 2026-10-21); `control_channel` in config.
pub const DEFAULT_CHANNEL: &str = "ee7289a1-e875-43fd-9fdb-42aa5451be05";
pub const TAG_TOPIC: &str = "control-topic";
pub const TAG_DAY: &str = "control-day";
pub const TAG_SOURCE: &str = "control-source";
pub const TAG_SCHEMA: &str = "control-schema";
pub const TAG_ACTION: &str = "control-action";
pub const TAG_APPROVER: &str = "control-approver";
pub const TAG_STATUS: &str = "control-status";
pub const DETAILS_FENCE: &str = "```json control-details";
/// How far back the view reads.
pub const DAYS_BACK: u64 = 60;

fn tag_rows<'a>(ev: &'a Event, name: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
    ev.tags
        .iter()
        .map(|t| t.as_slice())
        .filter(move |s| s.first().is_some_and(|n| n == name))
}

fn first(ev: &Event, name: &str) -> Option<String> {
    tag_rows(ev, name).find_map(|s| s.get(1).cloned())
}

/// A declared action: its name and the reaction that votes for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    pub emoji: String,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: String,
    pub author: String,
    pub created_at: u64,
    pub topic: String,
    pub day: Option<String>,
    pub source: Option<String>,
    pub schema: Option<String>,
    /// The note's own `control-status` tag (`open` on a request).
    pub status: Option<String>,
    pub actions: Vec<Action>,
    /// Lowercase hex.
    pub approvers: Vec<String>,
    pub media: Vec<Media>,
    pub summary: String,
    /// The parsed details, or why they couldn't be read.
    pub details: Result<Value, String>,
}

impl Note {
    pub fn is_request(&self) -> bool {
        !self.actions.is_empty()
    }
}

/// Split content into (summary, details): the details are the last
/// `control-details` fence that opens a line and ends it.
pub fn split_content(content: &str) -> (String, Result<Value, String>) {
    let opening = content
        .match_indices(DETAILS_FENCE)
        .map(|(i, _)| i)
        .filter(|&i| {
            let line_start = i == 0 || content[..i].ends_with('\n');
            let after = &content[i + DETAILS_FENCE.len()..];
            line_start && (after.starts_with('\n') || after.starts_with("\r\n"))
        })
        .last();
    let Some(start) = opening else {
        return (
            content.trim().to_string(),
            Err("no control-details block".into()),
        );
    };
    let summary = content[..start].trim().to_string();
    let rest = content[start + DETAILS_FENCE.len()..]
        .trim_start_matches('\r')
        .trim_start_matches('\n');
    let end = if rest.starts_with("```") {
        Some(0)
    } else {
        rest.find("\n```").map(|i| i + 1)
    };
    let Some(end) = end else {
        return (summary, Err("control-details block is not closed".into()));
    };
    let details =
        serde_json::from_str(&rest[..end]).map_err(|e| format!("details are not JSON: {e}"));
    (summary, details)
}

/// A reaction as a vote, exactly as aei's `cmd_poll` reads it (its jq
/// `norm`): U+FE0F and the skin tones U+1F3FB..U+1F3FF dropped, nothing
/// else. No trimming and no NIP-25 `""` -> `+`, so the tab never counts a
/// reaction the box ignores.
pub fn normalize_reaction(content: &str) -> String {
    content
        .chars()
        .filter(|c| *c != '\u{FE0F}' && !('\u{1F3FB}'..='\u{1F3FF}').contains(c))
        .collect()
}

/// The reactions `cmd_poll` also takes for an action, besides its own
/// emoji: ✅ and +1 approve, ❌ declines.
pub fn aliases(action: &str) -> &'static [&'static str] {
    match action {
        "approve" => &["\u{2705}", "+1"],
        "decline" => &["\u{274C}"],
        _ => &[],
    }
}

/// The thread root a kind 9 replies into, if it is a reply.
fn reply_root(ev: &Event) -> Option<String> {
    Message::from_event(ev).root.or_else(|| first(ev, "e"))
}

/// A control note, or None when the event isn't one.
pub fn parse_note(ev: &Event) -> Option<Note> {
    if ev.kind.as_u16() != 9 || reply_root(ev).is_some() {
        return None;
    }
    let topic = first(ev, TAG_TOPIC)?;
    let (summary, details) = split_content(&ev.content);
    Some(Note {
        id: ev.id.to_hex(),
        author: ev.pubkey.to_hex(),
        created_at: ev.created_at.as_secs(),
        topic,
        day: first(ev, TAG_DAY),
        source: first(ev, TAG_SOURCE),
        schema: first(ev, TAG_SCHEMA),
        status: first(ev, TAG_STATUS),
        actions: tag_rows(ev, TAG_ACTION)
            .filter_map(|s| match s {
                [_, name, emoji, ..] => Some(Action {
                    name: name.clone(),
                    emoji: emoji.clone(),
                }),
                _ => None,
            })
            .collect(),
        approvers: tag_rows(ev, TAG_APPROVER)
            .filter_map(|s| s.get(1).map(|p| p.to_ascii_lowercase()))
            .collect(),
        media: Message::from_event(ev).media,
        summary,
        details,
    })
}

#[derive(Clone, Debug)]
struct Reaction {
    by: String,
    emoji: String,
}

#[derive(Clone, Debug)]
struct Reply {
    id: String,
    by: String,
    at: u64,
    /// (status, outcome) from its `control-status` tag.
    status: Option<(String, Option<String>)>,
}

/// A request's status now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub status: Option<String>,
    pub outcome: Option<String>,
    /// The reply that set it, or the newer note that superseded it; None =
    /// the note's own tag.
    pub by_event: Option<String>,
}

/// One declared action and who voted for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub action: Action,
    pub voters: Vec<String>,
}

/// A topic on the first screen: the newest day it has a note for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicRow {
    pub topic: String,
    pub latest_day: String,
    pub days: usize,
    pub sources: Vec<String>,
    /// Requests still open in this topic.
    pub open: usize,
}

/// Everything read from the control channel.
#[derive(Default, Debug)]
pub struct Board {
    pub channel: String,
    pub loaded: bool,
    pub error: Option<String>,
    notes: HashMap<String, Note>,
    /// note id -> reaction id -> reaction
    reactions: HashMap<String, BTreeMap<String, Reaction>>,
    /// root id -> replies
    replies: HashMap<String, Vec<Reply>>,
}

impl Board {
    pub fn new(channel: &str) -> Self {
        Board {
            channel: channel.to_string(),
            ..Default::default()
        }
    }

    /// Fold one event in. True when it changed something.
    pub fn apply(&mut self, ev: &Event) -> bool {
        match ev.kind.as_u16() {
            9 => {
                if crate::model::tag(ev, "h") != Some(self.channel.as_str()) {
                    return false;
                }
                if let Some(root) = reply_root(ev) {
                    let list = self.replies.entry(root).or_default();
                    let id = ev.id.to_hex();
                    if list.iter().any(|r| r.id == id) {
                        return false;
                    }
                    list.push(Reply {
                        id,
                        by: ev.pubkey.to_hex(),
                        at: ev.created_at.as_secs(),
                        status: tag_rows(ev, TAG_STATUS)
                            .next()
                            .and_then(|s| Some((s.get(1)?.clone(), s.get(2).cloned()))),
                    });
                    return true;
                }
                match parse_note(ev) {
                    Some(n) => self.notes.insert(n.id.clone(), n).is_none(),
                    None => false,
                }
            }
            7 => {
                // NIP-25: the target is the last `e` tag.
                let Some(target) = tag_rows(ev, "e").filter_map(|s| s.get(1)).last() else {
                    return false;
                };
                self.reactions
                    .entry(target.clone())
                    .or_default()
                    .insert(
                        ev.id.to_hex(),
                        Reaction {
                            by: ev.pubkey.to_hex(),
                            emoji: normalize_reaction(&ev.content),
                        },
                    )
                    .is_none()
            }
            _ => false,
        }
    }

    pub fn note(&self, id: &str) -> Option<&Note> {
        self.notes.get(id)
    }

    pub fn note_ids(&self) -> Vec<String> {
        self.notes.keys().cloned().collect()
    }

    /// Notes that count: per (topic, day, source) the newest by created_at
    /// (then id). Notes without a day or source don't count.
    pub fn latest(&self) -> Vec<&Note> {
        let mut best: HashMap<(&str, &str, &str), &Note> = HashMap::new();
        for n in self.notes.values() {
            let (Some(d), Some(s)) = (n.day.as_deref(), n.source.as_deref()) else {
                continue;
            };
            let k = (n.topic.as_str(), d, s);
            match best.get(&k) {
                Some(b) if (b.created_at, &b.id) >= (n.created_at, &n.id) => {}
                _ => {
                    best.insert(k, n);
                }
            }
        }
        best.into_values().collect()
    }

    /// The first screen: one row per topic, by name.
    pub fn topics(&self) -> Vec<TopicRow> {
        let mut by: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>, usize)> = BTreeMap::new();
        for n in self.latest() {
            let e = by.entry(n.topic.clone()).or_default();
            e.0.insert(n.day.clone().unwrap_or_default());
            e.1.insert(n.source.clone().unwrap_or_default());
            if n.is_request() && self.status(n).status.as_deref() == Some("open") {
                e.2 += 1;
            }
        }
        by.into_iter()
            .map(|(topic, (days, sources, open))| TopicRow {
                topic,
                latest_day: days.iter().next_back().cloned().unwrap_or_default(),
                days: days.len(),
                sources: sources.into_iter().collect(),
                open,
            })
            .collect()
    }

    /// A topic's notes, newest day first, then by source.
    pub fn days(&self, topic: &str) -> Vec<&Note> {
        let mut v: Vec<&Note> = self
            .latest()
            .into_iter()
            .filter(|n| n.topic == topic)
            .collect();
        v.sort_by(|a, b| b.day.cmp(&a.day).then_with(|| a.source.cmp(&b.source)));
        v
    }

    /// Who voted for each declared action: kind-7s whose emoji matches (or
    /// one of `cmd_poll`'s aliases for it), from approvers only when the
    /// note names any. One vote per pubkey.
    pub fn votes(&self, note: &Note) -> Vec<Tally> {
        let rs = self.reactions.get(&note.id);
        note.actions
            .iter()
            .map(|a| {
                let want = normalize_reaction(&a.emoji);
                let also = aliases(&a.name);
                let voters: BTreeSet<String> = rs
                    .into_iter()
                    .flat_map(|m| m.values())
                    .filter(|r| r.emoji == want || also.contains(&r.emoji.as_str()))
                    .filter(|r| note.approvers.is_empty() || note.approvers.contains(&r.by))
                    .map(|r| r.by.clone())
                    .collect();
                Tally {
                    action: a.clone(),
                    voters: voters.into_iter().collect(),
                }
            })
            .collect()
    }

    /// Reactions to the note by emoji, every one (not only votes).
    pub fn reaction_counts(&self, note: &Note) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for r in self
            .reactions
            .get(&note.id)
            .into_iter()
            .flat_map(|m| m.values())
        {
            *out.entry(r.emoji.clone()).or_default() += 1;
        }
        out
    }

    /// Take back a reaction folded in before the relay answered (a vote
    /// sent at once), when the relay refused it.
    pub fn forget_reaction(&mut self, ev: &Event) -> bool {
        let Some(target) = tag_rows(ev, "e").filter_map(|s| s.get(1)).last() else {
            return false;
        };
        self.reactions
            .get_mut(target)
            .is_some_and(|m| m.remove(&ev.id.to_hex()).is_some())
    }

    pub fn reply_count(&self, note: &Note) -> usize {
        self.replies.get(&note.id).map_or(0, Vec::len)
    }

    /// The note that replaced a request, if any, from the same author and
    /// for the same topic and source: a newer note for its day (aei
    /// republishes the day's note on each event), or a newer request on
    /// any day (aei `cmd_poll` reads only its newest `alert_event_id`, and
    /// the 04:00 stage replaces yesterday's offer with today's). Another
    /// author's note never supersedes one.
    pub fn superseded_by(&self, note: &Note) -> Option<&Note> {
        let (Some(d), Some(s)) = (note.day.as_deref(), note.source.as_deref()) else {
            return None;
        };
        self.notes
            .values()
            .filter(|n| {
                n.author == note.author
                    && n.topic == note.topic
                    && n.source.as_deref() == Some(s)
                    && (n.day.as_deref() == Some(d) || (n.is_request() && n.day.is_some()))
                    && (n.created_at, &n.id) > (note.created_at, &note.id)
            })
            .max_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)))
    }

    /// A request's status: a `closed` reply by the author or an approver,
    /// else `superseded` (with the newer note as `by_event`) once
    /// `superseded_by` finds one, else the newest `control-status` reply,
    /// else the note's own tag.
    pub fn status(&self, note: &Note) -> Status {
        let newest = self
            .replies
            .get(&note.id)
            .into_iter()
            .flatten()
            .filter(|r| r.by == note.author || note.approvers.contains(&r.by))
            .filter(|r| r.status.is_some())
            .max_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        let closed = newest.is_some_and(|r| {
            r.status
                .as_ref()
                .is_some_and(|(s, _)| s.as_str() == "closed")
        });
        if note.is_request() && !closed {
            if let Some(n) = self.superseded_by(note) {
                return Status {
                    status: Some("superseded".into()),
                    outcome: None,
                    by_event: Some(n.id.clone()),
                };
            }
        }
        match newest {
            Some(r) => {
                let (s, o) = r.status.clone().unwrap_or_default();
                Status {
                    status: Some(s),
                    outcome: o,
                    by_event: Some(r.id.clone()),
                }
            }
            None => Status {
                status: note.status.clone(),
                outcome: None,
                by_event: None,
            },
        }
    }

    /// Whether `me` may vote `action` on `note`, and why not.
    pub fn can_vote(&self, note: &Note, action: usize, me: &str) -> Result<(), String> {
        if note.actions.get(action).is_none() {
            return Err(if note.actions.is_empty() {
                "not a request: this note declares no actions".into()
            } else {
                format!("this request has {} actions", note.actions.len())
            });
        }
        let st = self.status(note);
        match st.status.as_deref() {
            None | Some("open") => {}
            Some("closed") => {
                let o = st.outcome.map(|o| format!(" ({o})")).unwrap_or_default();
                return Err(format!("the request is closed{o}"));
            }
            Some("superseded") => {
                return Err(
                    "superseded: a newer note from its source replaced this request".into(),
                );
            }
            Some(other) => return Err(format!("the request is {other}")),
        }
        let me = me.to_ascii_lowercase();
        if !note.approvers.is_empty() && !note.approvers.contains(&me) {
            return Err("only the note's approvers can vote on it".into());
        }
        // One vote per request: approve then decline would drop the offer
        // (`cmd_poll` checks decline first).
        if let Some(t) = self.votes(note).iter().find(|t| t.voters.contains(&me)) {
            return Err(format!("you already voted {}", t.action.name));
        }
        Ok(())
    }
}

// ---------- reading the channel ----------

/// The control channel's messages for the last `DAYS_BACK` days before
/// `now`, and every reaction to its notes; with the note ids, for the live
/// tail of reactions (`tail_filters`).
pub async fn fetch(
    r: &crate::relay::Relay,
    channel: &str,
    now: u64,
) -> anyhow::Result<(Vec<Event>, Vec<String>)> {
    let since = now.saturating_sub(DAYS_BACK * 86_400);
    let f = json!({"kinds": [9], "#h": [channel], "since": since});
    let mut evs = r.query_all_at_most(f, 5000).await?;
    let ids: Vec<String> = evs
        .iter()
        .filter(|e| parse_note(e).is_some())
        .map(|e| e.id.to_hex())
        .collect();
    for chunk in ids.chunks(100) {
        evs.extend(
            r.query_all_at_most(json!({"kinds": [7], "#e": chunk}), 5000)
                .await?,
        );
    }
    Ok((evs, ids))
}

/// The live tail from `now`: new messages in the channel, and reactions to
/// `notes`.
pub fn tail_filters(channel: &str, notes: &[String], now: u64) -> Vec<Value> {
    let mut filters = vec![json!({"kinds": [9], "#h": [channel], "since": now, "limit": 0})];
    if !notes.is_empty() {
        filters.push(json!({"kinds": [7], "#e": notes, "since": now, "limit": 0}));
    }
    filters
}

// ---------- charts ----------

/// Every day from `first` to `last`, both included (YYYY-MM-DD).
pub fn day_range(first: &str, last: &str) -> Vec<String> {
    let parse = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    let (Some(mut d), Some(end)) = (parse(first), parse(last)) else {
        return vec![];
    };
    let mut out = vec![];
    while d <= end && out.len() < 3660 {
        out.push(d.format("%Y-%m-%d").to_string());
        match d.succ_opt() {
            Some(n) => d = n,
            None => break,
        }
    }
    out
}

/// One line on a chart: a value per day (None = no note that day, a gap)
/// and the reference line, if there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub label: String,
    pub points: Vec<Option<f64>>,
    pub baseline: Option<f64>,
}

impl Series {
    /// Runs of days with a value, as (day index, value), so a gap is drawn
    /// as a break in the line rather than a zero.
    pub fn segments(&self) -> Vec<Vec<(f64, f64)>> {
        let mut out: Vec<Vec<(f64, f64)>> = vec![];
        let mut run = vec![];
        for (i, p) in self.points.iter().enumerate() {
            match p {
                Some(v) => run.push((i as f64, *v)),
                None if !run.is_empty() => out.push(std::mem::take(&mut run)),
                None => {}
            }
        }
        if !run.is_empty() {
            out.push(run);
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Tokens,
    Pct,
    /// A plain count (runs, problems, events, notes).
    Count,
}

/// A trend chart: x = day, one series per entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub title: String,
    pub unit: Unit,
    pub series: Vec<Series>,
}

/// A trend chart's spec: (title, unit, [(details key, series label)]).
pub type Trend = (&'static str, Unit, &'static [(&'static str, &'static str)]);

/// The flat hooks/1 metrics charted day by day.
pub const HOOKS_TRENDS: &[Trend] = &[
    (
        "Peak context",
        Unit::Tokens,
        &[
            ("peak_context_subagent_tokens", "subagent"),
            ("peak_context_main_tokens", "main"),
        ],
    ),
    (
        "Spend above 300k",
        Unit::Pct,
        &[("share_spend_above_300k_pct", "share")],
    ),
    (
        "Ramp after handoff",
        Unit::Tokens,
        &[("ramp_after_handoff_median_tokens", "median")],
    ),
    (
        "Cost per useful call",
        Unit::Tokens,
        &[("cost_per_useful_call_tokens", "tokens")],
    ),
    (
        "Weighted spend",
        Unit::Tokens,
        &[("weighted_spend_tokens", "input-eq")],
    ),
];

/// The 2026-10-19 report's flag: a recommended reset this far off its
/// threshold is flagged, and a hooks metric this much worse than its
/// baseline is drawn as a flag.
pub const FLAG_PCT: f64 = 20.0;
/// A hooks metric this much worse than its baseline is drawn as a page.
pub const PAGE_PCT: f64 = 50.0;
/// Open requests the overview's strip shows (and j k can pick).
pub const STRIP_MAX: usize = 3;
/// Days in an overview sparkline, today included.
pub const SPARK_DAYS: usize = 14;
/// The topics producers write (aei `wiki/architecture/control-notes.md`).
/// They always get a tile, dim until their first note.
pub const KNOWN_TOPICS: &[&str] = &["hooks", "upgrade", "converge", "fleet-audit"];

/// The days a chart spans: every day from the source's first note in the
/// topic to its last, gaps included.
pub fn chart_days(notes: &[&Note]) -> Vec<String> {
    // A malformed day (a hand-built event) mustn't blank the whole range.
    let days: BTreeSet<&str> = notes
        .iter()
        .filter_map(|n| n.day.as_deref())
        .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        .collect();
    match (days.iter().next(), days.iter().next_back()) {
        (Some(a), Some(b)) => day_range(a, b),
        _ => vec![],
    }
}

/// hooks/1 trend charts for one source, from the notes that count. The
/// baselines come from `current` (the note on screen).
pub fn hooks_charts(board: &Board, source: &str, current: &Note) -> (Vec<String>, Vec<Chart>) {
    let notes: Vec<&Note> = board
        .days(&current.topic)
        .into_iter()
        .filter(|n| n.source.as_deref() == Some(source) && n.schema.as_deref() == Some("hooks/1"))
        .collect();
    let days = chart_days(&notes);
    let by_day: HashMap<&str, &Value> = notes
        .iter()
        .filter_map(|n| Some((n.day.as_deref()?, n.details.as_ref().ok()?)))
        .collect();
    let base = current.details.as_ref().ok().map(|d| &d["baseline"]);
    let mut charts: Vec<Chart> = HOOKS_TRENDS
        .iter()
        .map(|(title, unit, keys)| Chart {
            title: title.to_string(),
            unit: *unit,
            series: keys
                .iter()
                .map(|(key, label)| Series {
                    label: label.to_string(),
                    points: days
                        .iter()
                        .map(|d| by_day.get(d.as_str()).and_then(|v| v[*key].as_f64()))
                        .collect(),
                    baseline: base.and_then(|b| b[*key].as_f64()),
                })
                .collect(),
        })
        .collect();
    // Reset vs threshold: each whole-type row's recommended reset, with its
    // threshold as the reference line.
    let types: BTreeSet<String> = current
        .details
        .as_ref()
        .ok()
        .and_then(|d| d["agent_types"].as_array())
        .into_iter()
        .flatten()
        .filter(|r| r["repo"].is_null())
        .filter_map(|r| r["agent_type"].as_str().map(str::to_string))
        .collect();
    let row = |v: &Value, t: &str| -> Option<Value> {
        v["agent_types"]
            .as_array()?
            .iter()
            .find(|r| r["repo"].is_null() && r["agent_type"].as_str() == Some(t))
            .cloned()
    };
    if !types.is_empty() {
        charts.push(Chart {
            title: "Reset vs threshold".into(),
            unit: Unit::Tokens,
            series: types
                .iter()
                .map(|t| Series {
                    label: t.clone(),
                    points: days
                        .iter()
                        .map(|d| {
                            by_day
                                .get(d.as_str())
                                .and_then(|v| row(v, t))
                                .and_then(|r| r["recommended"].as_f64())
                        })
                        .collect(),
                    baseline: current
                        .details
                        .as_ref()
                        .ok()
                        .and_then(|v| row(v, t))
                        .and_then(|r| r["threshold"].as_f64()),
                })
                .collect(),
        });
    }
    (days, charts)
}

/// Each day's hooks status (`ok`, `partial`, `dead`), None for a gap.
pub fn hooks_status_strip(
    board: &Board,
    source: &str,
    current: &Note,
) -> Vec<(String, Option<String>)> {
    let notes: Vec<&Note> = board
        .days(&current.topic)
        .into_iter()
        .filter(|n| n.source.as_deref() == Some(source))
        .collect();
    let by_day: HashMap<&str, &Note> = notes
        .iter()
        .filter_map(|n| Some((n.day.as_deref()?, *n)))
        .collect();
    chart_days(&notes)
        .into_iter()
        .map(|d| {
            let s = by_day
                .get(d.as_str())
                .and_then(|n| n.details.as_ref().ok())
                .and_then(|v| v["status"].as_str())
                .map(str::to_string);
            (d, s)
        })
        .collect()
}

/// Tokens as people read them: 767k, 1.2M, 950.
pub fn short_tokens(v: f64) -> String {
    let a = v.abs();
    if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}k", v / 1e3)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

/// A value as people read it in its unit: 767k, 42%, 3, 2.5.
pub fn value_label(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Tokens => short_tokens(v),
        Unit::Pct => format!("{v:.0}%"),
        Unit::Count if v.fract() == 0.0 => format!("{v:.0}"),
        Unit::Count => format!("{v:.1}"),
    }
}

// ---------- the overview ----------

/// A topic's state on the overview: no note (dim), ok (green), warn
/// (amber), page (red). Ordered so the worst is the max.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    NoNote,
    Ok,
    Warn,
    Page,
}

/// What a note says about its topic. The details' `status` decides
/// (hooks/1 ok partial dead, converge/1 applied clean running held failed,
/// fleet-audit/1 ok partial failed, upgrade/1's events); a status this
/// doesn't know is a warn, and an open request is at least a warn.
pub fn note_health(board: &Board, note: &Note) -> Health {
    let open = note.is_request() && board.status(note).status.as_deref() == Some("open");
    let base = match &note.details {
        Err(_) => Health::Warn,
        Ok(d) => match d["status"].as_str() {
            None => Health::Ok,
            Some(
                "ok" | "clean" | "applied" | "running" | "rolled" | "followup_done" | "notice"
                | "declined" | "dropped" | "closed",
            ) => Health::Ok,
            Some(
                "dead" | "failed" | "fail" | "page" | "stage_failed" | "apply_failed"
                | "rolled_back" | "followup_failed",
            ) => Health::Page,
            Some(_) => Health::Warn,
        },
    };
    if open { base.max(Health::Warn) } else { base }
}

/// The number a topic's sparkline draws, one per day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMetric {
    /// The details key; None counts the day's notes.
    pub key: Option<&'static str>,
    pub label: &'static str,
    pub unit: Unit,
    /// Sources on one day: the mean (a rate) or the sum (a count).
    pub mean: bool,
}

/// Each schema's key metric: hooks cost per useful call; converge held
/// runs (the runs that wait on a human); fleet-audit problems; upgrade has
/// no one number, so its events. Anything else: notes that day.
pub fn key_metric(schema: Option<&str>) -> KeyMetric {
    let (key, label, unit, mean) = match schema {
        Some("hooks/1") => (
            Some("cost_per_useful_call_tokens"),
            "cost/call",
            Unit::Tokens,
            true,
        ),
        Some("converge/1") => (Some("held_n"), "held", Unit::Count, false),
        Some("fleet-audit/1") => (Some("problems_n"), "problems", Unit::Count, false),
        Some("upgrade/1") => (Some("events_n"), "events", Unit::Count, false),
        _ => (None, "notes", Unit::Count, false),
    };
    KeyMetric {
        key,
        label,
        unit,
        mean,
    }
}

/// One topic's tile on the overview.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub topic: String,
    pub health: Health,
    /// The first line of the worst note on the newest day, or "no note".
    pub line: String,
    pub latest_day: Option<String>,
    pub sources: Vec<String>,
    pub metric: KeyMetric,
    /// SPARK_DAYS values, oldest first, ending today; None = no value.
    pub spark: Vec<Option<f64>>,
    /// Requests still open in this topic.
    pub open: usize,
}

/// A summary's first line as a one-liner: no markdown heading or emphasis.
pub fn one_liner(summary: &str) -> String {
    let l = summary
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    l.trim_start_matches('#')
        .trim()
        .replace("**", "")
        .replace('`', "")
}

/// The days ending `today`, `n` of them, oldest first.
pub fn days_ending(today: &str, n: usize) -> Vec<String> {
    let Ok(end) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return vec![];
    };
    let start = end - chrono::Days::new(n.saturating_sub(1) as u64);
    day_range(&start.format("%Y-%m-%d").to_string(), today)
}

impl Board {
    /// The overview's tiles: the known topics in their order, then any
    /// other topic by name. `today` ends the sparklines (YYYY-MM-DD).
    pub fn tiles(&self, today: &str) -> Vec<Tile> {
        let latest = self.latest();
        let mut names: Vec<String> = KNOWN_TOPICS.iter().map(|s| s.to_string()).collect();
        let others: BTreeSet<&str> = latest
            .iter()
            .map(|n| n.topic.as_str())
            .filter(|t| !KNOWN_TOPICS.contains(t))
            .collect();
        names.extend(others.into_iter().map(str::to_string));
        let window = days_ending(today, SPARK_DAYS);
        names
            .into_iter()
            .map(|topic| {
                let notes: Vec<&Note> = latest
                    .iter()
                    .copied()
                    .filter(|n| n.topic == topic)
                    .collect();
                let latest_day = notes.iter().filter_map(|n| n.day.clone()).max();
                let mut newest: Vec<&Note> = notes
                    .iter()
                    .copied()
                    .filter(|n| n.day == latest_day)
                    .collect();
                newest.sort_by(|a, b| a.source.cmp(&b.source));
                // The worst note speaks for the day; the first source on a tie.
                let worst = newest
                    .iter()
                    .copied()
                    .rev()
                    .max_by_key(|n| note_health(self, n));
                let health = worst.map_or(Health::NoNote, |n| note_health(self, n));
                let line = worst.map_or_else(|| "no note".to_string(), |n| one_liner(&n.summary));
                let schema = worst.and_then(|n| n.schema.as_deref());
                let metric = key_metric(schema);
                let spark = window
                    .iter()
                    .map(|d| {
                        let day: Vec<&Note> = notes
                            .iter()
                            .copied()
                            .filter(|n| n.day.as_deref() == Some(d.as_str()))
                            .collect();
                        let Some(key) = metric.key else {
                            return (!day.is_empty()).then_some(day.len() as f64);
                        };
                        let vals: Vec<f64> = day
                            .iter()
                            .filter_map(|n| n.details.as_ref().ok()?[key].as_f64())
                            .collect();
                        if vals.is_empty() {
                            return None;
                        }
                        let sum: f64 = vals.iter().sum();
                        Some(if metric.mean {
                            sum / vals.len() as f64
                        } else {
                            sum
                        })
                    })
                    .collect();
                let sources: BTreeSet<String> =
                    newest.iter().filter_map(|n| n.source.clone()).collect();
                let open = self
                    .open_requests()
                    .iter()
                    .filter(|n| n.topic == topic)
                    .count();
                // An open request from any day keeps the tile at least amber.
                let health = if open > 0 {
                    health.max(Health::Warn)
                } else {
                    health
                };
                Tile {
                    topic,
                    health,
                    line,
                    latest_day,
                    sources: sources.into_iter().collect(),
                    metric,
                    spark,
                    open,
                }
            })
            .collect()
    }

    /// Requests still open, newest first: not closed, and not superseded
    /// (`superseded_by`) by a newer note for their day or a newer request
    /// from the same author, topic and source.
    pub fn open_requests(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self
            .notes
            .values()
            .filter(|n| n.is_request() && self.status(n).status.as_deref() == Some("open"))
            .collect();
        v.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
        v
    }

    /// The newest hooks/1 note (newest day, then newest written).
    pub fn newest_hooks(&self) -> Option<&Note> {
        self.latest()
            .into_iter()
            .filter(|n| n.schema.as_deref() == Some("hooks/1") && n.details.is_ok())
            .max_by(|a, b| (&a.day, a.created_at, &a.id).cmp(&(&b.day, b.created_at, &b.id)))
    }
}

/// How a delta reads: plain, better than the baseline (green), a flag
/// (amber) or a page (red). A flag is never green.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Better,
    Flag,
    Page,
}

/// The tone of `value` against `baseline` where lower is better (every
/// hooks/1 metric). `flagged` is the report's own flag when it has one
/// (agent types); otherwise worse by more than FLAG_PCT is a flag and by
/// more than PAGE_PCT a page.
pub fn delta_tone(value: f64, baseline: Option<f64>, flagged: Option<bool>) -> Tone {
    let Some(b) = baseline.filter(|b| *b != 0.0) else {
        return if flagged == Some(true) {
            Tone::Flag
        } else {
            Tone::Plain
        };
    };
    let rel = (value - b) / b.abs() * 100.0;
    match flagged {
        Some(true) => Tone::Flag,
        Some(false) if rel < 0.0 => Tone::Better,
        Some(false) => Tone::Plain,
        None if rel > PAGE_PCT => Tone::Page,
        None if rel > FLAG_PCT => Tone::Flag,
        None if rel < 0.0 => Tone::Better,
        None => Tone::Plain,
    }
}

/// The delta as people read it: percent off the baseline, or points for a
/// percentage.
pub fn delta_label(value: f64, baseline: f64, unit: Unit) -> String {
    match unit {
        Unit::Pct => format!("{:+.0}pt", value - baseline),
        _ if baseline == 0.0 => String::new(),
        _ => format!("{:+.0}%", (value - baseline) / baseline.abs() * 100.0),
    }
}

/// A hooks metric as a gauge: value, baseline, delta tone and the last
/// SPARK_DAYS days of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Gauge {
    pub title: String,
    pub value: f64,
    pub baseline: Option<f64>,
    pub unit: Unit,
    /// A side note, e.g. the sample size.
    pub aside: Option<String>,
    pub tone: Tone,
    pub spark: Vec<Option<f64>>,
}

/// The hooks/1 gauges, in reading order: (title, key, unit, sample-size key).
pub const HOOKS_GAUGES: &[(&str, &str, Unit, Option<&str>)] = &[
    (
        "peak subagent",
        "peak_context_subagent_tokens",
        Unit::Tokens,
        None,
    ),
    ("peak main", "peak_context_main_tokens", Unit::Tokens, None),
    ("spend >300k", "share_spend_above_300k_pct", Unit::Pct, None),
    (
        "ramp after handoff",
        "ramp_after_handoff_median_tokens",
        Unit::Tokens,
        Some("ramp_after_handoff_n"),
    ),
    (
        "cost / useful call",
        "cost_per_useful_call_tokens",
        Unit::Tokens,
        None,
    ),
];

/// The gauges for `current` (a hooks/1 note): the flat metrics against
/// their baselines, then each agent type's recommended reset against its
/// threshold. Sparklines end on the note's day, same source.
pub fn hooks_gauges(board: &Board, current: &Note) -> Vec<Gauge> {
    let Ok(d) = current.details.as_ref() else {
        return vec![];
    };
    let by_day: HashMap<String, &Value> = board
        .latest()
        .into_iter()
        .filter(|n| {
            n.topic == current.topic
                && n.source == current.source
                && n.schema.as_deref() == Some("hooks/1")
        })
        .filter_map(|n| Some((n.day.clone()?, n.details.as_ref().ok()?)))
        .collect();
    let window = days_ending(current.day.as_deref().unwrap_or(""), SPARK_DAYS);
    let spark = |f: &dyn Fn(&Value) -> Option<f64>| -> Vec<Option<f64>> {
        window
            .iter()
            .map(|day| by_day.get(day).and_then(|v| f(v)))
            .collect()
    };
    let mut out = vec![];
    for (title, key, unit, n) in HOOKS_GAUGES {
        let Some(value) = d[*key].as_f64() else {
            continue;
        };
        let baseline = d["baseline"][*key].as_f64();
        out.push(Gauge {
            title: title.to_string(),
            value,
            baseline,
            unit: *unit,
            aside: n.and_then(|k| d[k].as_u64()).map(|n| format!("n={n}")),
            tone: delta_tone(value, baseline, None),
            spark: spark(&|v| v[*key].as_f64()),
        });
    }
    let whole = |v: &Value, t: &str| -> Option<Value> {
        v["agent_types"]
            .as_array()?
            .iter()
            .find(|r| r["repo"].is_null() && r["agent_type"].as_str() == Some(t))
            .cloned()
    };
    let types: Vec<String> = d["agent_types"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["repo"].is_null())
        .filter_map(|r| r["agent_type"].as_str().map(str::to_string))
        .collect();
    for t in types {
        let Some(r) = whole(d, &t) else { continue };
        let Some(value) = r["recommended"].as_f64() else {
            continue;
        };
        let baseline = r["threshold"].as_f64();
        out.push(Gauge {
            title: format!("{t} reset"),
            value,
            baseline,
            unit: Unit::Tokens,
            aside: r["n"].as_u64().map(|n| format!("n={n}")),
            tone: delta_tone(value, baseline, Some(r["flagged"].as_bool() == Some(true))),
            spark: spark(&|v| whole(v, &t)?["recommended"].as_f64()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use serde_json::json;

    const CH: &str = "ee7289a1-e875-43fd-9fdb-42aa5451be05";

    fn ev(keys: &Keys, kind: u16, content: &str, tags: &[&[&str]], at: u64) -> Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.iter().map(|t| Tag::parse(t.iter().copied()).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn details(peak: u64) -> Value {
        json!({
            "status": "ok",
            "peak_context_subagent_tokens": peak,
            "peak_context_main_tokens": 604000,
            "share_spend_above_300k_pct": 42,
            "ramp_after_handoff_median_tokens": 108000,
            "cost_per_useful_call_tokens": 55100,
            "weighted_spend_tokens": 157100000,
            "baseline": {"peak_context_subagent_tokens": 612000, "peak_context_main_tokens": 432000,
                "share_spend_above_300k_pct": 41, "ramp_after_handoff_median_tokens": 79000,
                "cost_per_useful_call_tokens": 75200},
            "agent_types": [
                {"agent_type": "builder", "repo": null, "recommended": 161000, "threshold": 250000, "flagged": true},
                {"agent_type": "builder", "repo": "aei", "recommended": 183000, "threshold": 250000, "flagged": true}
            ]
        })
    }

    fn note(keys: &Keys, day: &str, source: &str, at: u64, d: &Value, extra: &[&[&str]]) -> Event {
        let content = format!("Summary for {day}.\n\n{DETAILS_FENCE}\n{d:#}\n```");
        let mut tags: Vec<&[&str]> = vec![
            &["h", CH],
            &["t", "control"],
            &[TAG_TOPIC, "hooks"],
            &[TAG_SCHEMA, "hooks/1"],
        ];
        let dt = [TAG_DAY, day];
        let st = [TAG_SOURCE, source];
        tags.push(&dt);
        tags.push(&st);
        tags.extend_from_slice(extra);
        ev(keys, 9, &content, &tags, at)
    }

    #[test]
    fn parses_a_note() {
        let k = Keys::generate();
        let e = note(
            &k,
            "2026-10-21",
            "build-box",
            100,
            &details(767000),
            &[
                &[TAG_ACTION, "approve", "👍"],
                &[TAG_APPROVER, "ABCDEF"],
                &[TAG_STATUS, "open"],
                &[
                    "imeta",
                    "url https://m.example/a.png",
                    "m image/png",
                    "dim 10x20",
                ],
            ],
        );
        let n = parse_note(&e).unwrap();
        assert_eq!(n.topic, "hooks");
        assert_eq!(n.day.as_deref(), Some("2026-10-21"));
        assert_eq!(n.source.as_deref(), Some("build-box"));
        assert_eq!(n.schema.as_deref(), Some("hooks/1"));
        assert_eq!(n.summary, "Summary for 2026-10-21.");
        assert_eq!(
            n.details.as_ref().unwrap()["peak_context_subagent_tokens"],
            767000
        );
        assert_eq!(
            n.actions,
            vec![Action {
                name: "approve".into(),
                emoji: "👍".into()
            }]
        );
        assert_eq!(n.approvers, vec!["abcdef".to_string()]);
        assert_eq!(n.status.as_deref(), Some("open"));
        assert_eq!(n.media[0].url, "https://m.example/a.png");
        assert!(n.is_request());
        // A reply, or a message without a topic, is not a note.
        let r = ev(
            &k,
            9,
            "hi",
            &[
                &["h", CH],
                &["e", &n.id, "", "reply"],
                &[TAG_TOPIC, "hooks"],
            ],
            101,
        );
        assert!(parse_note(&r).is_none());
        assert!(parse_note(&ev(&k, 9, "hi", &[&["h", CH]], 1)).is_none());
    }

    #[test]
    fn details_errors_and_fence_rules() {
        let (s, d) = split_content("just text");
        assert_eq!(s, "just text");
        assert!(d.unwrap_err().contains("no control-details"));
        let (_, d) = split_content(&format!("x\n\n{DETAILS_FENCE}\n{{\"a\":1}}"));
        assert!(d.unwrap_err().contains("not closed"));
        let (_, d) = split_content(&format!("x\n\n{DETAILS_FENCE}\n{{nope\n```"));
        assert!(d.unwrap_err().contains("not JSON"));
        // Mid-line fence text doesn't open a block; the last real one wins.
        let c = format!(
            "see {DETAILS_FENCE} inline\n\n{DETAILS_FENCE}\n{{\"a\":1}}\n```\n{DETAILS_FENCE}\r\n{{\"a\":2}}\n```"
        );
        let (_, d) = split_content(&c);
        assert_eq!(d.unwrap()["a"], 2);
    }

    #[test]
    fn newest_note_per_day_and_source_wins() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        let old = note(&k, "2026-10-21", "build-box", 100, &details(1), &[]);
        let new = note(&k, "2026-10-21", "build-box", 200, &details(2), &[]);
        let other = note(&k, "2026-10-21", "mac", 50, &details(3), &[]);
        let before = note(&k, "2026-10-19", "build-box", 10, &details(4), &[]);
        // Order of arrival doesn't matter.
        for e in [&new, &old, &other, &before] {
            assert!(b.apply(e));
        }
        assert!(!b.apply(&new), "same event twice");
        // Another channel's message is ignored.
        let stray = ev(&k, 9, "x", &[&["h", "other"], &[TAG_TOPIC, "hooks"]], 300);
        assert!(!b.apply(&stray));
        let days = b.days("hooks");
        let got: Vec<(&str, &str, u64)> = days
            .iter()
            .map(|n| {
                (
                    n.day.as_deref().unwrap(),
                    n.source.as_deref().unwrap(),
                    n.details.as_ref().unwrap()["peak_context_subagent_tokens"]
                        .as_u64()
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("2026-10-21", "build-box", 2),
                ("2026-10-21", "mac", 3),
                ("2026-10-19", "build-box", 4)
            ]
        );
        let t = b.topics();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].latest_day, "2026-10-21");
        assert_eq!(t[0].days, 2);
        assert_eq!(
            t[0].sources,
            vec!["build-box".to_string(), "mac".to_string()]
        );
    }

    #[test]
    fn a_missing_day_is_a_gap_not_a_zero() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        for (day, v) in [
            ("2026-10-18", 100),
            ("2026-10-19", 200),
            ("2026-10-21", 400),
        ] {
            b.apply(&note(&k, day, "build-box", 1, &details(v), &[]));
        }
        // A different source's day doesn't fill this one's gap.
        b.apply(&note(&k, "2026-10-20", "mac", 1, &details(999), &[]));
        let cur = b.days("hooks")[0].clone();
        assert_eq!(cur.day.as_deref(), Some("2026-10-21"));
        let (days, charts) = hooks_charts(&b, "build-box", &cur);
        assert_eq!(days, day_range("2026-10-18", "2026-10-21"));
        assert_eq!(days.len(), 4);
        let peak = &charts[0].series[0];
        assert_eq!(peak.label, "subagent");
        assert_eq!(
            peak.points,
            vec![Some(100.0), Some(200.0), None, Some(400.0)]
        );
        assert_eq!(peak.baseline, Some(612000.0));
        assert_eq!(
            peak.segments(),
            vec![vec![(0.0, 100.0), (1.0, 200.0)], vec![(3.0, 400.0)]]
        );
        let strip = hooks_status_strip(&b, "build-box", &cur);
        assert_eq!(strip[2], ("2026-10-20".to_string(), None));
        assert_eq!(strip[3].1.as_deref(), Some("ok"));
    }

    #[test]
    fn chart_series_from_the_seed_shape() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        b.apply(&note(
            &k,
            "2026-10-21",
            "build-box",
            1,
            &details(767000),
            &[],
        ));
        let cur = b.days("hooks")[0].clone();
        let (days, charts) = hooks_charts(&b, "build-box", &cur);
        assert_eq!(days, vec!["2026-10-21".to_string()]);
        let titles: Vec<&str> = charts.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Peak context",
                "Spend above 300k",
                "Ramp after handoff",
                "Cost per useful call",
                "Weighted spend",
                "Reset vs threshold"
            ]
        );
        assert_eq!(charts[0].series[1].points, vec![Some(604000.0)]);
        assert_eq!(charts[0].series[1].baseline, Some(432000.0));
        assert_eq!(charts[1].unit, Unit::Pct);
        assert_eq!(charts[4].series[0].baseline, None, "no baseline for spend");
        // Only the whole-type row (repo null) is charted.
        let reset = &charts[5];
        assert_eq!(reset.series.len(), 1);
        assert_eq!(reset.series[0].points, vec![Some(161000.0)]);
        assert_eq!(reset.series[0].baseline, Some(250000.0));
        assert_eq!(short_tokens(767000.0), "767k");
        assert_eq!(short_tokens(55100.0), "55k");
        assert_eq!(short_tokens(1000.0), "1.0k");
        assert_eq!(short_tokens(157100000.0), "157.1M");
    }

    #[test]
    fn votes_count_approvers_and_normalized_emoji() {
        let author = Keys::generate();
        let lucian = Keys::generate();
        let rando = Keys::generate();
        let mut b = Board::new(CH);
        let approver = lucian.public_key().to_hex().to_uppercase();
        let e = note(
            &author,
            "2026-10-21",
            "build-box",
            1,
            &details(1),
            &[
                &[TAG_ACTION, "approve", "👍"],
                &[TAG_ACTION, "hold", "⏳"],
                &[TAG_ACTION, "plus", "+"],
                &[TAG_APPROVER, &approver],
                &[TAG_STATUS, "open"],
            ],
        );
        b.apply(&e);
        let id = e.id.to_hex();
        let react = |k: &Keys, emoji: &str, at: u64| ev(k, 7, emoji, &[&["e", &id]], at);
        // Skin tone and FE0F variants match; a non-approver doesn't count.
        b.apply(&react(&lucian, "👍🏽", 2));
        b.apply(&react(&lucian, "👍\u{FE0F}", 3));
        b.apply(&react(&rando, "👍", 4));
        b.apply(&react(&lucian, "", 5));
        let n = b.note(&id).unwrap().clone();
        let v = b.votes(&n);
        let lp = lucian.public_key().to_hex();
        assert_eq!(v[0].action.name, "approve");
        assert_eq!(v[0].voters, vec![lp.clone()], "one vote per pubkey");
        assert!(v[1].voters.is_empty());
        // cmd_poll reads "" as "", not NIP-25's +.
        assert!(v[2].voters.is_empty(), "empty reaction is not +");
        assert_eq!(b.reaction_counts(&n)["👍"], 3);
        // Who may vote, and why not: one vote per request, any action.
        assert_eq!(
            b.can_vote(&n, 0, &lp).unwrap_err(),
            "you already voted approve"
        );
        assert_eq!(
            b.can_vote(&n, 1, &lp).unwrap_err(),
            "you already voted approve"
        );
        assert!(
            b.can_vote(&n, 1, &rando.public_key().to_hex())
                .unwrap_err()
                .contains("only the note's approvers")
        );
        // Without approvers anyone in the channel counts.
        let open = note(
            &author,
            "2026-10-20",
            "x",
            1,
            &details(1),
            &[&[TAG_ACTION, "approve", "👍"]],
        );
        b.apply(&open);
        let oid = open.id.to_hex();
        b.apply(&ev(&rando, 7, "👍", &[&["e", &oid]], 9));
        let on = b.note(&oid).unwrap().clone();
        assert_eq!(b.votes(&on)[0].voters, vec![rando.public_key().to_hex()]);
        // A plain report has nothing to vote on.
        let plain = note(&author, "2026-10-19", "x", 1, &details(1), &[]);
        b.apply(&plain);
        let pn = b.note(&plain.id.to_hex()).unwrap().clone();
        assert!(
            b.can_vote(&pn, 0, &lp)
                .unwrap_err()
                .contains("not a request")
        );
    }

    #[test]
    fn status_from_the_newest_valid_reply() {
        let author = Keys::generate();
        let lucian = Keys::generate();
        let rando = Keys::generate();
        let mut b = Board::new(CH);
        let e = note(
            &author,
            "2026-10-21",
            "build-box",
            1,
            &details(1),
            &[
                &[TAG_ACTION, "approve", "👍"],
                &[TAG_APPROVER, &lucian.public_key().to_hex()],
                &[TAG_STATUS, "open"],
            ],
        );
        b.apply(&e);
        let id = e.id.to_hex();
        let n = b.note(&id).unwrap().clone();
        assert_eq!(b.status(&n).status.as_deref(), Some("open"));
        assert_eq!(b.topics()[0].open, 1);
        let reply = |k: &Keys, st: &[&str], at: u64| {
            let e = ["e", id.as_str(), "", "root"];
            let t: Vec<&[&str]> = vec![&["h", CH], &e, st];
            ev(k, 9, "status", &t, at)
        };
        // Someone else can't close it.
        b.apply(&reply(&rando, &[TAG_STATUS, "closed", "approve"], 5));
        assert_eq!(b.status(&n).status.as_deref(), Some("open"));
        b.apply(&reply(&lucian, &[TAG_STATUS, "closed", "approve"], 6));
        b.apply(&reply(&author, &[TAG_STATUS, "open"], 4));
        let s = b.status(&n);
        assert_eq!(s.status.as_deref(), Some("closed"));
        assert_eq!(s.outcome.as_deref(), Some("approve"));
        assert_eq!(b.reply_count(&n), 3);
        assert_eq!(b.topics()[0].open, 0);
        assert_eq!(
            b.can_vote(&n, 0, &lucian.public_key().to_hex())
                .unwrap_err(),
            "the request is closed (approve)"
        );
    }

    #[test]
    fn a_malformed_day_does_not_blank_the_charts() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        b.apply(&note(&k, "2026-10-20", "build-box", 1, &details(1), &[]));
        b.apply(&note(&k, "zzz", "build-box", 1, &details(2), &[]));
        let cur = b
            .days("hooks")
            .into_iter()
            .find(|n| n.day.as_deref() == Some("2026-10-20"))
            .unwrap()
            .clone();
        let (days, _) = hooks_charts(&b, "build-box", &cur);
        assert_eq!(days, vec!["2026-10-20".to_string()]);
    }

    #[test]
    fn tiles_health_one_liner_and_sparkline() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        // hooks: two sources on the newest day, one dead: the dead one speaks.
        b.apply(&note(&k, "2026-10-19", "build-box", 1, &details(1), &[]));
        let mut dead = details(2);
        dead["status"] = json!("dead");
        dead["cost_per_useful_call_tokens"] = json!(60000);
        b.apply(&note(&k, "2026-10-21", "mac", 1, &dead, &[]));
        b.apply(&note(&k, "2026-10-21", "build-box", 1, &details(3), &[]));
        // converge held: a warn, held_n as its metric.
        let conv = format!(
            "**Converge** held: buzz_relay drift\n\n{DETAILS_FENCE}\n{}\n```",
            json!({"status": "held", "held_n": 2})
        );
        b.apply(&ev(
            &k,
            9,
            &conv,
            &[
                &["h", CH],
                &[TAG_TOPIC, "converge"],
                &[TAG_SCHEMA, "converge/1"],
                &[TAG_DAY, "2026-10-20"],
                &[TAG_SOURCE, "infra-box"],
            ],
            5,
        ));
        // A topic nobody planned for.
        b.apply(&ev(
            &k,
            9,
            "routing fine",
            &[
                &["h", CH],
                &[TAG_TOPIC, "routing"],
                &[TAG_DAY, "2026-10-21"],
                &[TAG_SOURCE, "x"],
            ],
            5,
        ));
        let t = b.tiles("2026-10-21");
        let names: Vec<&str> = t.iter().map(|t| t.topic.as_str()).collect();
        assert_eq!(
            names,
            ["hooks", "upgrade", "converge", "fleet-audit", "routing"]
        );
        let hooks = &t[0];
        assert_eq!(hooks.health, Health::Page);
        assert_eq!(hooks.line, "Summary for 2026-10-21.");
        assert_eq!(
            hooks.sources,
            vec!["build-box".to_string(), "mac".to_string()]
        );
        assert_eq!(hooks.metric.key, Some("cost_per_useful_call_tokens"));
        assert_eq!(hooks.spark.len(), SPARK_DAYS);
        // Oldest first, ending today; a day without a note is None, two
        // sources' rates are averaged.
        assert_eq!(hooks.spark[13], Some((60000.0 + 55100.0) / 2.0));
        assert_eq!(hooks.spark[12], None);
        assert_eq!(hooks.spark[11], Some(55100.0));
        assert_eq!(t[1].health, Health::NoNote);
        assert_eq!(t[1].line, "no note");
        assert!(t[1].spark.iter().all(Option::is_none));
        assert_eq!(t[2].health, Health::Warn);
        assert_eq!(t[2].line, "Converge held: buzz_relay drift");
        assert_eq!(t[2].spark[12], Some(2.0));
        // No details at all: a warn, counted by notes.
        assert_eq!(t[4].health, Health::Warn);
        assert_eq!(t[4].metric.key, None);
        assert_eq!(t[4].spark[13], Some(1.0));
        // Older than the window: off the sparkline.
        assert!(b.tiles("2026-11-30")[0].spark.iter().all(Option::is_none));
    }

    #[test]
    fn open_requests_newest_first_and_closed_ones_drop() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        // Two sources: one source's newer request would supersede the other.
        let req = |day: &str, source: &str, at: u64| {
            note(
                &k,
                day,
                source,
                at,
                &details(1),
                &[&[TAG_ACTION, "approve", "👍"], &[TAG_STATUS, "open"]],
            )
        };
        let a = req("2026-10-20", "mac", 10);
        let c = req("2026-10-21", "build-box", 20);
        b.apply(&a);
        b.apply(&c);
        let ids: Vec<String> = b.open_requests().iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![c.id.to_hex(), a.id.to_hex()]);
        assert_eq!(b.tiles("2026-10-21")[0].open, 2);
        assert_eq!(
            b.tiles("2026-10-21")[0].health,
            Health::Warn,
            "open request"
        );
        let cid = c.id.to_hex();
        b.apply(&ev(
            &k,
            9,
            "done",
            &[
                &["h", CH],
                &["e", &cid, "", "root"],
                &[TAG_STATUS, "closed", "approve"],
            ],
            30,
        ));
        assert_eq!(b.open_requests().len(), 1);
        // A healthy newer day doesn't turn the tile green while a request
        // from an earlier day is still open.
        b.apply(&note(&k, "2026-10-22", "build-box", 40, &details(1), &[]));
        let t = &b.tiles("2026-10-22")[0];
        assert_eq!((t.open, t.health), (1, Health::Warn));
    }

    /// aei `cmd_poll`'s rows (upgrade/buzz-upgrade.sh on main ac37313): for
    /// a request declaring approve=👍 decline=👎 with one approver, whether
    /// each reaction's content counts as (approve, decline). cmd_poll's jq
    /// `norm` drops U+FE0F and U+1F3FB..U+1F3FF and nothing else; approve
    /// also takes ✅ and +1, decline ❌. The expected column was checked by
    /// running cmd_poll's own jq on these rows.
    #[test]
    fn votes_count_exactly_what_cmd_poll_counts() {
        let rows: &[(&str, bool, bool)] = &[
            ("👍", true, false),
            ("👍🏻", true, false),
            ("👍🏽", true, false),
            ("👍🏿", true, false),
            ("👍\u{FE0F}", true, false),
            ("👍🏽\u{FE0F}", true, false),
            ("✅", true, false),
            ("✅\u{FE0F}", true, false),
            ("+1", true, false),
            ("👎", false, true),
            ("👎🏾", false, true),
            ("👎\u{FE0F}", false, true),
            ("❌", false, true),
            ("❌\u{FE0F}", false, true),
            // Not trimmed, not NIP-25's +, no other lookalikes.
            (" 👍", false, false),
            ("👍\n", false, false),
            (" 👍\n", false, false),
            ("+1 ", false, false),
            ("+", false, false),
            ("", false, false),
            ("-", false, false),
            ("✔️", false, false),
            ("☑️", false, false),
            ("✖️", false, false),
            ("❎", false, false),
            ("👍👍", false, false),
            ("thumbsup", false, false),
            ("🚀", false, false),
        ];
        let author = Keys::generate();
        let lucian = Keys::generate();
        for (i, (content, approve, decline)) in rows.iter().enumerate() {
            let mut b = Board::new(CH);
            let e = note(
                &author,
                "2026-10-21",
                "build-box",
                1,
                &details(1),
                &[
                    &[TAG_ACTION, "approve", "👍"],
                    &[TAG_ACTION, "decline", "👎"],
                    &[TAG_APPROVER, &lucian.public_key().to_hex()],
                    &[TAG_STATUS, "open"],
                ],
            );
            b.apply(&e);
            let id = e.id.to_hex();
            b.apply(&ev(&lucian, 7, content, &[&["e", &id]], 2 + i as u64));
            let n = b.note(&id).unwrap().clone();
            let v = b.votes(&n);
            let got = (!v[0].voters.is_empty(), !v[1].voters.is_empty());
            assert_eq!(got, (*approve, *decline), "row {i}: {content:?}");
        }
    }

    #[test]
    fn a_vote_for_one_action_blocks_the_others() {
        let author = Keys::generate();
        let lucian = Keys::generate();
        let lp = lucian.public_key().to_hex();
        let mut b = Board::new(CH);
        let e = note(
            &author,
            "2026-10-21",
            "build-box",
            1,
            &details(1),
            &[
                &[TAG_ACTION, "approve", "👍"],
                &[TAG_ACTION, "decline", "👎"],
                &[TAG_APPROVER, &lp],
                &[TAG_STATUS, "open"],
            ],
        );
        b.apply(&e);
        let id = e.id.to_hex();
        let n = b.note(&id).unwrap().clone();
        assert!(b.can_vote(&n, 0, &lp).is_ok());
        assert!(b.can_vote(&n, 1, &lp).is_ok());
        // ✅ from another client is an approve: no decline after it.
        let v = ev(&lucian, 7, "✅", &[&["e", &id]], 2);
        b.apply(&v);
        assert_eq!(
            b.can_vote(&n, 1, &lp).unwrap_err(),
            "you already voted approve"
        );
        assert_eq!(
            b.can_vote(&n, 0, &lp).unwrap_err(),
            "you already voted approve"
        );
    }

    #[test]
    fn a_refused_vote_is_forgotten() {
        let author = Keys::generate();
        let lucian = Keys::generate();
        let lp = lucian.public_key().to_hex();
        let mut b = Board::new(CH);
        let e = note(
            &author,
            "2026-10-21",
            "build-box",
            1,
            &details(1),
            &[&[TAG_ACTION, "approve", "👍"], &[TAG_APPROVER, &lp]],
        );
        b.apply(&e);
        let id = e.id.to_hex();
        let v = ev(&lucian, 7, "👍", &[&["e", &id]], 2);
        b.apply(&v);
        let n = b.note(&id).unwrap().clone();
        assert!(b.can_vote(&n, 0, &lp).is_err());
        // Taken back (the relay refused it): the request is free again.
        assert!(b.forget_reaction(&v));
        assert!(!b.forget_reaction(&v));
        assert!(b.can_vote(&n, 0, &lp).is_ok());
        assert!(b.votes(&n)[0].voters.is_empty());
    }

    #[test]
    fn a_newer_note_for_the_day_and_source_supersedes_a_request() {
        let k = Keys::generate();
        let lucian = Keys::generate();
        let lp = lucian.public_key().to_hex();
        let mut b = Board::new(CH);
        let req = |k: &Keys, day: &str, source: &str, at: u64| {
            note(
                k,
                day,
                source,
                at,
                &details(1),
                &[
                    &[TAG_ACTION, "approve", "👍"],
                    &[TAG_APPROVER, &lp],
                    &[TAG_STATUS, "open"],
                ],
            )
        };
        let open_ids =
            |b: &Board| -> Vec<String> { b.open_requests().iter().map(|n| n.id.clone()).collect() };
        let old = req(&k, "2026-10-21", "build-box", 10);
        let new = req(&k, "2026-10-21", "build-box", 20);
        let other_source = req(&k, "2026-10-21", "mac", 6);
        for e in [&old, &new, &other_source] {
            b.apply(e);
        }
        assert_eq!(
            open_ids(&b),
            vec![new.id.to_hex(), other_source.id.to_hex()]
        );
        let o = b.note(&old.id.to_hex()).unwrap().clone();
        let st = b.status(&o);
        assert_eq!(st.status.as_deref(), Some("superseded"));
        assert_eq!(st.by_event, Some(new.id.to_hex()));
        assert!(
            b.can_vote(&o, 0, &lp)
                .unwrap_err()
                .starts_with("superseded")
        );
        assert!(
            b.can_vote(b.note(&new.id.to_hex()).unwrap(), 0, &lp)
                .is_ok()
        );

        // Another author's note for the same day and source changes nothing.
        let stranger = Keys::generate();
        b.apply(&note(
            &stranger,
            "2026-10-21",
            "build-box",
            25,
            &details(1),
            &[],
        ));
        let strangers = req(&stranger, "2026-10-22", "build-box", 26);
        b.apply(&strangers);
        assert!(open_ids(&b).contains(&new.id.to_hex()));

        // A plain report from the source on a later day doesn't supersede it.
        b.apply(&note(&k, "2026-10-22", "build-box", 27, &details(1), &[]));
        assert!(open_ids(&b).contains(&new.id.to_hex()));

        // A newer request from the source on a later day does: the box's
        // 04:00 stage replaces yesterday's offer.
        let next = req(&k, "2026-10-23", "build-box", 30);
        b.apply(&next);
        let n = b.note(&new.id.to_hex()).unwrap().clone();
        assert_eq!(b.status(&n).by_event, Some(next.id.to_hex()));
        assert!(!open_ids(&b).contains(&new.id.to_hex()));

        // A plain report later that day (aei republishes the day's note on
        // each event, a decline or a roll) supersedes the request too.
        let nx = b.note(&next.id.to_hex()).unwrap().clone();
        assert_eq!(b.status(&nx).status.as_deref(), Some("open"));
        b.apply(&note(&k, "2026-10-23", "build-box", 40, &details(1), &[]));
        assert_eq!(b.status(&nx).status.as_deref(), Some("superseded"));
        assert_eq!(
            open_ids(&b),
            vec![strangers.id.to_hex(), other_source.id.to_hex()]
        );

        // An explicit close still reads as closed, not superseded.
        b.apply(&ev(
            &k,
            9,
            "done",
            &[
                &["h", CH],
                &["e", &o.id, "", "root"],
                &[TAG_STATUS, "closed", "decline"],
            ],
            50,
        ));
        assert_eq!(b.status(&o).status.as_deref(), Some("closed"));
    }

    #[test]
    fn delta_tones_never_green_for_a_flag() {
        assert_eq!(delta_tone(55100.0, Some(75200.0), None), Tone::Better);
        assert_eq!(delta_tone(42.0, Some(41.0), None), Tone::Plain);
        assert_eq!(delta_tone(767000.0, Some(612000.0), None), Tone::Flag);
        assert_eq!(delta_tone(1000.0, Some(600.0), None), Tone::Page);
        // The report flagged it: amber though it is lower.
        assert_eq!(delta_tone(161000.0, Some(250000.0), Some(true)), Tone::Flag);
        assert_eq!(
            delta_tone(240000.0, Some(250000.0), Some(false)),
            Tone::Better
        );
        assert_eq!(delta_tone(5.0, None, None), Tone::Plain);
        assert_eq!(delta_label(767000.0, 612000.0, Unit::Tokens), "+25%");
        assert_eq!(delta_label(42.0, 41.0, Unit::Pct), "+1pt");
        assert_eq!(delta_label(161000.0, 250000.0, Unit::Tokens), "-36%");
    }

    #[test]
    fn hooks_gauges_from_the_seed_shape() {
        let k = Keys::generate();
        let mut b = Board::new(CH);
        b.apply(&note(
            &k,
            "2026-10-20",
            "build-box",
            1,
            &details(700000),
            &[],
        ));
        b.apply(&note(
            &k,
            "2026-10-21",
            "build-box",
            1,
            &details(767000),
            &[],
        ));
        let cur = b.newest_hooks().unwrap().clone();
        assert_eq!(cur.day.as_deref(), Some("2026-10-21"));
        let g = hooks_gauges(&b, &cur);
        let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "peak subagent",
                "peak main",
                "spend >300k",
                "ramp after handoff",
                "cost / useful call",
                "builder reset"
            ]
        );
        assert_eq!(g[0].value, 767000.0);
        assert_eq!(g[0].baseline, Some(612000.0));
        assert_eq!(g[0].tone, Tone::Flag);
        assert_eq!(g[0].spark[12..], [Some(700000.0), Some(767000.0)]);
        assert_eq!(g[4].tone, Tone::Better);
        assert_eq!(g[5].baseline, Some(250000.0));
        assert_eq!(g[5].tone, Tone::Flag);
        assert_eq!(g[5].spark[13], Some(161000.0));
    }

    #[test]
    fn day_ranges() {
        assert_eq!(day_range("2026-10-30", "2026-11-02").len(), 4);
        assert_eq!(
            day_range("2026-10-21", "2026-10-21"),
            vec!["2026-10-21".to_string()]
        );
        assert!(day_range("2026-10-22", "2026-10-21").is_empty());
        assert!(day_range("bad", "2026-10-21").is_empty());
    }
}
