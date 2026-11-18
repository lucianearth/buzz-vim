//! pq: one-off post-quantum transfers, byte-compatible with the `pq` CLI
//! (pq.py). The format is the pq repo's docs/PQBOX1.md: ML-KEM-768 + X25519
//! hybrid KEM, HKDF-SHA256, ChaCha20-Poly1305, armored as
//! `-----BEGIN PQBOX REQUEST|MESSAGE-----` blocks.
//!
//! The receiver makes a [`Record`] (private) and posts its [`Request`] block;
//! the sender [`seal`]s bytes to the request and posts the MESSAGE block; the
//! receiver [`open`]s it with the record. [`parse_blocks`] finds both kinds in
//! message text. The terminal keeps records in pq's own inbox ([`Store`]);
//! iOS keeps the record JSON in its Keychain.

use anyhow::{Context, Result, anyhow, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use libcrux_ml_kem::mlkem768;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use x25519_dalek::{PublicKey, StaticSecret};

pub const VERSION: u32 = 1;
pub const SUITE: &str = "ML-KEM-768+X25519/HKDF-SHA256/ChaCha20-Poly1305";
const MAGIC: &[u8] = b"PQBOX1\n";
const DISPLAY_LIMIT: usize = 200;

/// The public half: what a REQUEST block carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub id: String,
    pub suite: String,
    #[serde(default)]
    pub label: String,
    pub ek: String,
    pub xpk: String,
}

/// The receiver's private key, pq.py's inbox record field for field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub v: u32,
    pub suite: String,
    #[serde(default)]
    pub label: String,
    pub created: u64,
    pub kem_seed: String,
    pub x_sk: String,
    pub ek: String,
    pub xpk: String,
}

/// A MESSAGE's plain header. `name` and `size` are readable without the
/// key; `name` is the sender's and untrusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub v: u32,
    pub id: String,
    pub suite: String,
    pub ct: String,
    pub epk: String,
    pub nonce: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// A parsed MESSAGE: its header and the whole blob (prefix + ciphertext).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub header: Header,
    pub blob: Vec<u8>,
    /// Magic, header length and header: the AEAD's associated data.
    prefix_len: usize,
}

/// A run of message text: plain text, or a block that decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Request(Request),
    Message(Message),
}

fn b64d(s: &str) -> Result<Vec<u8>> {
    B64.decode(s).context("bad base64")
}

fn b64_array<const N: usize>(s: &str, what: &str) -> Result<[u8; N]> {
    b64d(s)?
        .try_into()
        .map_err(|_| anyhow!("{what} is not {N} bytes"))
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the OS random source failed");
    b
}

/// pq.py's request ids: 16 lowercase hex characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One safe line from text the other party chose (pq.py `display_text`):
/// control/format/surrogate characters become spaces, whitespace runs
/// collapse, and it is cut to 200 characters.
pub fn display_text(s: &str) -> String {
    let blanked: String = s
        .chars()
        .map(|c| if is_cc_cf(c) { ' ' } else { c })
        .collect();
    let plain = blanked.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.chars().count() > DISPLAY_LIMIT {
        let cut: String = plain.chars().take(DISPLAY_LIMIT - 3).collect();
        return cut + "...";
    }
    plain
}

/// Unicode Cc and Cf (Cs cannot occur in a Rust string).
fn is_cc_cf(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
            | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
            | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

// ---------------------------------------------------------------- armor

fn begin(kind: &str) -> String {
    format!("-----BEGIN PQBOX {kind}-----")
}

fn end(kind: &str) -> String {
    format!("-----END PQBOX {kind}-----")
}

/// pq.py `armor`: `#` comment lines, a blank line, base64 at 76 columns.
pub fn armor(kind: &str, blob: &[u8], note: &str) -> String {
    let b64 = B64.encode(blob);
    let mut out = vec![begin(kind)];
    if !note.is_empty() {
        // Every line break Python's splitlines() knows, or pq.py would read
        // the rest of a comment line as base64.
        let note: String = note
            .chars()
            .map(|c| match c {
                '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                    '\n'
                }
                c => c,
            })
            .collect();
        out.extend(note.lines().map(|l| {
            if l.is_empty() {
                "#".to_string()
            } else {
                format!("# {l}")
            }
        }));
        out.push(String::new());
    }
    out.extend(
        b64.as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).into_owned()),
    );
    out.push(end(kind));
    out.join("\n")
}

/// Drop email quoting from a line: whitespace, `>`s, at most one space.
fn unquote(line: &str) -> &str {
    let t = line.trim_start();
    match t.strip_prefix('>') {
        Some(r) => {
            let r = r.trim_start_matches('>');
            r.strip_prefix(' ').unwrap_or(r)
        }
        None => line,
    }
}

/// pq.py `dearmor`: the payload of the first `kind` block in `text`.
pub fn dearmor(text: &str, kind: &str) -> Result<Vec<u8>> {
    let text = text.lines().map(unquote).collect::<Vec<_>>().join("\n");
    let (b, e) = (begin(kind), end(kind));
    let start = text
        .find(&b)
        .ok_or_else(|| anyhow!("no PQBOX {kind} block found"))?
        + b.len();
    let len = text[start..]
        .find(&e)
        .ok_or_else(|| anyhow!("no PQBOX {kind} block found"))?;
    let mut payload = String::new();
    for line in text[start..start + len].lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        ensure!(
            line.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=')),
            "unexpected text inside the PQBOX {kind} block"
        );
        payload.push_str(line);
    }
    B64.decode(payload)
        .with_context(|| format!("the PQBOX {kind} block is corrupt (bad base64)"))
}

impl Header {
    /// The name to show and save under. The sender chose it: only its last
    /// path part, through [`display_text`] and [`links::file_name`] (no
    /// dotfile); `message.out` when nothing is left, as pq does.
    pub fn shown_name(&self) -> String {
        let name = self.name.as_deref().map(display_text);
        crate::links::file_name("message.out", name.as_deref())
    }
}

impl Request {
    /// The label to show: the asker chose it, so through [`display_text`];
    /// "a secret" when nothing is left.
    pub fn shown_label(&self) -> String {
        let label = display_text(&self.label);
        if label.is_empty() {
            "a secret".into()
        } else {
            label
        }
    }

    /// A REQUEST payload, held to the version and suite this code speaks.
    pub fn from_json(bytes: &[u8]) -> Result<Request> {
        let req: Request = serde_json::from_slice(bytes).context("the request is not pq JSON")?;
        ensure!(
            req.v == VERSION,
            "request uses format v{}, this speaks v{VERSION}",
            req.v
        );
        ensure!(
            req.suite == SUITE,
            "request uses unsupported suite: {}",
            req.suite
        );
        ensure!(valid_id(&req.id), "request has a malformed id");
        Ok(req)
    }

    pub fn from_block(text: &str) -> Result<Request> {
        Request::from_json(&dearmor(text, "REQUEST")?)
    }

    /// The armored block, with pq.py's note so a CLI user knows what to do.
    pub fn block(&self) -> String {
        let mut note = "To send me a file, install pq (see the setup script), then run:\n    \
            pq send thisfile.txt yourfile.pdf > reply.txt\n\
            and send me reply.txt. This key is single-purpose and expires."
            .to_string();
        if !self.label.is_empty() {
            note = format!("For: {}\n\n{note}", self.label);
        }
        armor(
            "REQUEST",
            &serde_json::to_vec(self).expect("serialize"),
            &note,
        )
    }
}

impl Message {
    /// Split a blob into its header and keep the rest for [`open`].
    pub fn parse(blob: Vec<u8>) -> Result<Message> {
        ensure!(
            blob.starts_with(MAGIC),
            "this does not look like a pq message"
        );
        let hstart = MAGIC.len() + 4;
        let truncated = "message is truncated -- ask the sender to resend it";
        ensure!(blob.len() >= hstart, truncated);
        let hlen = u32::from_be_bytes(blob[MAGIC.len()..hstart].try_into().unwrap()) as usize;
        ensure!(hlen <= blob.len() - hstart, truncated);
        let header: Header = serde_json::from_slice(&blob[hstart..hstart + hlen])
            .context("message header is corrupt -- ask the sender to resend it")?;
        ensure!(valid_id(&header.id), "message names a malformed request id");
        Ok(Message {
            header,
            blob,
            prefix_len: hstart + hlen,
        })
    }

    pub fn from_block(text: &str) -> Result<Message> {
        Message::parse(dearmor(text, "MESSAGE")?)
    }
}

/// Split message text into plain text and the PQBOX blocks in it. A block
/// that does not decode stays text.
pub fn parse_blocks(text: &str) -> Vec<Piece> {
    let mut out = vec![];
    let mut plain = String::new();
    let mut rest = text;
    // Kinds with no END left in the text: no later BEGIN of theirs can
    // close, so they aren't searched again (hostile text stays linear).
    let mut unclosed = [false; 2];
    while let Some(i) = rest.find("-----BEGIN PQBOX ") {
        let after = &rest[i..];
        let kind = ["REQUEST", "MESSAGE"]
            .into_iter()
            .position(|k| after.starts_with(&begin(k)))
            .filter(|&k| !unclosed[k]);
        let block = kind.and_then(|k| {
            let e = end(["REQUEST", "MESSAGE"][k]);
            let Some(at) = after.find(&e) else {
                unclosed[k] = true;
                return None;
            };
            let len = at + e.len();
            let piece = match k {
                0 => Request::from_block(&after[..len]).ok().map(Piece::Request),
                _ => Message::from_block(&after[..len]).ok().map(Piece::Message),
            };
            Some((len, piece))
        });
        match block {
            Some((len, Some(piece))) => {
                // Email quoting before BEGIN belongs to the block.
                let before = &rest[..i];
                let line = before.rfind('\n').map_or(0, |n| n + 1);
                let quote = before[line..]
                    .chars()
                    .all(|c| c == '>' || c.is_whitespace());
                plain.push_str(if quote { &before[..line] } else { before });
                if !plain.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut plain)));
                }
                out.push(piece);
                rest = &rest[i + len..];
            }
            // A block that doesn't decode is text, up to its END, as in
            // pq.py, whose pattern also runs from a BEGIN to the next END.
            Some((len, None)) => {
                plain.push_str(&rest[..i + len]);
                rest = &rest[i + len..];
            }
            // No block here: keep the marker as text, go on.
            None => {
                let skip = i + "-----BEGIN PQBOX ".len();
                plain.push_str(&rest[..skip]);
                rest = &rest[skip..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Piece::Text(plain));
    }
    out
}

// ---------------------------------------------------------------- crypto

fn derive(ss_kem: &[u8], ss_ecdh: &[u8], transcript: &[u8]) -> [u8; 32] {
    let info = [b"pqbox-v1|", SUITE.as_bytes(), b"|", transcript].concat();
    let mut key = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, &[ss_kem, ss_ecdh].concat())
        .expand(&info, &mut key)
        .expect("32 bytes is a valid HKDF length");
    key
}

fn transcript(id: &str, ek: &[u8], xpk: &[u8], ct: &[u8], epk: &[u8]) -> Vec<u8> {
    [id.as_bytes(), ek, xpk, ct, epk].concat()
}

/// A fresh one-shot mailbox. Store the record; post `record.request().block()`.
pub fn request(label: &str) -> Record {
    let seed = random::<64>();
    let kp = mlkem768::generate_key_pair(seed);
    let x = random::<32>();
    let xpk = PublicKey::from(&StaticSecret::from(x));
    Record {
        id: hex::encode(random::<8>()),
        v: VERSION,
        suite: SUITE.to_string(),
        label: label.to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        kem_seed: B64.encode(seed),
        x_sk: B64.encode(x),
        ek: B64.encode(kp.public_key().as_slice()),
        xpk: B64.encode(xpk.as_bytes()),
    }
}

impl Record {
    pub fn request(&self) -> Request {
        Request {
            v: self.v,
            id: self.id.clone(),
            suite: self.suite.clone(),
            label: self.label.clone(),
            ek: self.ek.clone(),
            xpk: self.xpk.clone(),
        }
    }
}

/// Encrypt `data` to a request; the armored MESSAGE block to post.
pub fn seal(req: &Request, data: &[u8], name: &str) -> Result<String> {
    let ek_bytes = b64d(&req.ek)?;
    let ek = mlkem768::MlKem768PublicKey::try_from(ek_bytes.as_slice())
        .map_err(|_| anyhow!("the request's ML-KEM key is the wrong size"))?;
    ensure!(
        mlkem768::validate_public_key(&ek),
        "the request's ML-KEM key is invalid"
    );
    let xpk: [u8; 32] = b64_array(&req.xpk, "the request's X25519 key")?;

    let (ct, ss_kem) = mlkem768::encapsulate(&ek, random::<32>());
    let eph = StaticSecret::from(random::<32>());
    let epk = PublicKey::from(&eph);
    let ss_ecdh = eph.diffie_hellman(&PublicKey::from(xpk));
    ensure!(
        ss_ecdh.was_contributory(),
        "the request's X25519 key is invalid"
    );

    let key = derive(
        &ss_kem,
        ss_ecdh.as_bytes(),
        &transcript(&req.id, &ek_bytes, &xpk, ct.as_slice(), epk.as_bytes()),
    );
    let nonce = random::<12>();
    let header = serde_json::to_vec(&Header {
        v: VERSION,
        id: req.id.clone(),
        suite: SUITE.to_string(),
        ct: B64.encode(ct.as_slice()),
        epk: B64.encode(epk.as_bytes()),
        nonce: B64.encode(nonce),
        name: Some(name.to_string()),
        size: Some(data.len() as u64),
    })?;
    let mut blob = [MAGIC, &(header.len() as u32).to_be_bytes(), &header].concat();
    let ciphertext = ChaCha20Poly1305::new(&key.into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: data,
                aad: &blob,
            },
        )
        .map_err(|_| anyhow!("encryption failed"))?;
    blob.extend(ciphertext);
    Ok(armor("MESSAGE", &blob, ""))
}

/// Decrypt a message with the record its header names.
pub fn open(msg: &Message, rec: &Record) -> Result<Vec<u8>> {
    let h = &msg.header;
    ensure!(
        h.id == rec.id,
        "this message is for request {}, not {}",
        h.id,
        rec.id
    );
    let seed: [u8; 64] = b64_array(&rec.kem_seed, "kem_seed")?;
    let x_sk: [u8; 32] = b64_array(&rec.x_sk, "x_sk")?;
    let ct: [u8; 1088] = b64_array(&h.ct, "the message's ct")?;
    let epk: [u8; 32] = b64_array(&h.epk, "the message's epk")?;
    let nonce: [u8; 12] = b64_array(&h.nonce, "the message's nonce")?;

    let kp = mlkem768::generate_key_pair(seed);
    let ss_kem = mlkem768::decapsulate(kp.private_key(), &ct.into());
    let ss_ecdh = StaticSecret::from(x_sk).diffie_hellman(&PublicKey::from(epk));
    ensure!(
        ss_ecdh.was_contributory(),
        "the message's X25519 key is invalid"
    );

    let key = derive(
        &ss_kem,
        ss_ecdh.as_bytes(),
        &transcript(&rec.id, &b64d(&rec.ek)?, &b64d(&rec.xpk)?, &ct, &epk),
    );
    let (prefix, ciphertext) = msg.blob.split_at(msg.prefix_len);
    ChaCha20Poly1305::new(&key.into())
        .decrypt(
            &nonce.into(),
            Payload {
                msg: ciphertext,
                aad: prefix,
            },
        )
        .map_err(|_| {
            anyhow!("decryption failed -- wrong key, or the message was truncated or tampered with")
        })
}

// ---------------------------------------------------------------- store

/// pq's inbox: `$PQ_HOME/inbox/<id>.json` (default `~/.pqbox`), dirs 0700,
/// records 0600 and created exclusively, so pq and buzz-vim share keys.
pub struct Store {
    pub home: PathBuf,
}

impl Store {
    /// `$PQ_HOME`, else `~/.pqbox`.
    pub fn from_env() -> Option<Store> {
        let home = match std::env::var_os("PQ_HOME") {
            Some(h) => PathBuf::from(h),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".pqbox"),
        };
        Some(Store { home })
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(valid_id(id), "not a request id: {id:?}");
        Ok(self.home.join("inbox").join(format!("{id}.json")))
    }

    pub fn save(&self, rec: &Record) -> Result<()> {
        use std::io::Write;
        let path = self.path(&rec.id)?;
        let inbox = self.home.join("inbox");
        std::fs::create_dir_all(&inbox)?;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            for d in [&self.home, &inbox] {
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
            o.mode(0o600);
        }
        let mut f = o
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        f.write_all(&serde_json::to_vec_pretty(rec)?)?;
        Ok(())
    }

    /// The record for `id`, or None when it was never made here or was burned.
    pub fn load(&self, id: &str) -> Result<Option<Record>> {
        match std::fs::read(self.path(id)?) {
            Ok(b) => Ok(Some(
                serde_json::from_slice(&b).context("the key record is corrupt")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn burn(&self, id: &str) -> Result<()> {
        match std::fs::remove_file(self.path(id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_then_open_round_trips() {
        let rec = request("label");
        let block = seal(&rec.request(), b"hi\0there", "a.txt").unwrap();
        let msg = Message::from_block(&block).unwrap();
        assert_eq!(msg.header.name.as_deref(), Some("a.txt"));
        assert_eq!(msg.header.size, Some(8));
        assert_eq!(open(&msg, &rec).unwrap(), b"hi\0there");
        assert!(open(&msg, &request("other")).is_err());
    }

    #[test]
    fn all_zero_x25519_is_refused() {
        let mut req = request("").request();
        req.xpk = B64.encode([0u8; 32]);
        assert!(
            seal(&req, b"x", "x")
                .unwrap_err()
                .to_string()
                .contains("X25519")
        );
    }

    #[test]
    fn a_bad_mlkem_key_is_refused() {
        let mut req = request("").request();
        // Coefficients >= q fail FIPS 203's modulus check.
        req.ek = B64.encode([0xffu8; 1184]);
        assert!(
            seal(&req, b"x", "x")
                .unwrap_err()
                .to_string()
                .contains("invalid")
        );
    }

    #[test]
    fn display_text_matches_pq() {
        assert_eq!(display_text("a\u{202e}b\n\tc  d"), "a b c d");
        assert_eq!(display_text(&"x".repeat(250)).len(), 200);
        assert!(display_text(&"x".repeat(250)).ends_with("..."));
    }

    #[test]
    fn shown_label_is_safe_and_never_empty() {
        assert_eq!(
            request("db\u{202e}\n key").request().shown_label(),
            "db key"
        );
        assert_eq!(request(" \t").request().shown_label(), "a secret");
    }

    #[test]
    fn shown_name_is_a_safe_last_path_part() {
        let msg = Message::from_block(&seal(&request("").request(), b"x", "f").unwrap()).unwrap();
        let shown = |name: Option<&str>| {
            Header {
                name: name.map(str::to_string),
                ..msg.header.clone()
            }
            .shown_name()
        };
        assert_eq!(shown(Some("../../etc/passwd")), "passwd");
        assert_eq!(shown(Some("C:\\x\\a.pdf")), "a.pdf");
        assert_eq!(shown(Some("\u{200b}.bashrc")), "bashrc");
        assert_eq!(shown(Some("evil\u{202e}fdp.exe")), "evil fdp.exe");
        assert_eq!(shown(Some("dir/")), "message.out");
        assert_eq!(shown(None), "message.out");
    }

    #[test]
    fn hostile_text_parses_in_linear_time() {
        let t = "-----BEGIN PQBOX REQUEST-----\n".repeat(20_000) + "-----END PQBOX REQUEST-----";
        let t = t + &"-----BEGIN PQBOX MESSAGE-----\n".repeat(20_000);
        let start = std::time::Instant::now();
        assert_eq!(parse_blocks(&t), vec![Piece::Text(t.clone())]);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_label_with_python_line_breaks_still_armors() {
        let rec = request("db\u{2028}ok\x0cand\rmore");
        let block = rec.request().block();
        assert!(
            block
                .lines()
                .all(|l| !l.starts_with("ok") && !l.starts_with("and"))
        );
        assert_eq!(Request::from_block(&block).unwrap(), rec.request());
    }

    #[test]
    fn unreadable_blocks_stay_text() {
        let t = "see -----BEGIN PQBOX REQUEST-----\nnot base64!\n-----END PQBOX REQUEST----- ok";
        assert_eq!(parse_blocks(t), vec![Piece::Text(t.to_string())]);
        assert_eq!(parse_blocks("plain"), vec![Piece::Text("plain".into())]);
    }
}
