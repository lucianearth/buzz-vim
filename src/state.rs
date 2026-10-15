//! Local UI state (which sections are today's tabs), per relay and identity.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    pub chosen: Vec<String>,
    pub active: Option<String>,
    /// Our NIP-RS read-state slot and client id (see readstate.rs).
    #[serde(default)]
    pub read_slot: String,
    #[serde(default)]
    pub read_client: String,
}

fn path(relay: &str, me: &str) -> Option<std::path::PathBuf> {
    let host = relay
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .replace([':', '/'], "_");
    Some(
        dirs::config_dir()?
            .join("buzz-vim")
            .join(format!("state-{host}-{}.json", &me[..me.len().min(12)])),
    )
}

pub fn load(relay: &str, me: &str) -> State {
    path(relay, me)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(relay: &str, me: &str, s: &State) {
    if let Some(p) = path(relay, me) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string_pretty(s) {
            let _ = std::fs::write(p, body);
        }
    }
}
