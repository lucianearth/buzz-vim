//! Drawing. Immediate mode: everything is recomputed from `App` each frame,
//! and ratatui diffs cells so only changes reach the terminal.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui_image::Image;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Hit, Pane};
use crate::art;
use crate::garden;
use crate::keys::{Mode, legend};
use crate::live::ConnState;
use crate::media::Slot;
use crate::model::{Channel, Message};
use crate::theme;

const THUMB_COLS: u16 = 36;
const THUMB_MAX_ROWS: u16 = 12;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.needs = app.needs_rows();
    app.hits.clear();
    app.links.clear();
    app.empty_prev = app.empty.take();
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(theme::gray(0))), area);
    let compose_h = compose_height(app, area.width);
    let [top, body, compose, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(compose_h),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_tabs(f, app, top);

    // Threads and the agents view take over the main pane; h, ← or Esc
    // goes back up.
    let main = slider(f, app, body);
    let mut images = vec![];
    if app.needs_view {
        draw_needs(f, app, main);
    } else if app.agents_view {
        draw_agents(f, app, main);
    } else if app.checklist_view {
        draw_checklist(f, app, main);
    } else if app.thread.is_some() {
        draw_thread(f, app, main, &mut images);
    } else {
        draw_timeline(f, app, main, &mut images);
    }
    draw_compose(f, app, compose);
    draw_footer(f, app, foot);
    // Images last, so nothing paints over them. Overlays hide them: terminal
    // graphics sit on top of text, and a popup can't cover them otherwise.
    let overlay = matches!(
        app.mode,
        Mode::Sections
            | Mode::Order
            | Mode::Archive
            | Mode::Confirm
            | Mode::Help
            | Mode::Viewer
            | Mode::Quick
            | Mode::Reply
    ) || (app.mode == Mode::Insert && !app.mention_candidates().is_empty());
    for (rect, key) in images.into_iter().filter(|_| !overlay) {
        if let Some(p) = app.media.encoded.get(&key) {
            f.render_widget(Image::new(p), rect);
        }
    }
    match app.mode {
        Mode::Sections => draw_sections(f, app, area),
        Mode::Order | Mode::Archive => draw_order(f, app, area),
        Mode::Confirm => draw_confirm(f, app, area),
        Mode::Help => draw_help(f, area),
        Mode::Viewer => draw_viewer(f, app, area),
        Mode::Hint => draw_hints(f, app),
        Mode::Quick => draw_quick(f, app, area),
        Mode::Reply => draw_reply(f, app, area),
        _ => {}
    }
}

/// `f`: a letter chip over the start of every link on screen.
fn draw_hints(f: &mut Frame, app: &App) {
    let starts = app.link_starts();
    for ((rect, _), label) in starts.iter().zip(crate::links::hint_labels(starts.len())) {
        if !label.starts_with(&app.hint_typed) {
            continue;
        }
        let w = (label.width() as u16).min(f.area().right().saturating_sub(rect.x));
        f.render_widget(
            Paragraph::new(Span::styled(label, theme::key())),
            Rect { width: w, ..*rect },
        );
    }
}

/// The reading-width slider: the body's last column is a handle, and the
/// panes lay out in the width to its left.
fn slider(f: &mut Frame, app: &mut App, body: Rect) -> Rect {
    let max = body.width.saturating_sub(1);
    app.body_max = max;
    app.body_x = body.x;
    let w = app.read_width.map_or(max, |w| w.min(max));
    let handle = Rect {
        x: body.x + w,
        width: 1.min(body.width),
        ..body
    };
    let mid = handle.height / 2;
    let rows: Vec<Line> = (0..handle.height)
        .map(|r| {
            if r + 1 >= mid && r <= mid + 1 {
                let st = if app.dragging {
                    theme::bright()
                } else {
                    theme::muted()
                };
                Line::styled("┃", st)
            } else {
                Line::styled("│", theme::rule())
            }
        })
        .collect();
    f.render_widget(Paragraph::new(rows), handle);
    app.hits.push((handle, Hit::Handle));
    Rect { width: w, ..body }
}

// ---------- top bar ----------

fn draw_tabs(f: &mut Frame, app: &mut App, area: Rect) {
    let mut x = area.x;
    // The dot breathes while anything animates and rests bright otherwise.
    let level = if app.animating() {
        art::breath(app.tick)
    } else {
        23
    };
    let logo = Line::from(vec![
        Span::raw(" "),
        Span::styled("●", Style::new().fg(theme::gray(level))),
        Span::styled(" buzz ", theme::bright()),
    ]);
    let logo_w = logo.width() as u16;
    f.render_widget(
        Paragraph::new(logo),
        Rect {
            width: logo_w,
            ..area
        },
    );
    x += logo_w + 1;
    let msgs = app
        .current()
        .and_then(|c| app.store.channels.get(c))
        .map_or(0, |c| c.messages.len());
    let right = match app.conn {
        ConnState::Online => format!("{msgs} msgs  ● live "),
        ConnState::Connecting => format!(" {} connecting ", art::flight(app.tick, 3).trim()),
        ConnState::Offline => " ○ offline, retrying ".to_string(),
    };
    // Tabs get the whole row up to the count.
    let limit = area.x + area.width.saturating_sub(right.width() as u16 + 1);
    // Tab 0, Needs you, pinned first: it never scrolls away or closes.
    let n = crate::needs::answer_count(&app.needs);
    let label = if n > 0 {
        format!(" 0 ⚑ needs you {n} ")
    } else {
        " 0 needs you ".to_string()
    };
    let w = (label.width() as u16).min(limit.saturating_sub(x));
    let style = if app.needs_view {
        theme::tab_active()
    } else if n > 0 {
        theme::tab_idle().add_modifier(Modifier::BOLD)
    } else {
        theme::tab_idle()
    };
    let r = Rect {
        x,
        y: area.y,
        width: w,
        height: 1,
    };
    f.render_widget(Paragraph::new(Span::styled(label, style)), r);
    app.hits.push((r, Hit::NeedsTab));
    x += w + 1;
    let labels: Vec<(String, usize)> = app
        .tabs
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let ch = app.store.channels.get(id);
            let name = ch.map(channel_label).unwrap_or_else(|| "…".into());
            let unread = ch.map_or(0, |c| c.unread.len());
            // In a DM every message is to you.
            let mentions = ch.map_or(0, |c| {
                if c.is_dm {
                    c.unread.len()
                } else {
                    c.mentioned.len()
                }
            });
            // Two fixed cells after the name, unread dot then spinner, so
            // tabs never change width as these come and go; a mention
            // count replaces the dot.
            let dot = if mentions > 0 {
                format!("@{mentions}")
            } else if unread > 0 {
                "•".into()
            } else {
                " ".into()
            };
            let spin = if app.agents.working(&app.store, id, None).is_empty() {
                " "
            } else {
                SPIN[app.tick as usize % SPIN.len()]
            };
            let label = if i < 9 {
                format!(" {} {name} {dot}{spin}", i + 1)
            } else {
                format!(" {name} {dot}{spin}")
            };
            (label, unread)
        })
        .collect();
    // Scroll the tab strip so the active tab is always on screen.
    let room = limit.saturating_sub(x) as usize;
    let mut start = 0;
    while start < app.active
        && labels[start..=app.active.min(labels.len().saturating_sub(1))]
            .iter()
            .map(|(l, _)| l.width() + 1)
            .sum::<usize>()
            + 2
            > room
    {
        start += 1;
    }
    if start > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("…", theme::muted())),
            Rect {
                x,
                width: 1,
                ..area
            },
        );
        x += 2;
    }
    for (i, (label, unread)) in labels.into_iter().enumerate().skip(start) {
        let w = label.width() as u16;
        if x + w > limit {
            f.render_widget(
                Paragraph::new(Span::styled("…", theme::muted())),
                Rect {
                    x,
                    y: area.y,
                    width: 1,
                    height: 1,
                },
            );
            break;
        }
        let hue = theme::hue(&app.tabs[i]);
        let style = if i == app.active && !app.needs_view {
            theme::chip(hue)
        } else if unread > 0 {
            theme::tab_idle()
                .add_modifier(Modifier::BOLD)
                .fg(theme::ink(hue, 23))
        } else {
            theme::tab_idle()
        };
        let r = Rect {
            x,
            y: area.y,
            width: w,
            height: 1,
        };
        f.render_widget(Paragraph::new(Span::styled(label, style)), r);
        app.hits.push((r, Hit::Tab(i)));
        x += w + 1;
    }
    let style = if app.conn == ConnState::Online {
        theme::muted()
    } else {
        theme::bright()
    };
    f.render_widget(
        Paragraph::new(Span::styled(right, style)).right_aligned(),
        Rect {
            x: limit,
            width: area.width.saturating_sub(limit - area.x),
            ..area
        },
    );
}

/// One-cell braille spinner.
const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn channel_label(c: &Channel) -> String {
    let n = if c.name.is_empty() {
        c.id.chars().take(8).collect()
    } else {
        c.name.clone()
    };
    if c.is_dm {
        format!("@{n}")
    } else {
        format!("#{n}")
    }
}

// ---------- message panes ----------

struct BlockOut {
    id: String,
    lines: Vec<Line<'static>>,
    /// (row offset inside block, rows, url, cols)
    image: Option<(u16, u16, String, u16)>,
    /// (row, column inside the block, width, link number, url): each piece
    /// of each link.
    links: Vec<(u16, u16, u16, usize, String)>,
}

fn time_label(at: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(at as i64, 0).single() else {
        return String::new();
    };
    relative_time(t, Local::now())
}

/// "just now", "4m ago", "2h 15m ago" for the last 12 hours, then a date.
fn relative_time<Tz: chrono::TimeZone>(t: chrono::DateTime<Tz>, now: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Datelike;
    let mins = (now.clone() - t.clone()).num_minutes();
    match mins {
        m if m < 1 => "just now".into(),
        m if m < 60 => format!("{m}m ago"),
        m if m < 12 * 60 && m % 60 == 0 => format!("{}h ago", m / 60),
        m if m < 12 * 60 => format!("{}h {}m ago", m / 60, m % 60),
        _ if t.date_naive().year() == now.date_naive().year() => {
            t.format("%b %e %H:%M").to_string()
        }
        _ => t.format("%b %e %Y %H:%M").to_string(),
    }
}

/// "taskbot thinking ( ·.·)" for whoever is mid-turn, or None.
fn thinking_label(app: &App, names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let face = art::THINK[(app.tick as usize / 2) % art::THINK.len()];
    Some(format!("{} thinking {face}", names.join(", ")))
}

fn message_block(
    app: &App,
    ch: &Channel,
    m: &Message,
    width: u16,
    selected: bool,
    show_replies: bool,
    latest: bool,
) -> BlockOut {
    let mut lines = vec![];
    let mut links = vec![];
    let bar = if selected {
        Span::styled("▌", theme::bright())
    } else {
        Span::styled("▎", theme::rule())
    };
    let author = app.store.name_of(&m.author);
    let is_agent = app
        .store
        .profiles
        .get(&m.author)
        .is_some_and(|p| p.is_agent);
    let who = theme::name(&author);
    let mut head = vec![bar.clone(), Span::styled(author, who)];
    if is_agent {
        head.push(Span::styled(" ⚙", theme::faint()));
    }
    head.push(Span::styled(
        format!("  {}", time_label(m.created_at)),
        theme::faint(),
    ));
    // Agents answering this message: on the replies line when it has one,
    // else here, so a reply arriving doesn't shift anything twice.
    let working = if show_replies {
        thinking_label(app, &app.agents.working(&app.store, &ch.id, Some(&m.id)))
    } else {
        None
    };
    if ch.reply_count(&m.id) == 0 {
        if let Some(w) = &working {
            head.push(Span::styled(format!("  · {w}"), theme::bright()));
        }
    }
    if m.edited {
        head.push(Span::styled("  (edited)", theme::faint()));
    }
    if latest {
        head.push(Span::styled("  ◆ latest", theme::bright()));
    }
    lines.push(Line::from(head));
    let inner = width.saturating_sub(FRAME + 2).max(10) as usize;
    if m.deleted {
        lines.push(Line::from(vec![
            bar.clone(),
            Span::styled(
                "  message deleted",
                theme::faint().add_modifier(Modifier::ITALIC),
            ),
        ]));
    } else {
        let mention_names: Vec<String> = m
            .mentions
            .iter()
            .map(|pk| format!("@{}", app.store.name_of(pk)))
            .collect();
        // Attachments are drawn below, so drop their `![image](url)` lines.
        let body: String = m
            .content
            .split('\n')
            .filter(|l| {
                !m.media.iter().any(|x| {
                    l.trim().starts_with("![") && l.trim().ends_with(&format!("({})", x.url))
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let body = if m.system { system_text(&body) } else { body };
        let name = |c: &str| {
            app.store
                .channels
                .get(c)
                .filter(|c| !c.name.is_empty())
                .map(|c| c.name.clone())
        };
        let r = render_markdown_links(body.trim_end(), inner, &mention_names, &name);
        let top = lines.len();
        for (row, col, w, k) in r.at {
            links.push((
                (top + row) as u16,
                FRAME + col as u16,
                w as u16,
                k,
                r.urls[k].clone(),
            ));
        }
        for l in r.lines {
            let mut spans = vec![bar.clone(), Span::raw("  ")];
            spans.extend(l.spans);
            lines.push(Line::from(spans));
        }
    }
    let mut image = None;
    if !m.deleted {
        for media in &m.media {
            if media.mime.starts_with("image/") && image.is_none() && app.picker.is_some() {
                let cols = THUMB_COLS.min(width.saturating_sub(FRAME + 2));
                let rows = crate::media::thumb_rows(media.dim, cols, THUMB_MAX_ROWS);
                image = Some((lines.len() as u16, rows, media.url.clone(), cols));
                let label = match app.media.images.get(&media.url) {
                    Some(Slot::Failed(e)) => format!("▒ image unavailable: {e}"),
                    Some(Slot::Ready(_)) => String::new(),
                    _ => format!("▒ loading image {}", art::flight(app.tick, 6).trim_start()),
                };
                for r in 0..rows {
                    let txt = if r == rows / 2 {
                        label.clone()
                    } else {
                        String::new()
                    };
                    lines.push(Line::from(vec![
                        bar.clone(),
                        Span::raw("  "),
                        Span::styled(txt, theme::faint()),
                    ]));
                }
            } else {
                let name = media.url.rsplit('/').next().unwrap_or("file");
                lines.push(Line::from(vec![
                    bar.clone(),
                    Span::styled(format!("  ▣ {} {name}", media.mime), theme::muted()),
                ]));
            }
        }
    }
    if let Some(rs) = app.store.reactions.get(&m.id) {
        if !rs.is_empty() {
            let mut spans = vec![bar.clone(), Span::raw("  ")];
            for r in rs {
                let mine = r.by.contains_key(&app.store.me);
                let style = if mine {
                    theme::badge()
                } else {
                    theme::tab_idle()
                };
                spans.push(Span::styled(format!(" {} {} ", r.emoji, r.by.len()), style));
                spans.push(Span::raw(" "));
            }
            lines.push(Line::from(spans));
        }
    }
    if show_replies {
        let n = ch.reply_count(&m.id);
        if n > 0 {
            let last = ch
                .replies
                .get(&m.id)
                .and_then(|r| r.last())
                .and_then(|id| ch.messages.get(id));
            let last_at = last
                .map(|l| l.created_at)
                .max(ch.summary_replies.get(&m.id).map(|s| s.1));
            let who = last
                .filter(|l| Some(l.created_at) == last_at)
                .map(|l| format!("{} ", app.store.name_of(&l.author)))
                .unwrap_or_default();
            let when = last_at
                .filter(|t| *t > 0)
                .map(|t| format!("  last {who}{}", time_label(t)))
                .unwrap_or_default();
            let fresh =
                crate::unread::new_replies(ch, &m.id, &app.store.me, &app.read, crate::sync::now())
                    .len();
            lines.push(Line::from(vec![
                bar.clone(),
                Span::styled(
                    format!("↳ {n} {}", if n == 1 { "reply" } else { "replies" }),
                    theme::text().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if fresh > 0 {
                        format!(" ({fresh} new)")
                    } else {
                        String::new()
                    },
                    theme::bright().add_modifier(Modifier::BOLD),
                ),
                Span::styled(when, theme::faint()),
                Span::styled(
                    working.map(|w| format!("  · {w}")).unwrap_or_default(),
                    theme::bright(),
                ),
            ]));
        }
    }
    let name = app.store.name_of(&m.author);
    let mut out = frame(&name, lines, width, selected);
    out.push(Line::raw(""));
    BlockOut {
        id: m.id.clone(),
        lines: out,
        image,
        links,
    }
}

/// Header-rule layout: a bold initial, the first line (name, time, …),
/// then a dotted rule to the edge; the rest indented below. No fill. Each
/// line arrives as `[edge, maybe "  ", content…]`; the indent is dropped.
fn frame(name: &str, lines: Vec<Line<'static>>, width: u16, lit: bool) -> Vec<Line<'static>> {
    let hue = theme::hue(name);
    let edge = if lit {
        Span::styled("▌", Style::new().fg(theme::ink(hue, 23)))
    } else {
        Span::raw(" ")
    };
    let rest = |l: Line<'static>| {
        let mut s: Vec<Span> = l.spans.into_iter().skip(1).collect();
        if s.first()
            .is_some_and(|sp| sp.content == "  " || sp.content == " ")
        {
            s.remove(0);
        }
        s
    };
    let mut it = lines.into_iter();
    let initial: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    let mut top = vec![
        edge.clone(),
        Span::raw(" "),
        Span::styled(initial, theme::name(name)),
        Span::raw("  "),
    ];
    top.extend(rest(it.next().unwrap_or_default()));
    top.push(Span::raw(" "));
    let used: usize = top.iter().map(|sp| sp.content.width()).sum();
    top.push(Span::styled(
        "╌".repeat((width as usize).saturating_sub(used + 1)),
        Style::new().fg(theme::ink(hue, 6)),
    ));
    let mut out = vec![Line::from(top)];
    for l in it {
        let mut row = vec![edge.clone(), Span::raw("    ")];
        row.extend(rest(l));
        out.push(Line::from(row));
    }
    out
}

/// Columns before a message's content: edge, initial and spacing.
const FRAME: u16 = 5;

/// Blank an area for a popup. `Clear` alone leaves the terminal's own
/// background showing (e.g. a cream iTerm profile), so repaint ours.
fn clear(f: &mut Frame, area: Rect) {
    f.render_widget(Clear, area);
    f.render_widget(Block::new().style(Style::new().bg(theme::gray(0))), area);
}

/// Lays blocks out bottom-up and keeps the cursor block visible.
fn draw_blocks(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pane: Pane,
    blocks: Vec<BlockOut>,
    cursor: Option<&str>,
    images: &mut Vec<(Rect, (String, u16, u16))>,
) {
    let heights: Vec<usize> = blocks.iter().map(|b| b.lines.len()).collect();
    let total: usize = heights.iter().sum();
    let h = area.height as usize;
    app.page = (h / 3).max(1);
    let cur_idx = cursor.and_then(|c| blocks.iter().position(|b| b.id == c));
    // Default: newest at the bottom. With a cursor, scroll just enough.
    let mut top = total.saturating_sub(h);
    // The cursor has moved off the bell's landing: stop pinning it.
    if app
        .anchor
        .as_ref()
        .is_some_and(|(p, id)| *p == pane && Some(id.as_str()) != cursor)
    {
        app.anchor = None;
    }
    let anchored = |i: usize| {
        app.anchor
            .as_ref()
            .is_some_and(|(p, id)| *p == pane && *id == blocks[i].id)
    };
    if let Some(i) = cur_idx {
        let a: usize = heights[..i].iter().sum();
        let b = a + heights[i];
        if anchored(i) {
            // The bell's landing: this message at the top.
            top = a.min(top);
        }
        if b > top + h {
            top = b.saturating_sub(h);
        }
        if a < top {
            top = a;
        }
    }
    // The message under the cursor (the newest when following) is taller
    // than the pane: show the slice the pager offset asks for.
    let focus_idx =
        cur_idx.or_else(|| (cursor.is_none() && !blocks.is_empty()).then(|| blocks.len() - 1));
    let mut more = None;
    if pane == app.focus {
        app.tall = None;
    }
    if let Some(i) = focus_idx.filter(|&i| heights[i] > h) {
        let a: usize = heights[..i].iter().sum();
        let end = heights[i] - h;
        let id = &blocks[i].id;
        let off = match &app.peek {
            Some((p, o)) if p == id => (*o).min(end),
            // Following the newest shows its end; a picked one its start.
            _ if cur_idx.is_none() => end,
            _ => 0,
        };
        top = a + off;
        if pane == app.focus {
            app.tall = Some((id.clone(), heights[i], h, off));
        }
        more = Some((off, end - off));
    }
    let mut y = 0usize;
    for b in blocks {
        let bh = b.lines.len();
        let (start, end) = (y, y + bh);
        y = end;
        if end <= top || start >= top + h {
            continue;
        }
        let skip = top.saturating_sub(start);
        let row = area.y + (start + skip - top) as u16;
        let visible = (bh - skip).min(top + h - start - skip);
        let rect = Rect {
            x: area.x,
            y: row,
            width: area.width,
            height: visible as u16,
        };
        let lines: Vec<Line> = b.lines.into_iter().skip(skip).take(visible).collect();
        f.render_widget(Paragraph::new(lines), rect);
        app.hits.push((rect, Hit::Message(pane, b.id.clone())));
        for (r, c, w, k, url) in b.links {
            let r = r as usize;
            if r < skip || r >= skip + visible || c >= area.width {
                continue;
            }
            app.links.push(crate::app::LinkHit {
                rect: Rect {
                    x: area.x + c,
                    y: row + (r - skip) as u16,
                    width: w.min(area.width - c),
                    height: 1,
                },
                url,
                key: (b.id.clone(), k),
            });
        }
        if let Some((off, rows, url, cols)) = b.image {
            let off = off as usize;
            // Only draw a picture when it is fully on screen.
            if off >= skip && off + rows as usize <= skip + visible {
                let r = Rect {
                    x: area.x + FRAME,
                    y: row + (off - skip) as u16,
                    width: cols,
                    height: rows,
                };
                let key = (url.clone(), cols, rows);
                if !app.media.encoded.contains_key(&key) {
                    app.actions
                        .push(crate::app::Action::EncodeImage(key.clone()));
                }
                images.push((r, key));
            }
        }
    }
    // On a tall message the left edge becomes a scroll bar: a dim track the
    // pane's height, lit for the slice on screen.
    if let Some((above, below)) = more {
        let bh = h + above + below;
        let thumb = (h * h / bh).max(1);
        let at = h.saturating_sub(thumb) * above / (above + below);
        let rows: Vec<Line> = (0..h)
            .map(|r| {
                let lit = r >= at && r < at + thumb;
                Line::styled("▌", if lit { theme::bright() } else { theme::rule() })
            })
            .collect();
        f.render_widget(
            Paragraph::new(rows),
            Rect {
                width: 1.min(area.width),
                ..area
            },
        );
    }
    // How much of a tall message is off screen, on the pane's edges.
    if let Some((above, below)) = more {
        for (n, arrow, y) in [
            (above, "↑", area.y),
            (below, "↓", area.y + area.height.saturating_sub(1)),
        ] {
            if n == 0 {
                continue;
            }
            let tag = format!(
                " {arrow} {n} more {} ",
                if n == 1 { "line" } else { "lines" }
            );
            let w = (tag.width() as u16).min(area.width);
            f.render_widget(
                Paragraph::new(Span::styled(tag, theme::badge())),
                Rect {
                    x: area.x + area.width - w,
                    y,
                    width: w,
                    height: 1,
                },
            );
        }
    }
}

fn draw_timeline(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    images: &mut Vec<(Rect, (String, u16, u16))>,
) {
    let Some(id) = app.current().map(str::to_string) else {
        draw_empty(f, app, area, "no channels yet. press s to pick sections.");
        return;
    };
    let focused = app.focus == Pane::Timeline;
    // No title row: the tab says which channel. The row above the
    // messages is the operator loop's status line, when there is one.
    draw_status(f, app, Rect { height: 1, ..area }, &id, true);
    let area = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let Some(ch) = app.store.channels.get(&id) else {
        return;
    };
    if ch.timeline.is_empty() {
        let msg = if ch.loaded {
            art::EMPTY_LINES[(id.len() + ch.name.len()) % art::EMPTY_LINES.len()]
        } else {
            "loading…"
        };
        draw_empty(f, app, area, msg);
        return;
    }
    let cursor = app.selected.get(&id).cloned();
    let cursor_id = cursor.clone().or_else(|| ch.timeline.last().cloned());
    let latest = crate::unread::latest(ch);
    let start = ch.timeline.len().saturating_sub(400);
    let blocks: Vec<BlockOut> = ch.timeline[start..]
        .iter()
        .filter_map(|mid| ch.messages.get(mid))
        .map(|m| {
            let sel = focused
                && cursor_id.as_deref() == Some(m.id.as_str())
                && (cursor.is_some() || app.thread.is_none());
            let lit = latest.as_deref() == Some(m.id.as_str());
            message_block(app, ch, m, area.width, sel, true, lit)
        })
        .collect();
    draw_blocks(
        f,
        app,
        area,
        Pane::Timeline,
        blocks,
        cursor.as_deref(),
        images,
    );
}

fn draw_thread(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    images: &mut Vec<(Rect, (String, u16, u16))>,
) {
    let Some((ch_id, root)) = app.thread.clone() else {
        return;
    };
    let focused = app.focus == Pane::Thread;
    let Some(label) = app.store.channels.get(&ch_id).map(channel_label) else {
        return;
    };
    let working = app.thread.as_ref().and_then(|(ch, root)| {
        thinking_label(app, &app.agents.working(&app.store, ch, Some(root)))
    });
    let area = breadcrumb(f, app, area, &label, "thread", working);
    let Some(ch) = app.store.channels.get(&ch_id) else {
        return;
    };
    let mut ids = vec![root.clone()];
    ids.extend(ch.replies.get(&root).cloned().unwrap_or_default());
    let cursor = app.thread_selected.clone();
    let cursor_id = cursor.clone().or_else(|| ids.last().cloned());
    let blocks: Vec<BlockOut> = ids
        .iter()
        .filter_map(|mid| ch.messages.get(mid))
        .map(|m| {
            message_block(
                app,
                ch,
                m,
                area.width,
                focused && cursor_id.as_deref() == Some(m.id.as_str()),
                false,
                ids.last() == Some(&m.id),
            )
        })
        .collect();
    if blocks.is_empty() {
        draw_empty(f, app, area, "loading thread…");
        return;
    }
    draw_blocks(
        f,
        app,
        area,
        Pane::Thread,
        blocks,
        cursor.as_deref(),
        images,
    );
}

/// Breadcrumb for a view that has taken over the main pane:
/// `‹ #channel › thread`, the back arrow clickable, and a rule under it.
fn breadcrumb(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    channel: &str,
    leaf: &str,
    extra: Option<String>,
) -> Rect {
    let [t, rule, rest] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let mut line = Line::from(vec![
        Span::styled(" ‹ ", theme::key()),
        Span::styled(format!(" {channel}"), theme::muted()),
        Span::styled("  ›  ", theme::faint()),
        Span::styled(leaf.to_string(), theme::bright()),
        Span::styled("    h ← esc back", theme::faint()),
    ]);
    if let Some(w) = extra {
        line.push_span(Span::styled(format!("    {w}"), theme::bright()));
    }
    f.render_widget(Paragraph::new(line), t);
    let back = Rect {
        width: (4 + channel.chars().count() as u16).min(t.width),
        ..t
    };
    app.hits.push((back, Hit::CloseThread));
    f.render_widget(
        Block::new()
            .borders(Borders::TOP)
            .border_style(theme::rule()),
        rule,
    );
    rest
}

/// The garden in the middle of an empty or loading screen. While loading
/// it grows; an empty screen shows it grown and still.
fn draw_empty(f: &mut Frame, app: &mut App, area: Rect, msg: &str) {
    let grow = msg.starts_with("loading");
    let since = match app.empty_prev.take() {
        Some((m, t)) if m == msg => t,
        _ => app.tick,
    };
    app.empty = Some((msg.to_string(), since));
    let grid = garden::frame(app.tick - since, grow);
    let mut lines: Vec<Line> = vec![];
    let pad = area.height.saturating_sub(garden::HEIGHT as u16 + 2) / 2;
    for _ in 0..pad {
        lines.push(Line::raw(""));
    }
    for row in grid {
        let spans: Vec<Span> = row
            .into_iter()
            .enumerate()
            .map(|(x, (c, part))| {
                let style = match part {
                    garden::Part::Bloom => theme::bright().fg(theme::ink(theme::nth(x / 5), 22)),
                    garden::Part::Bee => theme::bright(),
                    garden::Part::Bud => theme::text(),
                    garden::Part::Stem | garden::Part::Leaf => theme::muted(),
                    garden::Part::Ground => theme::rule(),
                    garden::Part::Empty => Style::new(),
                };
                Span::styled(c.to_string(), style)
            })
            .collect();
        lines.push(Line::from(spans).centered());
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(msg.to_string(), theme::faint()).centered());
    f.render_widget(Paragraph::new(lines), area);
}

// ---------- agents ----------

/// Full-screen agents view (`a`): one card per online agent. Working
/// agents are bright with a chip and spinner; idle ones fade back.
fn draw_agents(f: &mut Frame, app: &mut App, area: Rect) {
    let label = app
        .current()
        .and_then(|c| app.store.channels.get(c))
        .map(channel_label)
        .unwrap_or_default();
    let area = breadcrumb(f, app, area, &label, "agents", None);
    let here = app.current().map(str::to_string);
    let rows = app.agents.rows(&app.store, here.as_deref());
    if rows.is_empty() {
        draw_empty(f, app, area, "none of your agents are online");
        return;
    }
    app.agents_cursor = app.agents_cursor.min(rows.len() - 1);
    let picked = app.agents_cursor;
    // Tool calls go in a right-hand column when there is room.
    let split = area.width >= 90;
    let right_w = if split { area.width / 3 } else { 0 };
    let left_w = area.width.saturating_sub(right_w + 2 + FRAME) as usize;
    let mut y = area.y + 1;
    for (i, r) in rows.into_iter().enumerate() {
        let edge = if r.busy {
            Span::styled("▌", theme::bright())
        } else {
            Span::styled("▎", theme::rule())
        };
        let name_style = if r.busy {
            theme::name(&r.name)
        } else {
            theme::muted().add_modifier(Modifier::BOLD)
        };
        let mut head = vec![
            edge.clone(),
            Span::raw(" "),
            Span::styled(r.name.clone(), name_style),
            Span::raw("  "),
        ];
        if r.busy {
            head.push(Span::styled(" WORKING ", theme::badge()));
            head.push(Span::styled(
                format!(" {}", SPIN[app.tick as usize % SPIN.len()]),
                theme::bright(),
            ));
        } else {
            head.push(Span::styled("[ IDLE ]", theme::faint()));
        }
        if i == picked {
            head.push(Span::styled("  r reset", theme::muted()));
        }
        let mut left = vec![Line::from(head)];
        let mut place = vec![];
        if let Some(s) = r.secs {
            place.push(format!("{s}s"));
        }
        if let Some(ch) = r.channel.as_ref().and_then(|c| app.store.channels.get(c)) {
            let mut p = channel_label(ch);
            if let Some(m) = r
                .thread
                .as_ref()
                .and_then(|t| app.store.find_message(t))
                .map(|(_, m)| m)
            {
                let first = m.content.lines().next().unwrap_or("");
                p.push_str(&format!(" › thread: {first}"));
            }
            place.push(p);
        }
        if let Some(c) = &r.elsewhere {
            let label = app
                .store
                .channels
                .get(c)
                .map(channel_label)
                .unwrap_or_else(|| "another channel".into());
            place.push(format!("working in {label}"));
        }
        if place.is_empty() {
            place.push("idle".into());
        }
        left.push(Line::from(vec![
            edge.clone(),
            Span::raw(" "),
            Span::styled(
                clip(&place.join(" · "), left_w),
                if r.busy {
                    theme::text()
                } else {
                    theme::faint()
                },
            ),
        ]));
        if !r.thought.is_empty() {
            left.push(Line::from(vec![
                edge.clone(),
                Span::raw(" "),
                Span::styled(clip(&r.thought, left_w), theme::muted()),
            ]));
        }
        let mut right: Vec<Line> = r
            .recent
            .iter()
            .take(3)
            .map(|a| {
                Line::styled(
                    format!(
                        "▸ {}",
                        clip(a, (right_w as usize).max(left_w).saturating_sub(4))
                    ),
                    if r.busy {
                        theme::text()
                    } else {
                        theme::faint()
                    },
                )
            })
            .collect();
        if let Some(c) = &r.context {
            right.push(Line::styled(format!("ctx {c}"), theme::muted()));
        }
        if let Some(t) = &r.last_turn {
            right.push(Line::styled(t.clone(), theme::faint()));
        }
        if !split {
            for l in right.drain(..) {
                let mut spans = vec![edge.clone(), Span::raw(" ")];
                spans.extend(l.spans);
                left.push(Line::from(spans));
            }
        }
        while left.len() < right.len() + 1 {
            left.push(Line::from(edge.clone()));
        }
        // The header row holds the head; keep the columns level.
        right.insert(0, Line::raw(""));
        let left = frame(&r.name, left, area.width, r.busy || i == picked);
        let h = left.len() as u16;
        if y + h > area.bottom() {
            break;
        }
        let card = Rect {
            x: area.x,
            y,
            width: area.width,
            height: h,
        };
        f.render_widget(Paragraph::new(left), card);
        if split {
            f.render_widget(
                Paragraph::new(right),
                Rect {
                    x: area.right().saturating_sub(right_w),
                    width: right_w.saturating_sub(1),
                    ..card
                },
            );
        }
        y += h + 1;
    }
}

// ---------- operator loop ----------

fn ago(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86_400),
    }
}

/// The creators of a contested channel's lists, by name.
fn contested_names(app: &App, creators: &[String]) -> String {
    creators
        .iter()
        .map(|c| app.store.name_of(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One channel's status line: mode, the watcher's state (and the ping it
/// sent, as a link), item counts, crashes today. Nothing when the channel
/// has no list, no watcher and no crashes.
fn draw_status(f: &mut Frame, app: &mut App, row: Rect, channel: &str, hint: bool) {
    let now = crate::sync::now();
    let sum = app
        .checklists
        .summary(channel, crate::checklist::local_midnight(now));
    let flags = crate::needs::answer_count(&app.needs);
    if row.height == 0 || (sum.is_empty() && flags == 0) {
        return;
    }
    let sep = || Span::styled("  ·  ", theme::faint());
    // ⚑ N: what needs an answer, in every channel; `!` goes through it.
    let flag = |spans: &mut Vec<Span<'static>>| {
        spans.push(Span::styled(format!("⚑ {flags}"), theme::bright()));
    };
    if sum.is_empty() {
        let mut spans = vec![Span::raw(" ")];
        flag(&mut spans);
        if hint {
            spans.push(Span::styled(
                "  need you · 0 to see · ! next",
                theme::faint(),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), row);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    if !sum.contested.is_empty() {
        // No list shows until the owner picks a driver.
        spans.push(Span::styled(
            format!("contested: {}", contested_names(app, &sum.contested)),
            theme::bright(),
        ));
    }
    match sum.mode {
        Some(crate::checklist::Mode::Running) => {
            spans.push(Span::styled(" RUNNING ", theme::badge()))
        }
        Some(m) => spans.push(Span::styled(format!("[ {} ]", m.label()), theme::muted())),
        None if !sum.contested.is_empty() => {}
        None => spans.push(Span::styled("no list", theme::faint())),
    }
    let mut link = None;
    if let Some(w) = &sum.watch {
        use crate::checklist::WatchState;
        spans.push(sep());
        let (glyph, style) = match w.state {
            WatchState::Working => (SPIN[app.tick as usize % SPIN.len()], theme::bright()),
            WatchState::Stalled => ("⚠", theme::bright()),
            WatchState::WaitingOnLucian | WatchState::WaitingOnRelease => ("⚑", theme::bright()),
            _ => ("○", theme::muted()),
        };
        let since = w
            .since
            .map(|t| format!(" {}", ago(now.saturating_sub(t))))
            .unwrap_or_default();
        spans.push(Span::styled(
            format!("{glyph} {}{since}", w.state.label()),
            style,
        ));
        if let Some(url) = &sum.ping_link {
            spans.push(Span::raw("  "));
            let x = row.x + Line::from(spans.clone()).width() as u16;
            let label = "ping ↗";
            link = Some((x, label.width() as u16, url.clone()));
            spans.push(Span::styled(label, theme::key()));
        }
    }
    let total = sum.open + sum.done + sum.parked;
    if total > 0 {
        spans.push(sep());
        spans.push(Span::styled(format!("{} open", sum.open), theme::text()));
        spans.push(Span::styled(
            format!(" · {} done", sum.done),
            theme::muted(),
        ));
        if sum.parked > 0 {
            spans.push(Span::styled(
                format!(" · {} parked", sum.parked),
                theme::muted(),
            ));
        }
    }
    if sum.crashes_today > 0 {
        spans.push(sep());
        let n = sum.crashes_today;
        spans.push(Span::styled(
            format!("✖ {n} crash{} today", if n == 1 { "" } else { "es" }),
            theme::bright(),
        ));
    }
    if flags > 0 {
        spans.push(Span::raw("   "));
        flag(&mut spans);
    }
    if hint && total > 0 {
        spans.push(Span::styled("    c checklist", theme::faint()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), row);
    if let Some((x, w, url)) = link {
        if x + w <= row.right() {
            app.links.push(crate::app::LinkHit {
                rect: Rect { x, width: w, ..row },
                url,
                key: ("status".into(), 0),
            });
        }
    }
}

/// The checklist pane (`c`): the channel's items in order, the picked
/// one's details, evidence and parked question beside or under them.
fn draw_checklist(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::checklist::Status;
    let Some(channel) = app.current().map(str::to_string) else {
        return;
    };
    let label = app
        .store
        .channels
        .get(&channel)
        .map(channel_label)
        .unwrap_or_default();
    let area = breadcrumb(f, app, area, &label, "checklist", None);
    draw_status(f, app, Rect { height: 1, ..area }, &channel, false);
    let area = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(2),
        ..area
    };
    let header = app.checklists.header(&channel);
    let items = app.checklists.items(&channel);
    if items.is_empty() && header.is_none() {
        draw_empty(
            f,
            app,
            area,
            "no checklist in this channel. agents start one with buzz checklist init",
        );
        return;
    }
    let mut top: Vec<Line> = vec![];
    if let Some(h) = &header {
        let mut l = vec![Span::styled(format!(" {}", h.title), theme::bright())];
        let r = &h.resume;
        let resume: Vec<&str> = [r.branch.as_str(), r.worktree.as_str(), r.worklog.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if !resume.is_empty() {
            l.push(Span::styled(
                format!("   {}", resume.join(" · ")),
                theme::faint(),
            ));
        }
        top.push(Line::from(l));
        top.push(Line::raw(""));
    }
    let top_h = top.len() as u16;
    f.render_widget(
        Paragraph::new(top),
        Rect {
            height: top_h,
            ..area
        },
    );
    let area = Rect {
        y: area.y + top_h,
        height: area.height.saturating_sub(top_h),
        ..area
    };
    if items.is_empty() {
        draw_empty(f, app, area, "no items yet");
        return;
    }
    app.checklist_cursor = app.checklist_cursor.min(items.len() - 1);
    let cur = app.checklist_cursor;
    // Details beside the list when there's room, under it otherwise.
    let wide = area.width >= 100;
    let (list, detail) = if wide {
        let [l, _, d] = Layout::horizontal([
            Constraint::Percentage(48),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .areas(area);
        (l, d)
    } else {
        let rows = (items.len() as u16 + 1).min(area.height / 2).max(3);
        let [l, d] = Layout::vertical([Constraint::Length(rows), Constraint::Min(0)]).areas(area);
        (l, d)
    };
    let rows = list.height as usize;
    let first = cur.saturating_sub(rows.saturating_sub(1));
    let mut lines = vec![];
    for (i, it) in items.iter().enumerate().skip(first).take(rows) {
        let (mark, st) = match it.status {
            Status::Open => ("○", theme::text()),
            Status::Done => ("✓", theme::muted()),
            Status::Parked => ("‖", theme::muted()),
        };
        let mut tail = String::new();
        if it.release {
            tail.push_str("  release");
        }
        if it.status == Status::Parked {
            tail.push_str("  parked");
        }
        if it.missing_evidence() {
            tail.push_str("  ⚠ no evidence");
        }
        let room = (list.width as usize).saturating_sub(tail.width() + 5);
        let row_style = if i == cur { theme::tab_active() } else { st };
        lines.push(Line::from(vec![
            Span::styled(format!(" {mark} "), row_style),
            Span::styled(clip(&it.text, room), row_style),
            Span::styled(tail, if i == cur { row_style } else { theme::faint() }),
        ]));
        app.hits.push((
            Rect {
                y: list.y + (i - first) as u16,
                height: 1,
                ..list
            },
            Hit::ChecklistItem(i),
        ));
    }
    f.render_widget(Paragraph::new(lines), list);
    // The picked item.
    let it = &items[cur];
    let w = detail.width.saturating_sub(2) as usize;
    let mut d: Vec<Line> = vec![];
    if !wide {
        d.push(Line::styled(
            "─".repeat(detail.width as usize),
            theme::rule(),
        ));
    }
    for l in wrap_plain(&it.text, w) {
        d.push(Line::styled(format!(" {l}"), theme::bright()));
    }
    let mut meta = vec![it.status.label().to_string()];
    if it.release {
        meta.push("release: needs lucian".into());
    }
    if let Some(s) = &it.standing {
        meta.push(format!("standing ({s})"));
    }
    meta.push(format!("by {}", app.store.name_of(&it.creator)));
    if it.signer != it.creator {
        meta.push(format!("edited by {}", app.store.name_of(&it.signer)));
    }
    meta.push(format!(
        "{} ago",
        ago(crate::sync::now().saturating_sub(it.updated_at))
    ));
    d.push(Line::styled(
        format!(" {}", meta.join(" · ")),
        theme::muted(),
    ));
    if let Some(q) = &it.parked_question {
        d.push(Line::raw(""));
        for l in wrap_plain(&format!("? {q}"), w) {
            d.push(Line::styled(format!(" {l}"), theme::bright()));
        }
    }
    // Evidence: msg and url entries are links (click, or f).
    let mut ev_links = vec![];
    if !it.evidence.is_empty() {
        d.push(Line::raw(""));
        d.push(Line::styled(" evidence", theme::faint()));
        for e in &it.evidence {
            let link = crate::checklist::evidence_link(&channel, e);
            let text = clip(e, w.saturating_sub(2));
            if let Some(url) = link {
                ev_links.push((d.len() as u16, text.width() as u16, url));
            }
            d.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(text, theme::text()),
            ]));
        }
    } else if it.missing_evidence() {
        d.push(Line::styled(" ⚠ ticked without evidence", theme::bright()));
    }
    if !it.details.trim().is_empty() {
        d.push(Line::raw(""));
        for l in render_markdown_links(&it.details, w, &[], &|_| None).lines {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(l.spans);
            d.push(Line::from(spans));
        }
    }
    let h = detail.height;
    for (row, width, url) in ev_links.into_iter().filter(|(r, _, _)| *r < h) {
        app.links.push(crate::app::LinkHit {
            rect: Rect {
                x: detail.x + 3,
                y: detail.y + row,
                width: width.min(detail.width.saturating_sub(3)),
                height: 1,
            },
            url,
            key: (format!("ev:{}", it.id), row as usize),
        });
    }
    f.render_widget(Paragraph::new(d), detail);
}

/// The Q box: a question for the agent in #quick, over everything.
fn draw_quick(f: &mut Frame, app: &mut App, area: Rect) {
    let l = &app.loop_;
    let title = format!(" Q · ask {} in #{} ", l.quick_agent, l.quick_channel);
    let here = app
        .quick_here()
        .and_then(|c| app.store.channels.get(c))
        .map(channel_label)
        .unwrap_or_default();
    let head = if app.quick_include {
        Line::from(vec![
            Span::styled(" + ", theme::badge()),
            Span::styled(
                format!(
                    " {here}: its id, link and latest {} messages",
                    crate::quick::RECENT
                ),
                theme::bright(),
            ),
        ])
    } else {
        let add = if here.is_empty() {
            String::new()
        } else {
            format!("  ^q again: + {here}")
        };
        Line::styled(
            format!(" a new thread in #{}.{add}", l.quick_channel),
            theme::faint(),
        )
    };
    let short = [
        ("⏎", "send"),
        (
            "^q",
            if app.quick_include {
                "drop channel"
            } else {
                "+ channel"
            },
        ),
        ("⇧⏎", "new line"),
        ("esc", "close"),
    ];
    draw_box(
        f,
        area,
        title,
        head,
        &app.quick_draft,
        "type your question",
        &short,
    );
}

/// `r` on tab 0: a reply to the picked row, into its thread.
fn draw_reply(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(r) = app.reply_to.clone() else {
        return;
    };
    let ch = app
        .store
        .channels
        .get(&r.channel)
        .map(channel_label)
        .unwrap_or_else(|| "#…".into());
    let title = if r.thread.is_some() {
        format!(" reply in {ch} › thread ")
    } else {
        format!(" message {ch} ")
    };
    let who = needs_who(app, &r);
    let head = Line::from(vec![
        Span::styled(format!(" {who}: "), theme::muted()),
        Span::styled(
            clip(
                &r.text,
                (area.width as usize).saturating_sub(who.width() + 12),
            ),
            theme::text(),
        ),
    ]);
    let short = [("⏎", "send"), ("⇧⏎", "new line"), ("esc", "close")];
    draw_box(
        f,
        area,
        title,
        head,
        &app.reply_draft,
        "type your reply",
        &short,
    );
}

/// A box over everything for a short text: a line saying where it goes,
/// the text, and its keys.
fn draw_box(
    f: &mut Frame,
    area: Rect,
    title: String,
    head: Line<'static>,
    d: &crate::draft::Draft,
    placeholder: &str,
    short: &[(&str, &str)],
) {
    let w = 76.min(area.width.saturating_sub(2));
    let cols = (w as usize).saturating_sub(4).max(4);
    let text_rows = d
        .text
        .split('\n')
        .map(|l| wrap_cols(&crate::draft::expand_tabs(l), cols).len())
        .sum::<usize>()
        .clamp(1, 8) as u16;
    let r = centered(area, w, text_rows + 6);
    clear(f, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::bright())
        .title(Span::styled(title, theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![head];
    lines.push(Line::raw(""));
    let text_top = inner.y + lines.len() as u16;
    if d.text.is_empty() {
        lines.push(Line::styled(format!(" {placeholder}"), theme::faint()));
    } else {
        for l in d.text.split('\n') {
            for l in wrap_cols(&crate::draft::expand_tabs(l), cols) {
                lines.push(Line::styled(format!(" {l}"), theme::text()));
            }
        }
    }
    let (cl, cc) = d.cursor_line_col();
    let cl = d
        .text
        .split('\n')
        .take(cl)
        .map(|l| wrap_cols(&crate::draft::expand_tabs(l), cols).len())
        .sum::<usize>()
        + cc / cols;
    let cc = cc % cols;
    let skip = (cl + 1).saturating_sub(text_rows as usize);
    let head: Vec<Line> = lines.drain(..2).collect();
    let body: Vec<Line> = lines
        .into_iter()
        .skip(skip)
        .take(text_rows as usize)
        .collect();
    let mut all = [head, body].concat();
    while all.len() < 2 + text_rows as usize + 1 {
        all.push(Line::raw(""));
    }
    // The footer has the long forms; here, what fits in the box.
    let mut legend = vec![Span::raw(" ")];
    for (i, (k, what)) in short.iter().enumerate() {
        legend.push(Span::styled(format!(" {k} "), theme::chip(theme::nth(i))));
        legend.push(Span::styled(format!(" {what}  "), theme::muted()));
    }
    all.push(Line::from(legend));
    f.render_widget(Paragraph::new(all), inner);
    f.set_cursor_position((
        (inner.x + 1 + cc as u16).min(inner.right().saturating_sub(1)),
        (text_top + cl.saturating_sub(skip) as u16).min(inner.bottom().saturating_sub(1)),
    ));
}

/// Who a tab-0 row is from: their name, or the watcher's when nobody is.
fn needs_who(app: &App, r: &crate::needs::Row) -> String {
    if let crate::needs::Reason::Contested(c) = &r.reason {
        contested_names(app, c)
    } else if r.who.is_empty() {
        "watcher".into()
    } else {
        app.store.name_of(&r.who)
    }
}

/// Tab 0, Needs you: what waits on the operator, then FYI (newest first, latest 20).
/// One row each: age, channel, who, one line.
fn draw_needs(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::needs::{Reason, Section};
    let now = crate::sync::now();
    let rows = app.needs.clone();
    let answer = crate::needs::answer_count(&rows);
    let [title, rule, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ⚑ needs you", theme::bright()),
            Span::styled(
                format!("   {answer} need an answer · {} FYI", rows.len() - answer),
                theme::muted(),
            ),
            Span::styled("    ⏎ open  r reply  ! next", theme::faint()),
        ])),
        title,
    );
    f.render_widget(
        Block::new()
            .borders(Borders::TOP)
            .border_style(theme::rule()),
        rule,
    );
    if rows.is_empty() {
        draw_empty(f, app, body, "nothing needs you");
        return;
    }
    app.needs_cursor = app.needs_cursor.min(rows.len() - 1);
    let cur = app.needs_cursor;
    let width = body.width as usize;
    // (line, row index) with the section headings in between.
    let mut lines: Vec<(Line<'static>, Option<usize>)> = vec![];
    let mut last = None;
    for (i, r) in rows.iter().enumerate() {
        if last != Some(r.section) {
            if last.is_some() {
                lines.push((Line::raw(""), None));
            }
            let (name, n) = match r.section {
                Section::Answer => ("needs an answer", answer),
                Section::Fyi => ("FYI", rows.len() - answer),
            };
            lines.push((
                Line::styled(format!(" {name} ({n})"), theme::bright()),
                None,
            ));
            last = Some(r.section);
        }
        let mark = match &r.reason {
            Reason::Watch(crate::checklist::WatchState::Stalled) => "⚠",
            Reason::Watch(_) => "⚑",
            Reason::Parked => "‖",
            Reason::Contested(_) => "⚑",
            Reason::Mention => "@",
            Reason::Dm => "✉",
            Reason::Bot => "·",
        };
        let ch = app
            .store
            .channels
            .get(&r.channel)
            .map(channel_label)
            .unwrap_or_else(|| format!("#{}", r.channel.chars().take(8).collect::<String>()));
        let head = format!(
            " {mark} {:>5}  {}  {}  ",
            ago(now.saturating_sub(r.at)),
            pad_to(&clip(&ch, 20), 19),
            pad_to(&clip(&needs_who(app, r), 13), 12),
        );
        let text = clip(&r.text, width.saturating_sub(head.width()));
        let (hs, ts) = if i == cur {
            (theme::tab_active(), theme::tab_active())
        } else if r.section == Section::Answer {
            (theme::muted(), theme::text())
        } else {
            (theme::faint(), theme::muted())
        };
        lines.push((
            Line::from(vec![
                Span::styled(head, hs),
                Span::styled(pad_to(&text, width.saturating_sub(0)), ts),
            ]),
            Some(i),
        ));
    }
    let h = body.height as usize;
    let at = lines.iter().position(|(_, i)| *i == Some(cur)).unwrap_or(0);
    let first = at.saturating_sub(h.saturating_sub(1));
    let mut out = vec![];
    for (y, (l, i)) in lines.into_iter().skip(first).take(h).enumerate() {
        if let Some(i) = i {
            app.hits.push((
                Rect {
                    y: body.y + y as u16,
                    height: 1,
                    ..body
                },
                Hit::NeedsRow(i),
            ));
        }
        out.push(l);
    }
    f.render_widget(Paragraph::new(out), body);
}

fn clip(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w.saturating_sub(1) {
            out.push('…');
            return out;
        }
        if c == '\n' {
            out.push(' ');
        } else {
            out.push(c);
        }
        used += cw;
    }
    out
}

// ---------- compose + footer ----------

fn compose_height(app: &App, width: u16) -> u16 {
    let d = app.draft_ref();
    let w = compose_cols(width);
    let lines = d
        .text
        .split('\n')
        .map(|l| wrap_cols(&crate::draft::expand_tabs(l), w).len())
        .sum::<usize>() as u16;
    let att = if d.attachments.is_empty() { 0 } else { 1 };
    lines.clamp(1, 8) + att + 2
}

/// Text columns in the compose box: one for the left pad, one so the
/// cursor fits at the end of a full row.
fn compose_cols(width: u16) -> usize {
    (width as usize).saturating_sub(4).max(4)
}

/// Hard-wrap one line at `w` columns. A line that exactly fills its last row
/// gets an empty row after it, which is where the cursor goes next.
fn wrap_cols(s: &str, w: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            rows.push(String::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(c);
        used += cw;
    }
    if used == w {
        rows.push(String::new());
    }
    rows
}

fn draw_compose(f: &mut Frame, app: &mut App, area: Rect) {
    app.hits.push((area, Hit::Compose));
    let insert = app.mode == Mode::Insert;
    let target = match (&app.thread, app.focus) {
        (Some(_), Pane::Thread) => "reply in thread".to_string(),
        _ => app
            .current()
            .and_then(|c| app.store.channels.get(c))
            .map(|c| format!("message {}", channel_label(c)))
            .unwrap_or_default(),
    };
    let border = if insert { theme::text() } else { theme::rule() };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Span::styled(
            format!(" {target} "),
            if insert {
                theme::bright()
            } else {
                theme::faint()
            },
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let d = app.draft_ref();
    let mut lines: Vec<Line> = vec![];
    if !d.attachments.is_empty() {
        let names: Vec<String> = d
            .attachments
            .iter()
            .map(|a| {
                format!(
                    "▣ {} {}",
                    a.mime_type,
                    crate::clip::human_size(a.size as usize)
                )
            })
            .collect();
        lines.push(Line::styled(
            format!(" {}", names.join("  ")),
            theme::muted(),
        ));
    }
    let text_top = inner.y + lines.len() as u16;
    let cols = compose_cols(area.width);
    if d.text.is_empty() && !insert {
        lines.push(Line::styled(
            " press i to write · ^v pastes an image",
            theme::faint(),
        ));
    } else {
        // Shade ``` blocks while typing so the message looks like what
        // everyone else will see.
        let mut fence = false;
        for l in d.text.split('\n') {
            let is_fence = l.trim_start().starts_with("```");
            let shaded = fence || is_fence;
            if is_fence {
                fence = !fence;
            }
            for l in wrap_cols(&crate::draft::expand_tabs(l), cols) {
                lines.push(if shaded {
                    Line::from(Span::styled(
                        pad_to(&format!(" {l}"), inner.width as usize),
                        if is_fence {
                            theme::code_label()
                        } else {
                            theme::code()
                        },
                    ))
                } else {
                    Line::styled(format!(" {l}"), theme::text())
                });
            }
        }
    }
    // Long lines wrap, so turn the cursor's line and column into the row
    // and column it sits at on screen.
    let (cl, cc) = d.cursor_line_col();
    let cl = d
        .text
        .split('\n')
        .take(cl)
        .map(|l| wrap_cols(&crate::draft::expand_tabs(l), cols).len())
        .sum::<usize>()
        + cc / cols;
    let cc = cc % cols;
    let visible = inner.height.saturating_sub(text_top - inner.y) as usize;
    let skip = (cl + 1).saturating_sub(visible);
    let head: Vec<Line> = lines
        .iter()
        .take((text_top - inner.y) as usize)
        .cloned()
        .collect();
    let body: Vec<Line> = lines
        .into_iter()
        .skip((text_top - inner.y) as usize + skip)
        .collect();
    f.render_widget(Paragraph::new([head, body].concat()), inner);
    if let Some((i, at)) = app.sent_flash {
        if at.elapsed() < Duration::from_millis(1400) {
            let s = format!(" {} ", art::SENT[i % art::SENT.len()]);
            let w = s.width() as u16;
            f.render_widget(
                Paragraph::new(Span::styled(s, theme::badge())),
                Rect {
                    x: inner.x + inner.width.saturating_sub(w + 1),
                    y: inner.y,
                    width: w,
                    height: 1,
                },
            );
        }
    }
    if insert {
        let x = inner.x + 1 + cc as u16;
        let y = text_top + cl.saturating_sub(skip) as u16;
        f.set_cursor_position((
            x.min(inner.right().saturating_sub(1)),
            y.min(inner.bottom().saturating_sub(1)),
        ));
        let cands = app.mention_candidates();
        if !cands.is_empty() {
            let h = cands.len() as u16 + 2;
            let w = cands.iter().map(|(n, _)| n.width()).max().unwrap_or(8) as u16 + 6;
            let r = Rect {
                x: x.saturating_sub(1).min(area.right().saturating_sub(w)),
                y: area.y.saturating_sub(h),
                width: w,
                height: h,
            };
            clear(f, r);
            let items: Vec<Line> = cands
                .iter()
                .enumerate()
                .map(|(i, (n, _))| {
                    let st = if i == app.mention_pick.min(cands.len() - 1) {
                        theme::tab_active()
                    } else {
                        theme::text()
                    };
                    Line::styled(format!(" @{n} "), st)
                })
                .collect();
            f.render_widget(
                Paragraph::new(items).block(
                    Block::bordered()
                        .border_style(theme::muted())
                        .title(Span::styled(" ⇥ pick ", theme::faint())),
                ),
                r,
            );
        }
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(format!(" {} ", app.mode.label()), theme::badge()),
        Span::raw(" "),
    ];
    let status = app
        .status
        .as_ref()
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(6))
        .map(|(s, _)| s.clone());
    if let (Mode::Reset, Some((_, name, _, _))) = (app.mode, &app.reset_ask) {
        spans.push(Span::styled(
            format!("reset {name}'s session here? "),
            theme::bright(),
        ));
    } else if let (Mode::Archive, Some(c)) = (app.mode, &app.archive_ask) {
        let name = app
            .store
            .channels
            .get(c)
            .map_or_else(String::new, channel_label);
        spans.push(Span::styled(
            format!("archive {name} for everyone? "),
            theme::bright(),
        ));
    } else if app.uploading {
        spans.push(Span::styled(
            format!("uploading {}", art::flight(app.tick, 8)),
            theme::bright(),
        ));
    } else if let Some(s) = &status {
        spans.push(Span::styled(s.clone(), theme::bright()));
    }
    // Where Backspace goes: each step back, then where you are now. It
    // outranks the key legend, which drops entries to make room.
    let here = app.here();
    let chain = if app.jumps.is_empty() {
        None
    } else {
        let mut all: Vec<String> = app
            .jumps
            .iter()
            .chain(here.as_ref())
            .map(|j| {
                let name = app
                    .store
                    .channels
                    .get(&j.channel)
                    .map(channel_label)
                    .unwrap_or_else(|| "…".into());
                if j.thread.is_some() {
                    format!("{name} thread")
                } else {
                    name
                }
            })
            .collect();
        // Keep the newest steps when the line is too narrow.
        let room = (area.width as usize).saturating_sub(Line::from(spans.clone()).width() + 2);
        let mut text = format!("⌫ {} ", all.join(" › "));
        while text.width() > room && all.len() > 1 {
            all.remove(0);
            text = format!("⌫ … › {} ", all.join(" › "));
        }
        (text.width() <= room).then_some(text)
    };
    // Context of the agent working here (or last to), bottom right, when
    // it fits after the chain.
    let usage = app
        .current()
        .and_then(|c| {
            let root = app
                .thread
                .as_ref()
                .filter(|t| t.0 == c)
                .map(|t| t.1.as_str());
            app.agents.context_here(&app.store, c, root)
        })
        .map(|u| format!(" {u} "))
        .filter(|u| {
            Line::from(spans.clone()).width()
                + chain.as_ref().map_or(0, |c| c.width() + 1)
                + u.width()
                <= area.width as usize
        });
    let usage_w = usage.as_ref().map_or(0, |u| u.width() as u16);
    if (!app.uploading && status.is_none()) || matches!(app.mode, Mode::Reset | Mode::Archive) {
        let mut room = (area.width as usize).saturating_sub(
            Line::from(spans.clone()).width()
                + chain.as_ref().map_or(0, |c| c.width() + 1)
                + usage_w as usize,
        );
        for (i, (k, what, _)) in legend(app.mode).iter().filter(|l| l.2).enumerate() {
            let (key, desc) = (format!(" {k} "), format!(" {what}  "));
            let w = key.width() + desc.width();
            if (chain.is_some() || usage.is_some()) && w > room {
                break;
            }
            room = room.saturating_sub(w);
            spans.push(Span::styled(key, theme::chip(theme::nth(i))));
            spans.push(Span::styled(desc, theme::muted()));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some(u) = usage {
        f.render_widget(
            Paragraph::new(Span::styled(u, theme::muted())),
            Rect {
                x: area.right() - usage_w,
                width: usage_w,
                ..area
            },
        );
    }
    if let Some(text) = chain {
        let w = text.width() as u16;
        f.render_widget(
            Paragraph::new(Span::styled(text, theme::muted())),
            Rect {
                x: area.right() - usage_w - w,
                width: w,
                ..area
            },
        );
    }
}

// ---------- overlays ----------

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_sections(f: &mut Frame, app: &mut App, area: Rect) {
    let h = (app.sections.len() as u16 + 11).max(14);
    let r = centered(area, 60, h);
    clear(f, r);
    let block = Block::bordered()
        .border_style(theme::text())
        .title(Span::styled(" your sections ", theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![
        Line::styled(" pick what you're working on today", theme::faint()),
        Line::raw(""),
    ];
    if app.sections.is_empty() {
        let msg = if app.sections_loaded {
            " no sections found. make some in Buzz Desktop, then R."
        } else {
            " loading sections…"
        };
        lines.push(Line::styled(msg, theme::muted()));
    }
    // Rows left for the list after the heading and the legend. A long list
    // scrolls to keep the cursor in view.
    let rows = (inner.height as usize).saturating_sub(8).max(1);
    let first = app
        .picker_cursor
        .saturating_sub(rows / 2)
        .min(app.sections.len().saturating_sub(rows));
    for (row, (i, s)) in app
        .sections
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .enumerate()
    {
        let mark = if app.picker_marks.contains(&s.id) {
            "[x]"
        } else {
            "[ ]"
        };
        let st = if i == app.picker_cursor {
            theme::tab_active()
        } else {
            theme::text()
        };
        let line = Line::from(vec![
            Span::styled(format!(" {mark} {} ", s.name), st),
            Span::styled(
                format!(
                    "  {} {}",
                    s.channels.len(),
                    if s.channels.len() == 1 {
                        "channel"
                    } else {
                        "channels"
                    }
                ),
                theme::faint(),
            ),
        ]);
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + 2 + row as u16,
                width: inner.width,
                height: 1,
            },
            Hit::Section(i),
        ));
        lines.push(line);
    }
    // Legend: Enter alone switches; marks are only for several at once.
    lines.push(Line::raw(""));
    for (i, (k, what)) in [
        ("j k", "move"),
        ("⏎", "switch to this section"),
        ("space", "mark [x] to show several, then ⏎"),
        ("m", "move this channel into it"),
        ("R", "reload  ·  esc  cancel"),
    ]
    .into_iter()
    .enumerate()
    {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(format!(" {k} "), theme::chip(theme::nth(i))),
            Span::styled(format!("  {what}"), theme::muted()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// T: today's tabs top to bottom, each with its section. The order is
/// synced, so the phone shows it too.
fn draw_order(f: &mut Frame, app: &mut App, area: Rect) {
    let h = (app.tabs.len() as u16 + 9).max(12);
    let r = centered(area, 60, h);
    clear(f, r);
    let block = Block::bordered()
        .border_style(theme::text())
        .title(Span::styled(" tab order ", theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let mut lines = vec![
        Line::styled(" synced with the phone", theme::faint()),
        Line::raw(""),
    ];
    let legend = crate::keys::legend(Mode::Order);
    let rows = (inner.height as usize)
        .saturating_sub(3 + legend.len())
        .max(1);
    let first = app
        .order_cursor
        .saturating_sub(rows / 2)
        .min(app.tabs.len().saturating_sub(rows));
    for (i, id) in app.tabs.iter().enumerate().skip(first).take(rows) {
        let name = app
            .store
            .channels
            .get(id)
            .map_or_else(|| id.chars().take(8).collect(), channel_label);
        let section = app
            .sections
            .iter()
            .filter(|s| app.chosen.is_empty() || app.chosen.contains(&s.id))
            .find(|s| s.channels.contains(id))
            .map_or("", |s| s.name.as_str());
        let st = if i == app.order_cursor {
            theme::tab_active()
        } else {
            theme::text()
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {name} "), st),
            Span::styled(format!("  {section}"), theme::faint()),
        ]));
    }
    lines.push(Line::raw(""));
    for (i, (k, what, _)) in legend.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(format!(" {k} "), theme::chip(theme::nth(i))),
            Span::styled(format!("  {what}"), theme::muted()),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(p) = &app.pending else { return };
    let r = centered(area, 72, 12);
    clear(f, r);
    let block = Block::bordered()
        .border_style(theme::bright())
        .title(Span::styled(" upload this? ", theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let dims = p
        .dims
        .map(|(w, h)| format!(" · {w}×{h}"))
        .unwrap_or_default();
    let host = app.relay_host.clone();
    let lines = vec![
        Line::from(vec![
            Span::styled(" from   ", theme::faint()),
            Span::styled(p.source, theme::text()),
        ]),
        Line::from(vec![
            Span::styled(" file   ", theme::faint()),
            Span::styled(p.name.clone(), theme::bright()),
        ]),
        Line::from(vec![
            Span::styled(" type   ", theme::faint()),
            Span::styled(
                format!("{} · {}{dims}", p.mime, p.size_label()),
                theme::text(),
            ),
        ]),
        Line::from(vec![
            Span::styled(" goes to ", theme::faint()),
            Span::styled(format!("{host}/upload (Buzz media)"), theme::text()),
        ]),
        Line::raw(""),
        Line::styled(
            " it attaches to your draft; nothing is sent until you press ⏎",
            theme::faint(),
        ),
        Line::raw(""),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    // Thumbnail on the right, for images.
    let key = (p.preview_key(), 14, 5);
    let thumb = Rect {
        x: inner.right().saturating_sub(16),
        y: inner.y,
        width: 14,
        height: 5,
    };
    if let Some(proto) = app.media.encoded.get(&key) {
        f.render_widget(Image::new(proto), thumb);
    } else if p.mime.starts_with("image/") && app.picker.is_some() {
        app.actions.push(crate::app::Action::EncodeImage(key));
    }
    let y = inner.y + inner.height.saturating_sub(1);
    let yes = Rect {
        x: inner.x + 1,
        y,
        width: 16,
        height: 1,
    };
    let no = Rect {
        x: inner.x + 19,
        y,
        width: 12,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Span::styled("  y  upload  ", theme::tab_active())),
        yes,
    );
    f.render_widget(
        Paragraph::new(Span::styled("  n  cancel  ", theme::tab_idle())),
        no,
    );
    app.hits.push((yes, Hit::ConfirmYes));
    app.hits.push((no, Hit::ConfirmNo));
}

fn draw_help(f: &mut Frame, area: Rect) {
    let right = [
        Mode::Insert,
        Mode::Sections,
        Mode::Order,
        Mode::Confirm,
        Mode::Quick,
        Mode::Reply,
    ];
    let tall = (legend(Mode::Normal).len() + 2)
        .max(right.iter().map(|m| legend(*m).len() + 2).sum::<usize>());
    let r = centered(area, 84, tall as u16 + art::BANNER.len() as u16 + 3);
    clear(f, r);
    let block = Block::bordered()
        .border_style(theme::text())
        .title(Span::styled(" keys ", theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let banner: Vec<Line> = art::BANNER
        .iter()
        .map(|l| Line::styled(format!("  {l}"), theme::bright()))
        .collect();
    let [top, body] = Layout::vertical([
        Constraint::Length(banner.len() as u16 + 1),
        Constraint::Min(0),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(banner), top);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let column = |modes: &[(Mode, &str)]| {
        let mut lines: Vec<Line> = vec![];
        for (mode, title) in modes {
            lines.push(Line::styled(format!(" {title}"), theme::bright()));
            for (k, what, _) in legend(*mode) {
                lines.push(Line::from(vec![
                    Span::styled(format!("   {k:<9} "), theme::text()),
                    Span::styled(what.to_string(), theme::muted()),
                ]));
            }
            lines.push(Line::raw(""));
        }
        lines
    };
    f.render_widget(Paragraph::new(column(&[(Mode::Normal, "normal")])), left);
    f.render_widget(
        Paragraph::new(column(&[
            (Mode::Insert, "insert (i)"),
            (Mode::Sections, "sections (s)"),
            (Mode::Order, "tab order (T)"),
            (Mode::Confirm, "upload"),
            (Mode::Quick, "Q box (Q ^q)"),
            (Mode::Reply, "reply (r on tab 0)"),
        ])),
        right,
    );
}

fn draw_viewer(f: &mut Frame, app: &mut App, area: Rect) {
    // Takes the whole screen; the image gets every cell but the title row.
    clear(f, area);
    let [title, inner] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let n = app.viewer_list.len();
    let at = app
        .viewer
        .as_ref()
        .and_then(|u| app.viewer_list.iter().position(|x| x == u))
        .map_or(0, |i| i + 1);
    // Same key chips as the footer, so the legend reads at a glance.
    let mut spans = vec![Span::styled(format!(" image {at}/{n}  "), theme::bright())];
    let mut keys = vec![];
    if n > 1 {
        keys.push(("← →", "move"));
    }
    keys.push(("esc", "close"));
    for (i, (k, what)) in keys.into_iter().enumerate() {
        spans.push(Span::styled(format!(" {k} "), theme::chip(theme::nth(i))));
        spans.push(Span::styled(format!(" {what}  "), theme::muted()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), title);
    let Some(url) = app.viewer.clone() else {
        return;
    };
    let key = (url.clone(), inner.width, inner.height);
    if let Some(p) = app.media.encoded.get(&key) {
        // Fit keeps the aspect ratio, so it rarely fills both ways: center it.
        let sz = p.size();
        let (w, h) = (sz.width.min(inner.width), sz.height.min(inner.height));
        let r = Rect {
            x: inner.x + (inner.width - w) / 2,
            y: inner.y + (inner.height - h) / 2,
            width: w,
            height: h,
        };
        f.render_widget(Image::new(p), r);
        return;
    }
    // A failed download or decode used to sit on "loading" forever.
    let msg = match app.media.images.get(&url) {
        Some(Slot::Failed(e)) => {
            Line::styled(format!("couldn't load this image: {e}"), theme::muted())
        }
        _ => {
            app.actions.push(crate::app::Action::EncodeImage(key));
            // Bytes so far, so slow and stuck look different.
            let got = match app.media.progress.get(&url) {
                Some(&(got, total)) => format!(" {}", crate::media::progress_label(got, total)),
                None => String::new(),
            };
            Line::styled(
                format!("loading{got} {}", art::flight(app.tick, 10)),
                theme::muted(),
            )
        }
    };
    let mid = Rect {
        y: inner.y + inner.height / 2,
        height: 1.min(inner.height),
        ..inner
    };
    f.render_widget(Paragraph::new(msg.centered()), mid);
}

// ---------- text ----------

/// Kind 40099 rows carry JSON like `{"type":"member_joined",...}`.
fn system_text(raw: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
    match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => format!("_{}_", t.replace('_', " ")),
        None => raw.to_string(),
    }
}

/// A span and the link it belongs to, if any.
type Tagged = (Span<'static>, Option<usize>);

/// Rendered message text and where its links landed.
#[derive(Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub urls: Vec<String>,
    /// (row, column, width, index into `urls`), one per piece of a link, so
    /// a link wrapped over two lines has two.
    pub at: Vec<(usize, usize, usize, usize)>,
}

#[cfg(test)]
pub fn render_markdown(src: &str, width: usize, mentions: &[String]) -> Vec<Line<'static>> {
    render_markdown_links(src, width, mentions, &|_| None).lines
}

/// Just enough markdown for chat: code fences, `inline code`, **bold**,
/// _italic_, @mentions and links. Wraps to `width` columns. `channel_name`
/// names the channels bare `buzz://` links point at.
pub fn render_markdown_links(
    src: &str,
    width: usize,
    mentions: &[String],
    channel_name: &dyn Fn(&str) -> Option<String>,
) -> Rendered {
    let mut r = Rendered::default();
    let mut out = vec![];
    let mut in_fence = false;
    let code_line = |out: &mut Vec<Line<'static>>, raw: &str| {
        let raw = crate::draft::expand_tabs(raw);
        for chunk in wrap_plain(&raw, width.saturating_sub(2)) {
            out.push(Line::from(Span::styled(
                pad_to(&format!(" {chunk}"), width),
                theme::code(),
            )));
        }
    };
    for raw in src.split('\n') {
        let t = raw.trim();
        if let Some(fence) = t.strip_prefix("```") {
            // ```one-liner``` on a single line.
            if !in_fence && fence.len() > 3 && fence.ends_with("```") {
                code_line(&mut out, fence.trim_end_matches('`'));
                continue;
            }
            in_fence = !in_fence;
            // The language tag, if any, labels the top of the block.
            if in_fence && !fence.trim().is_empty() {
                out.push(Line::from(Span::styled(
                    pad_to(&format!(" {}", fence.trim()), width),
                    theme::code_label(),
                )));
            }
            continue;
        }
        if in_fence {
            code_line(&mut out, raw);
            continue;
        }
        let (prefix, body) =
            if let Some(rest) = raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
                ("• ", rest)
            } else if let Some(rest) = raw.strip_prefix("> ") {
                ("│ ", rest)
            } else if let Some(rest) = raw
                .trim_start_matches('#')
                .strip_prefix(' ')
                .filter(|_| raw.starts_with('#'))
            {
                ("", rest)
            } else {
                ("", raw)
            };
        let heading = raw.starts_with('#');
        let spans = inline(body, mentions, heading, &mut r.urls, channel_name);
        for (i, l) in wrap_spans(spans, width.saturating_sub(prefix.width()))
            .into_iter()
            .enumerate()
        {
            let mut s = vec![Span::styled(
                if i == 0 {
                    prefix.to_string()
                } else {
                    " ".repeat(prefix.width())
                },
                theme::muted(),
            )];
            let mut col = prefix.width();
            for (span, link) in l {
                let w = span.content.width();
                if let Some(k) = link {
                    r.at.push((out.len(), col, w, k));
                }
                col += w;
                s.push(span);
            }
            out.push(Line::from(s));
        }
    }
    r.lines = out;
    r
}

fn inline(
    s: &str,
    mentions: &[String],
    heading: bool,
    urls: &mut Vec<String>,
    channel_name: &dyn Fn(&str) -> Option<String>,
) -> Vec<Tagged> {
    let base = if heading {
        theme::bright()
    } else {
        theme::text()
    };
    let mut out = vec![];
    let mut buf = String::new();
    let mut rest = s;
    let flush = |buf: &mut String, out: &mut Vec<Tagged>| {
        if !buf.is_empty() {
            out.push((Span::styled(std::mem::take(buf), base), None));
        }
    };
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            // A run of n backticks closes on the next run of exactly n, so
            // ``a `tick` b`` works.
            let n = rest.chars().take_while(|&c| c == '`').count();
            let tick = "`".repeat(n);
            let close = rest[n..]
                .match_indices(&tick)
                .map(|(i, _)| n + i)
                .find(|&i| !rest[i + n..].starts_with('`') && !rest[..i].ends_with('`'));
            if let Some(end) = close {
                flush(&mut buf, &mut out);
                let code = rest[n..end].to_string();
                let code = match code.strip_prefix(' ').and_then(|c| c.strip_suffix(' ')) {
                    Some(c) if !c.trim().is_empty() => c.to_string(),
                    _ => code,
                };
                out.push((Span::styled(code, theme::code()), None));
                rest = &rest[end + n..];
                continue;
            }
            buf.push_str(&tick);
            rest = &rest[n..];
            continue;
        }
        if rest.starts_with("**") {
            if let Some(end) = rest[2..].find("**") {
                flush(&mut buf, &mut out);
                out.push((
                    Span::styled(
                        rest[2..2 + end].to_string(),
                        base.add_modifier(Modifier::BOLD)
                            .fg(theme::tone(theme::Role::Text, 23)),
                    ),
                    None,
                ));
                rest = &rest[end + 4..];
                continue;
            }
        }
        if c == '@' {
            if let Some(m) = mentions
                .iter()
                .filter(|m| rest.starts_with(m.as_str()))
                .max_by_key(|m| m.len())
            {
                flush(&mut buf, &mut out);
                out.push((
                    Span::styled(
                        m.clone(),
                        theme::mention().fg(theme::ink(theme::hue(&m[1..]), 18)),
                    ),
                    None,
                ));
                rest = &rest[m.len()..];
                continue;
            }
        }
        // A link starts at a word boundary.
        let boundary = !buf
            .chars()
            .last()
            .is_some_and(|p| p.is_alphanumeric() || p == '/');
        if matches!(c, '[' | 'h' | 'b') && boundary {
            if let Some(found) = crate::links::at(rest) {
                flush(&mut buf, &mut out);
                let text = if found.text == found.url {
                    crate::links::label(&found.url, channel_name).unwrap_or(found.text)
                } else {
                    found.text
                };
                let desktop = crate::links::target(&found.url) == crate::links::Target::Desktop;
                out.push((Span::styled(text, theme::link()), Some(urls.len())));
                // The bee says Buzz Desktop opens this one.
                if desktop {
                    out.push((Span::styled(" 🐝", base), None));
                }
                urls.push(found.url);
                rest = &rest[found.end..];
                continue;
            }
        }
        buf.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut buf, &mut out);
    out
}

/// Right-pad with spaces to `width` display columns, so a background
/// colour fills the whole row instead of stopping at the text.
fn pad_to(s: &str, width: usize) -> String {
    let w = s.width();
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

fn wrap_plain(s: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = vec![String::new()];
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            out.push(String::new());
            w = 0;
        }
        if let Some(l) = out.last_mut() {
            l.push(c)
        }
        w += cw;
    }
    out
}

/// Word-wrap styled spans.
fn wrap_spans(spans: Vec<Tagged>, width: usize) -> Vec<Vec<Tagged>> {
    let width = width.max(4);
    let mut lines: Vec<Vec<Tagged>> = vec![vec![]];
    let mut w = 0usize;
    for (span, link) in spans {
        let style = span.style;
        // Split into words keeping the spaces attached.
        let text = span.content.to_string();
        for word in text.split_inclusive(' ') {
            let ww = word.width();
            if w + ww.min(width) > width && w > 0 {
                lines.push(vec![]);
                w = 0;
            }
            if ww > width {
                for chunk in wrap_plain(word, width) {
                    if w > 0 {
                        lines.push(vec![]);
                    }
                    w = chunk.width();
                    if let Some(l) = lines.last_mut() {
                        l.push((Span::styled(chunk, style), link))
                    }
                }
                continue;
            }
            let word = if w == 0 {
                word.trim_start().to_string()
            } else {
                word.to_string()
            };
            w += word.width();
            if let Some(l) = lines.last_mut() {
                l.push((Span::styled(word, style), link))
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    /// `BUZZ_VIM_SHOT=dir cargo test shot_` writes cell dumps of screens
    /// that need data the relay won't give us (your agents' live frames);
    /// `tools/shot.py` turns them into PNGs.
    #[test]
    fn shot_agents_screen() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let root = EventBuilder::new(
            Kind::Custom(9),
            "Can you make timestamps say minutes ago? And hours",
        )
        .tags([Tag::parse(["h", "c1"]).unwrap()])
        .sign_with_keys(&k)
        .unwrap();
        let rid = root.id.to_hex();
        app.store.apply(&root);
        app.store.channel_mut("c1").name = "buzz-vim".into();
        app.store.channel_mut("c2").name = "almanac-restructuring".into();
        app.tabs = vec!["c1".into(), "c2".into()];
        for (pk, name) in [
            ("a1", "taskbot"),
            ("a2", "judgebot"),
            ("a3", "labbot"),
            ("a4", "macbot"),
        ] {
            app.agents.own(pk, name);
            app.agents.presence(pk, "online");
        }
        let busy = |app: &mut App, pk: &str, ch: &str, root: &str, tools: &[&str], said: &str| {
            let mut evs = vec![
                serde_json::json!({"kind": "turn_started", "channelId": ch, "payload": {"threadRootEventId": root}}),
            ];
            for t in tools {
                evs.push(serde_json::json!({"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "tool_call", "title": t}}}}));
            }
            evs.push(serde_json::json!({"kind": "acp_read", "payload": {"params": {"update": {"sessionUpdate": "agent_message_chunk", "content": {"text": said}}}}}));
            app.agents.frame(
                pk,
                &serde_json::json!({"kind": "batch", "payload": {"events": evs}}),
            );
        };
        busy(
            &mut app,
            "a1",
            "c1",
            &rid,
            &["Read src/ui.rs", "Edit src/ui.rs", "Bash: cargo test"],
            "the identicons read better on a tile, checking the agents screen next",
        );
        busy(
            &mut app,
            "a2",
            "c2",
            "",
            &["Bash: buzz pr view 1078", "Read crates/registry.rs"],
            "reviewing the registry change",
        );
        app.agents.metric(
            "a3",
            &serde_json::json!({"model": "sonnet", "turn": {"totalTokens": 5200, "costUsd": 0.18}}),
        );
        for (pk, used) in [("a1", 141_600), ("a3", 38_000)] {
            app.agents.frame(pk, &serde_json::json!({"kind": "acp_read", "channelId": "c1", "payload": {"params": {"update": {"sessionUpdate": "usage_update", "used": used, "size": 1000000}}}}));
        }
        app.agents_view = true;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 34)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        dump(t.backend().buffer(), &format!("{dir}/agents.cells"));
    }

    /// The loading garden as it grows, then a channel in the light theme.
    #[test]
    fn shot_garden_and_light() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let mut app = App::new("me".into(), None);
        app.store.channel_mut("c1").name = "buzz-vim".into();
        app.store.channel_mut("c2").name = "beacon".into();
        app.store.channel_mut("c2").unread.insert("x".into(), 1);
        app.tabs = vec!["c1".into(), "c2".into()];
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        for tick in [1, 6, 14, 26, 40, 52, 60] {
            app.tick = tick;
            t.draw(|f| draw(f, &mut app)).unwrap();
            dump(
                t.backend().buffer(),
                &format!("{dir}/garden-{tick:02}.cells"),
            );
        }
        let mut post = |who: &Keys, text: &str| {
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags([Tag::parse(["h", "c1"]).unwrap()])
                .sign_with_keys(who)
                .unwrap();
            app.store.apply(&ev);
            ev.id.to_hex()
        };
        let (a, b) = (Keys::generate(), Keys::generate());
        post(&a, "Can you detect the Mac's light versus dark mode?");
        let yes = post(
            &b,
            "Yes. It follows the system setting and re-checks every 2 s.",
        );
        post(&a, "and unread dots on the tabs too");
        app.store.channel_mut("c1").loaded = true;
        app.selected.insert("c1".into(), yes);
        for light in [false, true] {
            crate::theme::set_light(light);
            t.draw(|f| draw(f, &mut app)).unwrap();
            dump(
                t.backend().buffer(),
                &format!(
                    "{dir}/channel-{}.cells",
                    if light { "light" } else { "dark" }
                ),
            );
        }
        crate::theme::set_light(false);
    }

    /// A channel in the operator loop: header, items, the watcher waiting
    /// on lucian with a ping, a crash notice, and a few messages.
    fn loop_fixture() -> App {
        use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let (lucian, bot, watcher) = (Keys::generate(), Keys::generate(), Keys::generate());
        let bk = bot.public_key().to_hex();
        let mut app = App::new(lucian.public_key().to_hex(), None);
        app.checklists = crate::checklist::Checklists::new(crate::checklist::Trust {
            owner: lucian.public_key().to_hex(),
            watchers: vec![watcher.public_key().to_hex()],
        });
        for (k, name, agent) in [(&lucian, "lucian", false), (&bot, "taskbot", true)] {
            app.store.profiles.insert(
                k.public_key().to_hex(),
                crate::model::Profile {
                    name: name.into(),
                    is_agent: agent,
                    ..Default::default()
                },
            );
        }
        app.store.channel_mut(ch).name = "operator-loop".into();
        app.store.channel_mut("c2").name = "beacon".into();
        app.store.channel_mut(crate::quick::CHANNEL_ID).name = "quick".into();
        app.tabs = vec![ch.into(), "c2".into()];
        let now = crate::sync::now();
        let rec = |k: &Keys, kind: u16, d: String, at: u64, v: serde_json::Value| {
            EventBuilder::new(Kind::Custom(kind), v.to_string())
                .tags(
                    [
                        vec!["d".to_string(), d],
                        vec!["h".into(), ch.into()],
                        vec!["t".into(), "buzz-checklist".into()],
                        vec!["channel".into(), ch.into()],
                        vec![
                            "checklist-type".into(),
                            match kind {
                                30470 => "item",
                                30471 => "header",
                                _ => "watch",
                            }
                            .into(),
                        ],
                    ]
                    .into_iter()
                    .map(|t| Tag::parse(t).unwrap()),
                )
                .custom_created_at(Timestamp::from(at))
                .sign_with_keys(k)
                .unwrap()
        };
        app.checklists.apply(&rec(&bot, 30471, format!("buzz-checklist/header/{ch}/{bk}"), now - 3000,
            serde_json::json!({"v": 1, "title": "Operator loop: buzz-vim", "mode": "running", "go": "b756649f",
                "resume": {"worktree": "buzz-vim-oploop", "branch": "agent/operator-loop", "worklog": "2026-10-18_OPERATOR_LOOP.md"}})));
        let items = [
            (
                "a1",
                "Reconcile the list with the conversation",
                "done",
                false,
                Some("first"),
                vec!["msg:7a1d46eb"],
                "",
                "",
            ),
            (
                "a2",
                "Checklist pane per channel",
                "done",
                false,
                None,
                vec!["commit:873d8c3", "url:https://buzz.gitcoin.co/pr/12"],
                "Items in order with status, release flag and evidence; **details** on select.",
                "",
            ),
            (
                "a3",
                "Status line per channel",
                "open",
                false,
                None,
                vec![],
                "Mode, watcher state with the ping link, open/done counts, crashes today.",
                "",
            ),
            ("a4", "Q pop-up", "open", false, None, vec![], "", ""),
            (
                "a5",
                "Migrate the prod relay to the patched image",
                "parked",
                true,
                None,
                vec![],
                "Needs the relay patch from R0.",
                "Restart prod relay tonight or wait for the nightly?",
            ),
            (
                "a6",
                "Wrap-up: voiceover demo video + plain report",
                "open",
                false,
                Some("last"),
                vec![],
                "",
                "",
            ),
        ];
        for (i, (id, text, status, release, standing, ev, details, q)) in
            items.into_iter().enumerate()
        {
            app.checklists.apply(&rec(&bot, 30470, format!("buzz-checklist/item/{ch}/{bk}/000000{id}"), now - 2000 + i as u64,
                serde_json::json!({"v": 1, "text": text, "details": details, "status": status, "release": release,
                    "order": i, "standing": standing, "evidence": ev, "parked_question": q})));
        }
        app.checklists.apply(&rec(
            &watcher,
            30472,
            format!("buzz-checklist/watch/{ch}"),
            now - 60,
            serde_json::json!({"v": 1, "state": "waiting_on_lucian", "since": now - 25 * 60,
                "thread": "5dbdf585", "ping": "c0ffee01", "nudges": 1}),
        ));
        let mut post = |who: &Keys, text: &str, at: u64| {
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags([Tag::parse(["h", ch]).unwrap()])
                .custom_created_at(Timestamp::from(at))
                .sign_with_keys(who)
                .unwrap();
            app.store.apply(&ev);
            app.checklists
                .note_message(ch, &ev.id.to_hex(), at, &ev.content);
        };
        post(&lucian, "go: build the operator loop", now - 3000);
        post(
            &bot,
            "⚠ agent process crashed (exit 137); respawned",
            now - 1200,
        );
        post(
            &bot,
            "Parked the relay item: restart prod tonight or wait for the nightly? @lucian",
            now - 25 * 60,
        );
        app.store.channel_mut(ch).loaded = true;
        app
    }

    fn screen_text(app: &mut App, w: u16, h: u16) -> String {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect()
    }

    /// A second list in the loop channel: the status line says contested
    /// with both creators, no list shows, and tab 0 asks for a driver.
    #[test]
    fn a_contested_channel_says_so() {
        use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
        let mut app = loop_fixture();
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let x = Keys::generate();
        let xk = x.public_key().to_hex();
        let d = format!("buzz-checklist/header/{ch}/{xk}");
        let ev = EventBuilder::new(Kind::Custom(30471), r#"{"title":"x","mode":"running"}"#)
            .tags(
                [
                    ["d", d.as_str()],
                    ["h", ch],
                    ["t", "buzz-checklist"],
                    ["channel", ch],
                    ["checklist-type", "header"],
                ]
                .map(|t| Tag::parse(t).unwrap()),
            )
            .custom_created_at(Timestamp::from(crate::sync::now() - 100))
            .sign_with_keys(&x)
            .unwrap();
        assert!(app.checklists.apply(&ev));
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("contested: ") && s.contains("taskbot"), "{s}");
        assert!(s.contains(&format!("{}…", &xk[..8])), "{s}");
        assert!(!s.contains("RUNNING") && !s.contains("3 open"), "{s}");
        assert!(
            app.needs
                .iter()
                .any(|r| r.text == "two lists claim this channel; pick a driver"),
            "{:?}",
            app.needs
        );
    }

    #[test]
    fn status_line_and_checklist_pane_show_the_loop() {
        let mut app = loop_fixture();
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("RUNNING"), "{s}");
        assert!(s.contains("waiting on lucian 25m"), "{s}");
        assert!(
            s.contains("ping ↗") && s.contains("3 open · 2 done · 1 parked"),
            "{s}"
        );
        assert!(
            s.contains("1 crash today") && s.contains("c checklist"),
            "{s}"
        );
        // The ping is a link: f and a click find it.
        let ping = app.links.iter().find(|l| l.key.0 == "status").unwrap();
        assert!(
            ping.url.ends_with("id=c0ffee01&thread=5dbdf585"),
            "{}",
            ping.url
        );
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('c').into()));
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("›  checklist") && s.contains("Operator loop: buzz-vim"),
            "{s}"
        );
        // Standing first item on top, wrap-up last, release flag shown.
        let first = s.find("Reconcile the list").unwrap();
        let last = s.find("Wrap-up: voiceover").unwrap();
        assert!(first < last && s.contains("release  parked"), "{s}");
        // Details of the picked item, its evidence linked.
        for _ in 0..4 {
            app.on_term(crossterm::event::Event::Key(KeyCode::Char('j').into()));
        }
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("? Restart prod relay tonight"), "{s}");
        assert!(
            s.contains("release: needs lucian") && s.contains("by taskbot"),
            "{s}"
        );
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('k').into()));
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('k').into()));
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('k').into()));
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("commit:873d8c3") && s.contains("details on select"),
            "{s}"
        );
        assert!(
            app.links
                .iter()
                .any(|l| l.url == "https://buzz.gitcoin.co/pr/12")
        );
        // Narrow: details under the list.
        let s = screen_text(&mut app, 80, 34);
        assert!(s.contains("commit:873d8c3"), "{s}");
        // No records: no status line, and the pane says how to start one.
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('L').into()));
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("no checklist in this channel"), "{s}");
    }

    /// Tab 0: what needs an answer, then FYI, newest first; ⏎ opens a row,
    /// r answers it in its thread (and the row goes), ! goes round them.
    #[test]
    fn tab_zero_lists_what_needs_you() {
        use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
        let mut app = loop_fixture();
        let lucian = app.store.me.clone();
        let (alice, judge) = (Keys::generate(), Keys::generate());
        for (k, name, agent) in [(&alice, "alice", false), (&judge, "judgebot", true)] {
            app.store.profiles.insert(
                k.public_key().to_hex(),
                crate::model::Profile {
                    name: name.into(),
                    is_agent: agent,
                    ..Default::default()
                },
            );
        }
        let now = crate::sync::now();
        let post = |app: &mut App, k: &Keys, text: &str, root: Option<&str>, at: u64| {
            let mut tags = vec![
                Tag::parse(["h", "c2"]).unwrap(),
                Tag::parse(["p", &lucian]).unwrap(),
            ];
            if let Some(r) = root {
                tags.push(Tag::parse(["e", r, "", "reply"]).unwrap());
            }
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags(tags)
                .custom_created_at(Timestamp::from(at))
                .sign_with_keys(k)
                .unwrap();
            app.store.apply(&ev);
            ev.id.to_hex()
        };
        let ask = post(
            &mut app,
            &alice,
            "@lucian can you review the beacon PR?",
            None,
            now - 3600,
        );
        post(
            &mut app,
            &judge,
            "@lucian deployed beacon v2",
            None,
            now - 120,
        );
        app.store.channel_mut("c2").loaded = true;
        // A channel with no list still shows the count.
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('2').into()));
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("⚑ 3  need you · 0 to see · ! next"), "{s}");
        assert!(s.contains("0 ⚑ needs you 3"), "{s}");
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('0').into()));
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("needs an answer (3)") && s.contains("FYI (1)"),
            "{s}"
        );
        let at = |t: &str| s.find(t).unwrap_or_else(|| panic!("{t}: {s}"));
        // Oldest first: alice an hour ago, the parked item, the watcher's ask.
        assert!(at("can you review the beacon PR?") < at("Restart prod relay tonight"));
        assert!(at("Restart prod relay tonight") < at("waiting on lucian"));
        assert!(
            at("waiting on lucian") < at("FYI (1)") && at("FYI (1)") < at("deployed beacon v2")
        );
        assert!(
            s.contains("#beacon") && s.contains("alice") && s.contains("judgebot"),
            "{s}"
        );
        // ⏎ opens alice's message.
        app.on_term(crossterm::event::Event::Key(KeyCode::Enter.into()));
        assert!(!app.needs_view);
        assert_eq!(
            (app.current(), app.selected.get("c2")),
            (Some("c2"), Some(&ask))
        );
        // r on her row: a reply into her thread.
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('0').into()));
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('r').into()));
        for c in "on it".chars() {
            app.on_term(crossterm::event::Event::Key(KeyCode::Char(c).into()));
        }
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("reply in #beacon › thread") && s.contains("alice: @lucian can you review"),
            "{s}"
        );
        app.actions.clear();
        app.on_term(crossterm::event::Event::Key(KeyCode::Enter.into()));
        assert!(matches!(app.actions.first(),
            Some(crate::app::Action::SendDraft { channel, root, draft, to })
                if channel == "c2" && root.as_ref() == Some(&ask) && draft.text == "on it"
                    && to == &vec![alice.public_key().to_hex()]));
        // Once the reply is in the store, her row is gone.
        let m = crate::model::Message {
            id: "lucian-reply".into(),
            author: lucian.clone(),
            created_at: now - 1,
            content: "on it".into(),
            root: Some(ask.clone()),
            ..Default::default()
        };
        app.store.channel_mut("c2").messages.insert(m.id.clone(), m);
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("needs an answer (2)") && !s.contains("can you review the beacon"),
            "{s}"
        );
        // ! goes oldest first: the parked item's thread, then the ping in it.
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('!').into()));
        assert_eq!(app.thread, Some((ch.to_string(), "5dbdf585".to_string())));
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .0
                .starts_with("⚑ 1/2  taskbot: Restart prod relay")
        );
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('!').into()));
        assert_eq!(app.thread_selected.as_deref(), Some("c0ffee01"));
        assert!(app.status.as_ref().unwrap().0.starts_with("⚑ 2/2"));
        // H from the first channel tab comes round to tab 0.
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('1').into()));
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('H').into()));
        assert!(app.needs_view);
        app.mode = Mode::Help;
        let s = screen_text(&mut app, 120, 60);
        assert!(
            s.contains("next ⚑, oldest first") && s.contains("reply (r on tab 0)"),
            "{s}"
        );
    }

    #[test]
    fn the_q_box_shows_what_it_sends() {
        let mut app = loop_fixture();
        app.channels_loaded = true;
        app.on_term(crossterm::event::Event::Key(KeyCode::Char('Q').into()));
        for c in "why is the relay item parked?".chars() {
            app.on_term(crossterm::event::Event::Key(KeyCode::Char(c).into()));
        }
        let s = screen_text(&mut app, 120, 34);
        assert!(s.contains("Q · ask taskbot in #quick"), "{s}");
        assert!(s.contains("^q again: + #operator-loop"), "{s}");
        app.on_term(crossterm::event::Event::Key(
            crossterm::event::KeyEvent::new(
                KeyCode::Char('q'),
                crossterm::event::KeyModifiers::CONTROL,
            ),
        ));
        let s = screen_text(&mut app, 120, 34);
        assert!(
            s.contains("#operator-loop: its id, link and latest 30 messages"),
            "{s}"
        );
        app.mode = Mode::Help;
        let s = screen_text(&mut app, 120, 40);
        assert!(
            s.contains("Q box (Q ^q)") && s.contains("checklist: items"),
            "{s}"
        );
    }

    /// The operator loop pane, status line and Q box, for README shots.
    #[test]
    fn shot_operator_loop() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        let mut app = loop_fixture();
        app.channels_loaded = true;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 34)).unwrap();
        let mut shot = |app: &mut App, name: &str| {
            t.draw(|f| draw(f, app)).unwrap();
            dump(t.backend().buffer(), &format!("{dir}/{name}.cells"));
        };
        shot(&mut app, "loop-status");
        let key = |app: &mut App, k: crossterm::event::KeyEvent| {
            app.on_term(crossterm::event::Event::Key(k))
        };
        key(&mut app, KeyCode::Char('c').into());
        key(&mut app, KeyCode::Char('j').into());
        shot(&mut app, "loop-checklist");
        for _ in 0..3 {
            key(&mut app, KeyCode::Char('j').into());
        }
        shot(&mut app, "loop-checklist-parked");
        key(&mut app, KeyCode::Esc.into());
        key(&mut app, KeyCode::Char('Q').into());
        for c in "why is the relay item parked?".chars() {
            key(&mut app, KeyCode::Char(c).into());
        }
        shot(&mut app, "loop-q");
        key(
            &mut app,
            crossterm::event::KeyEvent::new(
                KeyCode::Char('q'),
                crossterm::event::KeyModifiers::CONTROL,
            ),
        );
        shot(&mut app, "loop-q-channel");
    }

    /// A busy channel, dark and light.
    #[test]
    fn shot_palettes() {
        let Ok(dir) = std::env::var("BUZZ_VIM_SHOT") else {
            return;
        };
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let mut app = App::new("me".into(), None);
        app.store.channel_mut("c1").name = "buzz-vim".into();
        app.store.channel_mut("c2").name = "beacon".into();
        app.store.channel_mut("c3").name = "almanac".into();
        app.store.channel_mut("c2").unread.insert("x".into(), 1);
        app.tabs = vec!["c1".into(), "c2".into(), "c3".into()];
        let (a, b, c) = (Keys::generate(), Keys::generate(), Keys::generate());
        for (k, name) in [(&a, "lucian"), (&b, "taskbot"), (&c, "judgebot")] {
            app.store.profiles.insert(
                k.public_key().to_hex(),
                crate::model::Profile {
                    name: name.into(),
                    is_agent: name != "lucian",
                    ..Default::default()
                },
            );
        }
        let mut post = |who: &Keys, text: &str| {
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags([Tag::parse(["h", "c1"]).unwrap()])
                .sign_with_keys(who)
                .unwrap();
            app.store.apply(&ev);
            ev.id.to_hex()
        };
        post(
            &a,
            "Can we get **soft pastels** that read the same in grayscale?",
        );
        let sel = post(
            &b,
            "Yes. Each step keeps its gray's luminance:\n```rust\nlet y = 0.2126 * r + 0.7152 * g + 0.0722 * b;\n```\nUnder the Mac filter it reads as gray.",
        );
        post(&a, "@taskbot nice, trying it with the Mac filter on");
        post(
            &c,
            "@lucian every name gets its own hue, same gray underneath",
        );
        app.store.channel_mut("c1").loaded = true;
        app.selected.insert("c1".into(), sel);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        for light in [false, true] {
            crate::theme::set_light(light);
            t.draw(|f| draw(f, &mut app)).unwrap();
            let mode = if light { "light" } else { "dark" };
            dump(t.backend().buffer(), &format!("{dir}/pal-{mode}.cells"));
        }
        crate::theme::set_light(false);
    }

    fn dump(buf: &ratatui::buffer::Buffer, path: &str) {
        use std::fmt::Write;
        let mut out = format!("{} {}\n", buf.area.width, buf.area.height);
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                let col = |c: ratatui::style::Color| match c {
                    ratatui::style::Color::Rgb(r, g, b) => format!("{r:02x}{g:02x}{b:02x}"),
                    _ => "-".into(),
                };
                let bold = c.modifier.contains(Modifier::BOLD) as u8;
                let _ = writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    c.symbol(),
                    col(c.fg),
                    col(c.bg),
                    bold
                );
            }
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn active_tab_stays_visible() {
        let mut app = App::new("me".into(), None);
        for i in 0..8 {
            let c = app.store.channel_mut(&format!("c{i}"));
            c.name = format!("a-fairly-long-channel-{i}");
        }
        app.tabs = (0..8).map(|i| format!("c{i}")).collect();
        app.active = 6;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 10)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let top: String = (0..90).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(top.contains("channel-6"), "{top}");
    }

    /// A long section list scrolls; the cursor row and its click area stay
    /// inside the popup.
    #[test]
    fn section_picker_scrolls_to_the_cursor() {
        let mut app = App::new("me".into(), None);
        app.sections = (0..30)
            .map(|i| crate::sections::Section {
                id: format!("s{i}"),
                name: format!("section-{i}"),
                channels: vec![],
            })
            .collect();
        app.mode = crate::keys::Mode::Sections;
        app.picker_cursor = 25;
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let screen: String = (0..24)
            .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(screen.contains("section-25"), "{screen}");
        assert!(!screen.contains("section-0 "), "{screen}");
        let hit = app
            .hits
            .iter()
            .find(|(_, h)| matches!(h, Hit::Section(25)))
            .map(|(r, _)| *r)
            .unwrap();
        assert!(hit.bottom() < 24);
        let row: String = (0..80).map(|x| buf[(x, hit.y)].symbol()).collect();
        assert!(row.contains("section-25"), "{row}");
    }

    /// T lists today's tabs with their legend; ? lists the new keys.
    #[test]
    fn tab_order_popup_and_help_show_their_keys() {
        let mut app = App::new("me".into(), None);
        for id in ["a", "b"] {
            let c = app.store.channel_mut(id);
            c.name = format!("chan-{id}");
            c.is_member = true;
        }
        app.sections = vec![crate::sections::Section {
            id: "s".into(),
            name: "Work".into(),
            channels: vec!["a".into(), "b".into()],
        }];
        app.chosen = vec!["s".into()];
        app.rebuild_tabs();
        let screen = |app: &mut App| {
            let mut t =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..40)
                .map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
                .collect::<String>()
        };
        app.mode = crate::keys::Mode::Order;
        let s = screen(&mut app);
        assert!(s.contains("#chan-b") && s.contains("Work"), "{s}");
        assert!(s.contains("move it down / up"), "{s}");
        app.mode = crate::keys::Mode::Help;
        let s = screen(&mut app);
        assert!(s.contains("move this tab left / right"), "{s}");
        assert!(s.contains("tab order (T)"), "{s}");
    }

    /// After following links, the footer shows the way back.
    #[test]
    fn footer_shows_the_link_chain() {
        let mut app = App::new("me".into(), None);
        app.store.channel_mut("a").name = "buzz-vim".into();
        app.store.channel_mut("x").name = "beacon-ask-release".into();
        app.tabs = vec!["a".into()];
        let bottom = |app: &mut App, w: u16| {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, 12)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..w).map(|x| buf[(x, 11)].symbol()).collect::<String>()
        };
        assert!(!bottom(&mut app, 100).contains('⌫'));
        app.follow("buzz://message?channel=x&id=r1");
        let line = bottom(&mut app, 100);
        assert!(
            line.contains("⌫ #buzz-vim › #beacon-ask-release thread"),
            "{line}"
        );
        // Narrow: the oldest steps go first.
        let line = bottom(&mut app, 44);
        assert!(line.contains("⌫ … › #beacon-ask-release thread"), "{line}");
    }

    /// Tabs run up to the message count.
    #[test]
    fn tabs_fill_the_row() {
        let mut app = App::new("me".into(), None);
        for i in 0..5 {
            app.store.channel_mut(&format!("c{i}")).name = format!("chan-{i}");
        }
        app.tabs = (0..5).map(|i| format!("c{i}")).collect();
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(114, 10)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let top: String = (0..114).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(top.contains("chan-4") && !top.contains('…'), "{top}");
    }

    #[test]
    fn new_replies_latest_and_mention_counts_show() {
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let me = Keys::generate();
        let them = Keys::generate();
        let mut app = App::new(me.public_key().to_hex(), None);
        let ev = |k: &Keys, content: &str, tags: Vec<Vec<&str>>| {
            EventBuilder::new(Kind::Custom(9), content)
                .tags(tags.into_iter().map(|t| Tag::parse(t).unwrap()))
                .sign_with_keys(k)
                .unwrap()
        };
        let root = ev(&me, "question", vec![vec!["h", "c1"]]);
        let rid = root.id.to_hex();
        app.store.apply(&root);
        app.store.apply(&ev(
            &them,
            "answer",
            vec![vec!["h", "c1"], vec!["e", &rid, "", "reply"]],
        ));
        app.tabs = vec!["c1".into(), "c2".into()];
        let c2 = app.store.channel_mut("c2");
        c2.name = "other".into();
        c2.unread.insert("x".into(), 1);
        c2.mentioned.insert("x".into());
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(
            rows.iter().any(|r| r.contains("↳ 1 reply (1 new)")),
            "{rows:#?}"
        );
        assert!(rows.iter().any(|r| r.contains("◆ latest")), "{rows:#?}");
        assert!(rows[0].contains("other @1"), "{rows:#?}");
    }

    #[test]
    fn thinking_shows_on_thread_and_channel() {
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let ev = |content: &str, tags: Vec<Vec<&str>>| {
            EventBuilder::new(Kind::Custom(9), content)
                .tags(tags.into_iter().map(|t| Tag::parse(t).unwrap()))
                .sign_with_keys(&k)
                .unwrap()
        };
        let root = ev("long question", vec![vec!["h", "c1"]]);
        let rid = root.id.to_hex();
        app.store.apply(&root);
        app.store.apply(&ev(
            "answer",
            vec![vec!["h", "c1"], vec!["e", &rid, "", "reply"]],
        ));
        app.tabs = vec!["c1".into()];
        app.agents.own("ag", "taskbot");
        app.agents.frame(
            "ag",
            &serde_json::json!({"kind": "turn_started", "channelId": "c1",
                "payload": {"threadRootEventId": rid}}),
        );
        let screen = |app: &mut App| {
            let mut t =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let rows = screen(&mut app);
        let replies = rows.iter().find(|r| r.contains("↳ 1 reply")).unwrap();
        assert!(replies.contains("taskbot thinking"), "{rows:#?}");
        assert!(!rows[0].contains("taskbot working"), "header: {rows:#?}");
        app.thread = Some(("c1".into(), rid));
        let rows = screen(&mut app);
        assert!(
            rows[1].contains("taskbot thinking"),
            "thread header: {rows:#?}"
        );
    }

    #[test]
    fn tall_message_pages_through() {
        use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let body: Vec<String> = (1..=60).map(|i| format!("line{i:02}")).collect();
        for (i, text) in [body.join("\n\n"), "short".into()].iter().enumerate() {
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags([Tag::parse(["h", "c1"]).unwrap()])
                .custom_created_at(nostr::Timestamp::from(1_000 + i as u64))
                .sign_with_keys(&k)
                .unwrap();
            app.store.apply(&ev);
        }
        app.tabs = vec!["c1".into()];
        let screen = |app: &mut App| {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let key =
            |app: &mut App, c: char| app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Char(c))));
        app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Esc)));
        screen(&mut app);
        // Up from "short" lands on the tall one's bottom.
        key(&mut app, 'k');
        let s = screen(&mut app);
        assert!(s.contains("line60") && !s.contains("line01"), "{s}");
        assert!(s.contains("more lines"), "{s}");
        // k pages up through it to its top before going anywhere else.
        for _ in 0..60 {
            key(&mut app, 'k');
            screen(&mut app);
        }
        let s = screen(&mut app);
        assert!(s.contains("line01") && !s.contains("↑ "), "{s}");
        // And j walks back down to its end, then on to the next message.
        for _ in 0..60 {
            key(&mut app, 'j');
            screen(&mut app);
        }
        let s = screen(&mut app);
        assert!(s.contains("short"), "{s}");
    }

    #[test]
    fn tall_newest_entered_from_above_starts_at_top() {
        use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let body: Vec<String> = (1..=60).map(|i| format!("line{i:02}")).collect();
        for (i, text) in ["short".to_string(), body.join("\n\n")].iter().enumerate() {
            let ev = EventBuilder::new(Kind::Custom(9), text)
                .tags([Tag::parse(["h", "c1"]).unwrap()])
                .custom_created_at(nostr::Timestamp::from(1_000 + i as u64))
                .sign_with_keys(&k)
                .unwrap();
            app.store.apply(&ev);
        }
        app.tabs = vec!["c1".into()];
        let screen = |app: &mut App| {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let key =
            |app: &mut App, c: char| app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Char(c))));
        app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Esc)));
        screen(&mut app);
        // Page all the way up and onto "short".
        for _ in 0..60 {
            key(&mut app, 'k');
            screen(&mut app);
        }
        assert!(screen(&mut app).join("\n").contains("short"));
        // j into the tall newest one lands on its top, not its end.
        key(&mut app, 'j');
        let rows = screen(&mut app);
        let s = rows.join("\n");
        assert!(s.contains("line01") && !s.contains("line60"), "{s}");
        // The left edge runs the pane's full height as a scroll bar.
        assert!(rows[2..16].iter().all(|r| r.starts_with('▌')), "{s}");
    }

    #[test]
    fn viewer_shows_a_failed_image() {
        let mut app = App::new("00".repeat(32), None);
        app.mode = crate::keys::Mode::Viewer;
        app.viewer = Some("https://x/a.png".into());
        app.viewer_list = vec!["https://x/a.png".into()];
        app.media.images.insert(
            "https://x/a.png".into(),
            Slot::Failed("download failed (404)".into()),
        );
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let s: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|p| buf[p].symbol().to_string())
            .collect();
        assert!(
            s.contains("couldn't load this image: download failed (404)"),
            "{s}"
        );
        assert!(s.contains("esc") && !s.contains("loading"), "{s}");
    }

    #[test]
    fn viewer_shows_download_progress() {
        let mut app = App::new("00".repeat(32), None);
        app.mode = crate::keys::Mode::Viewer;
        app.viewer = Some("https://x/a.png".into());
        app.viewer_list = vec!["https://x/a.png".into()];
        app.media
            .images
            .insert("https://x/a.png".into(), Slot::Loading);
        app.media
            .progress
            .insert("https://x/a.png".into(), (1_200_000, Some(2_700_000)));
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let s: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|p| buf[p].symbol().to_string())
            .collect();
        assert!(s.contains("loading 1.2 / 2.7 MB"), "{s}");
    }

    #[test]
    fn viewer_centers_the_image() {
        let mut app = App::new("00".repeat(32), None);
        app.mode = crate::keys::Mode::Viewer;
        let url = "https://x/wide.png".to_string();
        app.viewer = Some(url.clone());
        app.viewer_list = vec![url.clone()];
        // Very wide: fit to 80x19 leaves bands above and below.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            400,
            20,
            image::Rgb([255, 255, 255]),
        ));
        let p = ratatui_image::picker::Picker::halfblocks();
        let proto = crate::media::encode(&p, &img, 80, 19).unwrap();
        app.media.encoded.insert((url, 80, 19), proto);
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        t.draw(|f| draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let painted: Vec<u16> = (1..20)
            .filter(|&y| {
                (0..80).any(|x| {
                    let c = &buf[(x, y)];
                    [c.fg, c.bg].contains(&ratatui::style::Color::Rgb(255, 255, 255))
                })
            })
            .collect();
        assert!(!painted.is_empty() && painted[0] > 2, "{painted:?}");
        assert!(*painted.last().unwrap() < 18, "{painted:?}");
    }

    #[test]
    fn slider_sets_the_reading_width() {
        use crossterm::event::{
            Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent,
            MouseEventKind,
        };
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let k = Keys::generate();
        let mut app = App::new(k.public_key().to_hex(), None);
        let ev = EventBuilder::new(Kind::Custom(9), "word ".repeat(60))
            .tags([Tag::parse(["h", "c1"]).unwrap()])
            .sign_with_keys(&k)
            .unwrap();
        app.store.apply(&ev);
        app.tabs = vec!["c1".into()];
        app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Esc)));
        let screen = |app: &mut App| {
            let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            let buf = t.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let mouse = |app: &mut App, kind, column| {
            app.on_term(TermEvent::Mouse(MouseEvent {
                kind,
                column,
                row: 5,
                modifiers: KeyModifiers::NONE,
            }))
        };
        screen(&mut app);
        let left = MouseButton::Left;
        mouse(&mut app, MouseEventKind::Down(left), 79);
        mouse(&mut app, MouseEventKind::Drag(left), 40);
        mouse(&mut app, MouseEventKind::Up(left), 40);
        assert_eq!(app.read_width, Some(40));
        let rows = screen(&mut app);
        // Words wrap left of the handle; nothing to its right.
        let words: Vec<&String> = rows.iter().filter(|r| r.contains("word")).collect();
        assert!(words.len() > 3, "{rows:#?}");
        for r in &words {
            let right: String = r.chars().skip(41).collect();
            assert!(right.trim().is_empty(), "{rows:#?}");
            assert!(matches!(r.chars().nth(40), Some('│' | '┃')), "{rows:#?}");
        }
        // The compose bar keeps the full width.
        assert!(rows[16].ends_with('╮'), "{rows:#?}");
        let key = |app: &mut App, c| app.on_term(TermEvent::Key(KeyEvent::from(KeyCode::Char(c))));
        key(&mut app, '[');
        assert_eq!(app.read_width, Some(36));
        for _ in 0..20 {
            key(&mut app, ']');
        }
        assert_eq!(app.read_width, None);
        // Double-click the handle: back to full width.
        app.read_width = Some(50);
        screen(&mut app);
        mouse(&mut app, MouseEventKind::Down(left), 50);
        mouse(&mut app, MouseEventKind::Up(left), 50);
        mouse(&mut app, MouseEventKind::Down(left), 50);
        assert_eq!(app.read_width, None);
    }

    #[test]
    fn relative_times() {
        use chrono::{TimeZone, Utc};
        let now = Utc.with_ymd_and_hms(2026, 10, 15, 15, 40, 0).unwrap();
        let ago = |m: i64| relative_time(now - chrono::Duration::minutes(m), now);
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(4), "4m ago");
        assert_eq!(ago(120), "2h ago");
        assert_eq!(ago(135), "2h 15m ago");
        assert_eq!(ago(13 * 60), "Oct 15 02:40");
        assert_eq!(ago(400 * 24 * 60), "Sep 10 2025 15:40");
    }

    #[test]
    fn compose_wraps_long_lines() {
        assert_eq!(super::wrap_cols("", 5), vec![""]);
        assert_eq!(super::wrap_cols("abcdefg", 5), vec!["abcde", "fg"]);
        assert_eq!(super::wrap_cols("abcde", 5), vec!["abcde", ""]);
    }

    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn links_are_found_through_wrapping() {
        let name = |c: &str| (c == "c1").then(|| "general".to_string());
        let r = render_markdown_links(
            "- see https://example.com/a/long/path and buzz://pr?id=1 or buzz://message?channel=c1&id=m",
            20,
            &[],
            &name,
        );
        assert_eq!(r.urls.len(), 3);
        let rows = text(&r.lines);
        // Every recorded piece covers exactly the text it claims.
        for &(row, col, w, k) in &r.at {
            let mut at = 0;
            let cells: String = rows[row]
                .chars()
                .filter(|c| {
                    at += c.width().unwrap_or(0);
                    at > col && at <= col + w
                })
                .collect();
            assert!(!cells.trim().is_empty(), "link {k} at {row}:{col} is blank");
        }
        assert!(rows.iter().any(|l| l.contains("pr 🐝")), "{rows:?}");
        assert!(rows.iter().any(|l| l.contains("#general")), "{rows:?}");
        // The long URL wrapped, and both halves point at it.
        assert!(r.at.iter().filter(|a| a.3 == 0).count() >= 2);
    }

    #[test]
    fn markdown_wraps_and_styles() {
        let l = render_markdown(
            "hello **big** world and `code` @judge bot here",
            14,
            &["@judge bot".into()],
        );
        let t = text(&l);
        assert!(t.iter().all(|s| s.width() <= 14), "{t:?}");
        assert_eq!(
            t.join(" ").split_whitespace().collect::<Vec<_>>().join(" "),
            "hello big world and code @judge bot here"
        );
        let fence = render_markdown("```\nlet x = 1;\n```\n- item", 14, &[]);
        assert_eq!(text(&fence), vec![" let x = 1;   ", "• item"]);
        let lang = render_markdown("```rust\n\tif a {\n\n```", 12, &[]);
        assert_eq!(
            text(&lang),
            vec![" rust       ", "     if a { ", "            "]
        );
        let one = render_markdown("```ls -la```", 10, &[]);
        assert_eq!(text(&one), vec![" ls -la   "]);
        let ticks = render_markdown("run ``a `b` c`` now", 40, &[]);
        assert_eq!(text(&ticks), vec!["run a `b` c now"]);
        let lone = render_markdown("it's a ` tick", 40, &[]);
        assert_eq!(text(&lone), vec!["it's a ` tick"]);
    }
}
