//! Dev tool: give the current identity Desktop-style sidebar sections, so
//! the section picker can be tested without Buzz Desktop.
//!
//! usage: cargo run --example seed_sections -- "Today=<uuid>,<uuid>" "Builds=<uuid>"
//! Uses BUZZ_PRIVATE_KEY / BUZZ_RELAY_URL / BUZZ_AUTH_TAG like buzz-vim.

use nostr::nips::nip44;
use nostr::{EventBuilder, Kind, Tag};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = std::env::var("BUZZ_PRIVATE_KEY")?;
    let keys = nostr::Keys::new(nostr::SecretKey::parse(key.trim())?);
    let relay = std::env::var("BUZZ_RELAY_URL").unwrap_or_else(|_| "wss://buzz.gitcoin.co".into());
    let auth = std::env::var("BUZZ_AUTH_TAG")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|j| buzz_sdk::nip_oa::parse_auth_tag(&j))
        .transpose()?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let dev = "b0220000000000aa";
    let (mut sections, mut assignments, mut s_meta, mut a_meta) =
        (vec![], json!({}), json!({}), json!({}));
    for (order, spec) in std::env::args().skip(1).enumerate() {
        let (name, chans) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("want Name=uuid,uuid"))?;
        let id = format!("sec-{order}-{}", name.to_lowercase().replace(' ', "-"));
        sections.push(json!({"id": id, "name": name, "order": order}));
        s_meta[&id] = json!({"name": [now_ms, dev, name], "order": [now_ms, dev, order], "live": [now_ms, dev, true]});
        for c in chans.split(',').filter(|c| !c.is_empty()) {
            assignments[c] = json!(id);
            a_meta[c] = json!([now_ms, dev, id]);
        }
    }
    let doc = json!({"version": 1, "sections": sections, "assignments": assignments, "meta": {"v": 1, "s": s_meta, "a": a_meta}});
    let ct = nip44::encrypt(
        keys.secret_key(),
        &keys.public_key(),
        doc.to_string(),
        nip44::Version::V2,
    )?;
    let mut b = EventBuilder::new(Kind::Custom(30078), ct).tags([
        Tag::parse(["d", "channel-sections"])?,
        Tag::parse(["t", "channel-sections"])?,
    ]);
    if let Some(t) = &auth {
        b = b.tags([t.clone()]);
    }
    let ev = b.sign_with_keys(&keys)?;
    let ok = buzz_ws_client::publish_event(&relay, ev, &keys, auth.as_ref(), 30).await?;
    println!("accepted={} {}", ok.accepted, ok.message);
    Ok(())
}
