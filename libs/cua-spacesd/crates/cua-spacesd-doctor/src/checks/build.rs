// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `build`: which builds actually run in the guest, and whether binaries
//! injected for a test (`cua sb create --overlay`) are the ones running.
//!
//! - `build.identity` (informational) carries the running identities as
//!   facts: the cua-spacesd process and the cua-driver core linked into it
//!   (from `health_report`'s `build`, computed inside the daemon, so it names
//!   the running executable even after the file on disk was replaced), a
//!   standalone `cua-driver` on PATH (`cua-driver doctor --json`), and every
//!   overlay record. `--expect` (see `cua_spacesd_client::expect`) reads it.
//! - `build.overlay.<name>` (required) for each record under
//!   [`OVERLAY_DIR`]: the file on disk is still the injected one, and no
//!   process of that program still runs an older executable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::expect::IDENTITY_CHECK;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{Ctx, Recorder};

/// Where `cua sb overlay` records what it injected (one JSON file per name).
pub const OVERLAY_DIR: &str = "/var/lib/cua/overlays";

/// One overlay record.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OverlayRecord {
    pub name: String,
    pub path: String,
    pub sha256: String,
    #[serde(default)]
    pub previous_sha256: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub applied_at: String,
}

/// Reads every record in `dir` (bounded: at most 64 files of 64 KiB).
pub fn read_records(dir: &Path) -> Vec<OverlayRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<OverlayRecord> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .take(64)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if meta.len() > 64 * 1024 {
                return None;
            }
            serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()
        })
        .filter(|r: &OverlayRecord| !r.name.is_empty() && !r.path.is_empty())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The overlay record of cua-spacesd itself, when the daemon was injected.
pub fn daemon_overlay() -> Option<OverlayRecord> {
    read_records(Path::new(OVERLAY_DIR))
        .into_iter()
        .find(|r| r.name == "cua-spacesd")
}

/// A manifest pin check that does not apply: the image's cua-spacesd was
/// replaced on purpose by an overlay (`build.overlay.cua-spacesd` verifies
/// the replacement instead).
pub fn overlaid_pin(id: &str, record: &OverlayRecord) -> Check {
    Check::new(
        id,
        Status::Skip,
        format!(
            "cua-spacesd was injected (sha256 {}); the manifest pins the build baked into the image",
            record.sha256.chars().take(12).collect::<String>()
        ),
    )
    .skip_reason("not_applicable")
    .fact("overlay_sha256", &record.sha256)
}

/// sha256 (hex) of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Processes whose executable is named `program` (or `program (deleted)`,
/// an executable replaced on disk while it runs): pid and the sha256 of
/// what the process actually runs. Linux only; bounded to 4096 entries.
pub fn running_executables(program: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    if !cfg!(target_os = "linux") {
        return out;
    }
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten().take(4096) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let exe = entry.path().join("exe");
        let Ok(target) = std::fs::read_link(&exe) else {
            continue;
        };
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name != program && name != format!("{program} (deleted)") {
            continue;
        }
        if let Ok(hash) = sha256_file(&exe) {
            out.push((pid, hash));
        }
    }
    out
}

fn first_on_path(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn str_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// Identity facts (`<component>.<field>`) the run could establish.
async fn identity(ctx: &Ctx, records: &[OverlayRecord]) -> BTreeMap<String, String> {
    let mut facts = BTreeMap::new();
    facts.insert("cua-spacesd.version".into(), ctx.caps.version.clone());
    // health_report runs inside cua-spacesd (the registry is linked in), so
    // its `build` is the daemon's own: git sha and running executable.
    match super::driver::call_json(
        ctx,
        "health_report",
        json!({"include": ["binary_version"]}),
        Duration::from_secs(20),
    )
    .await
    {
        Ok(report) => {
            facts.insert(
                "cua-driver.version".into(),
                str_of(&report, "driver_version"),
            );
            let build = &report["build"];
            if build.is_object() {
                let git = str_of(build, "git_sha");
                let exe = str_of(build, "exe_sha256");
                facts.insert("cua-driver.git_sha".into(), git.clone());
                facts.insert("cua-driver.exe_sha256".into(), exe.clone());
                facts.insert("cua-spacesd.git_sha".into(), git);
                facts.insert("cua-spacesd.exe_sha256".into(), exe);
            } else {
                facts.insert(
                    "build_identity".into(),
                    "not reported (build predates it)".into(),
                );
            }
        }
        Err(error) => {
            facts.insert("health_report_error".into(), error);
        }
    }
    if let Some(path) = first_on_path("cua-driver") {
        let resolved = crate::sys::resolve(&path).unwrap_or_else(|| path.clone());
        facts.insert(
            "cua-driver.standalone.path".into(),
            path.display().to_string(),
        );
        if let Ok(hash) = sha256_file(&resolved) {
            facts.insert("cua-driver.standalone.exe_sha256".into(), hash);
        }
        let program = path.display().to_string();
        if let Ok(out) = crate::sys::run_local(
            &program,
            &["doctor", "--json"],
            &[],
            Duration::from_secs(20),
        )
        .await
        {
            if let Ok(v) = serde_json::from_str::<Value>(&out.stdout) {
                let build = &v["build"];
                if build.is_object() {
                    facts.insert(
                        "cua-driver.standalone.version".into(),
                        str_of(build, "version"),
                    );
                    facts.insert(
                        "cua-driver.standalone.git_sha".into(),
                        str_of(build, "git_sha"),
                    );
                }
            }
        }
        if !facts.contains_key("cua-driver.standalone.version") {
            if let Ok(out) =
                crate::sys::run_local(&program, &["--version"], &[], Duration::from_secs(10)).await
            {
                if let Some(v) = out.stdout.split_whitespace().nth(1) {
                    facts.insert("cua-driver.standalone.version".into(), v.to_owned());
                }
            }
        }
    }
    for r in records {
        facts.insert(format!("overlay.{}.path", r.name), r.path.clone());
        facts.insert(format!("overlay.{}.sha256", r.name), r.sha256.clone());
        if !r.source.is_empty() {
            facts.insert(format!("overlay.{}.source", r.name), r.source.clone());
        }
    }
    facts
}

/// The verdict for one overlay record given what runs.
pub fn overlay_check(
    record: &OverlayRecord,
    on_disk: Result<String, String>,
    running_daemon_sha: Option<&str>,
    processes: &[(u32, String)],
) -> Check {
    let id = format!("build.overlay.{}", record.name);
    let want = record.sha256.to_ascii_lowercase();
    let short = |s: &str| s.chars().take(12).collect::<String>();
    let mut problems = Vec::new();
    match &on_disk {
        Ok(hash) if *hash == want => {}
        Ok(hash) => problems.push(format!(
            "{} was replaced after injection (sha256 {})",
            record.path,
            short(hash)
        )),
        Err(error) => problems.push(format!("{}: {error}", record.path)),
    }
    if record.name == "cua-spacesd" {
        match running_daemon_sha {
            Some(sha) if sha == want => {}
            Some("") => problems.push("the running cua-spacesd reports no build identity".into()),
            Some(sha) => problems.push(format!(
                "the running cua-spacesd is not the injected build (sha256 {}); restart it",
                short(sha)
            )),
            None => problems.push("the running cua-spacesd did not report its build".into()),
        }
    }
    for (pid, sha) in processes {
        if *sha != want {
            problems.push(format!(
                "pid {pid} still runs an older {} (sha256 {})",
                record.name,
                short(sha)
            ));
        }
    }
    let mut check = if problems.is_empty() {
        Check::new(
            id,
            Status::Pass,
            format!(
                "{} is the injected build (sha256 {}{})",
                record.path,
                short(&want),
                if processes.is_empty() {
                    String::new()
                } else {
                    format!(", {} running", processes.len())
                }
            ),
        )
    } else {
        Check::new(id, Status::Fail, problems.join("; "))
            .fix("re-run `cua sb overlay NAME NAME=PATH` (it replaces the file atomically and restarts the service)")
    };
    check = check
        .fact("path", &record.path)
        .fact("sha256", &want)
        .fact("previous_sha256", &record.previous_sha256)
        .fact("applied_at", &record.applied_at);
    check
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("build") {
        return;
    }
    let records = read_records(Path::new(OVERLAY_DIR));
    let facts = identity(ctx, &records).await;
    let mut check = Check::new(
        IDENTITY_CHECK,
        Status::Pass,
        format!(
            "cua-spacesd {} (git {}), cua-driver {}{}",
            ctx.caps.version,
            facts
                .get("cua-spacesd.git_sha")
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(12).collect::<String>())
                .unwrap_or_else(|| "unknown".into()),
            facts.get("cua-driver.version").cloned().unwrap_or_default(),
            if records.is_empty() {
                String::new()
            } else {
                format!(", {} overlay(s)", records.len())
            }
        ),
    );
    for (k, v) in &facts {
        check = check.fact(k.clone(), v);
    }
    let daemon_sha = facts.get("cua-spacesd.exe_sha256").cloned();
    rec.push(check, &[]).await;

    for record in &records {
        let on_disk = sha256_file(Path::new(&record.path)).map_err(|e| e.to_string());
        let program = Path::new(&record.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The daemon is checked through its own report (its /proc entry may
        // belong to another user); other programs by their processes.
        let processes = if record.name == "cua-spacesd" {
            Vec::new()
        } else {
            running_executables(&program)
        };
        rec.push(
            overlay_check(record, on_disk, daemon_sha.as_deref(), &processes),
            &["core"],
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn record(name: &str) -> OverlayRecord {
        OverlayRecord {
            name: name.into(),
            path: format!("/usr/local/bin/{name}"),
            sha256: A.into(),
            ..OverlayRecord::default()
        }
    }

    #[test]
    fn overlay_passes_only_when_disk_and_processes_match() {
        let r = record("cua-driver");
        assert_eq!(
            overlay_check(&r, Ok(A.into()), None, &[]).status,
            Status::Pass
        );
        assert_eq!(
            overlay_check(&r, Ok(A.into()), None, &[(7, A.into())]).status,
            Status::Pass
        );
        let stale = overlay_check(&r, Ok(A.into()), None, &[(7, B.into())]);
        assert_eq!(stale.status, Status::Fail);
        assert!(stale.message.contains("pid 7"), "{}", stale.message);
        let replaced = overlay_check(&r, Ok(B.into()), None, &[]);
        assert_eq!(replaced.status, Status::Fail);
        assert!(
            replaced.message.contains("replaced"),
            "{}",
            replaced.message
        );
        assert_eq!(
            overlay_check(&r, Err("missing".into()), None, &[]).status,
            Status::Fail
        );
    }

    #[test]
    fn a_daemon_overlay_needs_a_restart() {
        let r = record("cua-spacesd");
        assert_eq!(
            overlay_check(&r, Ok(A.into()), Some(A), &[]).status,
            Status::Pass
        );
        let not_restarted = overlay_check(&r, Ok(A.into()), Some(B), &[]);
        assert_eq!(not_restarted.status, Status::Fail);
        assert!(
            not_restarted.message.contains("restart"),
            "{}",
            not_restarted.message
        );
        assert_eq!(
            overlay_check(&r, Ok(A.into()), Some(""), &[]).status,
            Status::Fail
        );
        assert_eq!(
            overlay_check(&r, Ok(A.into()), None, &[]).status,
            Status::Fail
        );
    }

    #[test]
    fn records_are_read_sorted_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("b.json"),
            json!({"name": "cua-spacesd", "path": "/x", "sha256": A}).to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a.json"),
            json!({"name": "cua-driver", "path": "/y", "sha256": B}).to_string(),
        )
        .unwrap();
        std::fs::write(dir.path().join("junk.json"), "not json").unwrap();
        std::fs::write(dir.path().join("note.txt"), "{}").unwrap();
        let records = read_records(dir.path());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "cua-driver");
        assert_eq!(records[1].name, "cua-spacesd");
        assert!(read_records(Path::new("/nonexistent/cua")).is_empty());
    }

    #[test]
    fn this_test_process_is_found_by_its_executable_name() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        let found = running_executables(&name);
        assert!(found.iter().any(|(pid, _)| *pid == std::process::id()));
    }
}
