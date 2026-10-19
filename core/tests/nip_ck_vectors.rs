//! The shared NIP-CK reader vectors (rule v3), run through the production
//! reader: `Checklists::apply`, then `driver`, `header`, `items`, `watch`.
//! The file is the one every reader repo commits, byte for byte.

use buzz_vim_core::checklist::{self, Checklists, Trust};
use nostr::{Event, JsonUtil};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VECTORS: &[u8] = include_bytes!("nip-ck-vectors.json");
const SHA256: &str = "249cdb9bf543c7536b610210ec13d528911a674a76a6265629a7a6378fca40ee";

fn vectors() -> Value {
    serde_json::from_slice(VECTORS).unwrap()
}

#[test]
fn the_vectors_file_is_the_shared_one() {
    assert_eq!(hex::encode(Sha256::digest(VECTORS)), SHA256);
}

#[test]
fn built_in_defaults_match_the_vectors() {
    let v = vectors();
    assert_eq!(v["defaults"]["owner"], checklist::DEFAULT_OWNER);
    assert_eq!(v["defaults"]["watcher"], checklist::DEFAULT_WATCHER);
    let t = Trust::default();
    assert_eq!(t.owner, checklist::DEFAULT_OWNER);
    assert_eq!(t.watchers, [checklist::DEFAULT_WATCHER]);
}

#[test]
fn every_case_reads_as_expected() {
    let v = vectors();
    let channel = v["channel"].as_str().unwrap();
    let trust = Trust {
        owner: v["owner"].as_str().unwrap().to_string(),
        watchers: v["watchers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .collect(),
    };
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 25);
    let mut failed = vec![];
    for case in cases {
        let mut lists = Checklists::new(trust.clone());
        for e in case["events"].as_array().unwrap() {
            let ev = Event::from_json(e.to_string()).unwrap();
            lists.apply(&ev);
        }
        let d = lists.driver(channel);
        let mut items: Vec<String> = lists
            .items(channel)
            .into_iter()
            .map(|i| i.event_id)
            .collect();
        items.sort();
        let got = json!({
            "driver": d.driver,
            "contested": d.contested,
            "contested_running": d.contested_running,
            "header": lists.header(channel).map(|h| h.event_id),
            "items": items,
            "watch": lists.watch(channel).map(|w| w.event_id),
        });
        if got != case["expect"] {
            failed.push(format!(
                "{}: got {got}, want {}",
                case["name"], case["expect"]
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
