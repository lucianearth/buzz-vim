//! Who you are and which relay you talk to.
//!
//! Order: `BUZZ_PRIVATE_KEY` / `BUZZ_RELAY_URL` env (what agents and the test
//! harness use), then `buzz-vim/config.json` in the config dir, written by
//! `buzz-vim login`.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use nostr::{Keys, SecretKey, ToBech32};
use serde::{Deserialize, Serialize};

pub const DEFAULT_RELAY: &str = "wss://buzz.gitcoin.co";

#[derive(Serialize, Deserialize, Default)]
struct File {
    relay: Option<String>,
    nsec: Option<String>,
}

pub struct Config {
    pub relay: String,
    pub keys: Keys,
    /// NIP-OA owner delegation (`BUZZ_AUTH_TAG`), only for agent identities.
    pub auth_tag: Option<nostr::Tag>,
}

fn path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("no config dir")?
        .join("buzz-vim")
        .join("config.json"))
}

fn read_file() -> File {
    path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn parse_key(s: &str) -> Result<Keys> {
    let sk = SecretKey::parse(s.trim()).context("not a valid nsec or hex private key")?;
    Ok(Keys::new(sk))
}

pub fn load() -> Result<Config> {
    let file = read_file();
    let relay = std::env::var("BUZZ_RELAY_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .or(file.relay)
        .unwrap_or_else(|| DEFAULT_RELAY.to_string());
    let key = std::env::var("BUZZ_PRIVATE_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or(file.nsec);
    let Some(key) = key else {
        bail!("no key yet. Run `buzz-vim login` once, or set BUZZ_PRIVATE_KEY.");
    };
    let keys = parse_key(&key)?;
    let auth_tag = match std::env::var("BUZZ_AUTH_TAG")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(json) => {
            let tag = buzz_sdk::nip_oa::parse_auth_tag(json.trim())
                .map_err(|e| anyhow::anyhow!("bad BUZZ_AUTH_TAG: {e}"))?;
            buzz_sdk::nip_oa::verify_auth_tag(json.trim(), &keys.public_key())
                .map_err(|e| anyhow::anyhow!("BUZZ_AUTH_TAG doesn't match this key: {e}"))?;
            Some(tag)
        }
        None => None,
    };
    Ok(Config {
        relay,
        keys,
        auth_tag,
    })
}

/// Interactive one-time setup: asks for the relay and nsec, stores them 0600.
pub fn login() -> Result<()> {
    let file = read_file();
    let default_relay = file
        .relay
        .clone()
        .unwrap_or_else(|| DEFAULT_RELAY.to_string());
    print!("relay [{default_relay}]: ");
    std::io::stdout().flush()?;
    let mut relay = String::new();
    std::io::stdin().read_line(&mut relay)?;
    let relay = if relay.trim().is_empty() {
        default_relay
    } else {
        relay.trim().to_string()
    };
    // Not echoed, so the key doesn't end up in scrollback.
    let nsec = rpassword::prompt_password(
        "your nsec (Buzz Desktop → Settings → Profile → Private key, reveal it; typing is hidden): ",
    )?;
    let keys = parse_key(&nsec)?;
    let p = path()?;
    let dir = p.parent().context("bad config path")?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let body = serde_json::to_string_pretty(&File {
        relay: Some(relay),
        nsec: Some(keys.secret_key().to_bech32()?),
    })?;
    write_private(&p, body.as_bytes())?;
    println!(
        "saved to {} as {}",
        p.display(),
        keys.public_key().to_bech32()?
    );
    Ok(())
}

#[cfg(unix)]
fn write_private(p: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(p)?;
    // `mode` only applies to new files; tighten an older one before the key goes in.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.set_len(0)?;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(p: &std::path::Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(p, bytes)?;
    Ok(())
}
