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
use serde_json::Value;

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

/// A reaction as a vote: no U+FE0F, no skin tones, and empty is `+` (NIP-25).
pub fn normalize_reaction(content: &str) -> String {
    let s: String = content
        .trim()
        .chars()
        .filter(|c| *c != '\u{FE0F}' && !('\u{1F3FB}'..='\u{1F3FF}').contains(c))
        .collect();
    if s.is_empty() { "+".into() } else { s }
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
    /// The reply that set it; None = the note's own tag.
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

    /// Who voted for each declared action: kind-7s whose emoji matches,
    /// from approvers only when the note names any. One vote per pubkey.
    pub fn votes(&self, note: &Note) -> Vec<Tally> {
        let rs = self.reactions.get(&note.id);
        note.actions
            .iter()
            .map(|a| {
                let want = normalize_reaction(&a.emoji);
                let voters: BTreeSet<String> = rs
                    .into_iter()
                    .flat_map(|m| m.values())
                    .filter(|r| r.emoji == want)
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

    pub fn reply_count(&self, note: &Note) -> usize {
        self.replies.get(&note.id).map_or(0, Vec::len)
    }

    /// The newest `control-status` reply by the author or an approver wins
    /// over the note's own tag.
    pub fn status(&self, note: &Note) -> Status {
        let newest = self
            .replies
            .get(&note.id)
            .into_iter()
            .flatten()
            .filter(|r| r.by == note.author || note.approvers.contains(&r.by))
            .filter(|r| r.status.is_some())
            .max_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
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
        let Some(a) = note.actions.get(action) else {
            return Err(if note.actions.is_empty() {
                "not a request: this note declares no actions".into()
            } else {
                format!("this request has {} actions", note.actions.len())
            });
        };
        let st = self.status(note);
        if st.status.as_deref() == Some("closed") {
            let o = st.outcome.map(|o| format!(" ({o})")).unwrap_or_default();
            return Err(format!("the request is closed{o}"));
        }
        let me = me.to_ascii_lowercase();
        if !note.approvers.is_empty() && !note.approvers.contains(&me) {
            return Err("only the note's approvers can vote on it".into());
        }
        if self
            .votes(note)
            .iter()
            .any(|t| t.action == *a && t.voters.contains(&me))
        {
            return Err(format!("you already voted {}", a.name));
        }
        Ok(())
    }
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
        assert_eq!(v[2].voters, vec![lp.clone()], "empty reaction is +");
        assert_eq!(b.reaction_counts(&n)["👍"], 3);
        // Who may vote, and why not.
        assert!(b.can_vote(&n, 1, &lp).is_ok());
        assert_eq!(
            b.can_vote(&n, 0, &lp).unwrap_err(),
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
