//! What the cache saved over the last week, for Settings: counts and
//! numbers only, never a URL or a name (the file is `stats.jsonl`, one line
//! per event).

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::store::now_ms;

const WEEK_MS: u64 = 7 * 24 * 3600 * 1000;
/// The file is rewritten without what is older than a week when it is
/// larger than this.
const TRIM_AT: u64 = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Line {
    /// Unix ms.
    t: u64,
    /// `hit`, `miss` or `fallback`.
    k: String,
    /// Seconds a hit saved.
    #[serde(default)]
    s: f64,
    /// Bytes a hit did not download.
    #[serde(default)]
    b: u64,
}

/// What the last week shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Week {
    pub hits: u32,
    pub misses: u32,
    pub seconds_saved: u64,
    pub bytes_saved: u64,
}

fn path(root: &Path) -> PathBuf {
    root.join("stats.jsonl")
}

fn append(root: &Path, line: &Line) {
    let _ = std::fs::create_dir_all(root);
    let p = path(root);
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > TRIM_AT) {
        trim(root);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        && let Ok(mut s) = serde_json::to_string(line)
    {
        s.push('\n');
        let _ = f.write_all(s.as_bytes());
    }
}

fn trim(root: &Path) {
    let p = path(root);
    let Ok(text) = std::fs::read_to_string(&p) else {
        return;
    };
    let from = now_ms().saturating_sub(WEEK_MS);
    let kept: String = text
        .lines()
        .filter(|l| {
            serde_json::from_str::<Line>(l)
                .map(|l| l.t >= from)
                .unwrap_or(false)
        })
        .flat_map(|l| [l, "\n"])
        .collect();
    let _ = crate::store::write_atomic(&p, kept.as_bytes());
}

/// A clone served from a mirror.
pub fn hit(root: &Path, seconds_saved: f64, bytes_saved: u64) {
    append(
        root,
        &Line {
            t: now_ms(),
            k: "hit".into(),
            s: seconds_saved.max(0.0),
            b: bytes_saved,
        },
    );
}

/// A clone of a repository with no usable mirror.
pub fn miss(root: &Path) {
    append(
        root,
        &Line {
            t: now_ms(),
            k: "miss".into(),
            s: 0.0,
            b: 0,
        },
    );
}

/// A mirror that could not be used (the clone went the normal way).
pub fn fallback(root: &Path) {
    append(
        root,
        &Line {
            t: now_ms(),
            k: "fallback".into(),
            s: 0.0,
            b: 0,
        },
    );
}

/// The last week.
pub fn week(root: &Path) -> Week {
    let Ok(text) = std::fs::read_to_string(path(root)) else {
        return Week::default();
    };
    let from = now_ms().saturating_sub(WEEK_MS);
    let mut w = Week::default();
    let mut secs = 0.0;
    for l in text.lines() {
        let Ok(l) = serde_json::from_str::<Line>(l) else {
            continue;
        };
        if l.t < from {
            continue;
        }
        match l.k.as_str() {
            "hit" => {
                w.hits += 1;
                secs += l.s;
                w.bytes_saved += l.b;
            }
            "miss" | "fallback" => w.misses += 1,
            _ => {}
        }
    }
    w.seconds_saved = secs.round() as u64;
    w
}
