//! The shared NIP-CK v4 reader vectors (big items, sub-items, notes), run
//! through the production reader: `Checklists::apply`, then `items`. Per
//! item: its role, a big item's derived state, start and trip, a
//! sub-item's notes. The file is buzz's `docs/nips/NIP-CK-v4-vectors.json`,
//! byte for byte.

use buzz_vim_core::checklist::{Checklists, Trust};
use nostr::{Event, JsonUtil};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const VECTORS: &[u8] = include_bytes!("nip-ck-v4-vectors.json");
const SHA256: &str = "a488c39b3915d94bdb7e9354c5b5ced7b2e10875ff344c57a242414f92036e7a";

#[test]
fn the_vectors_file_is_the_shared_one() {
    assert_eq!(hex::encode(Sha256::digest(VECTORS)), SHA256);
}

#[test]
fn every_case_reads_as_expected() {
    let v: Value = serde_json::from_slice(VECTORS).unwrap();
    let channel = v["channel"].as_str().unwrap();
    let trust = Trust {
        owner: v["owner"].as_str().unwrap().to_string(),
        watchers: vec![],
    };
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12);
    let mut failed = vec![];
    for case in cases {
        let mut lists = Checklists::new(trust.clone());
        for e in case["events"].as_array().unwrap() {
            lists.apply(&Event::from_json(e.to_string()).unwrap());
        }
        let items: Map<String, Value> = lists
            .items(channel)
            .into_iter()
            .map(|i| {
                let got = match (&i.parent, i.state) {
                    (Some(_), _) => json!({
                        "role": "sub",
                        "notes": i.notes.iter().map(|n| n.event_id.clone()).collect::<Vec<_>>(),
                    }),
                    (None, Some(state)) => json!({
                        "role": "big",
                        "state": state.label().replace(" ", "_"),
                        "started_at": i.started_at,
                        "trip": i.trip,
                    }),
                    (None, None) => json!({"role": "plain"}),
                };
                (i.id, got)
            })
            .collect();
        let got = json!({ "items": items });
        if got != case["expect"] {
            failed.push(format!(
                "{}: got {got}, want {}",
                case["name"].as_str().unwrap_or_default(),
                case["expect"]
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
