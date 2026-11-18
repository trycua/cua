//! The newline-delimited framer.
//!
//! `FRICTION.md` §1: *"The obvious read loop — accumulate until a newline,
//! parse it, return — throws away everything that arrived after that newline
//! in the same chunk. The next call then reads the tail of a previous reply,
//! and from that point every response is matched to the wrong request. Because
//! each reply is a valid JSON object, nothing errors."*
//!
//! ## Why this cannot happen here
//!
//! The fix is structural, not a patched read loop. Three properties together
//! make the desync unrepresentable:
//!
//! 1. **No caller ever sees the buffer.** `buffer` is private and there is no
//!    accessor that hands it out, no `clear`, no `drain`, no `set`. The only
//!    ways to move bytes are `append` (add) and `take_frame` (remove exactly
//!    one frame plus its newline). There is therefore no API through which a
//!    consumer *could* drop a trailing byte.
//! 2. **`take_frame` removes exactly the frame and its delimiter**, and the
//!    remainder stays. This is asserted by `bytes_after_the_newline_survive`,
//!    which is §1 written as a test.
//! 3. **The framer refuses to invent a frame.** `take_frame` returns `None`
//!    when no complete line has arrived; only the read loop decides to wait,
//!    and a buffer that grows past `maximum_frame_bytes` without a newline is
//!    a framing *error* rather than an unbounded wait — a runaway buffer is
//!    the other way a desync hides.

use crate::client::error::{Result, SpacesError};

/// 64 MiB. A single JSON-RPC frame larger than this is a framing violation.
pub const DEFAULT_MAXIMUM_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct LineFramer {
    /// Every byte read past the end of the last complete frame. Private, and
    /// deliberately without an accessor: see the module comment.
    buffer: Vec<u8>,
    maximum_frame_bytes: usize,
}

impl Default for LineFramer {
    fn default() -> Self {
        LineFramer::new(DEFAULT_MAXIMUM_FRAME_BYTES)
    }
}

impl LineFramer {
    pub fn new(maximum_frame_bytes: usize) -> Self {
        LineFramer {
            buffer: Vec::new(),
            maximum_frame_bytes,
        }
    }

    pub fn maximum_frame_bytes(&self) -> usize {
        self.maximum_frame_bytes
    }

    /// How many bytes are still held. Non-zero at the end of a conversation
    /// means the peer stopped mid-frame. This is a count, not the bytes.
    pub fn buffered_byte_count(&self) -> usize {
        self.buffer.len()
    }

    pub fn append(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// The next complete frame, or `None` if one has not arrived yet.
    /// Everything after the newline stays in the buffer.
    pub fn take_frame(&mut self) -> Result<Option<Vec<u8>>> {
        match self.buffer.iter().position(|byte| *byte == b'\n') {
            Some(index) => {
                let frame = self.buffer[..index].to_vec();
                self.buffer.drain(..=index);
                Ok(Some(frame))
            }
            None => {
                if self.buffer.len() > self.maximum_frame_bytes {
                    return Err(SpacesError::TransportFraming(format!(
                        "no newline in {} buffered bytes",
                        self.buffer.len()
                    )));
                }
                Ok(None)
            }
        }
    }

    /// Every frame currently available, in order.
    pub fn take_frames(&mut self) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        while let Some(frame) = self.take_frame()? {
            out.push(frame);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(frame: Vec<u8>) -> String {
        String::from_utf8(frame).expect("utf8 frame")
    }

    /// `FRICTION.md` §1, as a test. One chunk carrying two whole replies and
    /// the head of a third must yield both replies and hold the head.
    #[test]
    fn bytes_after_the_newline_survive() {
        let mut framer = LineFramer::default();
        framer.append(b"{\"id\":1}\n{\"id\":2}\n{\"id\":3");

        assert_eq!(text(framer.take_frame().unwrap().unwrap()), "{\"id\":1}");
        assert_eq!(text(framer.take_frame().unwrap().unwrap()), "{\"id\":2}");
        assert!(framer.take_frame().unwrap().is_none());
        assert_eq!(framer.buffered_byte_count(), 7);

        framer.append(b"}\n");
        assert_eq!(text(framer.take_frame().unwrap().unwrap()), "{\"id\":3}");
        assert_eq!(framer.buffered_byte_count(), 0);
    }

    /// A frame split across arbitrarily many chunks reassembles, and nothing
    /// is emitted early.
    #[test]
    fn a_frame_split_byte_by_byte_reassembles() {
        let mut framer = LineFramer::default();
        let message = b"{\"jsonrpc\":\"2.0\",\"id\":7}\n";
        for byte in &message[..message.len() - 1] {
            framer.append(&[*byte]);
            assert!(framer.take_frame().unwrap().is_none());
        }
        framer.append(b"\n");
        assert_eq!(
            text(framer.take_frame().unwrap().unwrap()),
            "{\"jsonrpc\":\"2.0\",\"id\":7}"
        );
    }

    #[test]
    fn draining_yields_every_frame_in_order() {
        let mut framer = LineFramer::default();
        framer.append(b"a\nb\nc\nd");
        let frames: Vec<String> = framer
            .take_frames()
            .unwrap()
            .into_iter()
            .map(text)
            .collect();
        assert_eq!(
            frames,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(framer.buffered_byte_count(), 1);
    }

    #[test]
    fn an_empty_frame_is_a_frame_and_not_end_of_stream() {
        let mut framer = LineFramer::default();
        framer.append(b"\n\n");
        assert_eq!(text(framer.take_frame().unwrap().unwrap()), "");
        assert_eq!(text(framer.take_frame().unwrap().unwrap()), "");
        assert!(framer.take_frame().unwrap().is_none());
    }

    /// A runaway buffer with no newline is a framing error, not an unbounded
    /// wait. The other way a desync hides.
    #[test]
    fn a_runaway_frame_is_an_error_rather_than_an_unbounded_buffer() {
        let mut framer = LineFramer::new(8);
        framer.append(b"123456789");
        let error = framer.take_frame().unwrap_err();
        assert_eq!(error.tag(), "TransportFraming");
        assert!(error.to_string().contains("9 buffered bytes"), "{error}");
    }

    /// Multibyte UTF-8 is never split on a byte that is not the delimiter:
    /// the framer works on bytes and only `0x0A` ends a frame, which no
    /// continuation byte can be.
    #[test]
    fn multibyte_payloads_are_not_split() {
        let mut framer = LineFramer::default();
        framer.append("{\"t\":\"héllo — 世界\"}\n".as_bytes());
        assert_eq!(
            text(framer.take_frame().unwrap().unwrap()),
            "{\"t\":\"héllo — 世界\"}"
        );
    }
}
