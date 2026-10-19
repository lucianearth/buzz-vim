//! Push for a closed app (see `buzz_vim_core::push`). Swift owns what only
//! iOS can do: the APNs token, App Attest keys and signatures, the Keychain.
//! Everything else is here: the relay's push key, the gateway calls and the
//! transcripts App Attest signs, the lease, and the notification
//! extension's lookup.

use buzz_vim_core::push::{self, Challenge, Delegation, GatewayError};
use buzz_vim_core::relay::Relay;
use nostr::Keys;

use crate::{BuzzError, Client, now, tls};

/// The relay's push capability (its NIP-11 `push`).
#[derive(uniffi::Record, Clone)]
pub struct PushRelay {
    pub origin: String,
    pub key_id: String,
    /// What the gateway delegates to and the lease is encrypted for.
    pub pubkey: String,
    pub max_ttl: u64,
    pub max_plaintext: u64,
}

impl From<push::Descriptor> for PushRelay {
    fn from(d: push::Descriptor) -> Self {
        PushRelay {
            origin: d.origin,
            key_id: d.key_id,
            pubkey: d.pubkey,
            max_ttl: d.max_ttl,
            max_plaintext: d.max_plaintext as u64,
        }
    }
}

impl From<&PushRelay> for push::Descriptor {
    fn from(r: &PushRelay) -> Self {
        push::Descriptor {
            origin: r.origin.clone(),
            key_id: r.key_id.clone(),
            pubkey: r.pubkey.clone(),
            max_ttl: r.max_ttl,
            max_plaintext: r.max_plaintext as usize,
        }
    }
}

/// A single-use gateway challenge.
#[derive(uniffi::Record, Clone)]
pub struct PushChallenge {
    pub id: String,
    pub value: String,
}

impl From<&PushChallenge> for Challenge {
    fn from(c: &PushChallenge) -> Self {
        Challenge {
            id: c.id.clone(),
            value: c.value.clone(),
        }
    }
}

/// The installation (gateway handle) bound to one relay key at a
/// generation higher than the last for that pair.
#[derive(uniffi::Record, Clone)]
pub struct PushDelegation {
    pub installation: String,
    pub generation: i64,
    pub relay_pubkey: String,
    pub not_before: i64,
    pub expires_at: i64,
}

impl From<&PushDelegation> for Delegation {
    fn from(d: &PushDelegation) -> Self {
        Delegation {
            installation: d.installation.clone(),
            generation: d.generation,
            relay_pubkey: d.relay_pubkey.clone(),
            not_before: d.not_before,
            expires_at: d.expires_at,
        }
    }
}

/// What a push notification says.
#[derive(uniffi::Record, Clone)]
pub struct PushNote {
    pub title: String,
    pub body: String,
    /// Opens the message (`follow`).
    pub link: Option<String>,
}

fn gateway_error(e: anyhow::Error) -> BuzzError {
    match e.downcast_ref::<GatewayError>() {
        Some(g) if g.gone() => BuzzError::Gone(g.to_string()),
        _ => e.into(),
    }
}

/// The relay's push key and limits. Fails when the relay offers no push.
#[uniffi::export(async_runtime = "tokio")]
pub async fn push_relay(relay_url: String) -> Result<PushRelay, BuzzError> {
    tls();
    Ok(push::descriptor(&relay_url).await?.into())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn push_challenge(gateway: String) -> Result<PushChallenge, BuzzError> {
    tls();
    let c = push::challenge(&gateway).await.map_err(gateway_error)?;
    Ok(PushChallenge {
        id: c.id,
        value: c.value,
    })
}

/// What `attestKey` signs to enroll `endpoint` (the APNs token, lowercase
/// hex) under the App Attest key `key_id`.
#[uniffi::export]
pub fn push_enroll_hash(
    challenge: PushChallenge,
    key_id: String,
    endpoint: String,
    expires_at: i64,
) -> Vec<u8> {
    push::client_data_hash(&push::enroll_transcript(
        &(&challenge).into(),
        &key_id,
        &endpoint,
        expires_at,
    ))
}

/// Enroll; returns the installation handle.
#[uniffi::export(async_runtime = "tokio")]
pub async fn push_enroll(
    gateway: String,
    challenge: PushChallenge,
    key_id: String,
    attestation: Vec<u8>,
    endpoint: String,
    expires_at: i64,
) -> Result<String, BuzzError> {
    tls();
    push::enroll(
        &gateway,
        &(&challenge).into(),
        &key_id,
        &attestation,
        &endpoint,
        expires_at,
    )
    .await
    .map_err(gateway_error)
}

/// What `generateAssertion` signs to delegate.
#[uniffi::export]
pub fn push_delegate_hash(challenge: PushChallenge, delegation: PushDelegation) -> Vec<u8> {
    push::client_data_hash(&push::delegate_transcript(
        &(&challenge).into(),
        &(&delegation).into(),
    ))
}

/// Delegate; returns the endpoint grant the lease carries. `Gone` when the
/// gateway no longer knows the installation.
#[uniffi::export(async_runtime = "tokio")]
pub async fn push_delegate(
    gateway: String,
    challenge: PushChallenge,
    delegation: PushDelegation,
    assertion: Vec<u8>,
) -> Result<String, BuzzError> {
    tls();
    push::delegate(
        &gateway,
        &(&challenge).into(),
        &(&delegation).into(),
        &assertion,
    )
    .await
    .map_err(gateway_error)
}

/// What `generateAssertion` signs to revoke the delegation.
#[uniffi::export]
pub fn push_revoke_hash(challenge: PushChallenge, delegation: PushDelegation) -> Vec<u8> {
    push::client_data_hash(&push::revoke_transcript(
        &(&challenge).into(),
        &delegation.installation,
        &delegation.relay_pubkey,
        delegation.generation,
    ))
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn push_revoke(
    gateway: String,
    challenge: PushChallenge,
    delegation: PushDelegation,
    assertion: Vec<u8>,
) -> Result<(), BuzzError> {
    tls();
    push::revoke(
        &gateway,
        &(&challenge).into(),
        &delegation.installation,
        &delegation.relay_pubkey,
        delegation.generation,
        &assertion,
    )
    .await
    .map_err(gateway_error)
}

/// The notification extension: what to say for a wake, from the newest
/// event the lease (`subscriptions`, as leased) would have woken us for.
/// Never fails: anything amiss gives the fallback note.
#[uniffi::export(async_runtime = "tokio")]
pub async fn push_note(relay_url: String, secret: String, subscriptions: String) -> PushNote {
    tls();
    let note = async {
        let keys = Keys::parse(secret.trim())?;
        let subs = push::parse_subscriptions(&subscriptions)?;
        let relay = Relay::new(&relay_url, keys, None)?;
        push::resolve(&relay, &subs, now()).await
    }
    .await;
    let n = match note {
        Ok(Some(n)) => n,
        _ => push::Note::fallback(),
    };
    PushNote {
        title: n.title,
        body: n.body,
        link: n.link,
    }
}

#[uniffi::export]
impl Client {
    /// The lease's subscriptions as JSON (the watcher's asks), the same
    /// text for the same inputs: lease again when it changes. None until
    /// the channels are in.
    pub fn push_subscriptions(&self) -> Result<Option<String>, BuzzError> {
        let e = self.engine.lock().unwrap();
        if !e.channels_loaded {
            return Ok(None);
        }
        let subs = push::subscriptions(&e.store.me, &e.checklists.trust.watchers)?;
        Ok(Some(serde_json::Value::Array(subs).to_string()))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Publish the lease: wake this installation for `subscriptions`
    /// (from `push_subscriptions`) until the grant expires.
    pub async fn push_lease(
        &self,
        relay: PushRelay,
        installation: String,
        generation: u64,
        grant: String,
        grant_expires: u64,
        subscriptions: String,
    ) -> Result<(), BuzzError> {
        let r = self.engine.lock().unwrap().relay.clone();
        let subs = push::parse_subscriptions(&subscriptions)?;
        let ev = push::lease(
            &r.keys,
            &(&relay).into(),
            &installation,
            generation,
            &grant,
            grant_expires,
            &subs,
            now(),
        )?;
        Ok(r.publish(&ev).await?)
    }

    /// Stop the pushes (logout): an inactive lease at a higher generation.
    pub async fn push_tombstone(
        &self,
        relay: PushRelay,
        installation: String,
        generation: u64,
    ) -> Result<(), BuzzError> {
        let r = self.engine.lock().unwrap().relay.clone();
        let ev = push::tombstone(&r.keys, &(&relay).into(), &installation, generation, now())?;
        Ok(r.publish(&ev).await?)
    }
}
