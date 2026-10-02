// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `teleport`: the receive side of teleport (`TeleportService`), without
//! touching any real app profile.
//!
//! - `teleport.manifest`: `GetManifest` agrees with the advertised
//!   `teleport.<app>` features, speaks this build's bundle version, and
//!   refuses an unknown app with a limitation.
//! - `teleport.import.verify`: the `ImportSession` upload path (chunk
//!   resume, duplicate chunks, offset mismatch, checksum mismatch, a bundle
//!   whose provider is not the app asked for). Every bundle it sends is
//!   refused before an importer runs, so no profile is written.
//! - `teleport.import.fixture`: a canned Firefox bundle imported by the
//!   same importer code cua-spacesd links, into a throwaway home under the
//!   doctor's scratch directory, with process and keychain effects faked.
//! - `teleport.receive_files` (effectful): a real file transfer into a
//!   per-run subdirectory of the guest user's Downloads (ignore rules,
//!   chunked upload with a duplicate, rename on conflict, abort), checked
//!   through the files service and removed afterwards.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, SpacesdClient};
use sha2::{Digest as _, Sha256};

use crate::{Ctx, Recorder};

/// Provider id no guest has: bundles for it can never be imported.
const PROBE_PROVIDER: &str = "cua-doctor-probe";
const FILES_CLAIM: &str = "feature:teleport.files";

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("teleport") {
        return;
    }
    let advertised: Vec<String> = ctx
        .caps
        .features
        .iter()
        .filter(|f| f.supported)
        .filter_map(|f| f.name.strip_prefix("teleport.").map(str::to_owned))
        // `teleport.wipe` is a capability, not an app.
        .filter(|name| name != "wipe")
        .collect();
    let claims_owned: Vec<String> = advertised
        .iter()
        .map(|a| format!("feature:teleport.{a}"))
        .collect();
    let mut claims: Vec<&str> = claims_owned.iter().map(String::as_str).collect();
    if claims.is_empty() {
        claims.push("feature:teleport.*");
    }
    rec.run(
        "teleport.manifest",
        &claims,
        Duration::from_secs(20),
        manifest(&ctx.client, &advertised),
    )
    .await;
    if advertised.is_empty() {
        rec.skip(
            "teleport.import.verify",
            &claims,
            "optional_unavailable",
            "no teleport.<app> feature advertised".into(),
        )
        .await;
    } else {
        rec.run(
            "teleport.import.verify",
            &claims,
            Duration::from_secs(30),
            import_verify(&ctx.client, &advertised[0], &ctx.nonce),
        )
        .await;
    }
    if advertised.iter().any(|a| a == "firefox") {
        let home = std::path::Path::new(&ctx.scratch).join("teleport-home");
        rec.run(
            "teleport.import.fixture",
            &["feature:teleport.firefox"],
            Duration::from_secs(30),
            async move {
                let result = tokio::task::spawn_blocking({
                    let home = home.clone();
                    move || import_fixture(&home)
                })
                .await
                .unwrap_or_else(|e| {
                    Check::new("teleport.import.fixture", Status::Fail, e.to_string())
                });
                let _ = tokio::fs::remove_dir_all(&home).await;
                result
            },
        )
        .await;
    } else {
        rec.skip(
            "teleport.import.fixture",
            &["feature:teleport.firefox"],
            "optional_unavailable",
            "teleport.firefox not advertised".into(),
        )
        .await;
    }
    rec.run_effectful(
        "teleport.receive_files",
        &[FILES_CLAIM],
        Duration::from_secs(60),
        receive_files(&ctx.client, &ctx.nonce),
    )
    .await;
}

/// `GetManifest` for every advertised app, and for an unknown one.
pub async fn manifest(client: &SpacesdClient, advertised: &[String]) -> Check {
    let id = "teleport.manifest";
    let mut teleport = client.teleport();
    let discovery = match teleport
        .get_manifest(pb::GetManifestRequest {
            app: format!("{PROBE_PROVIDER}-unknown"),
            scope: pb::TeleportScope::Session as i32,
        })
        .await
    {
        Ok(r) => r.into_inner(),
        Err(status) => {
            return Check::new(
                id,
                Status::Fail,
                format!("GetManifest: {}", status.message()),
            )
        }
    };
    let mut problems = Vec::new();
    if discovery.supported || discovery.limitation.is_empty() {
        problems.push("an unknown app is not refused with a limitation".to_owned());
    }
    let listed: BTreeSet<String> = discovery.supported_apps.iter().cloned().collect();
    let features: BTreeSet<String> = advertised.iter().cloned().collect();
    if listed != features {
        problems.push(format!(
            "supported_apps {listed:?} != advertised teleport.* features {features:?}"
        ));
    }
    let mut installed = Vec::new();
    for app in listed.iter().take(32) {
        match teleport
            .get_manifest(pb::GetManifestRequest {
                app: app.clone(),
                scope: pb::TeleportScope::Profile as i32,
            })
            .await
        {
            Ok(r) => {
                let m = r.into_inner();
                if !m.supported {
                    problems.push(format!(
                        "{app}: listed but not supported ({})",
                        m.limitation
                    ));
                }
                if m.bundle_version != cua_teleport_bundle::BUNDLE_VERSION {
                    problems.push(format!(
                        "{app}: bundle_version {} != {}",
                        m.bundle_version,
                        cua_teleport_bundle::BUNDLE_VERSION
                    ));
                }
                if !m.scopes.contains(&(pb::TeleportScope::Session as i32)) {
                    problems.push(format!("{app}: no session scope"));
                }
                if m.app_installed {
                    installed.push(app.clone());
                }
            }
            Err(status) => problems.push(format!("{app}: {}", status.message())),
        }
    }
    if listed.is_empty() {
        return Check::new(id, Status::Skip, "the guest has no teleport providers")
            .skip_reason("optional_unavailable");
    }
    Check::new(
        id,
        super::verdict(problems.is_empty()),
        if problems.is_empty() {
            format!(
                "{} provider(s), bundle v{}; installed: {}",
                listed.len(),
                cua_teleport_bundle::BUNDLE_VERSION,
                if installed.is_empty() {
                    "none".to_owned()
                } else {
                    installed.join(", ")
                }
            )
        } else {
            problems.join("; ")
        },
    )
    .fact(
        "providers",
        listed.into_iter().collect::<Vec<_>>().join(","),
    )
    .fact("installed", installed.join(","))
}

fn probe_bundle(provider: &str) -> Result<Vec<u8>, String> {
    let mut writer = cua_teleport_bundle::bundle::BundleWriter::new(
        Vec::new(),
        provider,
        "cua doctor probe",
        cua_teleport_bundle::TransferScope::TabsOnly,
    );
    writer
        .add_bytes("probe/tabs.json", 0o644, br#"["about:blank"]"#)
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

/// The `ImportSession` upload and verification path. Nothing it sends can
/// be imported: the bundle names a provider no guest has.
pub async fn import_verify(client: &SpacesdClient, app: &str, nonce: &str) -> Check {
    let id = "teleport.import.verify";
    let mut teleport = client.teleport();
    let bundle = match probe_bundle(PROBE_PROVIDER) {
        Ok(b) => b,
        Err(e) => return Check::new(id, Status::Fail, format!("build probe bundle: {e}")),
    };
    let digest = sha256_hex(&bundle);
    let half = bundle.len() / 2;
    let request = |import_id: &str, offset: usize, end: usize, commit: bool, sha: &str| {
        pb::ImportSessionRequest {
            import_id: import_id.to_owned(),
            app: app.to_owned(),
            scope: pb::TeleportScope::Session as i32,
            offset: offset as u64,
            data: bundle[offset..end].to_vec(),
            commit,
            sha256: sha.to_owned(),
            options: Some(pb::ImportOptions {
                replace_existing: false,
                close_running_app: false,
                launch_after: false,
                expires_at_ms: 0,
                broker_grant: String::new(),
            }),
        }
    };
    let mut steps = Vec::new();
    let mut problems = Vec::new();

    // 1. An unknown app is refused on the first chunk.
    let mut unknown = request(&format!("cua-doctor-{nonce}-u"), 0, half, false, "");
    unknown.app = format!("{PROBE_PROVIDER}-unknown");
    match teleport.import_session(unknown).await {
        Ok(_) => problems.push("an unknown app was accepted".to_owned()),
        Err(s) => steps.push(format!("unknown app {:?}", s.code())),
    }

    // 2. Resume rules and a checksum mismatch.
    let a = format!("cua-doctor-{nonce}-a");
    match teleport
        .import_session(request(&a, 0, half, false, ""))
        .await
    {
        Ok(r) if r.get_ref().received_bytes == half as u64 => {}
        Ok(r) => problems.push(format!(
            "first chunk: received_bytes {} != {half}",
            r.get_ref().received_bytes
        )),
        Err(s) => return Check::new(id, Status::Fail, format!("first chunk: {}", s.message())),
    }
    match teleport
        .import_session(request(&a, 0, half, false, ""))
        .await
    {
        Ok(r) if r.get_ref().duplicate => steps.push("duplicate chunk acknowledged".into()),
        Ok(_) => problems.push("a resent chunk was not reported as a duplicate".into()),
        Err(s) => problems.push(format!("resent chunk: {}", s.message())),
    }
    match teleport
        .import_session(request(
            &a,
            half.saturating_sub(1).max(1),
            half + 1,
            false,
            "",
        ))
        .await
    {
        Ok(_) if half > 1 => problems.push("an overlapping offset was accepted".into()),
        Ok(_) => {}
        Err(s) if s.code() == tonic::Code::FailedPrecondition => {
            steps.push("offset mismatch FailedPrecondition".into())
        }
        Err(s) => problems.push(format!("offset mismatch: {:?} {}", s.code(), s.message())),
    }
    let wrong = "0".repeat(64);
    match teleport
        .import_session(request(&a, half, bundle.len(), true, &wrong))
        .await
    {
        Ok(_) => problems.push("a checksum mismatch was imported".into()),
        Err(s) if s.code() == tonic::Code::FailedPrecondition => {
            steps.push("checksum mismatch FailedPrecondition".into())
        }
        Err(s) => problems.push(format!("checksum mismatch: {:?} {}", s.code(), s.message())),
    }

    // 3. A complete, verified bundle whose provider is not `app`.
    let b = format!("cua-doctor-{nonce}-b");
    match teleport
        .import_session(request(&b, 0, bundle.len(), true, &digest))
        .await
    {
        Ok(_) => problems.push(format!("a {PROBE_PROVIDER} bundle was imported as {app}")),
        Err(s) if s.code() == tonic::Code::InvalidArgument => {
            steps.push("provider mismatch InvalidArgument".into())
        }
        Err(s) => problems.push(format!("provider mismatch: {:?} {}", s.code(), s.message())),
    }
    Check::new(
        id,
        super::verdict(problems.is_empty()),
        if problems.is_empty() {
            format!(
                "{} ({} byte probe bundle, app {app})",
                steps.join(", "),
                bundle.len()
            )
        } else {
            problems.join("; ")
        },
    )
}

/// Imports a canned Firefox bundle into `home` with the linked importer
/// and faked host effects, and checks what landed.
pub fn import_fixture(home: &std::path::Path) -> Check {
    use cua_teleport_bundle::layout::firefox::{root_for, DEST_PROFILE};
    let id = "teleport.import.fixture";
    if let Err(e) = std::fs::create_dir_all(home) {
        return Check::new(id, Status::Fail, format!("create {}: {e}", home.display()));
    }
    let mut writer = cua_teleport_bundle::bundle::BundleWriter::new(
        Vec::new(),
        "firefox",
        "Firefox",
        cua_teleport_bundle::TransferScope::FullProfile,
    );
    let entries: [(&str, u32, &[u8]); 3] = [
        (
            "firefox/tabs.json",
            0o644,
            br#"["https://example.invalid/"]"#,
        ),
        ("firefox/cookies.sqlite", 0o600, b"SQLite format 3\0doctor"),
        (
            "firefox/sessionstore-backups/recovery.jsonlz4",
            0o600,
            b"mozLz40\0doctor",
        ),
    ];
    for (rel, mode, bytes) in entries {
        if let Err(e) = writer.add_bytes(rel, mode, bytes) {
            return Check::new(id, Status::Fail, format!("bundle {rel}: {e}"));
        }
    }
    let bundle = match writer.finish() {
        Ok(b) => b,
        Err(e) => return Check::new(id, Status::Fail, format!("bundle: {e}")),
    };
    let host = std::sync::Arc::new(cua_spacesd_teleport::FakeHost::new());
    let receiver = cua_spacesd_teleport::Receiver::with_host(home.to_path_buf(), host.clone());
    let outcome = match receiver.import_bundle(Cursor::new(bundle), "firefox", false) {
        Ok(o) => o,
        Err(e) => return Check::new(id, Status::Fail, format!("import: {e}")),
    };
    let profile = home
        .join(root_for(cua_teleport_bundle::Platform::current()))
        .join("Profiles")
        .join(DEST_PROFILE);
    let cookies = std::fs::read(profile.join("cookies.sqlite")).ok();
    let session = std::fs::read(profile.join("sessionstore.jsonlz4")).ok();
    let user_js = std::fs::read_to_string(profile.join("user.js")).unwrap_or_default();
    let mut problems = Vec::new();
    if cookies.as_deref() != Some(&b"SQLite format 3\0doctor"[..]) {
        problems.push("cookies.sqlite did not land in the profile");
    }
    if session.as_deref() != Some(&b"mozLz40\0doctor"[..]) {
        problems.push("the session store was not seeded from the recovery store");
    }
    if !user_js.contains("network.proxy.type") {
        problems.push("user.js without the system-proxy preference");
    }
    if outcome.launched {
        problems.push("launched although launch was not asked");
    }
    Check::new(
        id,
        super::verdict(problems.is_empty()),
        if problems.is_empty() {
            format!(
                "imported {:?} into a throwaway home ({} faked host effect(s))",
                outcome.imported,
                host.calls().len()
            )
        } else {
            problems.join("; ")
        },
    )
}

struct Upload {
    rel: &'static str,
    bytes: Vec<u8>,
}

async fn upload_all(
    teleport: &mut pb::teleport_service_client::TeleportServiceClient<
        cua_spacesd_client::GrpcChannel,
    >,
    transfer_id: &str,
    accepted: &[pb::TransferEntry],
    files: &[Upload],
    chunk: usize,
    resend_first: bool,
) -> Result<bool, String> {
    let mut duplicate_seen = false;
    for (index, entry) in accepted.iter().enumerate() {
        if entry.directory {
            continue;
        }
        let Some(file) = files.iter().find(|f| f.rel == entry.relative_path) else {
            return Err(format!("accepted unknown entry {:?}", entry.relative_path));
        };
        let mut offset = 0usize;
        let mut sends = 0usize;
        while offset < file.bytes.len() {
            let end = (offset + chunk).min(file.bytes.len());
            let request = pb::ReceiveFilesChunkRequest {
                transfer_id: transfer_id.to_owned(),
                index: index as u32,
                offset: offset as u64,
                data: file.bytes[offset..end].to_vec(),
            };
            let r = teleport
                .receive_files_chunk(request.clone())
                .await
                .map_err(|s| format!("{}: chunk at {offset}: {}", file.rel, s.message()))?
                .into_inner();
            if r.received_bytes != end as u64 {
                return Err(format!(
                    "{}: received_bytes {} != {end}",
                    file.rel, r.received_bytes
                ));
            }
            if resend_first && sends == 0 && !duplicate_seen {
                let again = teleport
                    .receive_files_chunk(request)
                    .await
                    .map_err(|s| format!("{}: resent chunk: {}", file.rel, s.message()))?
                    .into_inner();
                if !again.duplicate || again.received_bytes != end as u64 {
                    return Err(format!("{}: a resent chunk was not a duplicate", file.rel));
                }
                duplicate_seen = true;
            }
            offset = end;
            sends += 1;
            if sends > 4096 {
                return Err("too many chunks".into());
            }
        }
    }
    Ok(duplicate_seen)
}

fn entry(rel: &str, bytes: Option<&[u8]>) -> pb::TransferEntry {
    pb::TransferEntry {
        relative_path: rel.to_owned(),
        directory: bytes.is_none(),
        size: bytes.map(|b| b.len() as u64).unwrap_or(0),
        sha256: bytes.map(sha256_hex).unwrap_or_default(),
        mode: if bytes.is_some() { 0o644 } else { 0o755 },
        modified_at: None,
    }
}

/// A file transfer into `Downloads/cua-doctor-<nonce>`, removed afterwards.
pub async fn receive_files(client: &SpacesdClient, nonce: &str) -> Check {
    let id = "teleport.receive_files";
    let subdir = format!("cua-doctor-{nonce}");
    let mut teleport = client.teleport();
    let blob: Vec<u8> = (0..96 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let files = vec![
        Upload {
            rel: "d/a.txt",
            bytes: format!("teleport {nonce}\n").into_bytes(),
        },
        Upload {
            rel: "b.bin",
            bytes: blob,
        },
        Upload {
            rel: "empty",
            bytes: Vec::new(),
        },
        Upload {
            rel: "skip.log",
            bytes: b"ignored".to_vec(),
        },
    ];
    let mut entries = vec![entry("d", None)];
    entries.extend(files.iter().map(|f| entry(f.rel, Some(&f.bytes))));
    let begin =
        |transfer_id: String, entries: Vec<pb::TransferEntry>| pb::BeginReceiveFilesRequest {
            transfer_id,
            destination_subdir: subdir.clone(),
            entries,
            ignore_patterns: vec!["*.log".into()],
            honor_gitignore: false,
            conflict_policy: pb::ConflictPolicy::Rename as i32,
            ttl: Some(pbjson_types::Duration {
                seconds: 300,
                nanos: 0,
            }),
        };
    let mut destination = String::new();
    let result = async {
        // 1. The transfer, with ignore rules and a resent chunk.
        let t1 = format!("cua-doctor-{nonce}-1");
        let begun = teleport
            .begin_receive_files(begin(t1.clone(), entries.clone()))
            .await
            .map_err(|s| format!("BeginReceiveFiles: {}", s.message()))?
            .into_inner();
        if !begun.ignored.iter().any(|p| p == "skip.log") {
            return Err(format!(
                "skip.log not ignored (ignored {:?})",
                begun.ignored
            ));
        }
        if begun.accepted.iter().any(|e| e.relative_path == "skip.log") {
            return Err("an ignored entry was accepted".into());
        }
        let chunk = (begun.max_chunk_bytes as usize).clamp(1024, 32 * 1024);
        let duplicate =
            upload_all(&mut teleport, &t1, &begun.accepted, &files, chunk, true).await?;
        let committed = teleport
            .commit_receive_files(pb::CommitReceiveFilesRequest {
                transfer_id: t1.clone(),
            })
            .await
            .map_err(|s| format!("CommitReceiveFiles: {}", s.message()))?
            .into_inner();
        destination = committed.destination.clone();
        if !destination.ends_with(&subdir) {
            return Err(format!(
                "destination {destination:?} is not the {subdir} subdirectory"
            ));
        }
        if committed.files.len() != 3 {
            return Err(format!("{} files committed, want 3", committed.files.len()));
        }
        // 2. Contents, read back through the files service.
        for received in &committed.files {
            let got = client
                .download(&received.path)
                .await
                .map_err(|e| format!("download {}: {e}", received.path))?;
            // Windows joins the destination with `\` (the relative part
            // keeps the sender's `/`).
            let rel = received
                .path
                .strip_prefix(destination.as_str())
                .map(|rest| rest.trim_start_matches(['/', '\\']))
                .unwrap_or(&received.path)
                .replace('\\', "/");
            let want = files.iter().find(|f| f.rel == rel);
            match want {
                Some(f) if got.as_ref() == f.bytes.as_slice() => {}
                Some(_) => return Err(format!("{rel}: contents differ")),
                None => return Err(format!("unexpected file {}", received.path)),
            }
        }
        // 3. The same file again: renamed, not overwritten.
        let t2 = format!("cua-doctor-{nonce}-2");
        let again = vec![entry("d", None), entry("d/a.txt", Some(&files[0].bytes))];
        let begun2 = teleport
            .begin_receive_files(begin(t2.clone(), again))
            .await
            .map_err(|s| format!("BeginReceiveFiles (conflict): {}", s.message()))?
            .into_inner();
        upload_all(&mut teleport, &t2, &begun2.accepted, &files, chunk, false).await?;
        let renamed = teleport
            .commit_receive_files(pb::CommitReceiveFilesRequest { transfer_id: t2 })
            .await
            .map_err(|s| format!("CommitReceiveFiles (conflict): {}", s.message()))?
            .into_inner();
        let renamed_path = renamed
            .files
            .first()
            .map(|f| f.path.clone())
            .unwrap_or_default();
        if renamed_path.is_empty() || renamed_path.replace('\\', "/").ends_with("/d/a.txt") {
            return Err(format!("conflict not renamed: {renamed_path:?}"));
        }
        // 4. An aborted transfer cannot be committed.
        let t3 = format!("cua-doctor-{nonce}-3");
        teleport
            .begin_receive_files(begin(t3.clone(), vec![entry("c.txt", Some(b"abort"))]))
            .await
            .map_err(|s| format!("BeginReceiveFiles (abort): {}", s.message()))?;
        teleport
            .abort_receive_files(pb::AbortReceiveFilesRequest {
                transfer_id: t3.clone(),
            })
            .await
            .map_err(|s| format!("AbortReceiveFiles: {}", s.message()))?;
        let after_abort = teleport
            .commit_receive_files(pb::CommitReceiveFilesRequest { transfer_id: t3 })
            .await;
        if after_abort.is_ok() {
            return Err("an aborted transfer was committed".into());
        }
        Ok((duplicate, renamed_path))
    }
    .await;
    // Remove what landed, whatever happened.
    let cleaned = if destination.is_empty() {
        true
    } else {
        client.remove(&destination, true).await.is_ok()
    };
    match result {
        Ok((duplicate, renamed)) => Check::new(
            id,
            super::verdict(duplicate && cleaned),
            format!(
                "3 files received into {destination} (skip.log ignored, resent chunk duplicate {duplicate}), \
                 conflict renamed to {}, abort ok, cleanup {}",
                renamed.rsplit(['/', '\\']).next().unwrap_or_default(),
                if cleaned { "ok" } else { "FAILED" }
            ),
        ),
        Err(message) => Check::new(id, Status::Fail, message),
    }
}
