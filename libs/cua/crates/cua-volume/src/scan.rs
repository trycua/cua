// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The secret scanner run on writes under `agents/`.
//!
//! Agent memory files are where tokens leak: a harness that was shown an API
//! key writes it into its notes, and the notes outlive the Space. The drive
//! refuses such a write (and a sync skips the file), records it in the
//! audit log by kind and line, and never repeats the value anywhere.
//! Patterns are high-precision on purpose: a false positive blocks an
//! agent's memory write.

use std::sync::OnceLock;

use regex::bytes::Regex;

/// One hit: what it looks like and where. Never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static str,
    /// 1-based line.
    pub line: usize,
}

fn patterns() -> &'static [(&'static str, Regex)] {
    static P: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    P.get_or_init(|| {
        [
            ("private key", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----"),
            ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
            ("GitHub token", r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,})\b"),
            ("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
            ("OpenAI API key", r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{32,}"),
            ("Slack token", r"\bxox[abposr]-[A-Za-z0-9\-]{10,}"),
            ("Stripe secret key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}"),
            ("Google API key", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
        ]
        .into_iter()
        .map(|(k, re)| (k, Regex::new(re).expect("valid pattern")))
        .collect()
    })
}

/// Scans `bytes` (any encoding; binary is scanned as bytes). Each line
/// reports at most one finding.
pub fn scan(bytes: &[u8]) -> Vec<Finding> {
    let mut out = vec![];
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        for (kind, re) in patterns() {
            if re.is_match(line) {
                // `sk-ant-` also matches the generic `sk-` shape; the
                // specific kind comes first and wins.
                out.push(Finding { kind, line: i + 1 });
                break;
            }
        }
        if out.len() >= 32 {
            break;
        }
    }
    out
}

/// Bytes read per step by [`scan_reader`].
const CHUNK: usize = 4 * 1024 * 1024;
/// A line longer than this is scanned in overlapping windows (every
/// pattern is far shorter than the overlap).
const MAX_LINE: usize = 16 * 1024 * 1024;
const OVERLAP: usize = 4096;

/// The first finding in a stream, scanned line by line with bounded memory
/// (a multi-gigabyte file never sits in memory whole). Line numbers match
/// [`scan`].
pub fn scan_reader(mut r: impl std::io::Read) -> std::io::Result<Option<Finding>> {
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK);
    let mut line = 1usize;
    let mut chunk = vec![0u8; CHUNK];
    let mut eof = false;
    while !eof {
        let n = r.read(&mut chunk)?;
        if n == 0 {
            eof = true;
        } else {
            buf.extend_from_slice(&chunk[..n]);
        }
        // Everything up to the last newline is complete lines.
        let cut = if eof {
            buf.len()
        } else {
            match buf.iter().rposition(|b| *b == b'\n') {
                Some(i) => i + 1,
                None if buf.len() > MAX_LINE => {
                    // One enormous line: scan it and keep an overlap.
                    if let Some(f) = scan(&buf).into_iter().next() {
                        return Ok(Some(Finding { line, ..f }));
                    }
                    let keep = buf.split_off(buf.len() - OVERLAP);
                    buf = keep;
                    continue;
                }
                None => continue,
            }
        };
        let rest = buf.split_off(cut);
        if let Some(f) = scan(&buf).into_iter().next() {
            return Ok(Some(Finding {
                line: line + f.line - 1,
                ..f
            }));
        }
        line += buf.iter().filter(|b| **b == b'\n').count();
        buf = rest;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_scanner_matches_the_buffer_scanner() {
        let gh = format!("gh{}_{}", "p", "A".repeat(36));
        let mut text = String::new();
        for i in 0..200_000 {
            text.push_str(&format!("line {i} of harmless notes\n"));
        }
        text.push_str(&format!("token={gh}\n"));
        let want = scan(text.as_bytes()).into_iter().next().unwrap();
        let got = scan_reader(text.as_bytes()).unwrap().unwrap();
        assert_eq!(got, want);
        assert_eq!(got.line, 200_001);
        assert!(scan_reader(&b"nothing here\n"[..]).unwrap().is_none());
    }

    #[test]
    fn finds_the_usual_suspects_by_kind_and_line() {
        // Built at runtime so this file does not trip other scanners.
        let ant = format!("sk-{}-{}", "ant", "api03-ABCDEFGHIJKLMNOPQRSTUVWXYZabcd");
        let gh = format!("gh{}_{}", "p", "A".repeat(36));
        let aws = format!("AK{}{}", "IA", "ABCDEFGHIJKLMNOP");
        let pem = format!("-----BEGIN {} KEY-----", "OPENSSH PRIVATE");
        let text = format!(
            "# Memory\nThe user likes tea.\nkey: {ant}\ntoken={gh}\n{aws}\n{pem}\nsk-short\n"
        );
        let f = scan(text.as_bytes());
        let kinds: Vec<(&str, usize)> = f.iter().map(|x| (x.kind, x.line)).collect();
        assert_eq!(
            kinds,
            [
                ("Anthropic API key", 3),
                ("GitHub token", 4),
                ("AWS access key", 5),
                ("private key", 6)
            ]
        );
        assert!(scan(b"just notes about sk-learn and AKIA in prose").is_empty());
    }
}
