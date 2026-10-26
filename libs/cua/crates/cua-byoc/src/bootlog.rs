// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The evidence of a machine that never came online: its console or serial
//! log, fetched before the failed create deletes the machine, filtered of
//! anything secret, kept under `<cua home>/cloud/boot-logs/<id>.log` (0600),
//! and summarized in the error the person sees.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the cloud gets to hand over the log. The rollback never waits
/// longer, whatever the cloud does.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Lines kept in the saved file (the end of the log: the failure is there).
pub const KEEP_LINES: usize = 200;

/// Lines quoted in the error.
pub const SHOW_LINES: usize = 10;

/// Words that make a line secret: the relay token (`cmt_...`), any token,
/// secret, password, key or credential, and the guest environment file. The
/// guest's own log filter (`bootstrap`) hides `token` and `cmt_`; this adds
/// the rest, because a console log also carries the kernel and cloud-init.
const SECRET_WORDS: &[&str] = &[
    "token",
    "cmt_",
    "secret",
    "password",
    "passwd",
    "bearer",
    "authorization",
    "api_key",
    "apikey",
    "access_key",
    "private key",
    "credential",
    "sandbox.env",
    "jwks",
];

/// A secret value shorter than this is not matched as a value (it would
/// hide ordinary words).
const MIN_SECRET_LEN: usize = 8;

fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

/// The last [`KEEP_LINES`] lines of `raw` without any line that mentions a
/// secret word or one of the `secrets` values themselves.
pub fn filter(raw: &str, secrets: &[&str]) -> Vec<String> {
    let secrets: Vec<&str> = secrets
        .iter()
        .copied()
        .filter(|s| s.len() >= MIN_SECRET_LEN)
        .collect();
    let mut lines: Vec<String> = raw
        .lines()
        .map(strip_escapes)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            !SECRET_WORDS.iter().any(|w| low.contains(w)) && !secrets.iter().any(|s| l.contains(s))
        })
        .collect();
    let drop = lines.len().saturating_sub(KEEP_LINES);
    lines.drain(..drop);
    lines
}

/// `id` as a file name.
fn file_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.log", safe.trim_start_matches('.'))
}

/// Where `id`'s boot log is kept under `home`.
pub fn path_of(home: &Path, id: &str) -> PathBuf {
    home.join("cloud").join("boot-logs").join(file_name(id))
}

/// Writes `lines` to the boot log of `id` (0600) and returns its path.
pub fn save(home: &Path, id: &str, lines: &[String]) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    let path = path_of(home, id);
    let dir = path.parent().expect("boot log dir");
    std::fs::create_dir_all(dir)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    for l in lines {
        writeln!(f, "{l}")?;
    }
    Ok(path)
}

/// What the error adds: where the log is and its last [`SHOW_LINES`] lines.
pub fn summary(path: Option<&Path>, lines: &[String]) -> String {
    let mut s = String::new();
    match path {
        Some(p) => s.push_str(&format!("boot log saved to {}", p.display())),
        None => s.push_str("boot log (could not be saved)"),
    }
    let from = lines.len().saturating_sub(SHOW_LINES);
    s.push_str(", last lines:");
    for l in &lines[from..] {
        s.push_str("\n  ");
        s.push_str(l);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_noise_are_dropped_and_the_end_is_kept() {
        let mut raw = String::new();
        for i in 0..300 {
            raw.push_str(&format!("\u{1b}[0;32mline {i}\u{1b}[0m\r\n"));
        }
        raw.push_str("relay token cmt_abcdef0123 joined\n");
        raw.push_str("Authorization: Bearer abc\n");
        raw.push_str("value hunter2hunter2 leaked\n");
        raw.push_str("last ok line\n\n");
        let out = filter(&raw, &["hunter2hunter2", "x"]);
        assert_eq!(out.len(), KEEP_LINES);
        assert_eq!(out.last().unwrap(), "last ok line");
        assert!(out.iter().all(|l| !l.contains("cmt_")
            && !l.contains("Bearer")
            && !l.contains("hunter2")
            && !l.contains('\u{1b}')));
        assert!(out.contains(&"line 299".to_string()));
    }

    #[test]
    fn saved_privately_and_summarized() {
        let dir = tempfile::tempdir().unwrap();
        let lines: Vec<String> = (0..30).map(|i| format!("l{i}")).collect();
        let p = save(dir.path(), "../i-0abc", &lines).unwrap();
        assert!(p.starts_with(dir.path().join("cloud/boot-logs")));
        assert_eq!(p.file_name().unwrap(), "_i-0abc.log");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let s = summary(Some(&p), &lines);
        assert!(
            s.contains("l29") && s.contains("l20") && !s.contains("l19"),
            "{s}"
        );
        assert!(s.contains(&p.display().to_string()));
    }
}
