//! buzz-vim: a small, keyboard-first terminal client for Buzz.

// Nested `if`s read better than let-chains in the key handlers.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

mod app;
mod art;
mod clip;
mod config;
mod draft;
mod keys;
mod live;
mod log;
mod media;
mod model;
mod relay;
mod sections;
mod state;
mod sync;
mod telemetry;
mod theme;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream,
};
use crossterm::execute;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use app::{Action, App};
use live::{ConnState, Live};
use sync::{Msg, Sync};

const HELP: &str = "buzz-vim: a small vim-style terminal client for Buzz

usage:
  buzz-vim          open it
  buzz-vim login    save your relay and nsec (once)

env: BUZZ_PRIVATE_KEY, BUZZ_RELAY_URL override the saved login.
inside: press ? for every key, q to quit.";

fn main() -> Result<()> {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("login") => return config::login(),
        Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("buzz-vim {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown argument {other:?}\n\n{HELP}"),
        None => {}
    }
    let cfg = config::load()?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // Ask the terminal what image protocol it speaks before taking over the screen.
    let picker = media::picker();
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let res = rt.block_on(run(&mut terminal, cfg, picker));
    execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    ratatui::restore();
    res
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    cfg: config::Config,
    picker: Option<ratatui_image::picker::Picker>,
) -> Result<()> {
    let relay = relay::Relay::new(&cfg.relay, cfg.keys.clone(), cfg.auth_tag.clone())?;
    let (live_tx, mut live_rx) = mpsc::unbounded_channel::<Live>();
    let live_cmd = live::spawn(
        relay.ws_url.clone(),
        cfg.keys.clone(),
        cfg.auth_tag.clone(),
        live_tx,
    );
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(cfg.keys.public_key().to_hex(), picker);
    app.relay_host = relay.base.clone();
    let saved = state::load(&relay.base, &app.store.me);
    app.chosen = saved.chosen.clone();
    let mut sync = Sync::new(relay, live_cmd, tx);
    sync.start();
    sync.want_profiles([app.store.me.clone()]);

    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(150));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(60));
    let mut channels_poll = tokio::time::interval(Duration::from_secs(300));
    channels_poll.tick().await;
    let mut was_online = false;
    let mut restore_active = saved.active.clone();
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut dirty = true;

    loop {
        if dirty && last_draw.elapsed() >= Duration::from_millis(16) {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            last_draw = Instant::now();
            dirty = false;
            // Drawing can ask for images.
            for a in std::mem::take(&mut app.actions) {
                sync.run(&mut app, a);
            }
        }
        tokio::select! {
            ev = input.next() => {
                let Some(Ok(ev)) = ev else { break };
                app.on_term(ev);
                dirty = true;
            }
            Some(msg) = rx.recv() => {
                handle(&mut app, &mut sync, msg, &mut restore_active);
                dirty = true;
            }
            Some(l) = live_rx.recv() => {
                match l {
                    Live::Event { sub, event } => {
                        let want = sync::fold(&mut app, &mut sync, &event, sub.starts_with("ch:"));
                        sync.want_profiles(want);
                    }
                    Live::Status(s) => {
                        app.conn = s;
                        if s == ConnState::Online {
                            sync.heartbeat();
                            if was_online {
                                sync.refetch_open();
                            }
                            was_online = true;
                        }
                    }
                    Live::Closed { sub, reason } => {
                        log::line(format!("closed {sub}: {reason}"));
                        if !reason.is_empty() {
                            app.say(format!("relay closed {sub}: {reason}"));
                        }
                    }
                    Live::Eose { sub } => log::line(format!("eose {sub}")),
                }
                dirty = true;
            }
            _ = tick.tick() => {
                app.tick += 1;
                // Only redraw on ticks when something is animating.
                if app.animating() {
                    dirty = true;
                }
            }
            _ = heartbeat.tick() => sync.heartbeat(),
            _ = channels_poll.tick() => sync.load_channels(),
        }
        let actions = std::mem::take(&mut app.actions);
        let mut quit = false;
        for a in actions {
            match a {
                Action::Quit => quit = true,
                Action::SaveState => {
                    state::save(
                        &sync.relay.base,
                        &app.store.me,
                        &state::State {
                            chosen: app.chosen.clone(),
                            active: app.current().map(str::to_string),
                        },
                    );
                }
                a => sync.run(&mut app, a),
            }
        }
        if quit {
            state::save(
                &sync.relay.base,
                &app.store.me,
                &state::State {
                    chosen: app.chosen.clone(),
                    active: app.current().map(str::to_string),
                },
            );
            break;
        }
    }
    Ok(())
}

fn handle(app: &mut App, sync: &mut Sync, msg: Msg, restore_active: &mut Option<String>) {
    match msg {
        Msg::Channels(Ok(evs)) => {
            for ev in &evs {
                app.store.apply(ev);
            }
            // DM names come from the other participants' profiles.
            let people: Vec<String> = app
                .store
                .channels
                .values()
                .filter(|c| c.is_dm)
                .flat_map(|c| c.members.clone())
                .collect();
            sync.want_profiles(people);
            refresh_sections(app, sync, restore_active);
        }
        Msg::Channels(Err(e)) => app.say(format!("couldn't load channels: {e}")),
        Msg::Lanes(evs) => {
            sync.apply_lanes(&evs);
            app.sections_loaded = true;
            refresh_sections(app, sync, restore_active);
        }
        Msg::Window {
            channel,
            events,
            older,
        } => {
            sync.window_done(&channel);
            match events {
                Ok(evs) => {
                    let mut want = vec![];
                    for ev in &evs {
                        if ev.kind.as_u16() == 39006 {
                            let v: Value = serde_json::from_str(&ev.content).unwrap_or_default();
                            let more = v.get("has_more").and_then(Value::as_bool).unwrap_or(false);
                            let cur = v.get("next_cursor").and_then(|c| {
                                Some((
                                    c.get("created_at")?.as_u64()?,
                                    c.get("id")?.as_str()?.to_string(),
                                ))
                            });
                            sync.cursors
                                .insert(channel.clone(), if more { cur } else { None });
                            continue;
                        }
                        want.extend(sync::fold(app, sync, ev, false));
                    }
                    let ch = app.store.channel_mut(&channel);
                    ch.loaded = true;
                    if older && evs.len() <= 1 {
                        ch.exhausted = true;
                    }
                    sync.want_profiles(want);
                }
                Err(e) => app.say(format!("couldn't load messages: {e}")),
            }
        }
        Msg::Thread { events } => match events {
            Ok(evs) => {
                let mut want = vec![];
                for ev in &evs {
                    want.extend(sync::fold(app, sync, ev, false));
                }
                sync.want_profiles(want);
            }
            Err(e) => app.say(format!("couldn't load thread: {e}")),
        },
        Msg::Events(evs) => {
            let mut want = vec![];
            let mut agents = false;
            for ev in &evs {
                agents |= ev.kind.as_u16() == 30177;
                want.extend(sync::fold(app, sync, ev, false));
            }
            sync.want_profiles(want);
            if agents {
                sync.want_presence(app.agents.by_pubkey.keys().cloned().collect());
            }
        }
        Msg::Sent { result } => match result {
            Ok(ev) => {
                app.pending_sends.remove(&ev.id.to_hex());
                app.sent_flash = Some((app.tick as usize % art::SENT.len(), Instant::now()));
            }
            Err(e) => {
                // Put the text back so nothing is lost.
                let id = app.pending_sends.keys().next().cloned();
                if let Some(id) = id {
                    if let Some(d) = app.pending_sends.remove(&id) {
                        app.store.remove_message(&id);
                        if app.draft_ref().is_empty() {
                            *app.draft_mut() = d;
                        }
                    }
                }
                app.say(format!("not sent: {e}"));
            }
        },
        Msg::Did(r) => match r {
            Ok(s) => app.say(s),
            Err(e) => app.say(format!("failed: {e}")),
        },
        Msg::Uploaded(r, to_thread) => {
            app.uploading = false;
            match r {
                Ok(d) => {
                    let draft = if to_thread {
                        &mut app.thread_draft
                    } else {
                        &mut app.draft
                    };
                    draft.attachments.push(d);
                    app.mode = keys::Mode::Insert;
                    app.say("attached. ⏎ sends it");
                }
                Err(e) => app.say(format!("upload failed: {e}")),
            }
        }
        Msg::Image { url, result } => {
            let slot = match result {
                Ok(img) => media::Slot::Ready(Arc::new(img)),
                Err(e) => media::Slot::Failed(e.to_string()),
            };
            app.media.images.insert(url, slot);
        }
        Msg::Encoded { key, result } => {
            app.media.encoding.remove(&key);
            match result {
                Ok(p) => {
                    if app.media.encoded.len() > 64 {
                        app.media.encoded.clear();
                    }
                    app.media.encoded.insert(key, p);
                }
                Err(e) => {
                    app.media
                        .images
                        .insert(key.0, media::Slot::Failed(e.to_string()));
                }
            }
        }
    }
}

fn refresh_sections(app: &mut App, sync: &mut Sync, restore_active: &mut Option<String>) {
    app.sections = sections::build(&sync.lanes, &app.store);
    app.chosen
        .retain(|c| app.sections.iter().any(|s| s.id == *c));
    app.rebuild_tabs();
    if let Some(want) = restore_active.take() {
        if let Some(i) = app.tabs.iter().position(|t| *t == want) {
            app.active = i;
            app.open_current();
        } else {
            *restore_active = Some(want);
        }
    }
    for t in app.tabs.clone() {
        sync.subscribe_channel(&t);
    }
    let tabs = app.tabs.clone();
    sync.unsubscribe_others(&tabs);
}
