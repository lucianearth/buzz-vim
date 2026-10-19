//! Push to a closed iPhone app, on upstream Buzz's native push
//! (`buzz-push-v1`, block/buzz 30873ba).
//!
//! The phone enrolls its APNs token with the push gateway (App Attest),
//! gets a delegation to the relay's push key back as an opaque
//! `endpoint_grant`, and publishes a kind 30350 lease: the grant and the
//! filters that should wake it, NIP-44 encrypted to the relay. When an event
//! matches, the relay asks the gateway to send APNs a content-free wake, and
//! the notification extension fetches the event itself (`resolve`).
//!
//! The filters say what tab 0 says: messages that p-tag us and every DM
//! message, minus our own and minus bots' (FYI never pushes), except the
//! watcher's (its asks do).

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine as _;
use nostr::nips::nip44;
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::model::{Message, Store, UNREAD_KINDS};
use crate::relay::{Relay, http_base};

pub const LEASE_KIND: u16 = 30350;
/// The relay's caps (its NIP-11 `push.limitation`, fixed at 30873ba).
const MAX_AUTHORS: usize = 20;
/// How far back the extension looks for the event that woke it.
const RESOLVE_WINDOW: u64 = 15 * 60;

/// The relay's push capability, from its NIP-11 document.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    /// `ws(s)://host`: the lease's `origin` must be exactly this.
    pub origin: String,
    /// The current executor key: its id goes in the lease's `exec` tag,
    /// its pubkey is what the gateway delegates to and the lease is
    /// encrypted for.
    pub key_id: String,
    pub pubkey: String,
    pub max_ttl: u64,
    pub max_plaintext: usize,
}

impl Descriptor {
    pub fn from_nip11(doc: &Value) -> Result<Self> {
        let advertised = doc
            .get("supported_extensions")
            .and_then(Value::as_array)
            .is_some_and(|x| x.iter().any(|e| e == "buzz-push-v1"));
        ensure!(advertised, "the relay doesn't offer push (buzz-push-v1)");
        let push = doc.get("push").context("NIP-11 has no push descriptor")?;
        let current: Vec<&Value> = push
            .get("keys")
            .and_then(Value::as_array)
            .map(|keys| keys.iter().filter(|k| k["current"] == true).collect())
            .unwrap_or_default();
        let [key] = current[..] else {
            bail!("push descriptor needs exactly one current key");
        };
        let pubkey = key["pubkey"].as_str().unwrap_or_default().to_string();
        ensure!(is_hex64(&pubkey), "bad push key pubkey");
        let lim = &push["limitation"];
        Ok(Descriptor {
            origin: push["origin"]
                .as_str()
                .filter(|o| o.starts_with("wss://") || o.starts_with("ws://"))
                .context("bad push origin")?
                .to_string(),
            key_id: key["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .context("push key has no id")?
                .to_string(),
            pubkey,
            max_ttl: lim["max_lease_ttl"].as_u64().context("no max_lease_ttl")?,
            max_plaintext: lim["max_plaintext_len"]
                .as_u64()
                .context("no max_plaintext_len")? as usize,
        })
    }
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// Fetch the relay's push descriptor (NIP-11 at its root).
pub async fn descriptor(relay_url: &str) -> Result<Descriptor> {
    let url = format!("{}/", http_base(relay_url));
    let r = http()?
        .get(&url)
        .header("Accept", "application/nostr+json")
        .send()
        .await?;
    ensure!(r.status().is_success(), "NIP-11 failed ({})", r.status());
    Descriptor::from_nip11(&r.json().await?)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The relay takes only lowercase v4 UUIDs in `#h`.
#[cfg(test)]
fn is_channel_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.get_version_num() == 4 && u.to_string() == s)
}

/// The lease's subscriptions: only the watcher wakes the phone (lucian
/// 2026-10-19, "just watcher"). Its posts that p-tag `me` are the asks it
/// decided need him; people's mentions and DMs wait in tab 0 unbuzzed.
pub fn subscriptions(me: &str, watchers: &[String]) -> Result<Vec<Value>> {
    ensure!(is_hex64(me), "bad pubkey");
    let watchers: BTreeSet<&String> = watchers
        .iter()
        .filter(|pk| is_hex64(pk) && *pk != me)
        .collect();
    ensure!(!watchers.is_empty(), "no watcher to push for");
    ensure!(
        watchers.len() <= MAX_AUTHORS,
        "too many watchers for one push lease ({}, at most {MAX_AUTHORS})",
        watchers.len()
    );
    Ok(vec![json!({
        "filter": {"kinds": UNREAD_KINDS, "authors": watchers, "#p": [me]},
        "class": "default",
        "ignore": [],
    })])
}

/// The next lease generation: the relay keeps only a strictly higher one.
pub fn next_generation(last: u64, now: u64) -> u64 {
    now.max(last + 1)
}

/// An active kind 30350 lease. It lives until the grant does, or the
/// relay's longest lease, whichever comes first. Signed without any NIP-OA
/// tag: the relay refuses unknown public tags.
#[allow(clippy::too_many_arguments)]
pub fn lease(
    keys: &Keys,
    relay: &Descriptor,
    installation: &str,
    generation: u64,
    grant: &str,
    grant_expires: u64,
    subs: &[Value],
    now: u64,
) -> Result<Event> {
    let expiration = grant_expires.min(now + relay.max_ttl);
    ensure!(expiration > now, "the push grant has expired");
    let body = json!({
        "v": 1,
        "origin": relay.origin,
        "transport": "apns",
        "endpoint": grant,
        "generation": generation,
        "active": true,
        "subscriptions": subs,
    });
    envelope(keys, relay, installation, expiration, &body, now)
}

/// The inactive lease that stops pushes for this installation (logout).
pub fn tombstone(
    keys: &Keys,
    relay: &Descriptor,
    installation: &str,
    generation: u64,
    now: u64,
) -> Result<Event> {
    let body = json!({
        "v": 1,
        "origin": relay.origin,
        "generation": generation,
        "active": false,
    });
    envelope(keys, relay, installation, now + relay.max_ttl, &body, now)
}

fn envelope(
    keys: &Keys,
    relay: &Descriptor,
    installation: &str,
    expiration: u64,
    body: &Value,
    now: u64,
) -> Result<Event> {
    let plain = body.to_string();
    ensure!(
        plain.len() <= relay.max_plaintext,
        "push lease too large ({} bytes, at most {})",
        plain.len(),
        relay.max_plaintext
    );
    let to = PublicKey::from_hex(&relay.pubkey)?;
    let content = nip44::encrypt(keys.secret_key(), &to, plain, nip44::Version::V2)?;
    Ok(EventBuilder::new(Kind::Custom(LEASE_KIND), content)
        .tags([
            Tag::parse(["d", installation])?,
            Tag::parse(["expiration", &expiration.to_string()])?,
            Tag::parse(["exec", &relay.key_id])?,
        ])
        .custom_created_at(Timestamp::from(now))
        .sign_with_keys(keys)?)
}

// ---------- the gateway ----------

/// App Attest transcripts: `domain \n compact JSON`, members in the
/// gateway's order (serde_json of its structs at 30873ba). The attestation
/// and assertions sign SHA-256 of these bytes. `audience` is NIP-PL's fixed
/// origin whatever gateway we talk to.
const AUDIENCE: &str = "https://push.buzz.xyz";
/// Every installation here has one endpoint epoch: a new APNs token
/// enrolls again rather than rotating.
pub const ENDPOINT_EPOCH: i64 = 1;

/// A gateway challenge, single use.
#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    pub id: String,
    pub value: String,
}

/// What a delegation binds: the installation to one relay key, at a
/// generation higher than the last for that pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Delegation {
    pub installation: String,
    pub generation: i64,
    pub relay_pubkey: String,
    pub not_before: i64,
    pub expires_at: i64,
}

fn transcript(domain: &str, members: &[(&str, Value)]) -> Vec<u8> {
    // Built by hand: a serde_json Map would sort the members.
    let inner: Vec<String> = members
        .iter()
        .map(|(k, v)| format!("{}:{v}", Value::from(*k)))
        .collect();
    format!("{domain}\n{{{}}}", inner.join(",")).into_bytes()
}

pub fn enroll_transcript(c: &Challenge, key_id: &str, endpoint: &str, expires_at: i64) -> Vec<u8> {
    transcript(
        "buzz.push.enroll.v1",
        &[
            ("v", json!(1)),
            ("audience", json!(format!("{AUDIENCE}/v1/installations"))),
            ("challenge_id", json!(c.id)),
            ("challenge", json!(c.value)),
            ("key_id", json!(key_id)),
            ("endpoint", json!(endpoint)),
            ("endpoint_epoch", json!(ENDPOINT_EPOCH)),
            ("expires_at", json!(expires_at)),
        ],
    )
}

pub fn delegate_transcript(c: &Challenge, d: &Delegation) -> Vec<u8> {
    transcript(
        "buzz.push.delegate.v1",
        &[
            ("v", json!(1)),
            ("audience", json!(format!("{AUDIENCE}/v1/delegations"))),
            ("challenge_id", json!(c.id)),
            ("challenge", json!(c.value)),
            ("installation_handle", json!(d.installation)),
            ("endpoint_epoch", json!(ENDPOINT_EPOCH)),
            ("generation", json!(d.generation)),
            ("relay_pubkey", json!(d.relay_pubkey)),
            ("not_before", json!(d.not_before)),
            ("expires_at", json!(d.expires_at)),
        ],
    )
}

pub fn revoke_transcript(
    c: &Challenge,
    installation: &str,
    relay_pubkey: &str,
    generation: i64,
) -> Vec<u8> {
    transcript(
        "buzz.push.revoke-delegation.v1",
        &[
            ("v", json!(1)),
            (
                "audience",
                json!(format!("{AUDIENCE}/v1/delegations/revoke")),
            ),
            ("challenge_id", json!(c.id)),
            ("challenge", json!(c.value)),
            ("installation_handle", json!(installation)),
            ("relay_pubkey", json!(relay_pubkey)),
            ("generation", json!(generation)),
        ],
    )
}

/// What App Attest signs for a transcript.
pub fn client_data_hash(transcript: &[u8]) -> Vec<u8> {
    Sha256::digest(transcript).to_vec()
}

/// A gateway refusal, with its status and error code.
#[derive(Debug)]
pub struct GatewayError {
    pub status: u16,
    pub code: String,
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "push gateway: {} {}", self.status, self.code)
    }
}

impl std::error::Error for GatewayError {}

impl GatewayError {
    /// The gateway no longer knows the installation (expired or revoked):
    /// enroll again.
    pub fn gone(&self) -> bool {
        self.status == 404
    }
}

async fn post(gateway: &str, route: &str, body: Value) -> Result<Value> {
    let url = format!("{}/{route}", gateway.trim_end_matches('/'));
    let r = http()?.post(&url).json(&body).send().await?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or_default();
    if !status.is_success() {
        let code = v["error"].as_str().unwrap_or("").to_string();
        return Err(GatewayError {
            status: status.as_u16(),
            code,
        }
        .into());
    }
    Ok(v)
}

pub async fn challenge(gateway: &str) -> Result<Challenge> {
    let v = post(gateway, "v1/installations/challenges", json!({"v": 1})).await?;
    let id = v["challenge_id"].as_str().context("no challenge_id")?;
    let value = v["challenge"].as_str().context("no challenge")?;
    Ok(Challenge {
        id: id.to_string(),
        value: value.to_string(),
    })
}

/// Enroll the APNs token (lowercase hex) under the attested key. Returns
/// the installation handle.
pub async fn enroll(
    gateway: &str,
    c: &Challenge,
    key_id: &str,
    attestation: &[u8],
    endpoint: &str,
    expires_at: i64,
) -> Result<String> {
    let v = post(
        gateway,
        "v1/installations",
        json!({
            "v": 1,
            "challenge_id": c.id,
            "challenge": c.value,
            "key_id": key_id,
            "attestation": base64::engine::general_purpose::STANDARD.encode(attestation),
            "endpoint": endpoint,
            "endpoint_epoch": ENDPOINT_EPOCH,
            "expires_at": expires_at,
        }),
    )
    .await?;
    Ok(v["installation_handle"]
        .as_str()
        .context("no installation_handle")?
        .to_string())
}

/// Delegate the installation to the relay key. Returns the endpoint grant.
pub async fn delegate(
    gateway: &str,
    c: &Challenge,
    d: &Delegation,
    assertion: &[u8],
) -> Result<String> {
    let v = post(
        gateway,
        "v1/delegations",
        json!({
            "v": 1,
            "challenge_id": c.id,
            "challenge": c.value,
            "installation_handle": d.installation,
            "endpoint_epoch": ENDPOINT_EPOCH,
            "generation": d.generation,
            "relay_pubkey": d.relay_pubkey,
            "not_before": d.not_before,
            "expires_at": d.expires_at,
            "assertion": base64::engine::general_purpose::STANDARD.encode(assertion),
        }),
    )
    .await?;
    Ok(v["endpoint_grant"]
        .as_str()
        .filter(|g| !g.is_empty())
        .context("no endpoint_grant")?
        .to_string())
}

pub async fn revoke(
    gateway: &str,
    c: &Challenge,
    installation: &str,
    relay_pubkey: &str,
    generation: i64,
    assertion: &[u8],
) -> Result<()> {
    post(
        gateway,
        "v1/delegations/revoke",
        json!({
            "v": 1,
            "challenge_id": c.id,
            "challenge": c.value,
            "installation_handle": installation,
            "relay_pubkey": relay_pubkey,
            "generation": generation,
            "assertion": base64::engine::general_purpose::STANDARD.encode(assertion),
        }),
    )
    .await?;
    Ok(())
}

// ---------- the notification extension ----------

/// What a push notification says: a tab 0 row's channel, who and line.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub title: String,
    pub body: String,
    /// Opens the message (`follow`), when there is one.
    pub link: Option<String>,
}

impl Note {
    /// When the event can't be found (or the relay can't be reached).
    pub fn fallback() -> Self {
        Note {
            title: "Buzz Vim".into(),
            body: "Something new needs you".into(),
            link: None,
        }
    }
}

/// The queries for recent events the lease could have woken us for. An
/// `#h` list (leases from before watcher-only push) splits into one filter
/// per channel, since a multi-`#h` filter narrows to one on `POST /query`.
pub fn resolve_filters(subs: &[Value], now: u64) -> Vec<Value> {
    let since = now.saturating_sub(RESOLVE_WINDOW);
    let mut out = vec![];
    for sub in subs {
        let Some(filter) = sub["filter"].as_object() else {
            continue;
        };
        let mut base = filter.clone();
        base.insert("since".into(), json!(since));
        base.insert("limit".into(), json!(5));
        match base.remove("#h") {
            Some(Value::Array(hs)) => {
                for h in hs {
                    let mut f = base.clone();
                    f.insert("#h".into(), json!([h]));
                    out.push(Value::Object(f));
                }
            }
            _ => out.push(Value::Object(base)),
        }
    }
    out
}

/// Does `ev` match a lease filter (the members we use: kinds, authors,
/// #p, #h)?
fn matches(filter: &Value, ev: &Event) -> bool {
    let k = u64::from(ev.kind.as_u16());
    let authors_ok = filter
        .get("authors")
        .and_then(Value::as_array)
        .is_none_or(|vs| vs.iter().any(|v| v.as_str() == Some(&ev.pubkey.to_hex())));
    let tag_ok = |key: &str, name: &str| {
        filter.get(key).and_then(Value::as_array).is_none_or(|vs| {
            crate::model::tags(ev, name).any(|t| {
                t.get(1)
                    .is_some_and(|x| vs.iter().any(|v| v.as_str() == Some(x)))
            })
        })
    };
    filter
        .get("kinds")
        .and_then(Value::as_array)
        .is_none_or(|ks| ks.iter().any(|v| v.as_u64() == Some(k)))
        && authors_ok
        && tag_ok("#p", "p")
        && tag_ok("#h", "h")
}

/// The newest event a subscription takes and none of its ignores drops.
pub fn newest<'a>(events: &'a [Event], subs: &[Value]) -> Option<&'a Event> {
    events
        .iter()
        .filter(|ev| {
            subs.iter().any(|s| {
                matches(&s["filter"], ev)
                    && !s["ignore"]
                        .as_array()
                        .is_some_and(|ig| ig.iter().any(|f| matches(f, ev)))
            })
        })
        .max_by_key(|ev| (ev.created_at, ev.id))
}

/// The note for `ev`, named from what `store` knows (its author's profile,
/// its channel's metadata), as tab 0 and the local notifications put it.
pub fn note(store: &Store, ev: &Event) -> Note {
    let m = Message::from_event(ev);
    let who = store.name_of(&m.author);
    let channel = crate::model::tag(ev, "h").unwrap_or_default();
    let label = store.label(channel);
    let text = crate::needs::one_line(&m.content);
    Note {
        title: format!("{label} · {who}"),
        body: if text.is_empty() {
            "sent an attachment".into()
        } else {
            text
        },
        link: Some(crate::checklist::message_link(
            channel,
            &m.id,
            m.root.as_deref(),
        )),
    }
}

/// The extension's whole job: find the newest event the lease would have
/// woken us for, and name it. `subs` is the lease's subscriptions as
/// leased.
pub async fn resolve(relay: &Relay, subs: &[Value], now: u64) -> Result<Option<Note>> {
    let events = relay.query(&resolve_filters(subs, now)).await?;
    let Some(ev) = newest(&events, subs) else {
        return Ok(None);
    };
    let mut store = Store::new(relay.keys.public_key().to_hex());
    let mut want = vec![json!({"kinds": [0], "authors": [ev.pubkey.to_hex()], "limit": 1})];
    if let Some(ch) = crate::model::tag(ev, "h") {
        want.push(json!({"kinds": [39000, 39002], "#d": [ch], "limit": 2}));
    }
    // Names are a nicety: without them the note still says what was said.
    for meta in relay.query(&want).await.unwrap_or_default() {
        store.apply(&meta);
    }
    Ok(Some(note(&store, ev)))
}

/// A lease's subscriptions back from the JSON we keep for the extension.
pub fn parse_subscriptions(json: &str) -> Result<Vec<Value>> {
    serde_json::from_str(json).map_err(|e| anyhow!("bad push subscriptions: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Profile;

    const DM1: &str = "bcd11c3d-f78b-4934-8e48-c08b6baed1c5";
    const CH: &str = "6b8bdafd-1111-4222-8333-444455556666";
    const NOW: u64 = 1_800_000_000;

    fn pk(k: &Keys) -> String {
        k.public_key().to_hex()
    }

    fn hexes(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{i:064x}")).collect()
    }

    /// The relay's NIP-11 push descriptor as 30873ba serves it.
    fn nip11(relay: &Keys) -> Value {
        json!({
            "supported_extensions": ["nip-ee", "buzz-push-v1"],
            "push": {
                "origin": "wss://buzz.gitcoin.co",
                "keys": [{"id": "k1", "pubkey": pk(relay), "current": true}],
                "push_kinds": [9, 40002, 45001, 45003],
                "h_grammar": "uuid-v4-lowercase",
                "class_support": {"apns": ["default"]},
                "limitation": {
                    "max_lease_ttl": 2592000, "max_leases_per_pubkey": 16,
                    "max_subscriptions_per_lease": 16, "max_kinds": 16, "max_authors": 20,
                    "max_h": 50, "max_tag_values": 20, "max_ignore": 8,
                    "max_content_len": 65536, "max_plaintext_len": 32768,
                    "max_endpoint_len": 4096, "max_string_len": 512
                }
            }
        })
    }

    /// The relay's filter rules (push_lease.rs validate_subscription at
    /// 30873ba), for leases we build.
    fn relay_accepts(subs: &[Value], me: &str) -> Result<(), String> {
        fn filter(f: &Value, me: &str, narrowed: bool) -> Result<(), String> {
            let f = f.as_object().ok_or("filter not an object")?;
            for k in f.keys() {
                if !["kinds", "authors", "#p", "#h", "#e"].contains(&k.as_str()) {
                    return Err(format!("member {k}"));
                }
            }
            let list = |k: &str, max: usize| -> Result<Option<Vec<Value>>, String> {
                match f.get(k) {
                    None => Ok(None),
                    Some(Value::Array(v)) if !v.is_empty() && v.len() <= max => Ok(Some(v.clone())),
                    _ => Err(format!("bad {k} count")),
                }
            };
            let kinds = list("kinds", 16)?.ok_or("no kinds")?;
            if kinds
                .iter()
                .any(|k| ![9, 40002, 45001, 45003].contains(&k.as_u64().unwrap_or(0)))
            {
                return Err("kind not push-eligible".into());
            }
            let authors = list("authors", 20)?;
            let p = list("#p", 20)?;
            let h = list("#h", 50)?;
            if narrowed && authors.is_none() && p.is_none() && h.is_none() {
                return Err("not narrowed".into());
            }
            for a in authors.iter().flatten() {
                if !is_hex64(a.as_str().unwrap_or("")) {
                    return Err("author".into());
                }
            }
            for v in p.iter().flatten() {
                if v != me {
                    return Err("p-tag must be self".into());
                }
            }
            for v in h.iter().flatten() {
                if !is_channel_id(v.as_str().unwrap_or("")) {
                    return Err("invalid h tag".into());
                }
            }
            Ok(())
        }
        if subs.is_empty() || subs.len() > 16 {
            return Err("subscription quota".into());
        }
        for s in subs {
            if s["class"] != "default" {
                return Err("class".into());
            }
            filter(&s["filter"], me, true)?;
            let ignore = s["ignore"].as_array().cloned().unwrap_or_default();
            if ignore.len() > 8 {
                return Err("ignore quota".into());
            }
            for f in &ignore {
                filter(f, me, false)?;
            }
        }
        Ok(())
    }

    #[test]
    fn transcripts_match_the_gateway_vectors() {
        // crates/buzz-push-gateway/tests/vectors/app_attest_transcripts.json @ 30873ba
        let c = Challenge {
            id: "11111111-1111-4111-8111-111111111111".into(),
            value: "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8".into(),
        };
        let endpoint = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let relay = "a".repeat(64);
        let handle = "22222222-2222-4222-8222-222222222222";
        let check = |got: Vec<u8>, want: &str, sha: &str| {
            assert_eq!(String::from_utf8(got.clone()).unwrap(), want);
            assert_eq!(hex::encode(client_data_hash(&got)), sha);
        };
        check(
            enroll_transcript(
                &c,
                "qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqo=",
                endpoint,
                1_752_624_000,
            ),
            "buzz.push.enroll.v1\n{\"v\":1,\"audience\":\"https://push.buzz.xyz/v1/installations\",\"challenge_id\":\"11111111-1111-4111-8111-111111111111\",\"challenge\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\",\"key_id\":\"qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqo=\",\"endpoint\":\"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20\",\"endpoint_epoch\":1,\"expires_at\":1752624000}",
            "792966649266bfcebdb79de54e8ed3880746f58efd77427a0f43e88c650d79d0",
        );
        let d = Delegation {
            installation: handle.into(),
            generation: 1,
            relay_pubkey: relay.clone(),
            not_before: 1_752_620_000,
            expires_at: 1_752_624_000,
        };
        check(
            delegate_transcript(&c, &d),
            "buzz.push.delegate.v1\n{\"v\":1,\"audience\":\"https://push.buzz.xyz/v1/delegations\",\"challenge_id\":\"11111111-1111-4111-8111-111111111111\",\"challenge\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\",\"installation_handle\":\"22222222-2222-4222-8222-222222222222\",\"endpoint_epoch\":1,\"generation\":1,\"relay_pubkey\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"not_before\":1752620000,\"expires_at\":1752624000}",
            "7466177cc2dc2a4f9a075fdbb461531692fc858778a171a5862b855cccfaa059",
        );
        check(
            revoke_transcript(&c, handle, &relay, 2),
            "buzz.push.revoke-delegation.v1\n{\"v\":1,\"audience\":\"https://push.buzz.xyz/v1/delegations/revoke\",\"challenge_id\":\"11111111-1111-4111-8111-111111111111\",\"challenge\":\"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\",\"installation_handle\":\"22222222-2222-4222-8222-222222222222\",\"relay_pubkey\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"generation\":2}",
            "d6bcd4b25235adcb519ef189820b08dd0386fc752fd4e4c77bc3ffb7a519a84a",
        );
    }

    #[test]
    fn descriptor_reads_the_relay_nip11() {
        let relay = Keys::generate();
        let d = Descriptor::from_nip11(&nip11(&relay)).unwrap();
        assert_eq!(
            d,
            Descriptor {
                origin: "wss://buzz.gitcoin.co".into(),
                key_id: "k1".into(),
                pubkey: pk(&relay),
                max_ttl: 2_592_000,
                max_plaintext: 32_768,
            }
        );
        let mut off = nip11(&relay);
        off["supported_extensions"] = json!(["nip-pl"]);
        assert!(Descriptor::from_nip11(&off).is_err());
        let mut two = nip11(&relay);
        two["push"]["keys"] = json!([
            {"id": "a", "pubkey": pk(&relay), "current": true},
            {"id": "b", "pubkey": pk(&relay), "current": true},
        ]);
        assert!(Descriptor::from_nip11(&two).is_err());
    }

    #[test]
    fn subscriptions_are_the_watchers_asks_only() {
        let me = "f".repeat(64);
        let watcher = "a".repeat(64);
        let subs = subscriptions(&me, &[watcher.clone(), me.clone(), "nope".into()]).unwrap();
        assert_eq!(
            Value::Array(subs.clone()),
            json!([{"filter": {"kinds": [9, 40002], "authors": [watcher], "#p": [me]},
                "class": "default", "ignore": []}])
        );
        relay_accepts(&subs, &me).unwrap();
        // No watcher, no lease; at the relay's author cap, then one past it.
        assert!(subscriptions(&me, &[]).is_err());
        assert!(subscriptions(&me, std::slice::from_ref(&me)).is_err());
        relay_accepts(&subscriptions(&me, &hexes(20)).unwrap(), &me).unwrap();
        assert!(subscriptions(&me, &hexes(21)).is_err());
    }

    #[test]
    fn lease_is_encrypted_to_the_relay_with_only_its_public_tags() {
        let (me, relay) = (Keys::generate(), Keys::generate());
        let d = Descriptor::from_nip11(&nip11(&relay)).unwrap();
        let subs = subscriptions(&pk(&me), &hexes(3)).unwrap();
        let inst = "0123456789abcdef0123456789abcdef";
        let ev = lease(&me, &d, inst, 7, "grant", NOW + 86_400, &subs, NOW).unwrap();
        ev.verify().unwrap();
        assert_eq!(ev.kind.as_u16(), 30350);
        assert_eq!(ev.created_at.as_secs(), NOW);
        let tags: Vec<Vec<String>> = ev.tags.iter().map(|t| t.as_slice().to_vec()).collect();
        let want = [
            ["d", inst],
            ["expiration", &(NOW + 86_400).to_string()],
            ["exec", "k1"],
        ];
        assert_eq!(tags, want.map(|t| t.map(String::from).to_vec()));
        let plain = nip44::decrypt(relay.secret_key(), &me.public_key(), &ev.content).unwrap();
        let body: Value = serde_json::from_str(&plain).unwrap();
        assert_eq!(
            body,
            json!({"v": 1, "origin": "wss://buzz.gitcoin.co", "transport": "apns",
                "endpoint": "grant", "generation": 7, "active": true, "subscriptions": subs})
        );
        // A grant longer than the relay keeps leases is cut to its max.
        let ev = lease(&me, &d, inst, 8, "grant", NOW + 90 * 86_400, &subs, NOW).unwrap();
        assert_eq!(crate::model::tag(&ev, "expiration"), Some("1802592000"));
        assert!(lease(&me, &d, inst, 9, "grant", NOW, &subs, NOW).is_err());
    }

    #[test]
    fn tombstone_is_the_minimal_inactive_lease() {
        let (me, relay) = (Keys::generate(), Keys::generate());
        let d = Descriptor::from_nip11(&nip11(&relay)).unwrap();
        let ev = tombstone(&me, &d, "0123456789abcdef0123456789abcdef", 9, NOW).unwrap();
        let plain = nip44::decrypt(relay.secret_key(), &me.public_key(), &ev.content).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&plain).unwrap(),
            json!({"v": 1, "origin": "wss://buzz.gitcoin.co", "generation": 9, "active": false})
        );
    }

    #[test]
    fn oversized_lease_is_refused_before_publishing() {
        let (me, relay) = (Keys::generate(), Keys::generate());
        let mut d = Descriptor::from_nip11(&nip11(&relay)).unwrap();
        d.max_plaintext = 200;
        let subs = subscriptions(&pk(&me), &hexes(20)).unwrap();
        let err = lease(&me, &d, "00", 1, "grant", NOW + 60, &subs, NOW).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn generations_only_go_up() {
        assert_eq!(next_generation(0, NOW), NOW);
        assert_eq!(next_generation(NOW, NOW), NOW + 1);
        assert_eq!(next_generation(NOW + 5, NOW), NOW + 6);
    }

    fn message(who: &Keys, ch: &str, text: &str, p: Option<&str>, at: u64) -> Event {
        let mut tags = vec![Tag::parse(["h", ch]).unwrap()];
        if let Some(p) = p {
            tags.push(Tag::parse(["p", p]).unwrap());
        }
        EventBuilder::new(Kind::Custom(9), text)
            .tags(tags)
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(who)
            .unwrap()
    }

    #[test]
    fn resolve_asks_for_the_watchers_mentions() {
        let me = "f".repeat(64);
        let watcher = "a".repeat(64);
        let subs = subscriptions(&me, std::slice::from_ref(&watcher)).unwrap();
        let since = NOW - RESOLVE_WINDOW;
        assert_eq!(
            Value::Array(resolve_filters(&subs, NOW)),
            json!([{"kinds": [9, 40002], "authors": [watcher], "#p": [me], "since": since, "limit": 5}])
        );
    }

    #[test]
    fn newest_is_the_watchers_ask_and_nothing_else() {
        let (me, human, bot, watcher) = (
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
        );
        let subs = subscriptions(&pk(&me), &[pk(&watcher)]).unwrap();
        let ask = message(&human, CH, "can you look?", Some(&pk(&me)), NOW - 30);
        let fyi = message(&bot, CH, "done", Some(&pk(&me)), NOW - 10);
        let dm = message(&human, DM1, "ping", None, NOW - 20);
        assert_eq!(newest(&[ask.clone(), fyi.clone(), dm.clone()], &subs), None);
        let watch = message(&watcher, CH, "waiting on you", Some(&pk(&me)), NOW - 40);
        let chatter = message(&watcher, CH, "nudge for the agent", None, NOW - 1);
        assert_eq!(
            newest(&[ask, watch.clone(), fyi, dm, chatter], &subs),
            Some(&watch)
        );
    }

    #[test]
    fn note_reads_like_a_tab_0_row() {
        let (me, alice) = (Keys::generate(), Keys::generate());
        let mut store = Store::new(pk(&me));
        store.profiles.insert(
            pk(&alice),
            Profile {
                name: "alice".into(),
                ..Default::default()
            },
        );
        store.channel_mut(CH).name = "general".into();
        let dm = store.channel_mut(DM1);
        dm.is_dm = true;
        dm.members = vec![pk(&me), pk(&alice)];
        let ev = message(&alice, CH, "\n  can you look?\nmore", Some(&pk(&me)), NOW);
        assert_eq!(
            note(&store, &ev),
            Note {
                title: "#general · alice".into(),
                body: "can you look?".into(),
                link: Some(format!("buzz://message?channel={CH}&id={}", ev.id.to_hex())),
            }
        );
        let ev = message(&alice, DM1, "", None, NOW);
        let n = note(&store, &ev);
        assert_eq!(
            (n.title.as_str(), n.body.as_str()),
            ("@alice · alice", "sent an attachment")
        );
    }
}
