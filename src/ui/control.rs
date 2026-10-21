//! The Control view (`C`): #control's notes, styled as an ops cockpit
//! (lucian's pick B, 2026-10-21). The overview is a strip of open requests
//! with their vote keys, a tile per topic (status color, one-liner,
//! 14-day sparkline of its key metric) and the newest hooks note's gauges
//! (value, baseline, delta). Behind a tile: the topic's days, and one note: its summary, images, request (actions,
//! votes, status), and its details. hooks/1 details get trend charts with
//! the baseline as a reference line and the seat / agent-type tables; any
//! other schema shows its JSON.
//!
//! A note is taller than the screen, so it is drawn into a buffer of its
//! full height and the rows on screen are copied out (charts are widgets,
//! not lines).

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Borders, Chart, Dataset, GraphType, Paragraph, Widget,
};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::{
    THUMB_COLS, THUMB_MAX_ROWS, breadcrumb, clip, draw_empty, pad_to, render_markdown_links,
    time_label,
};
use crate::app::App;
use crate::control::{self, Gauge, Health, Note, Tile, Tone, Unit};
use crate::media::Slot;
use crate::theme;

const CHART_ROWS: u16 = 10;
/// The tallest a note is drawn; past it, a line says it was cut.
const MAX_ROWS: u16 = 4000;
/// OKLab hues for a status: ok green, partial amber, dead red.
const OK: f32 = 145.0;
const PARTIAL: f32 = 75.0;
const DEAD: f32 = 25.0;
/// OKLab hue for data that carries no status: cyan.
const DATA: f32 = 215.0;

fn status_style(s: &str) -> Style {
    let hue = match s {
        "ok" => OK,
        "dead" => DEAD,
        _ => PARTIAL,
    };
    Style::new()
        .fg(theme::gray(1))
        .bg(theme::ink(hue, 17))
        .add_modifier(Modifier::BOLD)
}

fn series_style(i: usize) -> Style {
    Style::new().fg(theme::ink(theme::nth(i * 2 + 1), 19))
}

fn value_label(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Tokens => control::short_tokens(v),
        Unit::Pct => format!("{v:.0}%"),
        Unit::Count if v.fract() == 0.0 => format!("{v:.0}"),
        Unit::Count => format!("{v:.1}"),
    }
}

/// Every chart's y labels take this many columns, right-aligned, so the
/// plot areas of charts in one column line up (ratatui sizes the axis
/// gutter from the widest label). Fits "157.1M".
const Y_LABEL_W: usize = 6;
/// A topic tile is at least this wide; fewer per row on a narrow screen.
const TILE_MIN: u16 = 25;
const TILE_ROWS: u16 = 5;
/// A gauge tile is at least this wide.
const GAUGE_MIN: u16 = 38;
const GAUGE_ROWS: u16 = 5;

/// A status color: ok green, warn amber, page red, no note dim.
pub(super) fn health_color(h: Health) -> Color {
    match h {
        Health::Ok => theme::ink(OK, 19),
        Health::Warn => theme::ink(PARTIAL, 19),
        Health::Page => theme::ink(DEAD, 19),
        Health::NoNote => theme::gray(9),
    }
}

fn health_glyph(h: Health) -> &'static str {
    match h {
        Health::Ok => "✓",
        Health::Warn => "▲",
        Health::Page => "✗",
        Health::NoNote => "·",
    }
}

/// A delta's color: green better, amber a flag, red a page.
pub(super) fn tone_style(t: Tone) -> Style {
    match t {
        Tone::Better => Style::new().fg(theme::ink(OK, 19)),
        Tone::Flag => Style::new().fg(theme::ink(PARTIAL, 19)),
        Tone::Page => Style::new().fg(theme::ink(DEAD, 19)),
        Tone::Plain => theme::text(),
    }
    .add_modifier(Modifier::BOLD)
}

/// `s` cut at a word so it fits `w` columns: a number is never cut.
fn fit_words(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for word in s.split(' ') {
        let next = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if next.width() + 1 > w {
            break;
        }
        out = next;
    }
    if out.is_empty() {
        return clip(s, w);
    }
    out + "…"
}

/// A block-character sparkline, `cell` columns per day; a day without a
/// value is a dim dot.
fn sparkline(vals: &[Option<f64>], cell: usize, style: Style) -> Vec<Span<'static>> {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = vals.iter().flatten().fold(0.0f64, |a, b| a.max(*b));
    vals.iter()
        .map(|v| match v {
            Some(v) => {
                let i = if max <= 0.0 {
                    0
                } else {
                    ((v / max) * 7.0).round().clamp(0.0, 7.0) as usize
                };
                Span::styled(BARS[i].to_string().repeat(cell), style)
            }
            None => Span::styled(format!("{:<cell$}", "·"), theme::faint()),
        })
        .collect()
}

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Lay `n` tiles of at least `min` columns across `w`: (per row, width).
fn grid(n: usize, w: u16, min: u16) -> (u16, u16) {
    let per = (w / min).max(1).min(n.max(1) as u16);
    (per, w / per)
}

fn tile_block(title: Line<'static>, color: Color, picked: bool) -> Block<'static> {
    let b = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color))
        .title(title);
    if picked {
        b.border_type(BorderType::Thick)
    } else {
        b
    }
}

/// One topic's tile: the status in the border, a one-liner, the sparkline
/// with its newest value, and the day and sources.
fn render_tile(t: &Tile, picked: bool, r: Rect, buf: &mut Buffer) {
    let color = health_color(t.health);
    let mut title = vec![Span::styled(
        format!(" {} {} ", health_glyph(t.health), t.topic),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    if t.open > 0 {
        title.push(Span::styled(format!(" {} open ", t.open), theme::badge()));
    }
    let block = tile_block(Line::from(title), color, picked);
    let inner = block.inner(r);
    block.render(r, buf);
    let w = inner.width as usize;
    let line_style = if t.health == Health::NoNote {
        theme::faint()
    } else {
        Style::new().fg(color)
    };
    let mut lines = vec![Line::styled(fit_words(&t.line, w), line_style)];
    let mut spark = sparkline(&t.spark, 1, Style::new().fg(color));
    if let Some(v) = t.spark.iter().rev().flatten().next() {
        let v = value_label(*v, t.metric.unit);
        let label = format!(" {}", t.metric.label);
        if width_of(&spark) + 1 + v.width() <= w {
            spark.push(Span::styled(format!(" {v}"), theme::bright()));
            if width_of(&spark) + label.width() <= w {
                spark.push(Span::styled(label, theme::faint()));
            }
        }
    }
    lines.push(Line::from(spark));
    let day = t
        .latest_day
        .as_deref()
        .map(|d| format!("{} · {}", d.get(5..).unwrap_or(d), t.sources.join(", ")))
        .unwrap_or_else(|| "no note yet".into());
    lines.push(Line::styled(fit_words(&day, w), theme::faint()));
    Paragraph::new(lines).render(inner, buf);
}

/// A hooks gauge: value vs baseline with the delta at the right, a bar
/// with the baseline ticked, and the last 14 days.
fn render_gauge(g: &Gauge, r: Rect, buf: &mut Buffer) {
    let tone = tone_style(g.tone);
    // A plain delta draws its bar in the data cyan, never a status color.
    let bar_color = match g.tone {
        Tone::Plain => Style::new().fg(theme::ink(DATA, 19)),
        _ => tone,
    };
    let block = tile_block(
        Line::styled(
            format!(" {} ", g.title.to_uppercase()),
            bar_color.add_modifier(Modifier::BOLD),
        ),
        theme::gray(7),
        false,
    );
    let inner = block.inner(r);
    block.render(r, buf);
    let w = inner.width as usize;
    // Value and delta always; then the baseline and aside while they fit.
    let delta = g
        .baseline
        .map(|b| control::delta_label(g.value, b, g.unit))
        .unwrap_or_default();
    let mut left = vec![Span::styled(value_label(g.value, g.unit), theme::bright())];
    let room = |l: &[Span], extra: &str| width_of(l) + extra.width() + 1 + delta.width() <= w;
    if let Some(b) = g.baseline {
        let vs = format!("  vs {}", value_label(b, g.unit));
        if room(&left, &vs) {
            left.push(Span::styled(vs, theme::muted()));
        }
    }
    if let Some(a) = &g.aside {
        let a = format!("  {a}");
        if room(&left, &a) {
            left.push(Span::styled(a, theme::faint()));
        }
    }
    let gap = w.saturating_sub(width_of(&left) + delta.width());
    left.push(Span::raw(" ".repeat(gap)));
    left.push(Span::styled(delta, tone));
    // The bar: 0 to 1.25 x the larger of value and baseline.
    let top = g
        .value
        .max(g.baseline.unwrap_or(0.0))
        .max(f64::MIN_POSITIVE)
        * 1.25;
    let cell = |v: f64| ((v / top) * w as f64).round().clamp(0.0, w as f64) as usize;
    let fill = cell(g.value);
    let tick = g.baseline.map(|b| cell(b).min(w.saturating_sub(1)));
    let bar: Vec<Span> = (0..w)
        .map(|i| match (i < fill, Some(i) == tick) {
            (_, true) => Span::styled("┃", theme::bright()),
            (true, _) => Span::styled("█", bar_color),
            (false, _) => Span::styled("░", theme::faint()),
        })
        .collect();
    let cellw = (w / g.spark.len().max(1)).max(1);
    let spark = sparkline(&g.spark, cellw, bar_color);
    Paragraph::new(vec![Line::from(left), Line::from(bar), Line::from(spark)]).render(inner, buf);
}

/// Gauges in a grid of at least GAUGE_MIN columns each: the rows that fit
/// in `max_rows` lines, and how many lines they took.
fn render_gauges(gs: &[Gauge], area: Rect, max_rows: u16, buf: &mut Buffer) -> u16 {
    let (per, gw) = grid(gs.len(), area.width, GAUGE_MIN);
    let rows = (gs.len() as u16).div_ceil(per).min(max_rows / GAUGE_ROWS);
    for (i, g) in gs.iter().enumerate() {
        let (row, col) = (i as u16 / per, i as u16 % per);
        if row >= rows {
            break;
        }
        let r = Rect::new(area.x + col * gw, area.y + row * GAUGE_ROWS, gw, GAUGE_ROWS);
        render_gauge(g, r, buf);
    }
    rows * GAUGE_ROWS
}

/// "2h", "3d": how long ago `at` was.
fn age(at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let s = now.saturating_sub(at);
    match s {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The open requests: two lines each (what, then its vote keys, votes,
/// status, approvers and age), dropping the tail parts that don't fit.
fn request_strip(app: &App, ids: &[String], all: usize, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    for (i, id) in ids.iter().enumerate() {
        let Some(n) = app.control.note(id) else {
            continue;
        };
        let picked = app.control_cursor == i;
        let mark = if picked {
            Span::styled("▌", theme::bright())
        } else {
            Span::raw(" ")
        };
        let mut head = vec![
            mark.clone(),
            Span::styled(n.topic.clone(), theme::bright()),
            Span::styled(
                format!(
                    "  {}  {}  ",
                    n.day.as_deref().unwrap_or("?"),
                    n.source.as_deref().unwrap_or("?")
                ),
                theme::muted(),
            ),
        ];
        let rest = w.saturating_sub(width_of(&head));
        head.push(Span::styled(
            fit_words(&control::one_liner(&n.summary), rest),
            theme::text(),
        ));
        let mut keys = vec![mark, Span::raw(" ")];
        for (k, t) in app.control.votes(n).iter().enumerate() {
            let votes = t.voters.len();
            keys.push(Span::styled(format!(" {} ", k + 1), theme::key()));
            keys.push(Span::styled(
                format!(" {} {} ", t.action.name, t.action.emoji),
                theme::tab_idle(),
            ));
            keys.push(Span::styled(format!(" {votes}  "), theme::text()));
        }
        let st = app.control.status(n).status.unwrap_or_else(|| "?".into());
        let mut tail = vec![Span::styled(
            format!(" {st} "),
            status_style(if st == "open" { "partial" } else { "ok" }),
        )];
        if !n.approvers.is_empty() {
            let names: Vec<String> = n.approvers.iter().map(|p| app.store.name_of(p)).collect();
            tail.push(Span::styled(
                format!("  approver {}", names.join(", ")),
                theme::muted(),
            ));
        }
        tail.push(Span::styled(
            format!("  age {}", age(n.created_at)),
            theme::faint(),
        ));
        for s in tail {
            if width_of(&keys) + s.content.width() <= w {
                keys.push(s);
            }
        }
        let style = if picked {
            Style::new().bg(theme::gray(3))
        } else {
            Style::new()
        };
        out.push(Line::from(head).style(style));
        out.push(Line::from(keys).style(style));
    }
    if all > ids.len() {
        out.push(Line::styled(
            format!(" +{} more open: on their topic tiles", all - ids.len()),
            theme::faint(),
        ));
    }
    out
}

/// The newest hooks note's seats as dots, and its day and window.
fn seats_line(n: &Note, w: usize) -> Line<'static> {
    let d = n.details.as_ref().ok();
    let mut spans = vec![Span::styled(" seats ", theme::muted())];
    for s in d.and_then(|d| d["seats"].as_array()).into_iter().flatten() {
        let h = match s["status"].as_str() {
            Some("ok") => Health::Ok,
            Some("dead") => Health::Page,
            _ => Health::NoNote,
        };
        spans.push(Span::styled(
            format!(" {} ", text(&s["seat"])),
            theme::text(),
        ));
        spans.push(Span::styled("●", Style::new().fg(health_color(h))));
    }
    let tail = [
        format!(
            "   │ hooks {} {}",
            n.day.as_deref().and_then(|d| d.get(5..)).unwrap_or("?"),
            n.source.as_deref().unwrap_or("?")
        ),
        d.map(|d| {
            format!(
                "   │ window {} .. {}",
                text(&d["window_since"]),
                text(&d["window_until"])
            )
        })
        .unwrap_or_default(),
    ];
    for t in tail {
        if width_of(&spans) + t.width() <= w {
            spans.push(Span::styled(t, theme::faint()));
        }
    }
    Line::from(spans)
}

/// Draw the view. Images on a note go into `images`, as in a timeline.
pub(super) fn draw_control(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    images: &mut Vec<(Rect, (String, u16, u16))>,
) {
    let topic = app.control_topic.clone();
    let note = app.control_open_note().cloned();
    let leaf = match (&topic, &note) {
        (_, Some(n)) => format!(
            "{} › {} {}",
            n.topic,
            n.day.as_deref().unwrap_or("?"),
            n.source.as_deref().unwrap_or("?")
        ),
        (Some(t), None) => t.clone(),
        (None, None) => "overview".into(),
    };
    let keys = match (&topic, &note) {
        (_, Some(n)) if n.is_request() => "j k scroll  1-9 vote  o images  r reload",
        (_, Some(_)) => "j k scroll  o images  r reload",
        (None, None) => "j k ⏎ open  1-9 vote  r reload",
        _ => "j k ⏎ pick  r reload",
    };
    let area = breadcrumb(f, app, area, "control", &leaf, Some(keys.to_string()));
    if !app.control.loaded {
        return draw_empty(f, app, area, "loading #control…");
    }
    if let Some(e) = app.control.error.clone() {
        return draw_empty(f, app, area, &format!("couldn't read #control: {e}"));
    }
    match (topic, note) {
        (_, Some(n)) => draw_note(f, app, area, &n, images),
        (Some(t), None) => draw_days(f, app, area, &t),
        (None, None) => draw_overview(f, app, area),
    }
}

fn list_line(cells: Vec<Span<'static>>, picked: bool) -> Line<'static> {
    let edge = if picked {
        Span::styled("▌ ", theme::bright())
    } else {
        Span::styled("  ", theme::rule())
    };
    let mut spans = vec![edge];
    spans.extend(cells);
    let l = Line::from(spans);
    if picked {
        l.style(Style::new().bg(theme::gray(3)))
    } else {
        l
    }
}

/// The overview: open requests on top, a tile per topic, then the newest
/// hooks note's gauges and seats, each part while it fits.
fn draw_overview(f: &mut Frame, app: &mut App, area: Rect) {
    let (reqs, topics) = app.control_overview();
    let tiles = app.control.tiles(&app.control_day());
    let rows = reqs.len() + topics.len();
    app.control_cursor = app.control_cursor.min(rows.saturating_sub(1));
    let body = pad(area);
    let (w, bottom) = (body.width, body.bottom());
    let mut y = body.y;
    let buf = f.buffer_mut();
    // The requests strip.
    let all = app.control.open_requests().len();
    if reqs.is_empty() {
        let msg = if app.control.note_ids().is_empty() {
            format!(
                " no control notes in the last {} days. producers write them with buzz control publish",
                control::DAYS_BACK
            )
        } else {
            " no open requests".into()
        };
        if y < bottom {
            buf.set_line(body.x, y, &Line::styled(msg, theme::faint()), w);
        }
        y += 2;
    } else {
        let amber = health_color(Health::Warn);
        let block = tile_block(
            Line::styled(
                format!(" ▲ OPEN REQUESTS {all} "),
                Style::new().fg(amber).add_modifier(Modifier::BOLD),
            ),
            amber,
            false,
        );
        let lines = request_strip(app, &reqs, all, w.saturating_sub(2) as usize);
        let h = (lines.len() as u16 + 2).min(bottom.saturating_sub(y));
        let r = Rect::new(body.x, y, w, h);
        let inner = block.inner(r);
        block.render(r, buf);
        Paragraph::new(lines).render(inner, buf);
        y += h;
    }
    // A tile per topic.
    let (per, tw) = grid(tiles.len(), w, TILE_MIN);
    for (i, t) in tiles.iter().enumerate() {
        let (row, col) = (i as u16 / per, i as u16 % per);
        let ty = y + row * TILE_ROWS;
        if ty + TILE_ROWS > bottom {
            break;
        }
        let picked = app.control_cursor == reqs.len() + i;
        render_tile(
            t,
            picked,
            Rect::new(body.x + col * tw, ty, tw, TILE_ROWS),
            buf,
        );
    }
    y += (tiles.len() as u16).div_ceil(per) * TILE_ROWS;
    // The newest hooks note's gauges, then its seats.
    let Some(h) = app.control.newest_hooks().cloned() else {
        return;
    };
    if y + 1 + GAUGE_ROWS > bottom {
        return;
    }
    let gauges = control::hooks_gauges(&app.control, &h);
    let room = bottom.saturating_sub(y + 1);
    let used = render_gauges(&gauges, Rect::new(body.x, y, w, room), room, buf);
    y += used;
    if y < bottom {
        let l = seats_line(&h, w as usize);
        f.buffer_mut().set_line(body.x, y, &l, w);
    }
}

fn pad(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    }
}

fn draw_days(f: &mut Frame, app: &mut App, area: Rect, topic: &str) {
    let notes: Vec<Note> = app.control.days(topic).into_iter().cloned().collect();
    if notes.is_empty() {
        return draw_empty(f, app, area, "no notes for this topic");
    }
    app.control_cursor = app.control_cursor.min(notes.len() - 1);
    let sw = notes
        .iter()
        .map(|n| n.source.as_deref().unwrap_or("").width())
        .max()
        .unwrap_or(6)
        .max(6);
    let width = area.width.saturating_sub(4) as usize;
    let mut lines = vec![Line::styled(
        format!("  day         {}  status    summary", pad_to("source", sw)),
        theme::faint(),
    )];
    for (i, n) in notes.iter().enumerate() {
        let st = note_status(app, n);
        let chip = match st.as_deref() {
            Some(s) => Span::styled(format!(" {s} "), status_style(s)),
            None => Span::raw(""),
        };
        let chip_w = st.as_deref().map_or(0, |s| s.width() + 2);
        let used = 2 + 12 + sw + 2 + 10;
        let first = n.summary.lines().next().unwrap_or("");
        let cells = vec![
            Span::styled(pad_to(n.day.as_deref().unwrap_or(""), 12), theme::text()),
            Span::styled(
                pad_to(n.source.as_deref().unwrap_or(""), sw),
                theme::muted(),
            ),
            Span::raw("  "),
            chip,
            Span::raw(" ".repeat(10usize.saturating_sub(chip_w))),
            Span::styled(clip(first, width.saturating_sub(used)), theme::text()),
        ];
        lines.push(list_line(cells, i == app.control_cursor));
    }
    f.render_widget(Paragraph::new(lines), pad(area));
}

/// What a row's chip says: a request's status, else the details' status.
fn note_status(app: &App, n: &Note) -> Option<String> {
    if n.is_request() {
        return app.control.status(n).status;
    }
    if n.details.is_err() {
        return Some("unreadable".into());
    }
    n.details
        .as_ref()
        .ok()
        .and_then(|d| d["status"].as_str())
        .map(str::to_string)
}

/// A piece of a note, top to bottom.
enum Piece {
    Lines(Vec<Line<'static>>),
    /// Trend charts, side by side when there's room: (days, charts).
    Charts(Vec<String>, Vec<control::Chart>),
    /// hooks gauges, as on the overview.
    Gauges(Vec<Gauge>),
    /// An image: url, (w, h) if known.
    Image(String, Option<(u32, u32)>),
}

fn draw_note(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    n: &Note,
    images: &mut Vec<(Rect, (String, u16, u16))>,
) {
    let w = area.width.saturating_sub(2).max(20);
    let inner = w.saturating_sub(2) as usize;
    let mut pieces = vec![];
    let mut head = vec![
        Span::styled(
            format!(" {} ", n.day.as_deref().unwrap_or("no day")),
            theme::badge(),
        ),
        Span::styled(
            format!("  {}", n.source.as_deref().unwrap_or("no source")),
            theme::bright(),
        ),
        Span::styled(
            format!("  ·  {}", n.schema.as_deref().unwrap_or("no schema")),
            theme::muted(),
        ),
        Span::styled(
            format!(
                "  ·  by {} {}",
                app.store.name_of(&n.author),
                time_label(n.created_at)
            ),
            theme::faint(),
        ),
    ];
    let replies = app.control.reply_count(n);
    if replies > 0 {
        head.push(Span::styled(
            format!(
                "  ·  {replies} {}",
                if replies == 1 { "reply" } else { "replies" }
            ),
            theme::faint(),
        ));
    }
    let mut top = vec![Line::from(head), Line::raw("")];
    top.extend(render_markdown_links(&n.summary, inner, &[], &|_| None).lines);
    let counts = app.control.reaction_counts(n);
    if !counts.is_empty() {
        let mut spans = vec![];
        for (e, c) in counts {
            spans.push(Span::styled(format!(" {e} {c} "), theme::tab_idle()));
            spans.push(Span::raw(" "));
        }
        top.push(Line::from(spans));
    }
    top.push(Line::raw(""));
    pieces.push(Piece::Lines(top));
    for m in &n.media {
        if m.mime.starts_with("image/") && app.picker.is_some() {
            pieces.push(Piece::Image(m.url.clone(), m.dim));
        } else {
            let name = m.url.rsplit('/').next().unwrap_or("file");
            pieces.push(Piece::Lines(vec![Line::styled(
                format!("▣ {} {name}", m.mime),
                theme::muted(),
            )]));
        }
    }
    if n.is_request() {
        pieces.push(Piece::Lines(request_lines(app, n)));
    }
    match (&n.details, n.schema.as_deref()) {
        (Ok(d), Some("hooks/1")) => {
            let source = n.source.clone().unwrap_or_default();
            let strip = control::hooks_status_strip(&app.control, &source, n);
            pieces.push(Piece::Lines(strip_lines(&strip)));
            pieces.push(Piece::Gauges(control::hooks_gauges(&app.control, n)));
            let (days, charts) = control::hooks_charts(&app.control, &source, n);
            pieces.push(Piece::Charts(days, charts));
            pieces.push(Piece::Lines(hooks_tables(d, inner)));
        }
        (Ok(d), schema) => {
            let mut l = vec![Line::styled(
                format!("details ({}, shown as JSON)", schema.unwrap_or("no schema")),
                theme::muted(),
            )];
            let pretty = serde_json::to_string_pretty(d).unwrap_or_default();
            l.extend(
                pretty
                    .lines()
                    .map(|s| Line::styled(clip(s, inner), theme::code())),
            );
            pieces.push(Piece::Lines(l));
        }
        (Err(e), _) => pieces.push(Piece::Lines(vec![Line::styled(
            format!("details unreadable: {e}"),
            status_style("dead"),
        )])),
    }

    // Lay the pieces out at full height.
    let cols = if w >= 100 { 2 } else { 1 };
    let height = |p: &Piece| -> u16 {
        match p {
            Piece::Lines(l) => l.len().min(MAX_ROWS as usize) as u16,
            Piece::Charts(_, c) => (c.len() as u16).div_ceil(cols) * CHART_ROWS + 1,
            Piece::Gauges(g) => {
                (g.len() as u16).div_ceil(grid(g.len(), w, GAUGE_MIN).0) * GAUGE_ROWS + 1
            }
            Piece::Image(_, dim) => {
                crate::media::thumb_rows(*dim, THUMB_COLS.min(w), THUMB_MAX_ROWS) + 1
            }
        }
    };
    // Summed in usize: a huge JSON note mustn't overflow, and past MAX_ROWS
    // it is clipped with a line that says so.
    let want: usize = pieces.iter().map(|p| height(p) as usize).sum();
    let clipped = want > MAX_ROWS as usize;
    let total: u16 = want.min(MAX_ROWS as usize) as u16 + u16::from(clipped);
    let mut buf = Buffer::empty(Rect::new(0, 0, w, total));
    let mut y = 0u16;
    let mut placed = vec![];
    for p in &pieces {
        let room = MAX_ROWS.saturating_sub(y);
        let h = match p {
            Piece::Lines(_) => height(p).min(room),
            _ if height(p) > room => break,
            _ => height(p),
        };
        if h == 0 {
            break;
        }
        let r = Rect::new(0, y, w, h);
        match p {
            Piece::Lines(l) => Paragraph::new(l.clone()).render(r, &mut buf),
            Piece::Gauges(g) => {
                render_gauges(g, r, h, &mut buf);
            }
            Piece::Charts(days, charts) => {
                let cw = w / cols;
                for (i, c) in charts.iter().enumerate() {
                    let i = i as u16;
                    let cr =
                        Rect::new((i % cols) * cw, y + (i / cols) * CHART_ROWS, cw, CHART_ROWS);
                    render_chart(c, days, cr, &mut buf);
                }
            }
            Piece::Image(url, dim) => {
                let cols = THUMB_COLS.min(w);
                let rows = crate::media::thumb_rows(*dim, cols, THUMB_MAX_ROWS);
                let label = match app.media.images.get(url) {
                    Some(Slot::Failed(e)) => format!("▒ image unavailable: {e}"),
                    Some(Slot::Ready(_)) => String::new(),
                    _ => "▒ loading image".into(),
                };
                Paragraph::new(Line::styled(label, theme::faint()))
                    .render(Rect::new(0, y + rows / 2, w, 1), &mut buf);
                placed.push((y, rows, url.clone(), cols));
            }
        }
        y += h;
    }
    if clipped {
        Paragraph::new(Line::styled(
            format!("… cut at {MAX_ROWS} lines; buzz control get shows it all"),
            theme::faint(),
        ))
        .render(Rect::new(0, total - 1, w, 1), &mut buf);
    }

    // Copy the rows on screen out.
    let view = pad(area);
    let vh = view.height as usize;
    app.page = (vh / 2).max(1);
    app.control_max_scroll = (total as usize).saturating_sub(vh);
    app.control_scroll = app.control_scroll.min(app.control_max_scroll);
    let scroll = app.control_scroll as u16;
    let out = f.buffer_mut();
    for row in 0..view.height.min(total.saturating_sub(scroll)) {
        for x in 0..w.min(view.width) {
            out[(view.x + x, view.y + row)] = buf[(x, scroll + row)].clone();
        }
    }
    for (iy, rows, url, cols) in placed {
        // Only a picture that is fully on screen.
        if iy >= scroll && iy + rows <= scroll + view.height {
            let r = Rect::new(view.x, view.y + iy - scroll, cols, rows);
            let key = (url, cols, rows);
            if !app.media.encoded.contains_key(&key) {
                app.actions
                    .push(crate::app::Action::EncodeImage(key.clone()));
            }
            images.push((r, key));
        }
    }
    if app.control_max_scroll > 0 {
        let more = app.control_max_scroll - app.control_scroll;
        if more > 0 {
            let tag = format!(" ↓ {more} more lines ");
            let tw = (tag.width() as u16).min(view.width);
            f.render_widget(
                Paragraph::new(Span::styled(tag, theme::badge())),
                Rect::new(view.right() - tw, view.bottom().saturating_sub(1), tw, 1),
            );
        }
    }
}

/// A request: status, approvers, each action with its key and votes, and
/// whether you can vote.
fn request_lines(app: &App, n: &Note) -> Vec<Line<'static>> {
    let st = app.control.status(n);
    let mut out = vec![];
    let s = st.status.clone().unwrap_or_else(|| "no status".into());
    let mut head = vec![
        Span::styled("request  ", theme::bright()),
        Span::styled(
            format!(" {s} "),
            status_style(if s == "open" { "partial" } else { "ok" }),
        ),
    ];
    if let Some(o) = &st.outcome {
        head.push(Span::styled(format!("  {o}"), theme::text()));
    }
    let who = if n.approvers.is_empty() {
        "any channel member's vote counts".to_string()
    } else {
        let names: Vec<String> = n.approvers.iter().map(|p| app.store.name_of(p)).collect();
        format!("approvers: {}", names.join(", "))
    };
    head.push(Span::styled(format!("   {who}"), theme::muted()));
    out.push(Line::from(head));
    for (i, t) in app.control.votes(n).iter().enumerate() {
        let names: Vec<String> = t.voters.iter().map(|p| app.store.name_of(p)).collect();
        let mut l = vec![
            Span::styled(format!(" {} ", i + 1), theme::key()),
            Span::styled(
                format!(" {} {} ", t.action.name, t.action.emoji),
                theme::tab_idle(),
            ),
            Span::styled(
                format!(
                    "  {} {}",
                    t.voters.len(),
                    if t.voters.len() == 1 { "vote" } else { "votes" }
                ),
                theme::text(),
            ),
        ];
        if !names.is_empty() {
            l.push(Span::styled(
                format!(": {}", names.join(", ")),
                theme::muted(),
            ));
        }
        out.push(Line::from(l));
    }
    let can: Vec<usize> = (0..n.actions.len())
        .filter(|i| app.control.can_vote(n, *i, &app.store.me).is_ok())
        .collect();
    let hint = if !can.is_empty() {
        let keys: Vec<String> = can.iter().map(|i| (i + 1).to_string()).collect();
        Line::styled(
            format!("press {} to vote (asks first)", keys.join(" ")),
            theme::bright(),
        )
    } else {
        let why = app
            .control
            .can_vote(n, 0, &app.store.me)
            .err()
            .unwrap_or_default();
        Line::styled(format!("you can't vote: {why}"), theme::faint())
    };
    out.push(hint);
    out.push(Line::raw(""));
    out
}

fn strip_lines(strip: &[(String, Option<String>)]) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled("hooks alive  ", theme::bright())];
    for (d, s) in strip.iter().rev().take(14).rev() {
        let day = d.get(5..).unwrap_or(d).to_string();
        spans.push(Span::styled(format!("{day} "), theme::faint()));
        match s {
            Some(s) => spans.push(Span::styled(format!(" {s} "), status_style(s))),
            None => spans.push(Span::styled(" gap ", theme::faint())),
        }
        spans.push(Span::raw("  "));
    }
    vec![
        Line::from(spans),
        Line::styled(
            "charts: one point per day (•), a missing day is a gap; the dashed line and \"vs\" are the baseline",
            theme::faint(),
        ),
        Line::raw(""),
    ]
}

/// One trend chart: each series as lines over its runs of days (a lone day
/// is a dot), its baseline as a flat dim line, and the newest values in the
/// title.
fn render_chart(c: &control::Chart, days: &[String], r: Rect, buf: &mut Buffer) {
    let n = days.len().max(1) as f64;
    let (x0, x1) = (-0.5, n - 0.5);
    let mut ymax: f64 = 0.0;
    for s in &c.series {
        for v in s.points.iter().flatten().chain(s.baseline.iter()) {
            ymax = ymax.max(*v);
        }
    }
    let ymax = if ymax <= 0.0 { 1.0 } else { ymax * 1.15 };
    // The title with values and baselines; on a narrow chart the baselines
    // go, then the values, so a number is never cut.
    let title_at = |level: u8| {
        let mut title = vec![Span::styled(format!(" {} ", c.title), theme::bright())];
        for (i, s) in c.series.iter().enumerate() {
            let last = s.points.iter().rev().flatten().next();
            title.push(Span::styled(format!(" ━ {}", s.label), series_style(i)));
            if let Some(v) = last.filter(|_| level >= 1) {
                title.push(Span::styled(
                    format!(" {}", value_label(*v, c.unit)),
                    theme::text(),
                ));
            }
            if let Some(b) = s.baseline.filter(|_| level >= 2) {
                title.push(Span::styled(
                    format!(" vs {}", value_label(b, c.unit)),
                    theme::faint(),
                ));
            }
        }
        title
    };
    let fits = |t: &[Span]| width_of(t) + 2 <= r.width as usize;
    let title = [2, 1, 0]
        .map(title_at)
        .into_iter()
        .find(|t| fits(t))
        .unwrap_or_else(|| title_at(0));
    // Owned data first; the datasets borrow it.
    let segs: Vec<Vec<Vec<(f64, f64)>>> = c.series.iter().map(|s| s.segments()).collect();
    // A baseline is dashed (a dot every few columns), so it never reads as
    // data across days; data is a solid line or, for a lone day, a dot.
    let cols = r.width.saturating_sub(8).max(4) as usize;
    let dashes = (cols / 3).max(2);
    let bases: Vec<Option<Vec<(f64, f64)>>> = c
        .series
        .iter()
        .map(|s| {
            s.baseline.map(|b| {
                (0..dashes)
                    .map(|k| (x0 + (x1 - x0) * k as f64 / (dashes - 1) as f64, b))
                    .collect()
            })
        })
        .collect();
    let mut sets = vec![];
    for (i, b) in bases.iter().enumerate() {
        if let Some(b) = b {
            sets.push(
                Dataset::default()
                    .marker(Marker::Braille)
                    .graph_type(GraphType::Scatter)
                    .style(series_style(i).add_modifier(Modifier::DIM))
                    .data(b),
            );
        }
    }
    for (i, runs) in segs.iter().enumerate() {
        for run in runs {
            let (marker, kind) = if run.len() == 1 {
                (Marker::Dot, GraphType::Scatter)
            } else {
                (Marker::Braille, GraphType::Line)
            };
            sets.push(
                Dataset::default()
                    .marker(marker)
                    .graph_type(kind)
                    .style(series_style(i))
                    .data(run),
            );
        }
    }
    let day = |d: Option<&String>| {
        d.map(|d| d.get(5..).unwrap_or(d).to_string())
            .unwrap_or_default()
    };
    let xl = if days.len() > 1 {
        vec![day(days.first()), day(days.last())]
    } else {
        // One label is not drawn: centre it between two blanks.
        vec![String::new(), day(days.first()), String::new()]
    };
    let chart = Chart::new(sets)
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(theme::rule())
                .title(Line::from(title)),
        )
        .legend_position(None)
        .x_axis(
            Axis::default()
                .bounds([x0, x1])
                .labels(xl)
                .style(theme::faint()),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, ymax])
                .labels(
                    [0.0, ymax / 2.0, ymax]
                        .map(|v| format!("{:>Y_LABEL_W$}", value_label(v, c.unit))),
                )
                .style(theme::faint()),
        );
    chart.render(
        Rect {
            width: r.width.saturating_sub(1),
            ..r
        },
        buf,
    );
}

/// A table: header, then rows; `hot` rows are highlighted.
fn table(
    title: &str,
    head: &[&str],
    rows: Vec<(Vec<String>, Option<Style>)>,
    width: usize,
) -> Vec<Line<'static>> {
    let mut wide: Vec<usize> = head.iter().map(|h| h.width()).collect();
    for (r, _) in &rows {
        for (i, c) in r.iter().enumerate() {
            if let Some(w) = wide.get_mut(i) {
                *w = (*w).max(c.width());
            }
        }
    }
    let line = |cells: &[String]| -> String {
        let s: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| pad_to(c, wide.get(i).copied().unwrap_or(0)))
            .collect();
        clip(s.join("  ").trim_end(), width)
    };
    let mut out = vec![Line::styled(title.to_string(), theme::bright())];
    let h: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    out.push(Line::styled(line(&h), theme::faint()));
    for (r, hot) in rows {
        out.push(Line::styled(line(&r), hot.unwrap_or_else(theme::text)));
    }
    out.push(Line::raw(""));
    out
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

fn hooks_tables(d: &Value, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    let rows = |k: &str| d[k].as_array().cloned().unwrap_or_default();
    let seats = rows("seats")
        .iter()
        .map(|s| {
            let st = text(&s["status"]);
            let hot = (st == "dead").then(|| status_style("dead"));
            (
                vec![
                    text(&s["seat"]),
                    num(&s["sessions"]),
                    num(&s["first_edits"]),
                    num(&s["ramp_lines"]),
                    format!("{} / {}", num(&s["mains"]), num(&s["standups"])),
                    num(&s["pre_hooks"]),
                    st,
                    text(&s["problem"]),
                ],
                hot,
            )
        })
        .collect();
    out.extend(table(
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
        width,
    ));
    let types = rows("agent_types")
        .iter()
        .map(|a| {
            let flagged = a["flagged"].as_bool() == Some(true);
            let delta = a["delta_pct"]
                .as_f64()
                .map(|x| format!("{x:+.0}%"))
                .unwrap_or_else(|| "-".into());
            (
                vec![
                    text(&a["agent_type"]),
                    text(&a["repo"]),
                    num(&a["n"]),
                    num(&a["s"]),
                    num(&a["g"]),
                    num(&a["recommended"]),
                    text(&a["threshold_label"]),
                    if flagged {
                        format!("{delta} ⚑")
                    } else {
                        delta
                    },
                ],
                flagged.then(|| status_style("partial")),
            )
        })
        .collect();
    out.extend(table(
        "agent types (⚑ = recommended reset more than 20% off the threshold)",
        &[
            "agent type",
            "repo",
            "n",
            "S",
            "g",
            "recommended",
            "threshold",
            "delta",
        ],
        types,
        width,
    ));
    let repos = rows("ramp_by_repo")
        .iter()
        .map(|r| {
            (
                vec![
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
                None,
            )
        })
        .collect();
    out.extend(table(
        "ramp by repo (context at first edit; running is the mode we tune)",
        &[
            "repo",
            "running n",
            "ramp",
            "calls",
            "ideating n / ramp",
            "quick n / ramp",
        ],
        repos,
        width,
    ));
    let w = |k: &str| {
        d[k].as_str()
            .map(str::to_string)
            .unwrap_or_else(|| num(&d[k]))
    };
    out.push(Line::styled(
        format!(
            "window {} .. {}  ·  seats read {}, not checked {}  ·  ramp after handoff n = {}",
            w("window_since"),
            w("window_until"),
            num(&d["seats_read"]),
            num(&d["seats_not_checked"]),
            num(&d["ramp_after_handoff_n"]),
        ),
        theme::faint(),
    ));
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event as TermEvent, KeyCode};
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use serde_json::json;

    use crate::app::{Action, App};
    use crate::control::{self, DETAILS_FENCE};
    use crate::keys::Mode;

    const CH: &str = control::DEFAULT_CHANNEL;

    fn ev(keys: &Keys, kind: u16, content: &str, tags: Vec<Vec<String>>, at: u64) -> nostr::Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(tags.into_iter().map(|t| Tag::parse(t).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn t(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn hooks(peak: u64, seat_status: &str) -> serde_json::Value {
        json!({
            "window_since": "2026-10-20T15:40:37Z", "window_until": "2026-10-21T15:40:37Z",
            "status": if seat_status == "dead" { "dead" } else { "ok" },
            "seats_read": 2, "seats_not_checked": 0,
            "peak_context_subagent_tokens": peak, "peak_context_main_tokens": 604000,
            "share_spend_above_300k_pct": 42, "ramp_after_handoff_median_tokens": 108000,
            "ramp_after_handoff_n": 3, "cost_per_useful_call_tokens": 55100,
            "weighted_spend_tokens": 157100000,
            "baseline": {"peak_context_subagent_tokens": 612000, "peak_context_main_tokens": 432000,
                "share_spend_above_300k_pct": 41, "ramp_after_handoff_median_tokens": 79000,
                "cost_per_useful_call_tokens": 75200},
            "seats": [
                {"seat": "taskbot", "sessions": 122, "first_edits": 9, "ramp_lines": 9, "mains": 7,
                 "standups": 7, "pre_hooks": 32, "status": "ok", "problem": null},
                {"seat": "ghostbot", "sessions": 4, "first_edits": 2, "ramp_lines": 0, "mains": 1,
                 "standups": 0, "pre_hooks": 0, "status": seat_status, "problem": "no ramp lines"}
            ],
            "ramp_by_repo": [{"repo": "aei", "running_n": 3, "running_ramp_median_tokens": 108000,
                "running_calls_median": 25, "ideating_n": 0, "ideating_ramp_median_tokens": null,
                "quick_n": 0, "quick_ramp_median_tokens": null}],
            "agent_types": [{"agent_type": "builder", "repo": null, "n": 7, "s": 86000, "g": 1000,
                "recommended": 161000, "threshold_label": "builder handoff 250k", "threshold": 250000,
                "delta_pct": -35.6, "flagged": true}]
        })
    }

    fn note(
        keys: &Keys,
        topic: &str,
        schema: &str,
        day: &str,
        details: &serde_json::Value,
        extra: Vec<Vec<String>>,
    ) -> nostr::Event {
        let content = format!("Summary of {topic} on {day}.\n\n{DETAILS_FENCE}\n{details:#}\n```");
        let mut tags = vec![
            t(&["h", CH]),
            t(&["t", "control"]),
            t(&["control-topic", topic]),
            t(&["control-day", day]),
            t(&["control-source", "build-box"]),
            t(&["control-schema", schema]),
        ];
        tags.extend(extra);
        ev(keys, 9, &content, tags, 1_792_600_000)
    }

    fn key(app: &mut App, c: KeyCode) {
        app.on_term(TermEvent::Key(c.into()));
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::super::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect()
    }

    fn fixture() -> (App, Keys) {
        let me = Keys::generate();
        let mut app = App::new(me.public_key().to_hex(), None);
        app.control.loaded = true;
        app.control_today = Some("2026-10-21".into());
        let k = Keys::generate();
        app.control.apply(&note(
            &k,
            "hooks",
            "hooks/1",
            "2026-10-19",
            &hooks(500000, "ok"),
            vec![],
        ));
        app.control.apply(&note(
            &k,
            "hooks",
            "hooks/1",
            "2026-10-21",
            &hooks(767000, "dead"),
            vec![],
        ));
        app.control.apply(&note(
            &k,
            "upgrade",
            "upgrade/1",
            "2026-10-21",
            &json!({"stage": "81726e72", "relay": "821040a6"}),
            vec![
                t(&["control-action", "approve", "👍"]),
                t(&["control-action", "hold", "⏳"]),
                t(&["control-status", "open"]),
                t(&[
                    "imeta",
                    "url https://buzz.example/media/chart.png",
                    "m image/png",
                ]),
            ],
        ));
        (app, me)
    }

    #[test]
    fn topics_days_and_a_hooks_note() {
        let (mut app, _) = fixture();
        key(&mut app, KeyCode::Char('C'));
        assert!(app.control_view);
        let s = screen(&mut app, 120, 40);
        assert!(
            s.contains("hooks") && s.contains("2026-10-21") && s.contains("1 open"),
            "{s}"
        );
        // The request strip comes first; the hooks tile is next.
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);
        let s = screen(&mut app, 120, 40);
        assert!(s.contains("2026-10-19") && s.contains(" dead "), "{s}");
        key(&mut app, KeyCode::Enter);
        let s = screen(&mut app, 120, 120);
        for want in [
            "Summary of hooks on 2026-10-21.",
            "hooks alive",
            "10-20",
            " gap ",
            "Peak context",
            "767k",
            "vs 612k",
            "Reset vs threshold",
            "ghostbot",
            "-36% ⚑",
            "ramp by repo",
        ] {
            assert!(s.contains(want), "missing {want:?}:\n{s}");
        }
        // Not a request: a digit leaves for the tab, as elsewhere.
        assert!(app.control_note.is_some());
        key(&mut app, KeyCode::Char('h'));
        assert!(app.control_note.is_none() && app.control_topic.is_some());
        key(&mut app, KeyCode::Esc);
        // Back on the tile it came from.
        assert_eq!(app.control_cursor, 1);
        key(&mut app, KeyCode::Esc);
        assert!(!app.control_view);
    }

    #[test]
    fn a_dead_seat_row_is_highlighted() {
        let (mut app, _) = fixture();
        for c in [
            KeyCode::Char('C'),
            KeyCode::Char('j'),
            KeyCode::Enter,
            KeyCode::Enter,
        ] {
            key(&mut app, c);
        }
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 120)).unwrap();
        term.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let row = |name: &str| {
            (0..120u16)
                .find(|y| {
                    let l: String = (0..120u16).map(|x| buf[(x, *y)].symbol()).collect();
                    l.trim_start().starts_with(name)
                })
                .unwrap()
        };
        let bg = |y: u16| buf[(4, y)].bg;
        assert_ne!(bg(row("ghostbot")), bg(row("taskbot")));
        assert_eq!(bg(row("ghostbot")), super::status_style("dead").bg.unwrap());
    }

    #[test]
    fn unknown_schema_shows_json_and_a_request_votes() {
        let (mut app, me) = fixture();
        key(&mut app, KeyCode::Char('C'));
        // The strip's request opens straight onto its note.
        key(&mut app, KeyCode::Enter);
        let s = screen(&mut app, 120, 40);
        for want in [
            "details (upgrade/1, shown as JSON)",
            "\"stage\": \"81726e72\"",
            "request",
            " open ",
            "any channel member's vote counts",
            " 1  approve 👍",
            " 2  hold ⏳",
            "press 1 2 to vote",
            // Without a graphics-capable terminal an image is named, as in
            // a timeline.
            "▣ image/png chart.png",
        ] {
            assert!(s.contains(want), "missing {want:?}:\n{s}");
        }
        key(&mut app, KeyCode::Char('2'));
        assert_eq!(app.mode, Mode::Vote);
        let s = screen(&mut app, 120, 40);
        assert!(
            s.contains("Vote hold ⏳") && s.contains("on this request?"),
            "{s}"
        );
        app.actions.clear();
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.mode, Mode::Normal);
        let sent = app.actions.iter().find_map(|a| match a {
            Action::React { emoji, label, .. } => Some((emoji.clone(), label.clone())),
            _ => None,
        });
        assert_eq!(sent, Some(("⏳".to_string(), "hold".to_string())));
        // Once the vote is back from the relay, a second one is refused.
        let id = app.control_note.clone().unwrap();
        app.control
            .apply(&ev(&me, 7, "⏳", vec![t(&["e", &id])], 5));
        key(&mut app, KeyCode::Char('2'));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .0
                .contains("you already voted hold")
        );
        assert!(screen(&mut app, 120, 40).contains("1 vote"));
    }

    #[test]
    fn a_non_approver_is_told_why_not() {
        let (mut app, _) = fixture();
        let k = Keys::generate();
        let approver = Keys::generate().public_key().to_hex();
        app.control.apply(&note(
            &k,
            "routing",
            "routing/1",
            "2026-10-21",
            &json!({}),
            vec![
                t(&["control-action", "approve", "👍"]),
                t(&["control-approver", &approver]),
                t(&["control-status", "open"]),
            ],
        ));
        key(&mut app, KeyCode::Char('C'));
        // Two requests, then hooks upgrade converge fleet-audit routing.
        key(&mut app, KeyCode::Char('G'));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        let s = screen(&mut app, 120, 40);
        assert!(
            s.contains("you can't vote: only the note's approvers"),
            "{s}"
        );
        key(&mut app, KeyCode::Char('1'));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .0
                .contains("only the note's approvers")
        );
    }

    #[test]
    fn a_lone_day_is_a_dot_on_a_dashed_baseline() {
        let me = Keys::generate();
        let mut app = App::new(me.public_key().to_hex(), None);
        app.control.loaded = true;
        let k = Keys::generate();
        app.control.apply(&note(
            &k,
            "hooks",
            "hooks/1",
            "2026-10-21",
            &hooks(767000, "ok"),
            vec![],
        ));
        for c in [KeyCode::Char('C'), KeyCode::Enter, KeyCode::Enter] {
            key(&mut app, c);
        }
        let s = screen(&mut app, 140, 120);
        let is_braille = |c: char| ('\u{2801}'..='\u{28FF}').contains(&c);
        for l in s.lines() {
            let cs: Vec<char> = l.chars().collect();
            assert!(
                !cs.windows(2).any(|w| is_braille(w[0]) && is_braille(w[1])),
                "a solid line on a one-day chart:\n{l}"
            );
        }
        // Each chart's dot sits inside its own frame: between its y axis and
        // the next chart (two per row at this width, each half the body).
        let row = s
            .lines()
            .find(|l| l.contains("Ramp after handoff"))
            .map(|_| ())
            .and_then(|_| {
                let lines: Vec<&str> = s.lines().collect();
                let i = lines
                    .iter()
                    .position(|l| l.contains("Ramp after handoff"))?;
                Some(lines[i + 1..i + 9].join("\n"))
            })
            .unwrap();
        let dots: Vec<usize> = row
            .lines()
            .flat_map(|l| {
                l.chars()
                    .enumerate()
                    .filter(|(_, c)| *c == '•')
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(dots.len(), 2, "{row}");
        let half = 140 / 2;
        assert!(
            dots.iter().any(|&x| x < half) && dots.iter().any(|&x| x > half + 6),
            "{row}"
        );
    }

    #[test]
    fn paste_wheel_and_a_huge_note_stay_on_the_view() {
        let (mut app, _) = fixture();
        let k = Keys::generate();
        // ~70k rows of pretty JSON: clipped, not a u16 overflow.
        let big: Vec<serde_json::Value> = (0..23_000).map(|i| json!([i])).collect();
        app.control.apply(&note(
            &k,
            "zbig",
            "zbig/1",
            "2026-10-21",
            &json!(big),
            vec![],
        ));
        key(&mut app, KeyCode::Char('C'));
        app.on_term(TermEvent::Paste("hello".into()));
        assert_eq!(
            app.mode,
            Mode::Normal,
            "a paste must not open the hidden channel's draft"
        );
        key(&mut app, KeyCode::Char('G'));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Enter);
        assert!(app.control_note.is_some());
        let _ = screen(&mut app, 120, 40);
        key(&mut app, KeyCode::Char('G'));
        let s = screen(&mut app, 120, 40);
        assert!(s.contains("cut at 4000 lines"), "{s}");
        // The wheel scrolls the note.
        app.on_term(TermEvent::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }));
        assert_eq!(app.control_scroll, app.control_max_scroll - 3);
    }

    #[test]
    fn empty_and_loading() {
        let mut app = App::new("me".into(), None);
        key(&mut app, KeyCode::Char('C'));
        assert!(app.actions.iter().any(|a| matches!(a, Action::LoadControl)));
        assert!(screen(&mut app, 100, 24).contains("loading #control"));
        app.control.loaded = true;
        assert!(screen(&mut app, 100, 24).contains("no control notes in the last 60 days"));
        key(&mut app, KeyCode::Char('C'));
        assert!(!app.control_view);
    }

    /// Each line of a screen with its cells' styles, for color checks.
    fn cells(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::super::draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    /// Where `needle` starts on screen: (x, y) in cells.
    fn find(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
        let a = buf.area;
        for y in 0..a.height {
            let syms: Vec<&str> = (0..a.width).map(|x| buf[(x, y)].symbol()).collect();
            for x in 0..a.width as usize {
                let mut got = String::new();
                for s in &syms[x..] {
                    got.push_str(s);
                    if got.len() >= needle.len() {
                        break;
                    }
                }
                if got.starts_with(needle) {
                    return Some((x as u16, y));
                }
            }
        }
        None
    }

    #[test]
    fn overview_tiles_reflow_at_80_and_140_without_cutting_numbers() {
        for (w, per_row) in [(80u16, 3usize), (140, 4)] {
            let (mut app, _) = fixture();
            key(&mut app, KeyCode::Char('C'));
            let s = screen(&mut app, w, 50);
            for want in [
                "OPEN REQUESTS 1",
                "upgrade  2026-10-21  build-box",
                " 1 ",
                " approve 👍 ",
                " hold ⏳ ",
                "✗ hooks",
                "▲ upgrade",
                "· converge",
                "· fleet-audit",
                "PEAK SUBAGENT",
                "767k  vs 612k",
                "604k  vs 432k",
                "42%  vs 41%",
                "108k  vs 79k",
                "55k  vs 75k",
                "161k  vs 250k",
                "+25%",
                "-27%",
                "-36%",
                "seats  taskbot ● ghostbot ●",
            ] {
                assert!(s.contains(want), "{w} cols: missing {want:?}\n{s}");
            }
            // Tiles per row: the topic tiles' titles on one line.
            let titles = ["✗ hooks", "▲ upgrade", "· converge", "· fleet-audit"];
            let line = s.lines().find(|l| l.contains("✗ hooks")).unwrap();
            let n = titles.iter().filter(|t| line.contains(*t)).count();
            assert_eq!(n, per_row, "{w} cols:\n{s}");
            // Nothing is cut mid-number, and every tile closes inside the body.
            for l in s.lines() {
                let cs: Vec<char> = l.chars().collect();
                assert!(
                    !cs.windows(2).any(|p| p[0].is_ascii_digit() && p[1] == '…'),
                    "{w} cols: a number cut: {l}"
                );
            }
            let buf = cells(&mut app, w, 50);
            for y in 0..50 {
                for x in w - 2..w {
                    let c = buf[(x, y)].symbol();
                    assert!(
                        !["┐", "┘", "┓", "┛"].contains(&c),
                        "{w} cols: a tile runs to the edge at row {y}"
                    );
                }
            }
        }
    }

    #[test]
    fn delta_and_status_colors() {
        use super::{health_color, tone_style};
        use crate::control::{Health, Tone};
        for w in [80u16, 140] {
            let (mut app, _) = fixture();
            // A metric far worse than its baseline pages.
            let k = Keys::generate();
            let mut d = hooks(767000, "dead");
            d["peak_context_main_tokens"] = json!(1_000_000);
            app.control
                .apply(&note(&k, "hooks", "hooks/1", "2026-10-22", &d, vec![]));
            app.control_today = Some("2026-10-22".into());
            key(&mut app, KeyCode::Char('C'));
            let buf = cells(&mut app, w, 50);
            let fg = |needle: &str| {
                let (x, y) = find(&buf, needle).unwrap_or_else(|| panic!("{w}: no {needle:?}"));
                buf[(x, y)].fg
            };
            assert_eq!(fg("+25%"), tone_style(Tone::Flag).fg.unwrap(), "flag amber");
            assert_eq!(fg("+131%"), tone_style(Tone::Page).fg.unwrap(), "page red");
            assert_eq!(
                fg("-27%"),
                tone_style(Tone::Better).fg.unwrap(),
                "better green"
            );
            // Lower than its threshold but flagged: amber, never green.
            assert_eq!(fg("-36%"), tone_style(Tone::Flag).fg.unwrap());
            assert_ne!(fg("-36%"), tone_style(Tone::Better).fg.unwrap());
            assert_eq!(fg("+1pt"), tone_style(Tone::Plain).fg.unwrap());
            // A plain delta's bar wears no status color.
            let (x, y) = find(&buf, "42%  vs 41%").unwrap();
            let bar = buf[(x, y + 1)].fg;
            for t in [Tone::Better, Tone::Flag, Tone::Page] {
                assert_ne!(
                    bar,
                    tone_style(t).fg.unwrap(),
                    "{w}: plain bar colored {t:?}"
                );
            }
            let (x, y) = find(&buf, "55k  vs 75k").unwrap();
            assert_eq!(buf[(x, y + 1)].fg, tone_style(Tone::Better).fg.unwrap());
            // Tile borders take the status color.
            let border = |title: &str| {
                let (x, y) = find(&buf, title).unwrap();
                buf[(x - 2, y)].fg
            };
            assert_eq!(border("✗ hooks"), health_color(Health::Page));
            assert_eq!(border("▲ upgrade"), health_color(Health::Warn));
            assert_eq!(border("· converge"), health_color(Health::NoNote));
        }
    }

    #[test]
    fn a_digit_on_the_overview_votes_the_strip_request() {
        let (mut app, _) = fixture();
        key(&mut app, KeyCode::Char('C'));
        // On a tile: the only open request is the one asked about.
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('1'));
        assert_eq!(app.mode, Mode::Vote);
        let s = screen(&mut app, 100, 40);
        assert!(s.contains("Vote approve 👍"), "{s}");
        assert!(s.contains("upgrade › 2026-10-21 build-box"), "{s}");
        app.actions.clear();
        key(&mut app, KeyCode::Char('y'));
        let sent = app.actions.iter().find_map(|a| match a {
            Action::React { target, emoji, .. } => Some((target.clone(), emoji.clone())),
            _ => None,
        });
        let id = app.control.open_requests()[0].id.clone();
        assert_eq!(sent, Some((id, "👍".to_string())));
        // Enter on the strip opens the request; back is the overview.
        key(&mut app, KeyCode::Char('g'));
        key(&mut app, KeyCode::Enter);
        assert!(app.control_note.is_some() && app.control_topic.is_none());
        key(&mut app, KeyCode::Char('h'));
        assert!(app.control_note.is_none() && app.control_view);
    }

    #[test]
    fn chart_plot_areas_line_up() {
        let (mut app, _) = fixture();
        for c in [
            KeyCode::Char('C'),
            KeyCode::Char('j'),
            KeyCode::Enter,
            KeyCode::Enter,
        ] {
            key(&mut app, c);
        }
        let s = screen(&mut app, 140, 160);
        let from = s.find("Peak context").unwrap();
        let s = &s[from..];
        // Each chart's x axis corner: one column per chart column, however
        // wide its y labels ("0%" next to "180.7M").
        let mut corners: Vec<usize> = s
            .lines()
            .flat_map(|l| {
                l.chars()
                    .enumerate()
                    .filter(|(_, c)| *c == '└')
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            })
            .collect();
        let n = corners.len();
        corners.sort();
        corners.dedup();
        assert!(n >= 6, "{s}");
        assert_eq!(corners.len(), 2, "{corners:?}\n{s}");
    }

    /// The Control view's screens as plain text (BUZZ_VIM_SHOT=<dir>).
    #[test]
    fn shot_control() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        let (mut app, _) = fixture();
        key(&mut app, KeyCode::Char('C'));
        for (w, h) in [(80, 40), (140, 45)] {
            std::fs::write(
                format!("{dir}/control-overview-{w}.txt"),
                screen(&mut app, w, h),
            )
            .unwrap();
        }
        // Down to the hooks tile (past the one request).
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);
        std::fs::write(format!("{dir}/control-days.txt"), screen(&mut app, 120, 40)).unwrap();
        key(&mut app, KeyCode::Enter);
        std::fs::write(
            format!("{dir}/control-note.txt"),
            screen(&mut app, 120, 120),
        )
        .unwrap();
    }
}
