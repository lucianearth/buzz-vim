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
    /// Operator loop: who else may edit any checklist record (default lucian).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    /// The watcher's pubkeys; its state counts only from these. A list, or
    /// one string, comma separated; any other type is warned about and the
    /// default used, so it can't drop the rest of the file (the nsec too).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "watcher_keys"
    )]
    watcher: Vec<String>,
    /// The Q pop-up's channel name and the agent it @mentions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_agent: Option<String>,
}

pub struct Config {
    pub relay: String,
    pub keys: Keys,
    /// NIP-OA owner delegation (`BUZZ_AUTH_TAG`), only for agent identities.
    pub auth_tag: Option<nostr::Tag>,
    pub loop_: Loop,
}

/// Operator-loop settings: `owner`, `watcher`, `quick_channel` and
/// `quick_agent` in config.json, or `BUZZ_VIM_OWNER`, `BUZZ_VIM_WATCHER`
/// (comma separated), `BUZZ_VIM_QUICK_CHANNEL`, `BUZZ_VIM_QUICK_AGENT`.
#[derive(Clone, Debug)]
pub struct Loop {
    pub owner: String,
    pub watchers: Vec<String>,
    pub quick_channel: String,
    pub quick_agent: String,
}

impl Default for Loop {
    fn default() -> Self {
        Loop {
            owner: crate::checklist::DEFAULT_OWNER.to_string(),
            watchers: crate::checklist::WATCHERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            quick_channel: crate::quick::CHANNEL.to_string(),
            quick_agent: crate::quick::AGENT.to_string(),
        }
    }
}

/// `watcher` as a list of keys or one comma-separated string. Anything
/// else: a warning, and none (so the default).
fn watcher_keys<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let split = |s: &str| -> Vec<String> {
        s.split(',')
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect()
    };
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => split(&s),
        serde_json::Value::Array(a) if a.iter().all(|v| v.is_string()) => a
            .iter()
            .filter_map(|v| v.as_str())
            .flat_map(split)
            .collect(),
        serde_json::Value::Null => vec![],
        other => {
            let msg = format!(
                "config.json: `watcher` should be a list of pubkeys or a comma-separated string, not {other}; using the default"
            );
            eprintln!("buzz-vim: {msg}");
            crate::log::line(msg);
            vec![]
        }
    })
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.trim().is_empty())
}

fn loop_settings(file: &File) -> Loop {
    let d = Loop::default();
    Loop {
        owner: env("BUZZ_VIM_OWNER")
            .or(file.owner.clone())
            .unwrap_or(d.owner),
        watchers: env("BUZZ_VIM_WATCHER")
            .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
            .or_else(|| (!file.watcher.is_empty()).then(|| file.watcher.clone()))
            .unwrap_or(d.watchers),
        quick_channel: env("BUZZ_VIM_QUICK_CHANNEL")
            .or(file.quick_channel.clone())
            .unwrap_or(d.quick_channel),
        quick_agent: env("BUZZ_VIM_QUICK_AGENT")
            .or(file.quick_agent.clone())
            .unwrap_or(d.quick_agent),
    }
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
    let loop_ = loop_settings(&file);
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
        loop_,
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
        ..file
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

#[cfg(test)]
mod tests {
    use super::*;

    /// No `watcher` in config.json: the watcher seat's state still counts
    /// (it did not, so the status line never showed it).
    #[test]
    fn watchers_default_to_the_watcher_seat() {
        if env("BUZZ_VIM_WATCHER").is_some() {
            return;
        }
        let d = loop_settings(&File::default());
        assert_eq!(d.watchers, Loop::default().watchers);
        assert!(!d.watchers.is_empty());
        let set = File {
            watcher: vec!["abc".into()],
            ..File::default()
        };
        assert_eq!(loop_settings(&set).watchers, vec!["abc".to_string()]);
    }

    /// A `watcher` of the wrong type mustn't lose the file: a string is
    /// split on commas, a number warns and leaves the default, and the
    /// nsec and relay stay either way.
    #[test]
    fn a_mistyped_watcher_keeps_the_rest_of_the_config() {
        let parse = |w: &str| -> File {
            serde_json::from_str(&format!(
                r#"{{"relay": "wss://r", "nsec": "nsec1x", "watcher": {w}}}"#
            ))
            .unwrap()
        };
        let f = parse(r#""aa, bb,""#);
        assert_eq!(f.watcher, ["aa", "bb"]);
        assert_eq!(parse(r#"["aa", "bb"]"#).watcher, ["aa", "bb"]);
        for bad in ["42", "{}", "[1]", "true"] {
            let f = parse(bad);
            assert!(f.watcher.is_empty(), "{bad}");
            assert_eq!(
                (f.nsec.as_deref(), f.relay.as_deref()),
                (Some("nsec1x"), Some("wss://r"))
            );
        }
        let f: File = serde_json::from_str(r#"{"nsec": "nsec1x"}"#).unwrap();
        assert!(f.watcher.is_empty());
    }
}
