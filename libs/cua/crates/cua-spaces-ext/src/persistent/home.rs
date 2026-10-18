// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Moving an agent home between the Cua Volume and a Space.
//!
//! Restore (before a run starts) pulls `agents/<name>/` from the drive and
//! unpacks what differs into `~/cua-volume/agents/<name>` in the guest.
//! Save (after each turn, and on stop and pause) hashes the guest copy,
//! packs only the files whose SHA-256 differs from the drive's manifest,
//! and pushes them. Both move one tar archive over the Space's spacesd
//! channel, so a home of many small files costs one round trip, not one per
//! file. Both run as the agent (`agent:<name>` in its Space), so the drive's
//! access rules and secret scanner apply to what the agent wrote.

use crate::DriveResult as _;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::time::{Duration, Instant};

use cua_spacesd_client::{Command, SpacesdClient, UploadOptions, pb};
use cua_volume::sync::{self, FileState};
use serde::Serialize;

use cua_spaces::agents::quote;
use cua_spaces::{Error, Result};

/// Directories the guest never hashes (the drive ignores them anyway).
const PRUNE: &[&str] = &[
    "node_modules",
    ".git",
    "sessions",
    "log",
    "logs",
    "cache",
    ".cache",
    "credentials",
    "vault",
];

/// Separates the two listings in the guest manifest output.
const RS: &str = "--cua-manifest-hashes--";

/// Largest home moved in one transfer.
pub const MAX_HOME_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// What one restore or save moved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Transfer {
    pub files: usize,
    pub bytes: u64,
    /// Files present and unchanged.
    pub unchanged: usize,
    /// Files removed on the destination.
    pub removed: usize,
    /// Files the secret scanner kept out of the drive: `(path, kind)`.
    pub blocked: Vec<(String, String)>,
    pub millis: u64,
    /// The home is the agent's folder on the Space's mounted Cua Volume:
    /// nothing was copied, and writes land as the agent makes them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub mounted: bool,
}

impl Transfer {
    /// A home that lives on the mounted volume.
    pub fn mounted(millis: u64) -> Transfer {
        Transfer {
            mounted: true,
            millis,
            ..Default::default()
        }
    }
}

async fn sh(guest: &SpacesdClient, script: &str, timeout: Duration) -> Result<Vec<u8>> {
    let out = guest
        .run(Command::shell(script).timeout(timeout))
        .await
        .map_err(Error::Env)?;
    if !out.status.success() {
        return Err(Error::Agent(format!(
            "guest command failed ({:?}): {}",
            out.status.code,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout.to_vec())
}

/// Every file under `dir` in the guest with its SHA-256 and size (relative
/// `/` paths; ignored files left out).
pub async fn guest_manifest(
    guest: &SpacesdClient,
    dir: &str,
) -> Result<BTreeMap<String, FileState>> {
    let prune = PRUNE
        .iter()
        .map(|p| format!("-name {}", quote(p)))
        .collect::<Vec<_>>()
        .join(" -o ");
    // One pass for sizes, one batched sha256sum: two processes, not two
    // per file. Paths with a newline are skipped (sha256sum escapes them).
    let find = format!("find . \\( {prune} \\) -prune -o -type f");
    let script = format!(
        "[ -d {d} ] || exit 0; cd {d} && {find} -printf '%s\\t%p\\n' && echo {RS} && \
         {find} -print0 | xargs -0 -r sha256sum",
        d = quote(dir),
    );
    let out = sh(guest, &script, Duration::from_secs(300)).await?;
    let text = String::from_utf8_lossy(&out);
    let (sizes, hashes) = text.split_once(RS).unwrap_or((&text, ""));
    let mut size_of = std::collections::HashMap::new();
    for line in sizes.lines() {
        if let Some((s, p)) = line.split_once('\t') {
            size_of.insert(
                p.trim_start_matches("./").to_string(),
                s.trim().parse::<u64>().unwrap_or(0),
            );
        }
    }
    let mut m = BTreeMap::new();
    for line in hashes.lines() {
        if line.starts_with('\\') || line.len() < 67 {
            continue;
        }
        let (h, p) = line.split_at(64);
        let rel = p
            .trim_start_matches(' ')
            .trim_start_matches('*')
            .trim_start_matches("./")
            .to_string();
        if rel.is_empty() || sync::ignored(&rel) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let size = size_of.get(&rel).copied().unwrap_or(0);
        m.insert(
            rel,
            FileState {
                sha256: h.to_string(),
                size,
            },
        );
    }
    Ok(m)
}

/// Pulls the drive's copy of `name`'s home into `dir` in the guest.
pub async fn restore(
    guest: &SpacesdClient,
    session: &cua_volume::Session,
    name: &str,
    dir: &str,
) -> Result<Transfer> {
    let t0 = Instant::now();
    let prefix = cua_volume::path::agent_home(name).drive()?;
    let have = guest_manifest(guest, dir).await?;
    let pulled = sync::pull(session, &prefix, &have).await.drive()?;
    let mut report = Transfer {
        files: pulled.files.len(),
        bytes: pulled.bytes,
        unchanged: have.len().saturating_sub(pulled.removed.len()),
        removed: pulled.removed.len(),
        ..Default::default()
    };
    if report.bytes > MAX_HOME_BYTES {
        return Err(Error::invalid(format!(
            "agent home {name} is {} bytes; the limit per transfer is {MAX_HOME_BYTES}",
            report.bytes
        )));
    }
    sh(
        guest,
        &format!("umask 077; mkdir -p {}", quote(dir)),
        Duration::from_secs(30),
    )
    .await?;
    if !pulled.files.is_empty() {
        let mut ar = tar::Builder::new(Vec::new());
        for (rel, bytes) in &pulled.files {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o600);
            h.set_mtime(cua_volume::now_ms() / 1000);
            ar.append_data(&mut h, rel, bytes.as_slice())
                .map_err(|e| Error::Transfer(format!("pack home: {e}")))?;
        }
        let tarball = ar
            .into_inner()
            .map_err(|e| Error::Transfer(format!("pack home: {e}")))?;
        let tmp = format!("{dir}/.cua-restore-{}.tar", cua_volume::new_id());
        guest
            .upload(
                &tmp,
                tarball,
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    permissions: 0o600,
                    ..Default::default()
                },
            )
            .await
            .map_err(Error::Env)?;
        sh(
            guest,
            &format!(
                "cd {d} && tar -xf {t} && rm -f {t}",
                d = quote(dir),
                t = quote(&tmp)
            ),
            Duration::from_secs(300),
        )
        .await?;
    }
    if !pulled.removed.is_empty() {
        let list = pulled
            .removed
            .iter()
            .map(|r| quote(r))
            .collect::<Vec<_>>()
            .join(" ");
        sh(
            guest,
            &format!("cd {} && rm -f {list}", quote(dir)),
            Duration::from_secs(60),
        )
        .await?;
    }
    report.millis = t0.elapsed().as_millis() as u64;
    Ok(report)
}

/// Pushes what changed in `dir` in the guest into the drive's copy of
/// `name`'s home.
pub async fn save(
    guest: &SpacesdClient,
    session: &cua_volume::Session,
    name: &str,
    dir: &str,
) -> Result<Transfer> {
    let t0 = Instant::now();
    let prefix = cua_volume::path::agent_home(name).drive()?;
    let local = guest_manifest(guest, dir).await?;
    let total: u64 = local.values().map(|s| s.size).sum();
    if total > MAX_HOME_BYTES {
        return Err(Error::invalid(format!(
            "agent home {name} is {total} bytes; the limit per transfer is {MAX_HOME_BYTES}"
        )));
    }
    let remote = sync::manifest(session, &prefix).await.drive()?;
    let changed = sync::changed(&local, &remote);
    let mut files = vec![];
    if !changed.is_empty() {
        let id = cua_volume::new_id();
        let list = format!("{dir}/.cua-save-{id}.list");
        let tarball = format!("{dir}/.cua-save-{id}.tar");
        guest
            .upload(
                &list,
                changed.join("\n").into_bytes(),
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    permissions: 0o600,
                    ..Default::default()
                },
            )
            .await
            .map_err(Error::Env)?;
        sh(
            guest,
            &format!(
                "cd {d} && tar -cf {t} -T {l}",
                d = quote(dir),
                t = quote(&tarball),
                l = quote(&list)
            ),
            Duration::from_secs(300),
        )
        .await?;
        let bytes = guest.download(&tarball).await.map_err(Error::Env);
        let _ = sh(
            guest,
            &format!("rm -f {} {}", quote(&tarball), quote(&list)),
            Duration::from_secs(30),
        )
        .await;
        let bytes = bytes?;
        let mut ar = tar::Archive::new(bytes.as_ref());
        for entry in ar
            .entries()
            .map_err(|e| Error::Transfer(format!("read home archive: {e}")))?
        {
            let mut entry =
                entry.map_err(|e| Error::Transfer(format!("read home archive: {e}")))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .map_err(|e| Error::Transfer(e.to_string()))?
                .to_string_lossy()
                .trim_start_matches("./")
                .to_string();
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut buf)
                .map_err(|e| Error::Transfer(e.to_string()))?;
            files.push((rel, buf));
        }
    }
    let pushed = sync::push(session, &prefix, &local, files).await.drive()?;
    Ok(Transfer {
        files: pushed.uploaded.len(),
        bytes: pushed.bytes,
        unchanged: pushed.unchanged,
        removed: pushed.deleted.len(),
        blocked: pushed.blocked,
        millis: t0.elapsed().as_millis() as u64,
        mounted: false,
    })
}
