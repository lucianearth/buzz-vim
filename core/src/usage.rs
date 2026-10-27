//! Claude subscription usage: the 5-hour and weekly windows (aei F1).
//!
//! The box polls each Claude subscription and publishes one NIP-78 record per
//! subscription: kind 30078, `d` = `claude-usage/<person>`, signed by its
//! poster key (the same key as `buzz-subscription/people`). Content:
//!
//! ```text
//! {"v":1, "person", "label", "checked_at", "status", "resets_at",
//!  "five_hour": {"utilization", "resets_at", "status"},
//!  "seven_day": {"utilization", "resets_at", "status"},
//!  "agents": [<seat pubkey>], "error": null | "<why the probe failed>"}
//! ```
//!
//! `utilization` is a fraction (1 = at the limit), `resets_at` unix seconds,
//! `status` allowed | allowed_warning | rejected: the
//! `anthropic-ratelimit-unified-{5h,7d}-*` headers, the numbers Claude Code's
//! `/usage` shows. Only the publisher's records count, newest per `d` wins
//! (created_at, then the lower id), and an event whose signature does not
//! verify is ignored. The relay can't filter on a `d` prefix, so the fetch
//! is by author and the prefix is matched here. A port of Buzz Desktop's
//! `claudeUsage.ts` (aei desktop patch 80).

use std::collections::BTreeMap;

use nostr::Event;
use serde_json::{Value, json};

use crate::model::tag;
use crate::subscription::{KIND, MAX_SKEW, is_person};

pub const D_PREFIX: &str = "claude-usage/";
/// A reading older than this is stale (the poller runs every 5 minutes).
pub const STALE_SECS: u64 = 20 * 60;
/// At or past this percent a bar turns amber.
pub const WARN_PERCENT: u8 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Allowed,
    AllowedWarning,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    pub utilization: Option<f64>,
    pub resets_at: Option<u64>,
    pub status: Option<Status>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub person: String,
    pub label: String,
    pub checked_at: u64,
    pub five_hour: Window,
    pub seven_day: Window,
    pub error: Option<String>,
}

/// What a bar shows at `now`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// 0-100, rounded; None when the reading has no number.
    pub percent: Option<u8>,
    pub at_limit: bool,
    /// Seconds until the window resets, while a reset is pending.
    pub resets_in: Option<u64>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn status(v: Option<&Value>) -> Option<Status> {
    match v?.as_str()? {
        "allowed" => Some(Status::Allowed),
        "allowed_warning" => Some(Status::AllowedWarning),
        "rejected" => Some(Status::Rejected),
        _ => None,
    }
}

fn window(v: Option<&Value>) -> Window {
    let Some(w) = v.and_then(Value::as_object) else {
        return Window::default();
    };
    Window {
        utilization: w
            .get("utilization")
            .and_then(Value::as_f64)
            .filter(|u| u.is_finite())
            .map(|u| u.max(0.0)),
        resets_at: w
            .get("resets_at")
            .and_then(Value::as_f64)
            .filter(|r| r.is_finite() && *r > 0.0)
            .map(|r| r.floor() as u64),
        status: status(w.get("status")),
    }
}

/// One event as a usage record, or None when it is not a well-formed one.
/// No trust or signature check: [`Usage::apply`] does those.
pub fn parse(ev: &Event) -> Option<Record> {
    if ev.kind.as_u16() != KIND {
        return None;
    }
    let person = tag(ev, "d")?.strip_prefix(D_PREFIX)?;
    if !is_person(person) {
        return None;
    }
    let Ok(Value::Object(c)) = serde_json::from_str::<Value>(&ev.content) else {
        return None;
    };
    if c.get("person").and_then(Value::as_str) != Some(person) {
        return None;
    }
    let label = c
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(person);
    Some(Record {
        person: person.to_string(),
        label: label.to_string(),
        checked_at: c
            .get("checked_at")
            .and_then(Value::as_f64)
            .filter(|t| t.is_finite() && *t >= 0.0)
            .map_or(ev.created_at.as_secs(), |t| t as u64),
        five_hour: window(c.get("five_hour")),
        seven_day: window(c.get("seven_day")),
        error: c
            .get("error")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
            .map(str::to_string),
    })
}

/// A window at `now`. A window whose reset has passed has started over and
/// shows empty. `rejected` decides the limit: with extra usage a session runs
/// on past 100% while the status says allowed, so 100% counts only when the
/// reading has no status.
pub fn view(w: &Window, now: u64) -> View {
    let pending = w.resets_at.is_some_and(|r| r > now);
    let percent = w.utilization.map(|u| (u * 100.0).round().min(100.0) as u8);
    let at_limit = pending
        && (w.status == Some(Status::Rejected)
            || (w.status.is_none() && w.utilization.is_some_and(|u| u >= 1.0)));
    View {
        percent: if w.resets_at.is_some() && !pending {
            Some(0)
        } else {
            percent
        },
        at_limit,
        resets_in: w.resets_at.filter(|_| pending).map(|r| r - now),
    }
}

pub fn is_stale(r: &Record, now: u64) -> bool {
    now.saturating_sub(r.checked_at) > STALE_SECS
}

/// `2d 4h`, `3h 5m`, `12m` (at least `1m`).
pub fn duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h, m) {
        (d, 0, _) if d > 0 => format!("{d}d"),
        (d, h, _) if d > 0 => format!("{d}d {h}h"),
        (_, h, 0) if h > 0 => format!("{h}h"),
        (_, h, m) if h > 0 => format!("{h}h {m}m"),
        (_, _, m) => format!("{}m", m.max(1)),
    }
}

#[derive(Clone, Debug)]
struct Rec {
    at: u64,
    id: String,
    /// None: the newest record for this `d` is malformed.
    value: Option<Record>,
}

/// The newest record per subscription from the trusted publishers.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub publishers: Vec<String>,
    by_d: BTreeMap<String, Rec>,
}

impl Usage {
    pub fn new(publishers: Vec<String>) -> Self {
        Usage {
            publishers,
            by_d: BTreeMap::new(),
        }
    }

    /// Every 30078 the publishers sign: they sign few (these and the people
    /// list), and usage records are rewritten every few minutes, so they are
    /// always among the newest.
    pub fn filters(&self) -> Vec<Value> {
        vec![json!({"kinds": [KIND], "authors": self.publishers, "limit": 200})]
    }

    /// Kind 30078 with a `claude-usage/` `d`, before trust.
    pub fn is_record(ev: &Event) -> bool {
        ev.kind.as_u16() == KIND && tag(ev, "d").is_some_and(|d| d.starts_with(D_PREFIX))
    }

    /// Fold one event from the relay. True when it is now a record shown.
    pub fn apply(&mut self, ev: &Event) -> bool {
        if !Self::is_record(ev) || ev.created_at.as_secs() > unix_now() + MAX_SKEW {
            return false;
        }
        if !self.publishers.contains(&ev.pubkey.to_hex()) || ev.verify().is_err() {
            return false;
        }
        let Some(d) = tag(ev, "d") else {
            return false;
        };
        let (at, id) = (ev.created_at.as_secs(), ev.id.to_hex());
        if let Some(r) = self.by_d.get(d) {
            if r.id == id
                || (at, std::cmp::Reverse(id.as_str())) <= (r.at, std::cmp::Reverse(r.id.as_str()))
            {
                return false;
            }
        }
        // The newest record decides, well formed or not (Desktop's rule): a
        // malformed newer record hides an older reading, whatever order the
        // relay sends them in.
        let value = parse(ev);
        let shown = value.is_some();
        self.by_d.insert(d.to_string(), Rec { at, id, value });
        shown
    }

    /// The records, by label then person.
    pub fn records(&self) -> Vec<&Record> {
        let mut out: Vec<&Record> = self
            .by_d
            .values()
            .filter_map(|r| r.value.as_ref())
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.person.cmp(&b.person)));
        out
    }

    pub fn for_person(&self, person: &str) -> Option<&Record> {
        self.by_d
            .get(&format!("{D_PREFIX}{person}"))
            .and_then(|r| r.value.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

    fn rec(keys: &Keys, d: &str, content: Value, at: u64) -> Event {
        EventBuilder::new(Kind::from(KIND), content.to_string())
            .tags([Tag::parse(["d", d]).unwrap()])
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn body(person: &str, five: f64, week: f64, at: u64) -> Value {
        json!({
            "v": 1, "person": person, "label": format!("{person} max"), "checked_at": at,
            "five_hour": {"utilization": five, "resets_at": at + 3600, "status": "allowed"},
            "seven_day": {"utilization": week, "resets_at": at + 3 * 86400, "status": "allowed"},
            "agents": [], "error": null
        })
    }

    #[test]
    fn newest_trusted_record_per_subscription_wins() {
        let (publ, other) = (Keys::generate(), Keys::generate());
        let mut u = Usage::new(vec![publ.public_key().to_hex()]);
        assert!(u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("lucian", 0.2, 0.5, 100),
            100
        )));
        assert!(u.apply(&rec(
            &publ,
            "claude-usage/aaron",
            body("aaron", 0.9, 0.1, 100),
            100
        )));
        // Another signer with the same d does not count.
        assert!(!u.apply(&rec(
            &other,
            "claude-usage/lucian",
            body("lucian", 1.0, 1.0, 200),
            200
        )));
        // An older record does not replace a newer one.
        assert!(!u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("lucian", 0.7, 0.7, 50),
            50
        )));
        assert!(u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("lucian", 0.3, 0.6, 150),
            150
        )));
        let rs = u.records();
        assert_eq!(
            rs.iter().map(|r| r.person.as_str()).collect::<Vec<_>>(),
            ["aaron", "lucian"]
        );
        assert_eq!(rs[1].five_hour.utilization, Some(0.3));
        assert_eq!(rs[1].label, "lucian max");
        assert_eq!(
            u.for_person("aaron").unwrap().seven_day.utilization,
            Some(0.1)
        );
        // The people list from the same key is not a usage record.
        assert!(!Usage::is_record(&rec(
            &publ,
            "buzz-subscription/people",
            json!({}),
            1
        )));
    }

    #[test]
    fn malformed_records_are_dropped() {
        let publ = Keys::generate();
        let mut u = Usage::new(vec![publ.public_key().to_hex()]);
        // person in the content must match the d tag.
        assert!(!u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("aaron", 0.1, 0.1, 1),
            1
        )));
        assert!(!u.apply(&rec(
            &publ,
            "claude-usage/Bad_Name",
            body("Bad_Name", 0.1, 0.1, 1),
            1
        )));
        let ev = EventBuilder::new(Kind::from(KIND), "not json")
            .tags([Tag::parse(["d", "claude-usage/x"]).unwrap()])
            .sign_with_keys(&publ)
            .unwrap();
        assert!(!u.apply(&ev));
        assert!(u.records().is_empty());
        // A newer malformed record hides the older reading.
        assert!(u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("lucian", 0.1, 0.1, 10),
            10
        )));
        assert!(!u.apply(&rec(&publ, "claude-usage/lucian", json!({"person": 3}), 20)));
        assert!(u.records().is_empty());
        // ...also when the older one arrives after it.
        assert!(!u.apply(&rec(
            &publ,
            "claude-usage/lucian",
            body("lucian", 0.1, 0.1, 15),
            15
        )));
        assert!(u.records().is_empty());
    }

    #[test]
    fn rejected_decides_the_limit_and_a_passed_reset_starts_over() {
        let now = 1_000;
        let w = |u: f64, st: Option<Status>, reset: u64| Window {
            utilization: Some(u),
            resets_at: Some(reset),
            status: st,
        };
        let v = view(&w(0.42, Some(Status::Allowed), now + 7_500), now);
        assert_eq!(
            (v.percent, v.at_limit, v.resets_in),
            (Some(42), false, Some(7_500))
        );
        // Extra usage: past 100% but allowed is not the limit.
        let v = view(&w(1.3, Some(Status::Allowed), now + 60), now);
        assert_eq!((v.percent, v.at_limit), (Some(100), false));
        assert!(view(&w(0.5, Some(Status::Rejected), now + 60), now).at_limit);
        // No status: 100% is the limit.
        assert!(view(&w(1.0, None, now + 60), now).at_limit);
        // The reset passed: empty, no limit, no countdown.
        let v = view(&w(1.0, Some(Status::Rejected), now - 1), now);
        assert_eq!((v.percent, v.at_limit, v.resets_in), (Some(0), false, None));
        assert_eq!(view(&Window::default(), now).percent, None);
    }

    #[test]
    fn durations_and_staleness() {
        assert_eq!(duration(30), "1m");
        assert_eq!(duration(5 * 60), "5m");
        assert_eq!(duration(2 * 3600), "2h");
        assert_eq!(duration(2 * 3600 + 5 * 60), "2h 5m");
        assert_eq!(duration(3 * 86400 + 4 * 3600), "3d 4h");
        assert_eq!(duration(86400), "1d");
        let r = parse(&rec(
            &Keys::generate(),
            "claude-usage/a",
            body("a", 0.1, 0.1, 100),
            100,
        ))
        .unwrap();
        assert!(!is_stale(&r, 100 + STALE_SECS));
        assert!(is_stale(&r, 101 + STALE_SECS));
    }
}
