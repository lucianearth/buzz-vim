//! pq for the iPhone app: blocks become [`Segment`]s, and asking, sealing
//! and opening go through core's pq. Swift keeps each request's record JSON
//! in its Keychain (account `pq:<id>`), so records cross as strings.

use buzz_vim_core::pq::{self, Message, Piece, Request};

use crate::{BuzzError, Segment};

/// A request just made: post `block`, keep `record` (secret) under `id`.
#[derive(uniffi::Record, Clone)]
pub struct PqNewRequest {
    pub id: String,
    pub block: String,
    pub record: String,
}

#[uniffi::export]
pub fn pq_request(label: String) -> PqNewRequest {
    let rec = pq::request(&label);
    PqNewRequest {
        id: rec.id.clone(),
        block: rec.request().block(),
        record: serde_json::to_string(&rec).expect("serialize"),
    }
}

/// Seal `data` to a REQUEST block (a PqRequest segment's `block`); the
/// MESSAGE block to post.
#[uniffi::export]
pub fn pq_seal(request_block: String, data: Vec<u8>, name: String) -> Result<String, BuzzError> {
    Ok(pq::seal(
        &Request::from_block(&request_block)?,
        &data,
        &name,
    )?)
}

/// Open a MESSAGE block with the record JSON its request id names.
#[uniffi::export]
pub fn pq_open(message_block: String, record: String) -> Result<Vec<u8>, BuzzError> {
    let rec = serde_json::from_str(&record)
        .map_err(|e| BuzzError::Failed(format!("the key record is corrupt: {e}")))?;
    Ok(pq::open(&Message::from_block(&message_block)?, &rec)?)
}

/// pq blocks out of `src`; the text between them goes to `text`.
pub(crate) fn split(src: &str, mut text: impl FnMut(&str, &mut Vec<Segment>)) -> Vec<Segment> {
    let mut out = vec![];
    for piece in pq::parse_blocks(src) {
        match piece {
            Piece::Text(t) => text(&t, &mut out),
            Piece::Request(r) => out.push(Segment::PqRequest {
                id: r.id.clone(),
                label: pq::display_text(&r.label),
                block: r.block(),
            }),
            Piece::Message(m) => {
                let name = m.header.name.as_deref().unwrap_or("message.out");
                // The sender's name, untrusted: its last path part, made safe to show.
                let name = pq::display_text(name.rsplit(['/', '\\']).next().unwrap_or(""));
                out.push(Segment::PqMessage {
                    id: m.header.id.clone(),
                    name,
                    size: m.header.size.unwrap_or(0),
                    block: pq::armor("MESSAGE", &m.blob, ""),
                    unasked: String::new(),
                })
            }
        }
    }
    out
}
