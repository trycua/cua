// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Bounded scrollback ring with non-blocking fan-out.
//!
//! The process reader appends output here and never waits for anyone.
//! Subscribers do not receive pushed copies: each keeps its own cursor (a
//! combined output offset) and pulls from the ring when the `watch` channel
//! says the end moved. A subscriber slower than the producer by more than the
//! ring size loses the evicted bytes, and the gap is visible to its client
//! through `ProcessData.offset`. Memory is bounded by the ring, never by the
//! number or speed of subscribers.

use std::collections::VecDeque;
use std::sync::Mutex;

use bytes::Bytes;
use tokio::sync::watch;

/// Which stream a chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutKind {
    /// stdout.
    Stdout,
    /// stderr.
    Stderr,
    /// PTY (stdout and stderr merged).
    Pty,
}

/// One retained chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Combined offset of `data[0]`.
    pub offset: u64,
    /// Source stream.
    pub kind: OutKind,
    /// Bytes.
    pub data: Bytes,
}

#[derive(Debug)]
struct State {
    chunks: VecDeque<Chunk>,
    /// Offset of the oldest retained byte.
    start: u64,
    /// Offset one past the newest byte.
    end: u64,
    capacity: u64,
}

/// Progress published to subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    /// Offset one past the newest byte.
    pub end: u64,
    /// True once no more output will arrive.
    pub closed: bool,
}

/// The ring plus its change notification.
#[derive(Debug)]
pub struct Output {
    state: Mutex<State>,
    progress: watch::Sender<Progress>,
}

/// Result of [`Output::read_from`].
#[derive(Debug, Default)]
pub struct ReadBatch {
    /// Chunks in order. The first may start after the requested offset when
    /// the requested bytes were evicted.
    pub chunks: Vec<Chunk>,
    /// Cursor to pass next time.
    pub next: u64,
}

impl Output {
    /// Creates a ring holding at most `capacity` bytes (at least 1).
    pub fn new(capacity: u64) -> Self {
        let (progress, _) = watch::channel(Progress::default());
        Self {
            state: Mutex::new(State {
                chunks: VecDeque::new(),
                start: 0,
                end: 0,
                capacity: capacity.max(1),
            }),
            progress,
        }
    }

    /// Appends output. Never blocks on subscribers.
    pub fn push(&self, kind: OutKind, data: Bytes) {
        if data.is_empty() {
            return;
        }
        let end = {
            let mut state = self.state.lock().expect("ring lock");
            let offset = state.end;
            state.end += data.len() as u64;
            // Coalesce small writes of the same kind to keep chunk count low.
            let merged = match state.chunks.back_mut() {
                Some(last)
                    if last.kind == kind
                        && last.data.len() + data.len() <= 64 * 1024
                        && last.offset + last.data.len() as u64 == offset =>
                {
                    let mut joined = Vec::with_capacity(last.data.len() + data.len());
                    joined.extend_from_slice(&last.data);
                    joined.extend_from_slice(&data);
                    last.data = Bytes::from(joined);
                    true
                }
                _ => false,
            };
            if !merged {
                state.chunks.push_back(Chunk { offset, kind, data });
            }
            // Evict from the front down to capacity.
            while state.end - state.start > state.capacity {
                let excess = state.end - state.start - state.capacity;
                let front = state.chunks.front_mut().expect("non-empty ring");
                let len = front.data.len() as u64;
                if len <= excess {
                    state.start += len;
                    state.chunks.pop_front();
                } else {
                    front.data = front.data.slice(excess as usize..);
                    front.offset += excess;
                    state.start += excess;
                }
            }
            state.end
        };
        self.progress.send_modify(|p| p.end = end);
    }

    /// Marks the output complete.
    pub fn close(&self) {
        self.progress.send_modify(|p| p.closed = true);
    }

    /// Subscribes to progress changes.
    pub fn subscribe(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    /// Current progress.
    pub fn progress(&self) -> Progress {
        *self.progress.borrow()
    }

    /// `(start, end)` offsets.
    pub fn bounds(&self) -> (u64, u64) {
        let state = self.state.lock().expect("ring lock");
        (state.start, state.end)
    }

    /// Cursor for "replay the last `bytes` bytes".
    pub fn cursor_for_last(&self, bytes: u64) -> u64 {
        let (start, end) = self.bounds();
        end.saturating_sub(bytes).max(start)
    }

    /// Copies up to `max_bytes` of retained output starting at `from`.
    pub fn read_from(&self, from: u64, max_bytes: usize) -> ReadBatch {
        let state = self.state.lock().expect("ring lock");
        let from = from.max(state.start);
        let mut batch = ReadBatch {
            chunks: Vec::new(),
            next: from,
        };
        if from >= state.end {
            batch.next = state.end.max(from);
            return batch;
        }
        // Chunks are contiguous and sorted: binary search the first chunk
        // containing `from`.
        let index = state
            .chunks
            .partition_point(|c| c.offset + c.data.len() as u64 <= from);
        let mut budget = max_bytes.max(1);
        for chunk in state.chunks.iter().skip(index) {
            if budget == 0 {
                break;
            }
            let skip = from.saturating_sub(chunk.offset) as usize;
            let available = chunk.data.len() - skip;
            let take = available.min(budget);
            batch.chunks.push(Chunk {
                offset: chunk.offset + skip as u64,
                kind: chunk.kind,
                data: chunk.data.slice(skip..skip + take),
            });
            budget -= take;
            batch.next = chunk.offset + (skip + take) as u64;
            if take < available {
                break;
            }
            // `from` only matters for the first chunk.
            if skip > 0 {
                continue;
            }
        }
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(batch: &ReadBatch) -> String {
        batch
            .chunks
            .iter()
            .map(|c| String::from_utf8_lossy(&c.data).into_owned())
            .collect()
    }

    #[test]
    fn evicts_to_capacity_and_reports_gap_offsets() {
        let ring = Output::new(10);
        ring.push(OutKind::Stdout, Bytes::from_static(b"0123456789"));
        ring.push(OutKind::Stderr, Bytes::from_static(b"abcde"));
        assert_eq!(ring.bounds(), (5, 15));
        let batch = ring.read_from(0, 100);
        assert_eq!(batch.chunks[0].offset, 5, "gap visible via offset");
        assert_eq!(text(&batch), "56789abcde");
        assert_eq!(batch.next, 15);
        assert_eq!(batch.chunks[1].kind, OutKind::Stderr);
    }

    #[test]
    fn read_respects_budget_and_cursor() {
        let ring = Output::new(1024);
        for part in ["aaa", "bbb", "ccc"] {
            ring.push(OutKind::Pty, Bytes::from(part.as_bytes().to_vec()));
        }
        let first = ring.read_from(1, 4);
        assert_eq!(text(&first), "aabb");
        let second = ring.read_from(first.next, 100);
        assert_eq!(text(&second), "bccc");
        assert_eq!(second.next, 9);
        assert!(ring.read_from(9, 10).chunks.is_empty());
        assert_eq!(ring.cursor_for_last(4), 5);
        assert_eq!(ring.cursor_for_last(100), 0);
    }

    #[test]
    fn keeps_kinds_separate_when_coalescing() {
        let ring = Output::new(1024);
        ring.push(OutKind::Stdout, Bytes::from_static(b"o1"));
        ring.push(OutKind::Stdout, Bytes::from_static(b"o2"));
        ring.push(OutKind::Stderr, Bytes::from_static(b"e1"));
        let batch = ring.read_from(0, 100);
        assert_eq!(batch.chunks.len(), 2);
        assert_eq!(&batch.chunks[0].data[..], b"o1o2");
        assert_eq!(batch.chunks[1].offset, 4);
    }

    #[tokio::test]
    async fn producer_never_waits_for_a_stalled_subscriber() {
        let ring = std::sync::Arc::new(Output::new(1024));
        let _stalled = ring.subscribe(); // never polled
        let started = std::time::Instant::now();
        for _ in 0..100_000 {
            ring.push(OutKind::Stdout, Bytes::from_static(b"0123456789abcdef"));
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let (start, end) = ring.bounds();
        assert_eq!(end, 1_600_000);
        assert_eq!(end - start, 1024);
    }
}
