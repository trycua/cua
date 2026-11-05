//! JSON Lines, read in bounded pieces: the head and tail of a file for a
//! listing, and every line (skipping any too long to be a message) for a
//! history.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Bytes read from each end of a file for a listing.
pub(crate) const EDGE_BYTES: u64 = 256 * 1024;
/// A line longer than this (an image, a huge tool output) is skipped.
pub(crate) const LINE_MAX: usize = 8 * 1024 * 1024;

/// The whole JSON objects in the first [`EDGE_BYTES`] of `path`, and those in
/// the last [`EDGE_BYTES`] (empty when the head already covered the file).
pub(crate) fn edges(path: &Path) -> std::io::Result<(Vec<Value>, Vec<Value>)> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = vec![];
    (&mut f).take(EDGE_BYTES).read_to_end(&mut head)?;
    let whole = len <= EDGE_BYTES;
    let head_objs = parse(&head, false, !whole);
    if whole {
        return Ok((head_objs, vec![]));
    }
    let start = len.saturating_sub(EDGE_BYTES).max(EDGE_BYTES);
    if start >= len {
        return Ok((head_objs, vec![]));
    }
    f.seek(SeekFrom::Start(start))?;
    let mut tail = vec![];
    f.take(len - start).read_to_end(&mut tail)?;
    Ok((head_objs, parse(&tail, true, false)))
}

/// The objects in `buf`, one per line. `skip_first`/`skip_last` drop a line
/// cut by the read's edge.
fn parse(buf: &[u8], skip_first: bool, skip_last: bool) -> Vec<Value> {
    let mut parts: Vec<&[u8]> = buf.split(|b| *b == b'\n').collect();
    if skip_first && !parts.is_empty() {
        parts.remove(0);
    }
    if skip_last {
        parts.pop();
    }
    parts.into_iter().filter_map(object).collect()
}

fn object(line: &[u8]) -> Option<Value> {
    let line = line.trim_ascii();
    if line.first() != Some(&b'{') {
        return None;
    }
    serde_json::from_slice::<Value>(line)
        .ok()
        .filter(Value::is_object)
}

/// Calls `f` with every JSON object line of `path`, in order. Lines over
/// [`LINE_MAX`] and lines that are not JSON objects are skipped.
pub(crate) fn each(path: &Path, mut f: impl FnMut(Value)) -> std::io::Result<()> {
    let mut r = BufReader::with_capacity(64 * 1024, File::open(path)?);
    let mut line: Vec<u8> = vec![];
    let mut too_long = false;
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            if !too_long && let Some(v) = object(&line) {
                f(v);
            }
            return Ok(());
        }
        let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !too_long {
            if line.len() + chunk.len() > LINE_MAX {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = done.unwrap_or(buf.len());
        r.consume(used);
        if done.is_some() {
            if !too_long && let Some(v) = object(&line) {
                f(v);
            }
            line.clear();
            too_long = false;
        }
    }
}

/// What a history read may spend on one chat's file: it looks at the newest
/// part of the file only, however big the file is (a chat can run to
/// hundreds of megabytes of tool output).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Bytes read from the end of the file.
    pub bytes: u64,
    /// Wall-clock time.
    pub time: std::time::Duration,
}

/// The default: 16 MiB (thousands of messages) within 2 seconds.
pub(crate) const HISTORY_BUDGET: Budget = Budget {
    bytes: 16 * 1024 * 1024,
    time: std::time::Duration::from_secs(2),
};

/// Bytes read per step from the end of a file.
const REV_CHUNK: usize = 256 * 1024;

#[cfg(test)]
thread_local! {
    /// Bytes [`rev_each`] read on this thread (tests assert a bound).
    pub(crate) static REV_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Calls `f` with the JSON object lines of `path`, newest (last) first,
/// until `f` returns `false`, the file start is reached, or `budget` is
/// spent. Lines over [`LINE_MAX`] and lines that are not JSON objects are
/// skipped; a line cut by the budget is dropped. Returns whether the start
/// of the file was reached (nothing older is left unread).
pub(crate) fn rev_each(
    path: &Path,
    budget: Budget,
    mut f: impl FnMut(Value) -> bool,
) -> std::io::Result<bool> {
    let mut file = File::open(path)?;
    let mut pos = file.metadata()?.len();
    let started = std::time::Instant::now();
    let mut spent = 0u64;
    // The bytes of the line that starts before the bytes read so far.
    let mut carry: Vec<u8> = vec![];
    // Inside a line too long to keep, looking for its start.
    let mut skipping = false;
    while pos > 0 {
        if spent >= budget.bytes || started.elapsed() >= budget.time {
            return Ok(false);
        }
        let n = (pos as usize).min(REV_CHUNK);
        pos -= n as u64;
        let mut chunk = vec![0u8; n];
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut chunk)?;
        spent += n as u64;
        #[cfg(test)]
        REV_BYTES_READ.with(|c| c.set(c.get() + n as u64));
        if skipping {
            match chunk.iter().rposition(|b| *b == b'\n') {
                Some(i) => {
                    chunk.truncate(i);
                    skipping = false;
                }
                None => continue,
            }
        }
        chunk.extend_from_slice(&carry);
        carry.clear();
        let mut parts = chunk.split(|b| *b == b'\n');
        // Before the file start, the first piece may begin in an earlier chunk.
        let first = if pos > 0 { parts.next() } else { None };
        let complete: Vec<&[u8]> = parts.collect();
        for line in complete.into_iter().rev() {
            if line.len() <= LINE_MAX
                && let Some(v) = object(line)
                && !f(v)
            {
                return Ok(false);
            }
        }
        if let Some(first) = first {
            if first.len() > LINE_MAX {
                skipping = true;
            } else {
                carry = first.to_vec();
            }
        }
    }
    Ok(true)
}

/// `v[a][b]...` as a string.
pub(crate) fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn edges_drop_cut_lines_and_bad_json() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut f = File::create(&p).unwrap();
        writeln!(f, "{{\"n\":0}}").unwrap();
        writeln!(f, "not json").unwrap();
        let pad = "x".repeat(1000);
        for n in 1..1000 {
            writeln!(f, "{{\"n\":{n},\"pad\":\"{pad}\"}}").unwrap();
        }
        drop(f);
        let (head, tail) = edges(&p).unwrap();
        assert_eq!(head[0]["n"], 0);
        assert!(head.iter().all(|v| v.is_object()));
        assert_eq!(tail.last().unwrap()["n"], 999);
        // Every object read is whole.
        assert!(
            tail.iter()
                .all(|v| v["pad"].as_str().unwrap().len() == 1000)
        );
    }

    #[test]
    fn each_skips_a_line_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.jsonl");
        let mut f = File::create(&p).unwrap();
        writeln!(f, "{{\"n\":1}}").unwrap();
        writeln!(f, "{{\"big\":\"{}\"}}", "y".repeat(LINE_MAX + 10)).unwrap();
        write!(f, "{{\"n\":3}}").unwrap();
        drop(f);
        let mut seen = vec![];
        each(&p, |v| seen.push(v)).unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1]["n"], 3);
    }

    fn numbered(path: &Path, n: usize, pad: usize) {
        let mut f = std::io::BufWriter::new(File::create(path).unwrap());
        let pad = "x".repeat(pad);
        for i in 0..n {
            writeln!(f, "{{\"n\":{i},\"pad\":\"{pad}\"}}").unwrap();
        }
    }

    #[test]
    fn rev_each_goes_newest_first_across_chunk_edges() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.jsonl");
        // Lines of 100 KB straddle the 256 KB chunks.
        numbered(&p, 40, 100_000);
        let mut seen = vec![];
        let whole = rev_each(&p, HISTORY_BUDGET, |v| {
            seen.push(v["n"].as_u64().unwrap());
            true
        })
        .unwrap();
        assert!(whole);
        assert_eq!(seen, (0..40).rev().collect::<Vec<_>>());
    }

    #[test]
    fn rev_each_stops_when_asked_and_reads_only_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("d.jsonl");
        numbered(&p, 400, 100_000); // about 40 MB
        REV_BYTES_READ.with(|c| c.set(0));
        let mut seen = vec![];
        let whole = rev_each(&p, HISTORY_BUDGET, |v| {
            seen.push(v["n"].as_u64().unwrap());
            seen.len() < 3
        })
        .unwrap();
        assert!(!whole);
        assert_eq!(seen, vec![399, 398, 397]);
        // Three lines of 100 KB: a few chunks of 256 KB, not the 40 MB file.
        assert!(REV_BYTES_READ.with(|c| c.get()) <= 2 * 1024 * 1024);
    }

    #[test]
    fn rev_each_spends_no_more_than_its_budget() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("e.jsonl");
        numbered(&p, 400, 100_000);
        REV_BYTES_READ.with(|c| c.set(0));
        let budget = Budget {
            bytes: 1024 * 1024,
            time: std::time::Duration::from_secs(30),
        };
        let mut n = 0;
        let whole = rev_each(&p, budget, |_| {
            n += 1;
            true
        })
        .unwrap();
        assert!(!whole);
        assert!((5..20).contains(&n), "{n}");
        assert!(REV_BYTES_READ.with(|c| c.get()) <= budget.bytes + REV_CHUNK as u64);
    }

    #[test]
    fn rev_each_skips_a_line_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.jsonl");
        let mut f = File::create(&p).unwrap();
        writeln!(f, "{{\"n\":1}}").unwrap();
        writeln!(f, "{{\"big\":\"{}\"}}", "y".repeat(LINE_MAX + 10)).unwrap();
        write!(f, "{{\"n\":3}}").unwrap();
        drop(f);
        let mut seen = vec![];
        let whole = rev_each(&p, HISTORY_BUDGET, |v| {
            seen.push(v["n"].as_u64().unwrap());
            true
        })
        .unwrap();
        assert!(whole);
        assert_eq!(seen, vec![3, 1]);
    }
}
