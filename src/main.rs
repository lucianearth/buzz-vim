//! buzz-vim: a small, keyboard-first terminal client for Buzz.

// Nested `if`s read better than let-chains in the key handlers.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

mod app;
mod art;
mod clip;
mod config;
mod garden;
mod keys;
mod media;
mod state;
mod sync;
mod theme;
mod ui;

use buzz_vim_core::{
    checklist, draft, links, live, log, model, needs, quick, readstate, relay, sections, telemetry,
    unread,
};

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
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
    theme::watch();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // Ask the terminal what image protocol it speaks before taking over the screen.
    let picker = media::picker();
    let mut terminal = ratatui::init();
    // ratatui's panic hook undoes raw mode and the alternate screen; mouse
    // capture and bracketed paste are ours to undo, before it runs.
    let ratatui_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The UI runs on the main thread and keeps going after a background
        // task panics (a blocking decode comes back as an error; a spawned
        // task just stops), so the terminal has to stay as it is.
        if std::thread::current().name() != Some("main") {
            log::line(format!("background panic: {info}"));
            return;
        }
        let _ = execute!(
            std::io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            DisableBracketedPaste
        );
        tmux_keys(false);
        ratatui_hook(info);
    }));
    // Terminals that speak the kitty keyboard protocol (iTerm2, kitty,
    // WezTerm, Ghostty) can then tell Shift+Enter apart from Enter. That
    // takes reporting every key: with less, Enter keeps its legacy byte
    // whatever the modifiers. Alternate keys turn Shift+a back into 'A'. Others
    // ignore the request. Probing first costs a stdin round-trip that can
    // leave the terminal cooked, so just ask.
    let _ = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );
    tmux_keys(true);
    let res = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)
        .map_err(anyhow::Error::from)
        .and_then(|_| rt.block_on(run(&mut terminal, cfg, picker)));
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    tmux_keys(false);
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
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
    app.checklists = checklist::Checklists::new(checklist::Trust {
        owner: cfg.loop_.owner.clone(),
        watchers: cfg.loop_.watchers.clone(),
    });
    app.loop_ = cfg.loop_.clone();
    let saved = state::load(&relay.base, &app.store.me);
    app.chosen = saved.chosen.clone();
    app.read_width = saved.read_width;
    app.last_bot = saved.last_bot.clone();
    app.agents.restore_context(&saved.context);
    let mut sync = Sync::new(relay, live_cmd, tx);
    if !saved.read_slot.is_empty() && !saved.read_client.is_empty() {
        app.read = readstate::ReadState::new(saved.read_slot.clone(), saved.read_client.clone());
    }
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
    let mut last_minute = 0u64;

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
                match ev {
                    Some(Ok(ev)) => app.on_term(ev),
                    // The terminal went away: quit the normal way so state is saved.
                    _ => app.actions.push(Action::Quit),
                }
                dirty = true;
            }
            Some(msg) = rx.recv() => {
                handle(&mut app, &mut sync, msg, &mut restore_active);
                dirty = true;
            }
            Some(l) = live_rx.recv() => {
                match l {
                    Live::Event { sub, event } => {
                        // Tab 0's tail also carries tabs' messages; whichever
                        // sub brings one first, it counts as a tab's.
                        let tab = sub == "needs"
                            && model::tag(&event, "h").is_some_and(|h| app.tabs.iter().any(|t| t == h));
                        let want = sync::fold(&mut app, &mut sync, &event, sub.starts_with("ch:") || tab);
                        sync.want_profiles(want);
                    }
                    Live::Status(s) => {
                        app.conn = s;
                        if s == ConnState::Online {
                            sync.heartbeat();
                            if was_online {
                                sync.refetch_open();
                                if let Some(c) = app.current().map(str::to_string) {
                                    sync.load_checklist(&c);
                                }
                                sync.load_needs(&app.store, &app.checklists.trust.owner);
                            }
                            was_online = true;
                        }
                    }
                    Live::Closed { sub, reason } => {
                        log::line(format!("closed {sub}: {reason}"));
                        // A relay without the checklist kinds refuses that
                        // tail; the rest works without it, so no need to say.
                        if !reason.is_empty() && sub != "checklist" && sub != "needs" {
                            app.say(format!("relay closed {sub}: {reason}"));
                        }
                    }
                    Live::Eose { sub } => log::line(format!("eose {sub}")),
                }
                dirty = true;
            }
            _ = tick.tick() => {
                app.tick += 1;
                // Only redraw on ticks when something is animating, or
                // once a minute so "4m ago" stays true.
                let minute = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() / 60);
                if app.animating() || minute != last_minute {
                    last_minute = minute;
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
                    state::save(&sync.relay.base, &app.store.me, &saved_state(&app));
                }
                a => sync.run(&mut app, a),
            }
        }
        sync.check_unread(&app.read, &app.tabs);
        sync.flush_read(&mut app.read);
        if quit {
            state::save(&sync.relay.base, &app.store.me, &saved_state(&app));
            sync.flush_read_now(&mut app.read).await;
            break;
        }
    }
    Ok(())
}

fn saved_state(app: &App) -> state::State {
    state::State {
        chosen: app.chosen.clone(),
        active: app.current().map(str::to_string),
        read_slot: app.read.slot().to_string(),
        read_client: app.read.client_id().to_string(),
        read_width: app.read_width,
        last_bot: app.last_bot.clone(),
        context: app.agents.saved_context(),
    }
}

fn handle(app: &mut App, sync: &mut Sync, msg: Msg, restore_active: &mut Option<String>) {
    match msg {
        Msg::ReadState(r) => {
            match r {
                Ok(evs) => {
                    sync.apply_read(&mut app.read, &evs);
                    app.read.loaded = true;
                }
                Err(e) => log::line(format!("read state load: {e}")),
            }
            // Even on failure, so unread still works from the last week.
            app.read.tried = true;
            sync::prune_unread(app);
        }
        Msg::Unread { channel, events } => match events {
            Ok(evs) => {
                // Replies go in the store: they make "(N new)" and the bell.
                let mut want = vec![];
                for ev in evs
                    .iter()
                    .filter(|e| model::Message::from_event(e).root.is_some())
                {
                    want.extend(sync::fold(app, sync, ev, false));
                }
                sync.want_profiles(want);
                sync::fold_unread(app, &channel, &evs);
            }
            Err(e) => log::line(format!("unread {channel}: {e}")),
        },
        Msg::Checklist {
            channel,
            records,
            today,
        } => {
            let mut want = vec![];
            match records {
                Ok(evs) => {
                    for ev in &evs {
                        if app.checklists.apply(ev) {
                            want.push(ev.pubkey.to_hex());
                        }
                    }
                }
                // Before the relay knows the kinds this is expected.
                Err(e) => log::line(format!("checklist {channel}: {e}")),
            }
            match today {
                Ok(evs) => {
                    for ev in &evs {
                        let id = ev.id.to_hex();
                        let at = ev.created_at.as_secs();
                        app.checklists.note_message(&channel, &id, at, &ev.content);
                    }
                }
                Err(e) => log::line(format!("crashes {channel}: {e}")),
            }
            sync.want_profiles(want);
        }
        Msg::QuickSent { id, result, text } => match result {
            Ok(()) => {
                let agent = app.loop_.quick_agent.clone();
                app.say(format!(
                    "asked {agent} in #{}; the answer lands in that thread",
                    app.loop_.quick_channel
                ));
            }
            Err(e) => {
                // It never landed: off the screen, and its text back in the
                // box, after anything typed since.
                app.store.remove_message(&id);
                let d = &mut app.quick_draft;
                if !d.is_empty() {
                    d.end();
                    d.insert("\n\n");
                }
                d.insert(&text);
                app.say(format!("Q not sent: {e}. Q opens it again."));
            }
        },
        Msg::Archived(r) => match r {
            Ok(pks) => app.archived = pks.into_iter().collect(),
            Err(e) => log::line(format!("archived: {e}")),
        },
        Msg::Channels(Ok(evs)) => {
            app.channels_loaded = true;
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
            sync.load_needs(&app.store, &app.checklists.trust.owner);
        }
        Msg::Channels(Err(e)) => app.say(format!("couldn't load channels: {e}")),
        Msg::Lanes(Ok(evs)) => {
            sync.apply_lanes(&evs);
            app.sections_loaded = true;
            refresh_sections(app, sync, restore_active);
        }
        Msg::Moved(Ok((ev, label))) => {
            sync.apply_lanes(&[ev]);
            // Stay on the channel even if it just left today's tabs.
            if app.extra.is_none() {
                app.extra = app.current().map(str::to_string);
            }
            refresh_sections(app, sync, restore_active);
            app.say(label);
        }
        Msg::Moved(Err(e)) => app.say(format!("couldn't move: {e}")),
        Msg::Ordered(Ok(_)) => {}
        Msg::Ordered(Err(e)) => app.say(format!("tab order not saved: {e}")),
        Msg::Lanes(Err(e)) => app.say(format!("couldn't load sections: {e}")),
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
        Msg::Sent { id, result } => {
            let pending = app.pending_sends.remove(&id);
            match result {
                Ok(()) => {
                    app.sent_flash = Some((app.tick as usize % art::SENT.len(), Instant::now()));
                }
                Err(e) => {
                    app.store.remove_message(&id);
                    // Put the text back so nothing is lost: in its own box if that
                    // conversation is still open, otherwise in the one in front of you.
                    let Some((channel, root, mut d)) = pending else {
                        app.say(format!("not sent: {e}"));
                        return;
                    };
                    // A relay that refused it would refuse the same event
                    // again; only a send lost in transit is worth repeating.
                    if e.to_string().starts_with("relay rejected") {
                        d.sent_as = None;
                    }
                    match app.draft_for(&channel, root.as_deref()) {
                        Some(slot) => {
                            slot.restore(d);
                            app.say(format!("not sent: {e}. Your text is back in its box."));
                        }
                        None => {
                            app.draft_mut().restore(d);
                            let name = app
                                .store
                                .channels
                                .get(&channel)
                                .map(ui::channel_label)
                                .unwrap_or(channel);
                            app.say(format!(
                                "not sent to {name}: {e}. Your text is in this box now."
                            ));
                        }
                    }
                }
            }
        }
        Msg::Did(r) => match r {
            Ok(s) => app.say(s),
            Err(e) => app.say(format!("failed: {e}")),
        },
        Msg::ChannelArchived { channel, result } => {
            let name = app
                .store
                .channels
                .get(&channel)
                .map_or_else(String::new, |c| c.name.clone());
            match result {
                Ok(()) => app.say(format!("archived #{name}")),
                Err(e) => {
                    app.unarchive(&channel);
                    app.say(format!("#{name} not archived: {e}"));
                }
            }
        }
        Msg::Added {
            channel,
            pubkeys,
            result,
        } => match result {
            Ok(()) => {
                let names: Vec<String> = pubkeys.iter().map(|pk| app.store.name_of(pk)).collect();
                // No live member updates here, so count them in ourselves.
                app.store.channel_mut(&channel).members.extend(pubkeys);
                app.say(format!("added {} to the channel", names.join(", ")));
            }
            Err(e) => app.say(format!("couldn't add to the channel: {e}")),
        },
        Msg::Uploaded(r, (channel, root)) => {
            app.uploading = false;
            match r {
                Ok(d) => {
                    let here = app.draft_target() == Some((channel.clone(), root.clone()));
                    match app.draft_for(&channel, root.as_deref()) {
                        Some(draft) => {
                            draft.attachments.push(d);
                            if here {
                                app.mode = keys::Mode::Insert;
                            }
                            app.say("attached. ⏎ sends it");
                        }
                        None => app.say(format!(
                            "uploaded, but that conversation is closed, so it wasn't attached: {}",
                            d.url
                        )),
                    }
                }
                Err(e) => app.say(format!("upload failed: {e}")),
            }
        }
        Msg::Image { url, result } => {
            let slot = match result {
                Ok(img) => media::Slot::Ready(Arc::new(img)),
                Err(e) => media::Slot::Failed(e.to_string()),
            };
            app.media.progress.remove(&url);
            app.media.put_image(url, slot);
        }
        Msg::Progress { url, got, total } => {
            app.media.progress.insert(url, (got, total));
        }
        Msg::Bytes { url, bytes } => {
            app.media.keep_bytes(url, bytes);
        }
        Msg::Attachment { label, result } => match (result, label) {
            (Ok(p), _) => app.attach(p),
            (Err(e), Some(path)) => app.say(format!("can't attach {path}: {e}")),
            (Err(e), None) => app.say(e.to_string()),
        },
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
    // Forget saved choices only once both halves have loaded; before that a
    // missing section just hasn't arrived yet.
    if app.sections_loaded && app.channels_loaded {
        app.chosen
            .retain(|c| app.sections.iter().any(|s| s.id == *c));
    }
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

/// tmux drops the kitty keyboard request, so inside it Shift+Enter arrives
/// as a plain Enter. It does forward modifyOtherKeys to apps that ask, given
/// `extended-keys on`; crossterm reads that only in tmux's `csi-u` format.
fn tmux_keys(on: bool) {
    use std::io::Write;
    if std::env::var_os("TMUX").is_none() {
        return;
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(if on { b"\x1b[>4;2m" } else { b"\x1b[>4m" });
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A send that fails comes back to the box with its signed event, and
    /// sending the same text again re-publishes that event rather than a twin.
    #[tokio::test]
    async fn resend_after_failure_reuses_the_event() {
        let relay = relay::Relay::new("http://127.0.0.1:1", nostr::Keys::generate(), None).unwrap();
        let (live_tx, _live_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sync = sync::Sync::new(relay, live_tx, tx);
        let mut app = App::new("me".into(), None);
        let ch = "2130646b-d6c8-4ec3-93e3-36f77bee1a37".to_string();
        app.tabs = vec![ch.clone()];
        let other = "bcd11c3d-f78b-4934-8e48-c08b6baed1c5".to_string();
        let mut restore = None;
        let mut sent = vec![];
        // Type, resend as is, edit the restored text and send, then send the
        // same text somewhere else.
        for (text, to) in [("hello", &ch), ("", &ch), (" again", &ch), ("", &other)] {
            app.draft_mut().insert(text);
            let channel = to.clone();
            sync.run(
                &mut app,
                Action::Send {
                    channel,
                    root: None,
                    parent: None,
                },
            );
            let msg = rx.recv().await.unwrap();
            let sync::Msg::Sent { id, result } = &msg else {
                panic!("expected a send result");
            };
            assert!(result.is_err());
            sent.push(id.clone());
            handle(&mut app, &mut sync, msg, &mut restore);
            assert!(!app.draft_ref().is_empty());
        }
        // Unchanged text: the same event. Edited text, or another channel:
        // a new one.
        assert_eq!(sent[0], sent[1]);
        assert_ne!(sent[1], sent[2]);
        assert_ne!(sent[2], sent[3]);
    }

    /// A Q the relay never took leaves #quick and goes back in the box,
    /// after anything typed since.
    #[tokio::test]
    async fn a_failed_q_comes_back_to_the_box() {
        let relay = relay::Relay::new("http://127.0.0.1:1", nostr::Keys::generate(), None).unwrap();
        let (live_tx, _live_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sync = sync::Sync::new(relay, live_tx, tx);
        let mut app = App::new("me".into(), None);
        let q = quick::CHANNEL_ID.to_string();
        sync.run(
            &mut app,
            Action::SendQuick {
                channel: q.clone(),
                content: "[Q]\n@taskbot why?".into(),
                agent: quick::AGENT_PUBKEY.into(),
                text: "why?".into(),
            },
        );
        assert_eq!(app.store.channels[&q].timeline.len(), 1);
        app.quick_draft.insert("next one");
        let msg = rx.recv().await.unwrap();
        handle(&mut app, &mut sync, msg, &mut None);
        assert!(app.store.channels[&q].timeline.is_empty());
        assert_eq!(app.quick_draft.text, "next one\n\nwhy?");
    }

    /// A dropped file is read off the UI loop, then offered for upload.
    #[tokio::test]
    async fn dropped_files_are_read_in_the_background() {
        let dir = std::env::temp_dir().join(format!("bv-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("shot.png");
        std::fs::write(&f, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let relay = relay::Relay::new("http://127.0.0.1:1", nostr::Keys::generate(), None).unwrap();
        let (live_tx, _live_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sync = sync::Sync::new(relay, live_tx, tx);
        let mut app = App::new("me".into(), None);
        app.on_term(crossterm::event::Event::Paste(f.display().to_string()));
        assert!(app.pending.is_none());
        for a in std::mem::take(&mut app.actions) {
            sync.run(&mut app, a);
        }
        let msg = rx.recv().await.unwrap();
        handle(&mut app, &mut sync, msg, &mut None);
        assert_eq!(
            app.pending.as_ref().map(|p| p.mime.as_str()),
            Some("image/png")
        );
        assert_eq!(app.mode, keys::Mode::Confirm);
        // Finishing while a popup is up leaves the popup alone.
        app.pending = None;
        app.mode = keys::Mode::Sections;
        app.on_term(crossterm::event::Event::Paste(f.display().to_string()));
        for a in std::mem::take(&mut app.actions) {
            sync.run(&mut app, a);
        }
        let msg = rx.recv().await.unwrap();
        handle(&mut app, &mut sync, msg, &mut None);
        assert!(app.pending.is_none());
        assert_eq!(app.mode, keys::Mode::Sections);
        std::fs::remove_dir_all(&dir).ok();
    }
}
