//! HTTP side of the relay: NIP-98 `/query` and `/events`, Blossom upload and
//! authenticated media download. Signing helpers mirror `buzz-cli`'s client.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag, Timestamp};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// MIME types the relay's Blossom endpoint accepts (`buzz-cli` ALLOWED_MIMES).
pub const ALLOWED_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
];
pub const MAX_IMAGE_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_VIDEO_BYTES: usize = 500 * 1024 * 1024;

#[derive(Clone)]
pub struct Relay {
    http: reqwest::Client,
    /// `https://host` form.
    pub base: String,
    /// `wss://host` form.
    pub ws_url: String,
    pub keys: Keys,
    pub auth_tag: Option<Tag>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlobDescriptor {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(rename = "type", default)]
    pub mime_type: String,
    #[serde(default)]
    pub dim: Option<String>,
    #[serde(default)]
    pub blurhash: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
}

impl BlobDescriptor {
    /// NIP-92 `imeta` tag, same shape `buzz-cli` builds.
    pub fn imeta_tag(&self) -> Vec<String> {
        let mut tag = vec![
            "imeta".to_string(),
            format!("url {}", self.url),
            format!("m {}", self.mime_type),
            format!("x {}", self.sha256),
            format!("size {}", self.size),
        ];
        if let Some(dim) = &self.dim {
            tag.push(format!("dim {dim}"));
        }
        if let Some(bh) = &self.blurhash {
            tag.push(format!("blurhash {bh}"));
        }
        if let Some(th) = &self.thumb {
            tag.push(format!("thumb {th}"));
        }
        tag
    }
}

pub fn http_base(url: &str) -> String {
    url.replace("wss://", "https://")
        .replace("ws://", "http://")
        .trim_end_matches('/')
        .to_string()
}

pub fn ws_base(url: &str) -> String {
    http_base(url)
        .replace("https://", "wss://")
        .replace("http://", "ws://")
}

fn authority(url: &str) -> Result<String> {
    let a = buzz_core::tenant::relay_url_authority(url);
    if a.is_empty() {
        bail!("invalid relay url: {url}");
    }
    Ok(a)
}

impl Relay {
    pub fn new(url: &str, keys: Keys, auth_tag: Option<Tag>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            http,
            base: http_base(url),
            ws_url: ws_base(url),
            keys,
            auth_tag,
        })
    }

    /// Sign an event we publish, carrying the NIP-OA tag when we have one.
    pub fn sign(&self, builder: EventBuilder) -> Result<Event> {
        let builder = match &self.auth_tag {
            Some(t) => builder.tags([t.clone()]),
            None => builder,
        };
        Ok(builder.sign_with_keys(&self.keys)?)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_tag {
            Some(t) => req.header(
                "x-auth-tag",
                serde_json::to_string(t.as_slice()).unwrap_or_default(),
            ),
            None => req,
        }
    }

    fn nip98(&self, method: &str, url: &str, body: Option<&[u8]>) -> Result<String> {
        let mut tags = vec![
            Tag::parse(["u", url])?,
            Tag::parse(["method", method])?,
            Tag::parse(["nonce", &format!("{:x}", rand_nonce())])?,
        ];
        if let Some(b) = body {
            tags.push(Tag::parse(["payload", &hex::encode(Sha256::digest(b))])?);
        }
        let ev = EventBuilder::new(Kind::Custom(27235), "")
            .tags(tags)
            .sign_with_keys(&self.keys)?;
        Ok(format!("Nostr {}", STANDARD.encode(ev.as_json())))
    }

    /// One-shot historical query (`POST /query`). Filters are ORed.
    pub async fn query(&self, filters: &[Value]) -> Result<Vec<Event>> {
        let url = format!("{}/query", self.base);
        let body = serde_json::to_vec(filters)?;
        let mut last = None;
        for attempt in 0..3u64 {
            let auth = self.nip98("POST", &url, Some(&body))?;
            let res = self
                .authed(self.http.post(&url))
                .header("Authorization", auth)
                .header("Content-Type", "application/json")
                .body(body.clone())
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let raw: Vec<Value> = r.json().await?;
                    return Ok(raw
                        .into_iter()
                        .filter_map(|v| serde_json::from_value::<Event>(v).ok())
                        .collect());
                }
                Ok(r) => {
                    let s = r.status();
                    let t = r.text().await.unwrap_or_default();
                    if !(s.as_u16() == 429 || s.is_server_error()) {
                        bail!("query failed ({s}): {t}");
                    }
                    last = Some(anyhow!("query failed ({s}): {t}"));
                }
                Err(e) => last = Some(e.into()),
            }
            tokio::time::sleep(Duration::from_millis(400 * (attempt + 1))).await;
        }
        Err(last.unwrap_or_else(|| anyhow!("query failed")))
    }

    /// Submit a signed event (`POST /events`). Errors when the relay rejects it.
    pub async fn publish(&self, event: &Event) -> Result<()> {
        let url = format!("{}/events", self.base);
        let body = event.as_json().into_bytes();
        let auth = self.nip98("POST", &url, Some(&body))?;
        let r = self
            .authed(self.http.post(&url))
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;
        let s = r.status();
        let t = r.text().await.unwrap_or_default();
        // A 5xx may have stored it anyway, so it isn't a rejection: sending
        // the same event again is the safe retry.
        if s.is_server_error() {
            bail!("relay error ({s}): {t}");
        }
        if !s.is_success() {
            bail!("relay rejected event ({s}): {t}");
        }
        if let Ok(v) = serde_json::from_str::<Value>(&t) {
            if v.get("accepted").and_then(Value::as_bool) == Some(false) {
                bail!(
                    "relay rejected event: {}",
                    v.get("message").and_then(Value::as_str).unwrap_or("")
                );
            }
        }
        Ok(())
    }

    fn blossom_auth(&self, verb: &str, sha256: Option<&str>, server_url: &str) -> Result<String> {
        let exp = (Timestamp::now().as_secs() + 60).to_string();
        let mut tags = vec![Tag::parse(["t", verb])?];
        if let Some(x) = sha256 {
            tags.push(Tag::parse(["x", x])?);
        }
        tags.push(Tag::parse(["expiration", &exp])?);
        tags.push(Tag::parse(["server", &authority(server_url)?])?);
        let content = if verb == "upload" {
            "Upload file"
        } else {
            "Get media"
        };
        let ev = EventBuilder::new(Kind::from(24242), content)
            .tags(tags)
            .sign_with_keys(&self.keys)?;
        Ok(format!("Nostr {}", URL_SAFE_NO_PAD.encode(ev.as_json())))
    }

    /// True when `url` is on our relay's media host (same scheme, host and
    /// port), the only place our Blossom auth may go.
    pub fn is_ours(&self, url: &str) -> bool {
        same_origin(url, &self.base)
    }

    /// BUD-02 upload. Caller has already checked type and size.
    pub async fn upload(&self, bytes: Vec<u8>, mime: &str) -> Result<BlobDescriptor> {
        let sha = hex::encode(Sha256::digest(&bytes));
        // Cheap to clone for the legacy-endpoint retry.
        let bytes = bytes::Bytes::from(bytes);
        let timeout = if mime.starts_with("video/") { 600 } else { 120 };
        let mut url = format!("{}/upload", self.base);
        for legacy in [false, true] {
            if legacy {
                url = format!("{}/media/upload", self.base);
            }
            let auth = self.blossom_auth("upload", Some(&sha), &self.base)?;
            let r = self
                .authed(self.http.put(&url))
                .timeout(Duration::from_secs(timeout))
                .header("Authorization", auth)
                .header("Content-Type", mime)
                .header("X-SHA-256", &sha)
                .body(bytes.clone())
                .send()
                .await?;
            let s = r.status();
            if s.is_success() {
                let mut d: BlobDescriptor = r.json().await.context("bad upload response")?;
                if d.mime_type.is_empty() {
                    d.mime_type = mime.to_string();
                }
                return Ok(d);
            }
            let t = r.text().await.unwrap_or_default();
            if !legacy && (s.as_u16() == 404 || s.as_u16() == 405) {
                continue;
            }
            bail!("upload failed ({s}): {t}");
        }
        unreachable!()
    }

    /// Authenticated BUD-01 GET of a media URL on this relay.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let auth = self.blossom_auth("get", None, url)?;
        let r = self
            .authed(self.http.get(url))
            .timeout(Duration::from_secs(120))
            .header("Authorization", auth)
            .send()
            .await?;
        let s = r.status();
        if !s.is_success() {
            bail!("download failed ({s})");
        }
        Ok(r.bytes().await?.to_vec())
    }

    /// Archived identities (NIP-IA): the `p` tags of the newest kind 13535
    /// signed by the relay's own NIP-11 `self` key.
    pub async fn archived(&self) -> Result<Vec<String>> {
        let info: Value = self
            .authed(self.http.get(&self.base))
            .header("Accept", "application/nostr+json")
            .send()
            .await?
            .json()
            .await?;
        let me = info
            .get("self")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("relay info has no self key"))?
            .to_ascii_lowercase();
        let evs = self
            .query(&[serde_json::json!({"kinds": [13535], "authors": [me], "limit": 1})])
            .await?;
        Ok(evs
            .iter()
            .filter(|e| e.pubkey.to_hex() == me)
            .max_by_key(|e| e.created_at)
            .map(|e| {
                crate::model::tags(e, "p")
                    .filter_map(|t| t.get(1).cloned())
                    .collect()
            })
            .unwrap_or_default())
    }
}

fn rand_nonce() -> u128 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    t ^ (std::process::id() as u128) << 64
}

fn same_origin(a: &str, b: &str) -> bool {
    let origin = |u: &str| {
        reqwest::Url::parse(u).ok().map(|u| {
            (
                u.scheme().to_string(),
                u.host_str().map(str::to_string),
                u.port_or_known_default(),
            )
        })
    };
    matches!((origin(a), origin(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_origin_is_exact() {
        let base = "https://buzz.gitcoin.co";
        assert!(super::same_origin(
            "https://buzz.gitcoin.co/media/a.png",
            base
        ));
        assert!(super::same_origin(
            "https://buzz.gitcoin.co:443/media/a.png",
            base
        ));
        assert!(!super::same_origin(
            "https://buzz.gitcoin.co.evil.com/a.png",
            base
        ));
        assert!(!super::same_origin(
            "https://buzz.gitcoin.co@evil.com/a.png",
            base
        ));
        assert!(!super::same_origin("http://buzz.gitcoin.co/a.png", base));
    }
}
