//! Reliable framed write to the cua-driver daemon socket.
//!
//! Split out of `serve.rs::send_request` so its EAGAIN-retry logic is unit
//! testable without linking the platform crates (and their Swift/Metal interop)
//! that the `cua-driver` binary pulls in.

use std::io::Write;
use std::time::Instant;

/// Write `bytes` in full to a daemon socket that has `SO_SNDTIMEO` set,
/// treating a write timeout (`WouldBlock`/`TimedOut`, i.e. EAGAIN) as "the
/// daemon is still draining, keep waiting" rather than a fatal transport error.
///
/// This is the write-side mirror of `send_request`'s read loop (#1997 for
/// #1864): a daemon momentarily too busy to read our request is not a transport
/// failure, just as a daemon still computing a slow response is not. Without it,
/// a single 5s `SO_SNDTIMEO` write timeout surfaced as a fatal `daemon transport
/// error forwarding '<tool>': Resource temporarily unavailable (os error 35)`
/// even for a tiny request. Bounded by `deadline` so a genuinely stuck daemon
/// still surfaces an error instead of blocking forever.
pub fn write_all_with_retry<W: Write>(
    w: &mut W,
    bytes: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        match w.write(&bytes[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "daemon closed the connection mid-request",
                ));
            }
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // Write timeout (SO_SNDTIMEO) / non-blocking EAGAIN: the daemon is
            // momentarily not reading. Keep waiting until the deadline rather
            // than surfacing a fatal transport error.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "daemon did not drain the socket in time \
                             (wrote {written}/{} bytes)",
                            bytes.len()
                        ),
                    ));
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::write_all_with_retry;
    use std::io::{Error, ErrorKind, Write};
    use std::time::{Duration, Instant};

    /// Returns `WouldBlock` (EAGAIN) for its first `eagain_left` write attempts,
    /// then accepts data — models a daemon briefly too busy to drain the socket
    /// (the backpressure that triggers the bug).
    struct FlakyWriter {
        eagain_left: usize,
        written: Vec<u8>,
    }
    impl Write for FlakyWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.eagain_left > 0 {
                self.eagain_left -= 1;
                return Err(Error::new(ErrorKind::WouldBlock, "eagain"));
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn retries_through_transient_eagain() {
        let mut w = FlakyWriter {
            eagain_left: 3,
            written: vec![],
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        write_all_with_retry(&mut w, b"hello\n", deadline)
            .expect("transient EAGAIN should be retried, not fatal");
        assert_eq!(w.written, b"hello\n");
    }

    #[test]
    fn times_out_when_daemon_never_drains() {
        let mut w = FlakyWriter {
            eagain_left: usize::MAX,
            written: vec![],
        };
        let deadline = Instant::now() + Duration::from_millis(30);
        let err = write_all_with_retry(&mut w, b"hello\n", deadline)
            .expect_err("a never-draining daemon must eventually surface an error");
        assert_eq!(
            err.kind(),
            ErrorKind::TimedOut,
            "deadline breach should report TimedOut, not a bare EAGAIN"
        );
    }

    #[test]
    fn accumulates_partial_writes() {
        // Accepts only 2 bytes per call — exercises the offset bookkeeping so a
        // short write keeps going until every byte lands.
        struct ChunkWriter {
            written: Vec<u8>,
        }
        impl Write for ChunkWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let n = buf.len().min(2);
                self.written.extend_from_slice(&buf[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = ChunkWriter { written: vec![] };
        let deadline = Instant::now() + Duration::from_secs(5);
        write_all_with_retry(&mut w, b"abcdefg\n", deadline).unwrap();
        assert_eq!(w.written, b"abcdefg\n");
    }
}
