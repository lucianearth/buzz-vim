//! Pair against a real pairing relay with an in-process stand-in for
//! Desktop: it shows a QR, our `Pairing` scans it, both sides say yes, and
//! the stand-in sends `{relayUrl, pubkey, nsec}` as Desktop does. Prints
//! what the phone would log in with. `PAIR_RELAY` defaults to Buzz's.

use std::time::Duration;

use buzz_core::kind::KIND_PAIRING;
use buzz_core::pairing::qr::encode_qr;
use buzz_core::pairing::{PairingSession, PayloadType};
use buzz_vim_ffi::{Listener, PairStep, Pairing};
use buzz_ws_client::{NostrWsConnection, RelayMessage};
use nostr::{EventBuilder, Keys, RelayUrl, ToBech32};
use serde_json::json;

struct Quiet;
impl Listener for Quiet {
    fn changed(&self) {}
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let relay = std::env::var("PAIR_RELAY").unwrap_or("wss://pairing.buzz.gitcoin.co".into());
    let (mut source, qr) = PairingSession::new_source(relay.clone());
    let mut conn = NostrWsConnection::connect(&relay).await.expect("connect");
    if let Ok(RelayMessage::Auth { challenge }) = conn.next_event(Duration::from_secs(3)).await {
        let auth = source
            .sign_event(EventBuilder::auth(
                challenge,
                RelayUrl::parse(&relay).unwrap(),
            ))
            .unwrap();
        conn.send_raw(&json!(["AUTH", auth])).await.unwrap();
    }
    let me = source.pubkey().to_hex();
    conn.send_raw(&json!(["REQ", "pair", {"kinds": [KIND_PAIRING], "#p": [me]}]))
        .await
        .unwrap();
    loop {
        if let RelayMessage::Eose { .. } = conn.next_event(Duration::from_secs(10)).await.unwrap() {
            break;
        }
    }

    // The phone scans the QR.
    let phone = tokio::task::spawn_blocking({
        let uri = encode_qr(&qr);
        move || Pairing::new(uri, Box::new(Quiet)).expect("pairing")
    })
    .await
    .unwrap();

    let identity = Keys::generate();
    let nsec = identity.secret_key().to_bech32().unwrap();
    loop {
        let RelayMessage::Event { event, .. } = conn
            .next_event(Duration::from_secs(30))
            .await
            .expect("offer")
        else {
            continue;
        };
        if let Ok(sas) = source.handle_offer(&event) {
            let shown = match phone.step() {
                PairStep::Compare { code, .. } => code,
                _ => "nothing yet".into(),
            };
            println!("desktop shows {sas}, phone shows {shown}");
            break;
        }
    }
    let confirm = source.confirm_sas().unwrap();
    conn.send_raw(&json!(["EVENT", confirm])).await.unwrap();
    let payload = json!({"relayUrl": "https://buzz.gitcoin.co",
        "pubkey": identity.public_key().to_hex(), "nsec": nsec});
    let ev = source
        .send_payload(PayloadType::Custom, payload.to_string().into())
        .unwrap();
    conn.send_raw(&json!(["EVENT", ev])).await.unwrap();

    // The person taps yes on the phone once Desktop has confirmed.
    for _ in 0..100 {
        if matches!(
            phone.step(),
            PairStep::Compare {
                desktop_said_yes: true,
                ..
            }
        ) {
            phone.answer(true);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for _ in 0..100 {
        match phone.step() {
            PairStep::Done { relay, secret } => {
                println!("phone logs in to {relay}, key matches: {}", secret == nsec);
                break;
            }
            PairStep::Failed { message } => {
                println!("failed: {message}");
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let done = loop {
        match conn.next_event(Duration::from_secs(10)).await {
            Ok(RelayMessage::Event { event, .. }) => {
                if source.handle_complete(&event).is_ok() {
                    break true;
                }
            }
            Ok(_) => continue,
            Err(_) => break false,
        }
    };
    println!("desktop got complete: {done}");
}
