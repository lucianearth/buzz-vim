//! The pq repo's recorded PQBOX1 fixtures (tests/vectors there, spec in its
//! docs/PQBOX1.md), made by pq.py and pinned here by sha256: core's pq opens
//! what the CLI sealed, byte for byte. The other direction (pq.py opens what
//! core seals) is the pq repo's tests/test_vectors.py on the rust_* fixtures
//! that `write_rust_fixtures` below makes.

use buzz_vim_core::pq::{self, Message, Piece, Record, Request, Store};
use sha2::{Digest, Sha256};

const REQUEST: &str = include_str!("pq-vectors/pqpy_request.txt");
const RECORD: &str = include_str!("pq-vectors/pqpy_record.json");
const MESSAGE: &str = include_str!("pq-vectors/pqpy_message.txt");
const PLAINTEXT: &[u8] = include_bytes!("pq-vectors/pqpy_plaintext.bin");

#[test]
fn the_fixtures_are_the_pq_repos() {
    for (bytes, sha) in [
        (
            REQUEST.as_bytes(),
            "ea40a7a4d543c81f998beca509827759bf9496d6528ed0fec4704255f459c090",
        ),
        (
            RECORD.as_bytes(),
            "593cd66e48f452c6171829348145909f9eac2812851cb139c6634602d9d6665a",
        ),
        (
            MESSAGE.as_bytes(),
            "c18993b012c490b0ba047b858fb58bd93b4fd66beea4c1570cbbc59ffa436269",
        ),
        (
            PLAINTEXT,
            "e7e4e3381400e8999cb433d56a8ea0290713e137df86f6aee818a922792681da",
        ),
    ] {
        assert_eq!(hex::encode(Sha256::digest(bytes)), sha);
    }
}

fn record() -> Record {
    serde_json::from_str(RECORD).unwrap()
}

#[test]
fn core_opens_what_pq_sealed() {
    let msg = Message::from_block(MESSAGE).unwrap();
    assert_eq!(msg.header.name.as_deref(), Some("secret.bin"));
    assert_eq!(msg.header.size, Some(PLAINTEXT.len() as u64));
    assert_eq!(pq::open(&msg, &record()).unwrap(), PLAINTEXT);
}

#[test]
fn the_request_is_the_records_public_half() {
    let req = Request::from_block(REQUEST).unwrap();
    assert_eq!(req, record().request());
    assert_eq!(req.label, "tax docs \u{2014} 2026");
}

#[test]
fn blocks_are_found_in_quoted_chat_text() {
    let quoted: String = MESSAGE.lines().map(|l| format!("> {l}\n")).collect();
    let text = format!("here you go:\n{quoted}\nthanks\n\n{}", REQUEST.trim_end());
    let pieces = pq::parse_blocks(&text);
    assert_eq!(pieces.len(), 4);
    assert!(matches!(&pieces[0], Piece::Text(t) if t == "here you go:\n"));
    let Piece::Message(msg) = &pieces[1] else {
        panic!("not a message")
    };
    assert_eq!(pq::open(msg, &record()).unwrap(), PLAINTEXT);
    assert!(matches!(&pieces[2], Piece::Text(t) if t == "\n\nthanks\n\n"));
    assert_eq!(pieces[3], Piece::Request(record().request()));
}

#[test]
fn tampering_fails() {
    let mut msg = Message::from_block(MESSAGE).unwrap();
    let last = msg.blob.len() - 1;
    msg.blob[last] ^= 1;
    assert!(
        pq::open(&msg, &record())
            .unwrap_err()
            .to_string()
            .contains("decryption failed")
    );
    // The plain header is AAD too.
    let mut msg = Message::from_block(MESSAGE).unwrap();
    let at = msg
        .blob
        .windows(10)
        .position(|w| w == b"secret.bin")
        .unwrap();
    msg.blob[at] = b'S';
    assert!(pq::open(&msg, &record()).is_err());
}

#[test]
fn the_store_writes_pqs_inbox_and_burns() {
    let dir = std::env::temp_dir().join(format!("pq-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store { home: dir.clone() };
    let rec = record();
    store.save(&rec).unwrap();
    assert!(store.save(&rec).is_err(), "records are created exclusively");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("inbox")), 0o700);
        assert_eq!(
            mode(&dir.join("inbox").join(format!("{}.json", rec.id))),
            0o600
        );
    }
    assert_eq!(store.load(&rec.id).unwrap(), Some(rec.clone()));
    store.burn(&rec.id).unwrap();
    assert_eq!(store.load(&rec.id).unwrap(), None);
    assert!(store.load("../../etc/passwd").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Makes the pq repo's rust_* fixtures:
/// `PQ_VECTORS_OUT=<pq>/tests/vectors cargo test -p buzz-vim-core --test pq_vectors -- --ignored write_rust_fixtures`
#[test]
#[ignore]
fn write_rust_fixtures() {
    let out = std::path::PathBuf::from(std::env::var("PQ_VECTORS_OUT").unwrap());
    let plain = b"sealed by buzz-vim core\n\x00\xff\xfe";
    let block = pq::seal(&Request::from_block(REQUEST).unwrap(), plain, "rust.bin").unwrap();
    std::fs::write(out.join("rust_message.txt"), block + "\n").unwrap();
    std::fs::write(out.join("rust_plaintext.bin"), plain).unwrap();
    let rec = pq::request("from rust");
    std::fs::write(out.join("rust_request.txt"), rec.request().block() + "\n").unwrap();
    std::fs::write(
        out.join("rust_record.json"),
        serde_json::to_vec_pretty(&rec).unwrap(),
    )
    .unwrap();
}

/// Both directions live against the real CLI, through a shared `$PQ_HOME`:
/// `PQ_CMD="<python> <pq>/pq.py" cargo test -p buzz-vim-core --test pq_vectors -- --ignored live_cross_with_pq_cli`
#[test]
#[ignore]
fn live_cross_with_pq_cli() {
    use std::process::Command;
    let cmd = std::env::var("PQ_CMD").unwrap();
    let mut words = cmd.split_whitespace();
    let (prog, pre): (&str, Vec<&str>) = (words.next().unwrap(), words.collect());
    let home = std::env::temp_dir().join(format!("pq-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let store = Store { home: home.clone() };
    let pq_cli = |args: &[&str], stdin: &[u8]| {
        use std::io::Write;
        let mut child = Command::new(prog)
            .args(&pre)
            .args(args)
            .env("PQ_HOME", &home)
            .env("PQ_NO_CLIPBOARD", "1")
            .env("PQ_NO_DIALOG", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };

    // core asks, the CLI answers, core opens with the record it stored.
    let rec = pq::request("live\n-----END PQBOX REQUEST-----\nlabel tail");
    store.save(&rec).unwrap();
    let req_file = home.join("req.txt");
    std::fs::write(&req_file, rec.request().block()).unwrap();
    let reply = pq_cli(&["send", req_file.to_str().unwrap(), "-"], b"cli to core");
    let msg = Message::from_block(std::str::from_utf8(&reply).unwrap()).unwrap();
    let rec = store.load(&msg.header.id).unwrap().unwrap();
    assert_eq!(pq::open(&msg, &rec).unwrap(), b"cli to core");

    // The CLI asks, core answers, the CLI opens (and burns) with its own key.
    let block = pq_cli(&["receive", "from cli"], b"");
    let req = Request::from_block(std::str::from_utf8(&block).unwrap()).unwrap();
    let sealed = pq::seal(&req, b"core to cli", "c.txt").unwrap();
    let opened = pq_cli(&["open", "--stdout", "--burn", "-"], sealed.as_bytes());
    assert_eq!(opened, b"core to cli");
    assert_eq!(store.load(&req.id).unwrap(), None);
    std::fs::remove_dir_all(&home).unwrap();
}
