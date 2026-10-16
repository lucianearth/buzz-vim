//! Log in by pairing with Buzz Desktop (NIP-AB), the way the official apps
//! do: Desktop shows a `nostrpair://` QR, we scan it, both screens show the
//! same six digits, both people say yes, and Desktop sends the identity
//! over the pairing relay, encrypted to this session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use buzz_core::pairing::qr::decode_qr;
use buzz_core::pairing::{AbortReason, PairingSession};
use buzz_vim_core::relay::ws_base;
use buzz_ws_client::{NostrWsConnection, RelayMessage};
use nostr::{EventBuilder, RelayUrl};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::{BuzzError, Listener, RT};

use buzz_core::kind::KIND_PAIRING;

#[derive(uniffi::Enum, Clone, PartialEq, Eq)]
pub enum PairStep {
    Connecting,
    /// Show `code`; `desktop_said_yes` turns true once Desktop confirmed it.
    Compare {
        code: String,
        desktop_said_yes: bool,
    },
    Receiving,
    Done {
        relay: String,
        secret: String,
    },
    Failed {
        message: String,
    },
}

struct State {
    step: PairStep,
    answer: Option<oneshot::Sender<bool>>,
}

#[derive(uniffi::Object)]
pub struct Pairing {
    state: Arc<Mutex<State>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl Pairing {
    /// Start pairing from the scanned QR text.
    #[uniffi::constructor]
    pub fn new(qr: String, listener: Box<dyn Listener>) -> Result<Arc<Self>, BuzzError> {
        let qr = decode_qr(qr.trim())
            .map_err(|e| BuzzError::Failed(format!("that isn't a Buzz pairing code: {e}")))?;
        let relay = qr
            .relays
            .first()
            .cloned()
            .ok_or_else(|| BuzzError::Failed("the pairing code names no relay".into()))?;
        let (session, offer) =
            PairingSession::new_target(&qr).map_err(|e| BuzzError::Failed(e.to_string()))?;
        let code = session.sas_code().unwrap_or_default();
        let state = Arc::new(Mutex::new(State {
            step: PairStep::Connecting,
            answer: None,
        }));
        let (tx, rx) = oneshot::channel();
        state.lock().unwrap().answer = Some(tx);
        let st = state.clone();
        let listener: Arc<dyn Listener> = Arc::from(listener);
        let task = RT.spawn(async move {
            let set = |step: PairStep| {
                st.lock().unwrap().step = step;
                listener.changed();
            };
            let result = run(session, offer, &relay, &code, rx, &set).await;
            if let Err(message) = result {
                set(PairStep::Failed { message });
            }
        });
        Ok(Arc::new(Pairing {
            state,
            task: Mutex::new(Some(task)),
        }))
    }

    pub fn step(&self) -> PairStep {
        self.state.lock().unwrap().step.clone()
    }

    /// The person's answer: do both screens show the same code?
    pub fn answer(&self, same: bool) {
        if let Some(tx) = self.state.lock().unwrap().answer.take() {
            let _ = tx.send(same);
        }
    }

    pub fn cancel(&self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }
}

impl Drop for Pairing {
    fn drop(&mut self) {
        self.cancel();
    }
}

async fn run(
    mut session: PairingSession,
    offer: nostr::Event,
    relay: &str,
    code: &str,
    answer: oneshot::Receiver<bool>,
    set: &(dyn Fn(PairStep) + Sync),
) -> Result<(), String> {
    let mut conn = NostrWsConnection::connect(relay)
        .await
        .map_err(|e| format!("couldn't reach the pairing relay: {e}"))?;
    // The pairing relay may ask for NIP-42 auth; answer as this session.
    if let Ok(RelayMessage::Auth { challenge }) = conn.next_event(Duration::from_secs(3)).await {
        let url = RelayUrl::parse(relay).map_err(|e| e.to_string())?;
        let auth = session
            .sign_event(EventBuilder::auth(challenge, url))
            .map_err(|e| e.to_string())?;
        conn.send_raw(&json!(["AUTH", auth]))
            .await
            .map_err(|e| e.to_string())?;
    }
    let me = session.pubkey().to_hex();
    conn.send_raw(&json!(["REQ", "pair", {"kinds": [KIND_PAIRING], "#p": [me]}]))
        .await
        .map_err(|e| e.to_string())?;
    // Subscribed before the offer goes out, so a fast answer isn't missed.
    loop {
        match next(&mut conn, 10).await? {
            RelayMessage::Eose { .. } => break,
            RelayMessage::Closed { message, .. } => {
                return Err(format!("the pairing relay refused: {message}"));
            }
            _ => {}
        }
    }
    conn.send_raw(&json!(["EVENT", offer]))
        .await
        .map_err(|e| e.to_string())?;
    set(PairStep::Compare {
        code: code.to_string(),
        desktop_said_yes: false,
    });

    loop {
        let ev = event(&mut conn, 180).await?;
        if session.handle_abort(&ev).is_ok() {
            return Err("Desktop stopped the pairing.".into());
        }
        match session.handle_sas_confirm(&ev) {
            Ok(_) => break,
            Err(buzz_core::pairing::PairingError::TranscriptMismatch) => {
                if let Ok(Some(a)) = session.abort(AbortReason::SasMismatch) {
                    let _ = conn.send_raw(&json!(["EVENT", a])).await;
                }
                return Err("The codes didn't check out, so pairing stopped. Try again.".into());
            }
            Err(_) => continue,
        }
    }
    set(PairStep::Compare {
        code: code.to_string(),
        desktop_said_yes: true,
    });

    if !answer.await.unwrap_or(false) {
        if let Ok(Some(a)) = session.abort(AbortReason::SasMismatch) {
            let _ = conn.send_raw(&json!(["EVENT", a])).await;
        }
        return Err("Stopped: the codes didn't match.".into());
    }
    session.confirm_target_sas().map_err(|e| e.to_string())?;
    set(PairStep::Receiving);

    let payload: Zeroizing<String> = loop {
        let ev = event(&mut conn, 60).await?;
        if session.handle_abort(&ev).is_ok() {
            return Err("Desktop stopped the pairing.".into());
        }
        if let Ok((_, p)) = session.handle_payload(&ev) {
            break p;
        }
    };
    // Desktop sends {"relayUrl", "pubkey", "nsec"}; older ones a bare nsec.
    let (relay_url, secret) = match serde_json::from_str::<Value>(&payload) {
        Ok(v) => (
            v.get("relayUrl").and_then(Value::as_str).map(ws_base),
            v.get("nsec")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or("Desktop sent no key")?,
        ),
        Err(_) => (None, payload.to_string()),
    };
    if let Ok(done) = session.send_complete() {
        let _ = conn.send_raw(&json!(["EVENT", done])).await;
        // Give the completion a moment to leave before the socket drops.
        let _ = next(&mut conn, 2).await;
    }
    set(PairStep::Done {
        relay: relay_url.unwrap_or_default(),
        secret,
    });
    Ok(())
}

async fn next(conn: &mut NostrWsConnection, secs: u64) -> Result<RelayMessage, String> {
    conn.next_event(Duration::from_secs(secs))
        .await
        .map_err(|e| match e {
            buzz_ws_client::WsClientError::Timeout => {
                "Pairing timed out. Make a new code on Desktop and try again.".to_string()
            }
            e => format!("pairing connection lost: {e}"),
        })
}

async fn event(conn: &mut NostrWsConnection, secs: u64) -> Result<nostr::Event, String> {
    loop {
        if let RelayMessage::Event { event, .. } = next(conn, secs).await? {
            return Ok(*event);
        }
    }
}
