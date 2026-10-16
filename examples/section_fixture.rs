//! Dev tool: add or remove one test sidebar section on the current
//! identity without touching anything else in Desktop's sections doc.
//! Removal is a tombstone (`live: false`), the same way Desktop deletes.
//!
//! usage: cargo run --example section_fixture -- list
//!        cargo run --example section_fixture -- add "Fixture"   # prints the new id
//!        cargo run --example section_fixture -- remove <id>     # also unassigns its channels
//! Uses BUZZ_PRIVATE_KEY / BUZZ_RELAY_URL / BUZZ_AUTH_TAG like buzz-vim.

use buzz_vim_core::relay::Relay;
use buzz_vim_core::sections::{self, LANE_SECTIONS, Lanes};
use nostr::nips::nip44;
use serde_json::{Value, json};

async fn lanes(relay: &Relay) -> anyhow::Result<Lanes> {
    let me = relay.keys.public_key();
    let heads = relay
        .query(&[
            json!({"kinds": [30078], "authors": [me.to_hex()], "#d": [LANE_SECTIONS], "limit": 1}),
        ])
        .await?;
    let mut l = Lanes::default();
    if let Some(e) = heads
        .iter()
        .filter(|e| e.pubkey == me)
        .max_by_key(|e| e.created_at)
    {
        let doc: Value =
            serde_json::from_str(&nip44::decrypt(relay.keys.secret_key(), &me, &e.content)?)?;
        sections::parse_sections(&doc, &mut l);
    }
    Ok(l)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = std::env::var("BUZZ_PRIVATE_KEY")?;
    let keys = nostr::Keys::new(nostr::SecretKey::parse(key.trim())?);
    let url = std::env::var("BUZZ_RELAY_URL").unwrap_or_else(|_| "wss://buzz.gitcoin.co".into());
    let auth = std::env::var("BUZZ_AUTH_TAG")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|j| buzz_sdk::nip_oa::parse_auth_tag(j.trim()))
        .transpose()?;
    let relay = Relay::new(&url, keys, auth)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] => {}
        ["add", name] => {
            let l = lanes(&relay).await?;
            let order = l.sections.iter().map(|s| s.2).fold(-1.0, f64::max) + 1.0;
            let id = format!("fixture-{}", chrono::Utc::now().timestamp_millis());
            sections::publish_edit(
                &relay,
                &[
                    (vec!["s", &id, "name"], json!(name)),
                    (vec!["s", &id, "icon"], Value::Null),
                    (vec!["s", &id, "order"], json!(order as i64)),
                    (vec!["s", &id, "live"], json!(true)),
                ],
            )
            .await?;
            println!("added {id}");
        }
        ["remove", id] => {
            let l = lanes(&relay).await?;
            let mut writes = vec![(vec!["s", *id, "live"], json!(false))];
            for (ch, sid) in &l.assignments {
                if sid == id {
                    writes.push((vec!["a", ch.as_str()], Value::Null));
                }
            }
            sections::publish_edit(&relay, &writes).await?;
            println!("removed {id} ({} channel(s) unassigned)", writes.len() - 1);
        }
        _ => anyhow::bail!("usage: section_fixture list | add <name> | remove <id>"),
    }
    let l = lanes(&relay).await?;
    for (id, name, order) in &l.sections {
        let n = l.assignments.values().filter(|s| *s == id).count();
        println!("{order:>4} {id}  {name}  ({n} channel(s))");
    }
    Ok(())
}
