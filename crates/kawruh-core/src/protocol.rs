//! WebSocket wire protocol between the desktop app and the server.
//!
//! One socket per workspace; binary frames
//! `[kind u8][doc_id_len u8][doc_id utf8][payload]`.
//! `Sync` payloads are y-sync messages (step1 / step2 / update),
//! `Awareness` payloads are y-protocols awareness updates.

use yrs::sync::SyncMessage;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Sync = 0,
    Awareness = 1,
    Subscribe = 2,
    Unsubscribe = 3,
    Error = 4,
}

impl TryFrom<u8> for Kind {
    type Error = ProtocolError;
    fn try_from(v: u8) -> Result<Self, ProtocolError> {
        Ok(match v {
            0 => Kind::Sync,
            1 => Kind::Awareness,
            2 => Kind::Subscribe,
            3 => Kind::Unsubscribe,
            4 => Kind::Error,
            _ => return Err(ProtocolError::Malformed("unknown frame kind")),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    #[error("bad sync message: {0}")]
    Sync(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub doc_id: String,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: Kind, doc_id: &str, payload: Vec<u8>) -> Frame {
        Frame {
            kind,
            doc_id: doc_id.to_string(),
            payload,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let id = self.doc_id.as_bytes();
        let mut out = Vec::with_capacity(2 + id.len() + self.payload.len());
        out.push(self.kind as u8);
        out.push(id.len().min(255) as u8);
        out.extend_from_slice(&id[..id.len().min(255)]);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Frame, ProtocolError> {
        if bytes.len() < 2 {
            return Err(ProtocolError::Malformed("too short"));
        }
        let kind = Kind::try_from(bytes[0])?;
        let len = bytes[1] as usize;
        let id = bytes
            .get(2..2 + len)
            .ok_or(ProtocolError::Malformed("doc id truncated"))?;
        let doc_id = std::str::from_utf8(id)
            .map_err(|_| ProtocolError::Malformed("doc id not utf8"))?
            .to_string();
        Ok(Frame {
            kind,
            doc_id,
            payload: bytes[2 + len..].to_vec(),
        })
    }

    pub fn error(doc_id: &str, code: &str, message: &str) -> Frame {
        let body = serde_json::json!({ "code": code, "message": message }).to_string();
        Frame::new(Kind::Error, doc_id, body.into_bytes())
    }
}

pub fn step1(doc: &Doc) -> Vec<u8> {
    SyncMessage::SyncStep1(doc.transact().state_vector()).encode_v1()
}

pub fn update_message(update: Vec<u8>) -> Vec<u8> {
    SyncMessage::Update(update).encode_v1()
}

/// What a received sync message asks of the receiver.
pub enum SyncAction {
    /// Reply with this payload (a step2) to the sender.
    Reply(Vec<u8>),
    /// The remote content was applied.
    Applied,
}

pub fn decode_sync(payload: &[u8]) -> Result<SyncMessage, ProtocolError> {
    SyncMessage::decode_v1(payload).map_err(|e| ProtocolError::Sync(e.to_string()))
}

/// Answers a step1 with the missing updates.
pub fn answer_step1(doc: &Doc, sv: &StateVector) -> Vec<u8> {
    SyncMessage::SyncStep2(doc.transact().encode_state_as_update_v1(sv)).encode_v1()
}

/// Applies a step2 / update into `doc` with the given transaction origin.
pub fn apply_update(doc: &Doc, update: &[u8], origin: &str) -> Result<(), ProtocolError> {
    let update = Update::decode_v1(update).map_err(|e| ProtocolError::Sync(e.to_string()))?;
    doc.transact_mut_with(origin)
        .apply_update(update)
        .map_err(|e| ProtocolError::Sync(e.to_string()))
}

/// Handles one sync payload: answers step1, applies step2 / update.
pub fn handle_sync(doc: &Doc, payload: &[u8], origin: &str) -> Result<SyncAction, ProtocolError> {
    match decode_sync(payload)? {
        SyncMessage::SyncStep1(sv) => Ok(SyncAction::Reply(answer_step1(doc, &sv))),
        SyncMessage::SyncStep2(u) | SyncMessage::Update(u) => {
            apply_update(doc, &u, origin)?;
            Ok(SyncAction::Applied)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{GetString, Text};

    #[test]
    fn frame_roundtrip() {
        let f = Frame::new(Kind::Sync, "doc-1", vec![1, 2, 3]);
        assert_eq!(Frame::decode(&f.encode()).unwrap(), f);
        assert!(Frame::decode(&[9, 0]).is_err());
        assert!(Frame::decode(&[0, 5, b'a']).is_err());
    }

    #[test]
    fn two_docs_converge_via_step1_step2() {
        let a = Doc::new();
        let b = Doc::new();
        a.get_or_insert_text("t")
            .insert(&mut a.transact_mut(), 0, "from a ");
        b.get_or_insert_text("t")
            .insert(&mut b.transact_mut(), 0, "from b");

        for (from, to) in [(&a, &b), (&b, &a)] {
            let SyncAction::Reply(step2) = handle_sync(from, &step1(to), "x").unwrap() else {
                panic!("step1 must be answered");
            };
            assert!(matches!(
                handle_sync(to, &step2, "remote").unwrap(),
                SyncAction::Applied
            ));
        }
        let ta = a.get_or_insert_text("t").get_string(&a.transact());
        let tb = b.get_or_insert_text("t").get_string(&b.transact());
        assert_eq!(ta, tb);
        assert!(ta.contains("from a") && ta.contains("from b"));
    }
}
