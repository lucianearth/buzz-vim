//! The Control view (`C`): #control's notes. Topics with their newest day,
//! a topic's days, and one note: its summary, images, request (actions,
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
use ratatui::style::{Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Widget};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use super::{
    THUMB_COLS, THUMB_MAX_ROWS, breadcrumb, clip, draw_empty, pad_to, render_markdown_links,
    time_label,
};
use crate::app::App;
use crate::control::{self, Note, Unit};
use crate::media::Slot;
use crate::theme;

const CHART_ROWS: u16 = 10;
/// The tallest a note is drawn; past it, a line says it was cut.
const MAX_ROWS: u16 = 4000;
/// OKLab hues for a status: ok green, partial amber, dead red.
const OK: f32 = 145.0;
const PARTIAL: f32 = 75.0;
const DEAD: f32 = 25.0;

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
    }
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
        (None, None) => "topics".into(),
    };
    let keys = match (&topic, &note) {
        (_, Some(n)) if n.is_request() => "j k scroll  1-9 vote  o images  r reload",
        (_, Some(_)) => "j k scroll  o images  r reload",
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
        (None, None) => draw_topics(f, app, area),
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

fn draw_topics(f: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.control.topics();
    if rows.is_empty() {
        let msg = format!(
            "no control notes in the last {} days. producers write them with buzz control publish",
            control::DAYS_BACK
        );
        return draw_empty(f, app, area, &msg);
    }
    app.control_cursor = app.control_cursor.min(rows.len() - 1);
    let tw = rows
        .iter()
        .map(|r| r.topic.width())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut lines = vec![Line::styled(
        format!("  {}  latest day   days  sources", pad_to("topic", tw)),
        theme::faint(),
    )];
    for (i, r) in rows.iter().enumerate() {
        let mut cells = vec![
            Span::styled(pad_to(&r.topic, tw), theme::bright()),
            Span::raw("  "),
            Span::styled(pad_to(&r.latest_day, 11), theme::text()),
            Span::raw("  "),
            Span::styled(pad_to(&r.days.to_string(), 4), theme::muted()),
            Span::raw("  "),
            Span::styled(r.sources.join(", "), theme::muted()),
        ];
        if r.open > 0 {
            cells.push(Span::raw("  "));
            cells.push(Span::styled(format!(" {} open ", r.open), theme::badge()));
        }
        lines.push(list_line(cells, i == app.control_cursor));
    }
    f.render_widget(Paragraph::new(lines), pad(area));
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
    let mut title = vec![Span::styled(format!(" {} ", c.title), theme::bright())];
    for (i, s) in c.series.iter().enumerate() {
        let last = s.points.iter().rev().flatten().next();
        title.push(Span::styled(format!(" ━ {}", s.label), series_style(i)));
        if let Some(v) = last {
            title.push(Span::styled(
                format!(" {}", value_label(*v, c.unit)),
                theme::text(),
            ));
        }
        if let Some(b) = s.baseline {
            title.push(Span::styled(
                format!(" vs {}", value_label(b, c.unit)),
                theme::faint(),
            ));
        }
    }
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
                .labels(vec![
                    "0".to_string(),
                    value_label(ymax / 2.0, c.unit),
                    value_label(ymax, c.unit),
                ])
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
        key(&mut app, KeyCode::Esc);
        assert!(!app.control_view);
    }

    #[test]
    fn a_dead_seat_row_is_highlighted() {
        let (mut app, _) = fixture();
        for c in [KeyCode::Char('C'), KeyCode::Enter, KeyCode::Enter] {
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
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);
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
        // hooks, routing, upgrade
        key(&mut app, KeyCode::Char('j'));
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

    /// The Control view's screens as plain text (BUZZ_VIM_SHOT=<dir>).
    #[test]
    fn shot_control() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        let (mut app, _) = fixture();
        key(&mut app, KeyCode::Char('C'));
        std::fs::write(
            format!("{dir}/control-topics.txt"),
            screen(&mut app, 120, 40),
        )
        .unwrap();
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
