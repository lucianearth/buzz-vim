//! Drawing. Immediate mode: everything is recomputed from `App` each frame,
//! and ratatui diffs cells so only changes reach the terminal.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui_image::Image;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Hit, Pane};
use crate::art;
use crate::keys::{Mode, legend};
use crate::live::ConnState;
use crate::media::Slot;
use crate::model::{Channel, Message};
use crate::theme;

const THUMB_COLS: u16 = 36;
const THUMB_MAX_ROWS: u16 = 12;

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits.clear();
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

    let agents_w = if app.show_agents && body.width >= 100 {
        34
    } else {
        0
    };
    let [main, agents] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(agents_w)]).areas(body);
    let (timeline, thread) = if app.thread.is_some() {
        if main.width >= 90 {
            let [a, b] =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .areas(main);
            (Some(a), b)
        } else {
            (None, main)
        }
    } else {
        (Some(main), Rect::default())
    };
    let mut images = vec![];
    if let Some(t) = timeline {
        draw_timeline(f, app, t, &mut images);
    }
    if app.thread.is_some() {
        draw_thread(f, app, thread, &mut images);
    }
    if agents_w > 0 {
        draw_agents(f, app, agents);
    }
    draw_compose(f, app, compose);
    draw_footer(f, app, foot);
    // Images last, so nothing paints over them. Overlays hide them: terminal
    // graphics sit on top of text, and a popup can't cover them otherwise.
    let overlay = matches!(
        app.mode,
        Mode::Sections | Mode::Confirm | Mode::Help | Mode::Viewer
    ) || (app.mode == Mode::Insert && !app.mention_candidates().is_empty());
    for (rect, key) in images.into_iter().filter(|_| !overlay) {
        if let Some(p) = app.media.encoded.get(&key) {
            f.render_widget(Image::new(p), rect);
        }
    }
    match app.mode {
        Mode::Sections => draw_sections(f, app, area),
        Mode::Confirm => draw_confirm(f, app, area),
        Mode::Help => draw_help(f, area),
        Mode::Viewer => draw_viewer(f, app, area),
        _ => {}
    }
}

// ---------- top bar ----------

fn draw_tabs(f: &mut Frame, app: &mut App, area: Rect) {
    let mut x = area.x;
    let logo = format!(
        " {} buzz ",
        if (app.tick / 4).is_multiple_of(2) {
            "ʚ●"
        } else {
            "ɞ●"
        }
    );
    f.render_widget(
        Paragraph::new(Span::styled(logo.clone(), theme::bright())),
        Rect {
            width: logo.width() as u16,
            ..area
        },
    );
    x += logo.width() as u16 + 1;
    let right = match app.conn {
        ConnState::Online => " ● live ".to_string(),
        ConnState::Connecting => format!(" {} connecting ", art::flight(app.tick, 3).trim()),
        ConnState::Offline => " ○ offline, retrying ".to_string(),
    };
    let right_w = right.width() as u16 + 1;
    let limit = area.x + area.width.saturating_sub(right_w);
    for (i, id) in app.tabs.clone().iter().enumerate() {
        let ch = app.store.channels.get(id);
        let name = ch.map(channel_label).unwrap_or_else(|| "…".into());
        let unread = ch.map_or(0, |c| c.unread);
        let dot = if unread > 0 { "•" } else { "" };
        let label = if i < 9 {
            format!(" {} {name}{dot} ", i + 1)
        } else {
            format!(" {name}{dot} ")
        };
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
        let style = if i == app.active {
            theme::tab_active()
        } else if unread > 0 {
            theme::tab_idle()
                .add_modifier(Modifier::BOLD)
                .fg(theme::gray(23))
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
}

fn time_label(at: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(at as i64, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%b %e %H:%M").to_string()
    }
}

fn message_block(
    app: &App,
    ch: &Channel,
    m: &Message,
    width: u16,
    selected: bool,
    show_replies: bool,
) -> BlockOut {
    let mut lines = vec![];
    let bar = if selected {
        Span::styled("▌", theme::bright())
    } else {
        Span::raw(" ")
    };
    let author = app.store.name_of(&m.author);
    let is_agent = app
        .store
        .profiles
        .get(&m.author)
        .is_some_and(|p| p.is_agent);
    let mut head = vec![bar.clone(), Span::styled(author, theme::bright())];
    if is_agent {
        head.push(Span::styled(" ⚙", theme::faint()));
    }
    head.push(Span::styled(
        format!("  {}", time_label(m.created_at)),
        theme::faint(),
    ));
    if m.edited {
        head.push(Span::styled("  (edited)", theme::faint()));
    }
    lines.push(Line::from(head));
    let inner = width.saturating_sub(3).max(10) as usize;
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
        for l in render_markdown(body.trim_end(), inner, &mention_names) {
            let mut spans = vec![bar.clone(), Span::raw("  ")];
            spans.extend(l.spans);
            lines.push(Line::from(spans));
        }
    }
    let mut image = None;
    if !m.deleted {
        for media in &m.media {
            if media.mime.starts_with("image/") && image.is_none() && app.picker.is_some() {
                let cols = THUMB_COLS.min(width.saturating_sub(4));
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
            lines.push(Line::from(vec![
                bar.clone(),
                Span::styled(
                    format!("  ↳ {n} {}", if n == 1 { "reply" } else { "replies" }),
                    theme::text().add_modifier(Modifier::BOLD),
                ),
                Span::styled(when, theme::faint()),
            ]));
        }
    }
    lines.push(Line::raw(""));
    BlockOut {
        id: m.id.clone(),
        lines,
        image,
    }
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
    if let Some(i) = cur_idx {
        let a: usize = heights[..i].iter().sum();
        let b = a + heights[i];
        if b > top + h {
            top = b.saturating_sub(h);
        }
        if a < top {
            top = a;
        }
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
        if let Some((off, rows, url, cols)) = b.image {
            let off = off as usize;
            // Only draw a picture when it is fully on screen.
            if off >= skip && off + rows as usize <= skip + visible {
                let r = Rect {
                    x: area.x + 3,
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
}

fn pane_title(f: &mut Frame, area: Rect, title: String, hint: &str, focused: bool) -> Rect {
    let style = if focused {
        theme::bright()
    } else {
        theme::muted()
    };
    let [t, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let line = Line::from(vec![
        Span::styled(format!(" {title}"), style),
        Span::styled(format!("  {hint}"), theme::faint()),
    ]);
    f.render_widget(Paragraph::new(line), t);
    rest
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
    let (title, hint) = match app.store.channels.get(&id) {
        Some(c) => (
            channel_label(c),
            c.about.lines().next().unwrap_or("").to_string(),
        ),
        None => ("…".into(), String::new()),
    };
    let area = pane_title(f, area, title, &hint, focused);
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
    let start = ch.timeline.len().saturating_sub(400);
    let blocks: Vec<BlockOut> = ch.timeline[start..]
        .iter()
        .filter_map(|mid| ch.messages.get(mid))
        .map(|m| {
            let sel = focused
                && cursor_id.as_deref() == Some(m.id.as_str())
                && (cursor.is_some() || app.thread.is_none());
            message_block(app, ch, m, area.width, sel, true)
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
    let [rule, area] = Layout::horizontal([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::rule()),
        rule,
    );
    let focused = app.focus == Pane::Thread;
    let area = pane_title(f, area, "thread".into(), "h closes", focused);
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
            )
        })
        .collect();
    if blocks.is_empty() {
        draw_empty(f, app, area, "loading thread…");
        return;
    }
    let close = Rect {
        x: area.x + area.width.saturating_sub(3),
        y: area.y.saturating_sub(1),
        width: 3,
        height: 1,
    };
    f.render_widget(Paragraph::new(Span::styled(" ✕ ", theme::muted())), close);
    app.hits.push((close, Hit::CloseThread));
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

fn draw_empty(f: &mut Frame, app: &App, area: Rect, msg: &str) {
    let mut lines: Vec<Line> = vec![];
    let pad = area.height.saturating_sub(art::BEE.len() as u16 + 3) / 2;
    for _ in 0..pad {
        lines.push(Line::raw(""));
    }
    let bob = (app.tick / 3).is_multiple_of(2);
    for (i, l) in art::BEE.iter().enumerate() {
        let s = if bob && i > 0 {
            format!(" {l}")
        } else {
            l.to_string()
        };
        lines.push(Line::styled(s, theme::muted()).centered());
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(msg.to_string(), theme::faint()).centered());
    f.render_widget(Paragraph::new(lines), area);
}

// ---------- agents ----------

fn draw_agents(f: &mut Frame, app: &mut App, area: Rect) {
    let [rule, area] = Layout::horizontal([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::rule()),
        rule,
    );
    let mut lines = vec![Line::from(vec![
        Span::styled(" agents", theme::bright()),
        Span::styled("  t hides", theme::faint()),
    ])];
    let rows = app.agents.rows(&app.store);
    if rows.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("  no agents reporting yet", theme::faint()));
    }
    let w = area.width.saturating_sub(4) as usize;
    for r in rows {
        lines.push(Line::raw(""));
        let (dot, st) = if r.busy {
            (
                art::THINK[(app.tick as usize / 2) % art::THINK.len()].to_string(),
                theme::bright(),
            )
        } else if r.online {
            ("●".to_string(), theme::text())
        } else {
            ("○".to_string(), theme::faint())
        };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(dot, st),
            Span::raw(" "),
            Span::styled(r.name.clone(), theme::bright()),
        ]));
        lines.push(Line::styled(
            format!("   {}", clip(&r.status, w)),
            theme::muted(),
        ));
        for a in r.recent.iter().take(4) {
            lines.push(Line::styled(
                format!("   ▸ {}", clip(a, w.saturating_sub(2))),
                theme::faint(),
            ));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
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

fn compose_height(app: &App, _width: u16) -> u16 {
    let d = app.draft_ref();
    let lines = d.text.matches('\n').count() as u16 + 1;
    let att = if d.attachments.is_empty() { 0 } else { 1 };
    lines.clamp(1, 8) + att + 2
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
        .borders(Borders::TOP | Borders::BOTTOM)
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
            let l = crate::draft::expand_tabs(l);
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
    let (cl, cc) = d.cursor_line_col();
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
        let y = text_top + (cl - skip) as u16;
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
            f.render_widget(Clear, r);
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
    if app.uploading {
        spans.push(Span::styled(
            format!("uploading {}", art::flight(app.tick, 8)),
            theme::bright(),
        ));
    } else if let Some(s) = status {
        spans.push(Span::styled(s, theme::bright()));
    } else {
        for (k, what, footer) in legend(app.mode) {
            if *footer {
                spans.push(Span::styled(format!(" {k} "), theme::key()));
                spans.push(Span::styled(format!(" {what}  "), theme::muted()));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let h = (app.sections.len() as u16 + 6).max(9);
    let r = centered(area, 60, h);
    f.render_widget(Clear, r);
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
    for (i, s) in app.sections.iter().enumerate() {
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
                y: inner.y + 2 + i as u16,
                width: inner.width,
                height: 1,
            },
            Hit::Section(i),
        ));
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(p) = &app.pending else { return };
    let r = centered(area, 72, 12);
    f.render_widget(Clear, r);
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
    let r = centered(area, 84, 27);
    f.render_widget(Clear, r);
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
            (Mode::Confirm, "upload"),
        ])),
        right,
    );
}

fn draw_viewer(f: &mut Frame, app: &mut App, area: Rect) {
    let r = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(3),
    };
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_style(theme::text())
        .title(Span::styled(" image · esc closes ", theme::bright()));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let Some(url) = app.viewer.clone() else {
        return;
    };
    let key = (url.clone(), inner.width, inner.height);
    match app.media.encoded.get(&key) {
        Some(p) => f.render_widget(Image::new(p), inner),
        None => {
            app.actions.push(crate::app::Action::EncodeImage(key));
            f.render_widget(
                Paragraph::new(
                    Line::styled(
                        format!("loading {}", art::flight(app.tick, 10)),
                        theme::faint(),
                    )
                    .centered(),
                ),
                inner,
            );
        }
    }
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

/// Just enough markdown for chat: code fences, `inline code`, **bold**,
/// _italic_, and @mentions. Wraps to `width` columns.
pub fn render_markdown(src: &str, width: usize, mentions: &[String]) -> Vec<Line<'static>> {
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
        let spans = inline(body, mentions, heading);
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
            s.extend(l);
            out.push(Line::from(s));
        }
    }
    out
}

fn inline(s: &str, mentions: &[String], heading: bool) -> Vec<Span<'static>> {
    let base = if heading {
        theme::bright()
    } else {
        theme::text()
    };
    let mut out = vec![];
    let mut buf = String::new();
    let mut rest = s;
    let flush = |buf: &mut String, out: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            out.push(Span::styled(std::mem::take(buf), base));
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
                out.push(Span::styled(code, theme::code()));
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
                out.push(Span::styled(
                    rest[2..2 + end].to_string(),
                    base.add_modifier(Modifier::BOLD).fg(theme::gray(23)),
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
                out.push(Span::styled(m.clone(), theme::mention()));
                rest = &rest[m.len()..];
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
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(4);
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![]];
    let mut w = 0usize;
    for span in spans {
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
                        l.push(Span::styled(chunk, style))
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
                l.push(Span::styled(word, style))
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
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
