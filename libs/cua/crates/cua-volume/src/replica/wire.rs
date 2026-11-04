// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The peer protocol: framed JSON with an optional binary tail.
//!
//! ```text
//! [len: u32 BE][json_len: u32 BE][json][data]      len = 4 + json_len + data
//! ```
//!
//! Two links join a pair of machines. The *control* link carries heartbeats,
//! era changes, log records and small requests; it never carries bulk bytes,
//! so a slow link cannot delay a heartbeat behind a file. The *data* link
//! carries block transfers and large writes.

use serde::{Deserialize, Serialize};

use super::MachineId;
use super::era::{EraRec, Pos};
use super::oplog::Op;
use super::roles::Facts;
use crate::Error;
use crate::backend::{Condition, ObjectMeta, VersionInfo};

/// Largest frame accepted.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Which link of the pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chan {
    Control,
    Data,
}

/// The first message each side sends on a new link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub machine: MachineId,
    pub name: String,
    pub chan: Chan,
    pub lineage: String,
    pub eras: Vec<EraRec>,
    pub applied: Pos,
    pub tail: Pos,
    pub facts: Facts,
    /// Protocol version.
    pub v: u32,
}

/// Protocol version.
pub const VERSION: u32 = 1;

/// A heartbeat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beat {
    pub epoch: u64,
    pub owner: MachineId,
    /// Which volume this machine belongs to (two machines of different
    /// lineages that find each other merge, the less reliable one joining).
    #[serde(default)]
    pub lineage: String,
    /// The newest eras the sender knows (so a machine that missed a
    /// takeover learns it from any heartbeat).
    #[serde(default)]
    pub eras: Vec<EraRec>,
    /// Why the sender cannot keep a copy (a full disk), when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    pub applied: Pos,
    pub tail: Pos,
    pub facts: Facts,
    /// Changes waiting in the sender's outbox.
    #[serde(default)]
    pub pending: u64,
}

/// A precondition on the wire.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "c", content = "etag", rename_all = "snake_case")]
pub enum Cond {
    #[default]
    None,
    IfNoneMatch,
    IfMatch(String),
}

impl From<Condition> for Cond {
    fn from(c: Condition) -> Cond {
        match c {
            Condition::None => Cond::None,
            Condition::IfNoneMatch => Cond::IfNoneMatch,
            Condition::IfMatch(e) => Cond::IfMatch(e),
        }
    }
}

impl From<Cond> for Condition {
    fn from(c: Cond) -> Condition {
        match c {
            Cond::None => Condition::None,
            Cond::IfNoneMatch => Condition::IfNoneMatch,
            Cond::IfMatch(e) => Condition::IfMatch(e),
        }
    }
}

/// Where the bytes of a version a peer wants live on the sender.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "s", rename_all = "snake_case")]
pub enum Src {
    /// The sender's data store (history is kept).
    Data { key: String, version: String },
    /// The sender's spool of changes waiting for the owner.
    Spool { key: String, version: String },
}

/// A change waiting for the owner, as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub key: String,
    /// `None` for a delete.
    pub meta: Option<ObjectMeta>,
    /// The head version the writer saw before its change.
    pub base: Option<String>,
    pub cond: Cond,
    pub machine: MachineId,
    pub name: String,
    pub ts_ms: u64,
    /// Where the content is on the sender.
    pub src: Src,
    /// Offset and length of an inline body in the frame's data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<(u64, u64)>,
}

/// What the owner did with one pending change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum Outcome {
    /// Applied as the new head.
    Applied { meta: ObjectMeta, pos: Pos },
    /// A delete was applied.
    Deleted { pos: Pos },
    /// The owner already had this content at the key.
    Merged,
    /// The key changed meanwhile; the content is at `key` as a conflict copy.
    Copy { key: String, pos: Pos },
    /// A delete lost to a newer write; the write stays.
    Kept,
    /// A precondition failed.
    Precondition { message: String },
    /// Could not apply.
    Failed { message: String },
}

/// A forwarded `Backend` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "q", rename_all = "snake_case")]
pub enum Req {
    /// Body in `data`.
    Put {
        key: String,
        cond: Cond,
    },
    Delete {
        key: String,
        cond: Cond,
    },
    Head {
        key: String,
    },
    Get {
        key: String,
        version: Option<String>,
    },
    GetRange {
        key: String,
        version: String,
        offset: u64,
        len: u64,
    },
    List {
        prefix: String,
    },
    ListDir {
        folder: String,
    },
    Versions {
        key: String,
    },
    /// Heads after `after_key`, at most `max` (a snapshot's pages).
    Heads {
        after_key: String,
        max: usize,
    },
}

/// The answer to a [`Req`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "p", rename_all = "snake_case")]
pub enum Rep {
    Meta {
        meta: ObjectMeta,
        pos: Pos,
        /// Machines holding it when the owner answered.
        copies: u32,
        /// The owner counted it durable (enough copies).
        durable: bool,
    },
    Done {
        pos: Pos,
        copies: u32,
        durable: bool,
    },
    Head {
        meta: Option<ObjectMeta>,
    },
    /// The bytes are in `data`.
    Bytes {
        meta: ObjectMeta,
    },
    Range,
    List {
        items: Vec<ObjectMeta>,
    },
    ListDir {
        files: Vec<ObjectMeta>,
        folders: Vec<String>,
    },
    Versions {
        items: Vec<VersionInfo>,
    },
    Heads {
        items: Vec<ObjectMeta>,
        done: bool,
        at: Pos,
    },
    Fail {
        kind: String,
        message: String,
    },
}

impl Rep {
    pub fn fail(e: &Error) -> Rep {
        Rep::Fail {
            kind: e.tag().to_string(),
            message: match e {
                Error::Invalid(m)
                | Error::NotFound(m)
                | Error::Forbidden(m)
                | Error::Precondition(m)
                | Error::Backend(m)
                | Error::NotConfirmed(m) => m.clone(),
                other => other.to_string(),
            },
        }
    }

    /// The error a `Fail` stands for.
    pub fn into_error(kind: &str, message: String) -> Error {
        match kind {
            "invalid_argument" => Error::Invalid(message),
            "not_found" => Error::NotFound(message),
            "forbidden" => Error::Forbidden(message),
            "precondition_failed" => Error::Precondition(message),
            _ => Error::Backend(message),
        }
    }
}

/// A record on the wire; inline bodies are slices of the frame's data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordW {
    pub pos: Pos,
    pub op: Op,
    pub at_ms: u64,
    /// Offset and length of the body in the frame's data (small puts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<(u64, u64)>,
}

/// Everything that crosses a link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    Hello(Hello),
    Ping(Beat),
    Pong(Beat),
    /// The sender is going away (sleep, shutdown, network lost).
    Bye {
        reason: String,
    },
    /// Log records from the owner.
    Append {
        epoch: u64,
        records: Vec<RecordW>,
    },
    /// Replica to owner: applied through `applied`.
    Ack {
        applied: Pos,
    },
    /// The sender is below the receiver's epoch; here is the history.
    Fenced {
        eras: Vec<EraRec>,
    },
    /// The receiver holds records past this era's base: keep up to `keep`.
    Diverged {
        keep: Pos,
        eras: Vec<EraRec>,
    },
    /// Replica to owner: a gap; resend after `applied`.
    Behind {
        applied: Pos,
    },
    /// Owner to replica: the log no longer reaches you; pull a snapshot at `at`.
    Snapshot {
        at: Pos,
    },
    /// A candidate asks to take over at `epoch`.
    Prepare {
        epoch: u64,
        base: Pos,
    },
    Promise {
        ok: bool,
        reason: String,
    },
    /// A new era (takeover or hand-back).
    Era {
        recs: Vec<EraRec>,
    },
    /// The owner offers the era to `to` at `epoch`, having logged through `base`.
    Handoff {
        epoch: u64,
        to: MachineId,
        base: Pos,
    },
    HandoffReply {
        ok: bool,
        reason: String,
    },
    /// Changes waiting for the owner.
    Submit {
        ops: Vec<Pending>,
    },
    Submitted {
        outcomes: Vec<Outcome>,
    },
    Req(Req),
    Rep(Rep),
    /// Block hashes of a source: `count` hashes from block `from`.
    Hashes {
        src: Src,
        block: u64,
        from: u64,
        count: u64,
    },
    HashesAre {
        hashes: Vec<String>,
        size: u64,
    },
    /// One block of a source, bytes in `data`.
    Block {
        src: Src,
        block: u64,
        idx: u64,
    },
    BlockIs,
    /// A refusal or failure of any request.
    Error {
        kind: String,
        message: String,
    },
}

/// One frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub id: u64,
    /// The id of the request this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    pub msg: Msg,
}

/// Encodes a frame and its data.
pub fn encode(f: &Frame, data: &[u8]) -> std::io::Result<Vec<u8>> {
    let json = serde_json::to_vec(f)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = 4 + json.len() + data.len();
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("frame of {len} bytes"),
        ));
    }
    let mut out = Vec::with_capacity(8 + json.len() + data.len());
    out.extend_from_slice(&(len as u32).to_be_bytes());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(data);
    Ok(out)
}

/// Reads one frame; `None` at a clean end of stream.
pub async fn read<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
) -> std::io::Result<Option<(Frame, Vec<u8>)>> {
    use tokio::io::AsyncReadExt as _;
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if !(4..=MAX_FRAME).contains(&len) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let json_len = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if 4 + json_len > len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "json longer than its frame",
        ));
    }
    let data = body.split_off(4 + json_len);
    let frame: Frame = serde_json::from_slice(&body[4..])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some((frame, data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_with_data() {
        let f = Frame {
            id: 7,
            re: Some(3),
            msg: Msg::Block {
                src: Src::Data {
                    key: "a/b".into(),
                    version: "v".into(),
                },
                block: 1 << 20,
                idx: 4,
            },
        };
        let bytes = encode(&f, b"payload").unwrap();
        let mut r = std::io::Cursor::new(bytes);
        let (g, d) = read(&mut r).await.unwrap().unwrap();
        assert_eq!(f, g);
        assert_eq!(d, b"payload");
        assert!(read(&mut r).await.unwrap().is_none());
    }

    #[test]
    fn errors_keep_their_kind_across_the_wire() {
        for e in [
            Error::NotFound("k".into()),
            Error::Precondition("changed".into()),
            Error::Invalid("x".into()),
        ] {
            let Rep::Fail { kind, message } = Rep::fail(&e) else {
                panic!()
            };
            assert_eq!(Rep::into_error(&kind, message), e);
        }
    }
}
