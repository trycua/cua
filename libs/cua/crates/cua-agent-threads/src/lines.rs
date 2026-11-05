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
}
