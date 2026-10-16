//! One authenticated WebSocket to the relay, owned by a background task.
//! The app sends subscribe/close commands; stored events, EOSE and live
//! events come back on a channel. A REQ returns history and then streams,
//! so there is no gap between backfill and live. On disconnect the task
//! reconnects with backoff and re-issues every open subscription.

use std::collections::BTreeMap;
use std::time::Duration;

use buzz_ws_client::{NostrWsConnection, RelayMessage};
use nostr::{Event, Keys};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// How long the relay may say nothing before we call the socket dead.
const SILENCE: Duration = Duration::from_secs(90);

pub enum Cmd {
    Sub {
        id: String,
        filters: Vec<Value>,
    },
    Close {
        id: String,
    },
    /// Fire-and-forget publish over the socket (ephemeral kinds like presence).
    Publish(Box<Event>),
}

#[derive(Debug)]
pub enum Live {
    Event { sub: String, event: Box<Event> },
    Eose { sub: String },
    Closed { sub: String, reason: String },
    Status(ConnState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Online,
    Offline,
}

pub fn spawn(
    url: String,
    keys: Keys,
    auth: Option<nostr::Tag>,
    out: mpsc::UnboundedSender<Live>,
) -> mpsc::UnboundedSender<Cmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(url, keys, auth, rx, out));
    tx
}

async fn run(
    url: String,
    keys: Keys,
    auth: Option<nostr::Tag>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    out: mpsc::UnboundedSender<Live>,
) {
    let mut subs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut backoff = 1u64;
    loop {
        let _ = out.send(Live::Status(ConnState::Connecting));
        let mut conn =
            match NostrWsConnection::connect_authenticated(&url, &keys, auth.as_ref()).await {
                Ok(c) => c,
                Err(e) => {
                    crate::log::line(format!("ws connect {url}: {e}"));
                    let _ = out.send(Live::Status(ConnState::Offline));
                    // Keep collecting commands while offline so nothing is lost.
                    let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            cmd = rx.recv() => match cmd {
                                Some(c) => apply(&mut subs, c),
                                None => return,
                            },
                        }
                    }
                    backoff = (backoff * 2).min(30);
                    continue;
                }
            };
        backoff = 1;
        let _ = out.send(Live::Status(ConnState::Online));
        let mut ok = true;
        for (id, filters) in &subs {
            if send_req(&mut conn, id, filters).await.is_err() {
                ok = false;
                break;
            }
        }
        // One deadline for the whole connection, pushed back only by what the
        // relay sends. The 60 s presence heartbeat gets an OK back, so a live
        // socket never goes 90 s quiet.
        let dead = tokio::time::sleep(SILENCE);
        tokio::pin!(dead);
        while ok {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    let res = match &cmd {
                        Cmd::Sub { id, filters } => send_req(&mut conn, id, filters).await,
                        Cmd::Close { id } => conn.send_raw(&json!(["CLOSE", id])).await,
                        Cmd::Publish(ev) => conn.send_raw(&json!(["EVENT", ev])).await,
                    };
                    apply(&mut subs, cmd);
                    ok = res.is_ok();
                }
                _ = &mut dead => {
                    crate::log::line("ws silent, reconnecting");
                    ok = false;
                }
                msg = conn.next_event(Duration::from_secs(3600)) => {
                    if msg.is_ok() {
                        dead.as_mut().reset(tokio::time::Instant::now() + SILENCE);
                    }
                    match msg {
                        Ok(RelayMessage::Event { subscription_id, event }) => {
                            let _ = out.send(Live::Event { sub: subscription_id, event });
                        }
                        Ok(RelayMessage::Eose { subscription_id }) => {
                            let _ = out.send(Live::Eose { sub: subscription_id });
                        }
                        Ok(RelayMessage::Closed { subscription_id, message }) => {
                            subs.remove(&subscription_id);
                            let _ = out.send(Live::Closed { sub: subscription_id, reason: message });
                        }
                        Ok(_) => {}
                        Err(
                            buzz_ws_client::WsClientError::Timeout
                            | buzz_ws_client::WsClientError::ConnectionClosed
                            | buzz_ws_client::WsClientError::WebSocket(_),
                        ) => {
                            crate::log::line("ws dropped");
                            ok = false
                        }
                        // A frame we couldn't parse isn't a reason to drop the socket.
                        Err(_) => {}
                    }
                }
            }
        }
        let _ = out.send(Live::Status(ConnState::Offline));
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn apply(subs: &mut BTreeMap<String, Vec<Value>>, cmd: Cmd) {
    match cmd {
        Cmd::Sub { id, filters } => {
            subs.insert(id, filters);
        }
        Cmd::Close { id } => {
            subs.remove(&id);
        }
        // Presence is re-sent by its own timer; nothing to replay.
        Cmd::Publish(_) => {}
    }
}

async fn send_req(
    conn: &mut NostrWsConnection,
    id: &str,
    filters: &[Value],
) -> Result<(), buzz_ws_client::WsClientError> {
    let mut req = vec![json!("REQ"), json!(id)];
    req.extend(filters.iter().cloned());
    conn.send_raw(&Value::Array(req)).await
}
