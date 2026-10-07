//! Moving files between this host and a Space.
//!
//! - [`Space::upload`] / [`Space::download`]: arbitrary paths, through
//!   `FilesystemService` (chunked, resumable, SHA-256 verified).
//! - [`Space::send_file`]: the Teleport drop-zone transfer, through
//!   `TeleportService.BeginReceiveFiles` / `ReceiveFilesChunk` /
//!   `CommitReceiveFiles`. Files land in the Space user's `~/Downloads`, every
//!   file is verified by SHA-256 on the guest before the transfer is
//!   published, and folders honor ignore files ([`crate::walk`]).
//!
//! None of these has the old 25 MiB cap: nothing is base64'd into a shell.

use crate::error::{Error, Result};
use crate::space::Space;
use crate::walk::{self, WalkEntry};
use cua_spacesd_client::{DownloadOptions, UploadOptions, pb};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Largest chunk sent to `ReceiveFilesChunk` regardless of what the driver
/// allows (bounds memory per in-flight chunk).
pub const MAX_SEND_CHUNK: usize = 4 * 1024 * 1024;
/// Chunk used when the driver does not say.
pub const DEFAULT_SEND_CHUNK: usize = 1024 * 1024;

/// What happens when a sent file already exists in the Space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conflict {
    /// Replace it. Opt-in: nothing is overwritten unless asked.
    Overwrite,
    /// Keep both: the new file becomes `name (1).ext` (the default).
    #[default]
    Rename,
    /// Keep the existing file.
    Skip,
}

impl Conflict {
    fn to_pb(self) -> pb::ConflictPolicy {
        match self {
            Conflict::Overwrite => pb::ConflictPolicy::Overwrite,
            Conflict::Rename => pb::ConflictPolicy::Rename,
            Conflict::Skip => pb::ConflictPolicy::Skip,
        }
    }
}

/// Options for [`Space::send_file`].
#[derive(Clone, Debug)]
pub struct SendFileOptions {
    /// Subdirectory of `~/Downloads` (`""` = Downloads itself).
    pub subdir: String,
    /// Honor ignore files when sending a folder. Default true.
    pub respect_ignore_files: bool,
    /// Existing-file policy.
    pub conflict: Conflict,
}

impl Default for SendFileOptions {
    fn default() -> Self {
        Self {
            subdir: String::new(),
            respect_ignore_files: true,
            conflict: Conflict::default(),
        }
    }
}

/// One file placed by [`Space::send_file`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SentFile {
    /// Final absolute guest path.
    pub path: String,
    /// Bytes.
    pub size: u64,
    /// SHA-256, verified by the guest and matched against this host's.
    pub sha256: String,
}

/// The result of [`Space::send_file`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct SendFileReport {
    /// Host source.
    pub source: String,
    /// Absolute guest path of the transfer root (`~/Downloads[/subdir]`).
    pub destination: String,
    /// Where the file or folder itself landed.
    pub dest: String,
    /// `file` or `folder`.
    pub kind: &'static str,
    /// Files placed.
    pub files: Vec<SentFile>,
    /// Total bytes.
    pub bytes: u64,
    /// Whether ignore files were honored (folders only).
    pub respect_ignorefiles: bool,
    /// Paths left out by ignore rules (first 200).
    pub skipped_by_ignorefiles: Vec<String>,
    /// How many were left out.
    pub skipped_count: usize,
    /// Paths the guest skipped (conflict policy `skip`).
    pub skipped_by_guest: Vec<String>,
    /// Every file's guest SHA-256 matched this host's.
    pub verified: bool,
}

/// The result of [`Space::upload`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct UploadReport {
    /// Host source.
    pub source: String,
    /// Guest destination.
    pub dest: String,
    /// `file` or `folder`.
    pub kind: &'static str,
    /// Files written.
    pub files: usize,
    /// Directories created.
    pub directories: usize,
    /// Total bytes.
    pub bytes: u64,
    /// SHA-256 of the file (single-file uploads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Every file verified by SHA-256 against the driver's.
    pub verified: bool,
}

/// The result of [`Space::download`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct DownloadReport {
    /// Guest source.
    pub source: String,
    /// Host destination.
    pub dest: String,
    /// `file` or `folder`.
    pub kind: &'static str,
    /// Files written.
    pub files: usize,
    /// Total bytes.
    pub bytes: u64,
    /// SHA-256 of the file (single-file downloads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Every segment verified by SHA-256 against the driver's.
    pub verified: bool,
}

async fn sha256_file(path: &Path) -> Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| Error::invalid(format!("{} has no file name", path.display())))
}

fn timestamp(t: Option<std::time::SystemTime>) -> Option<pbjson_types::Timestamp> {
    let d = t?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(pbjson_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    })
}

/// Normalizes a `send_file` target to a Downloads subdirectory: accepts
/// `None`/`""`, `~/Downloads[/sub]`, `<home>/Downloads[/sub]` (when `home` is
/// known) and a relative `sub/dir`. Anything else is refused: `send_file`
/// only ever writes under Downloads (use `upload` for arbitrary paths).
pub fn downloads_subdir(target: Option<&str>, home: Option<&str>) -> Result<String> {
    let t = target.unwrap_or("").trim().trim_end_matches('/');
    let rest = if t.is_empty() {
        ""
    } else if let Some(r) = t.strip_prefix("~/Downloads") {
        if !r.is_empty() && !r.starts_with('/') {
            return Err(outside(t));
        }
        r.trim_start_matches('/')
    } else if let Some(r) = home
        .map(|h| format!("{}/Downloads", h.trim_end_matches('/')))
        .and_then(|d| t.strip_prefix(&d).map(str::to_string))
    {
        if !r.is_empty() && !r.starts_with('/') {
            return Err(outside(t));
        }
        return check_relative(r.trim_start_matches('/'), t);
    } else if t.starts_with('/') || t.starts_with('~') || t.contains(':') || t.starts_with('\\') {
        return Err(outside(t));
    } else {
        t
    };
    check_relative(rest, t)
}

fn check_relative(rest: &str, original: &str) -> Result<String> {
    if rest.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(Error::invalid(format!(
            "target_directory {original:?} must not contain `.` or `..`"
        )));
    }
    Ok(rest.to_string())
}

fn outside(t: &str) -> Error {
    Error::invalid(format!(
        "send_file lands in the Space user's ~/Downloads; {t:?} is outside it. Pass a \
         subdirectory (for example \"inbox\"), or use `upload` with `dest` for an arbitrary path"
    ))
}

/// A path separator in a guest path: `/`, and on a Windows Space also `\\`
/// (a Linux or macOS file name may contain a backslash).
fn is_guest_sep(c: char, windows: bool) -> bool {
    c == '/' || (windows && c == '\\')
}

/// The last component of a guest path, the name a download takes on the
/// host; `root` for a filesystem or drive root.
fn guest_file_name(remote: &str, windows: bool) -> &str {
    remote
        .rsplit(|c| is_guest_sep(c, windows))
        .next()
        .filter(|n| !n.is_empty() && !(windows && n.ends_with(':')))
        .unwrap_or("root")
}

impl Space {
    /// Sends a host file or folder into `~/Downloads[/subdir]` through
    /// TeleportService, verified file by file.
    pub async fn send_file(
        &self,
        local: &Path,
        options: SendFileOptions,
    ) -> Result<SendFileReport> {
        let meta = tokio::fs::metadata(local)
            .await
            .map_err(|e| Error::invalid(format!("host path {}: {e}", local.display())))?;
        let name = file_name(local)?;
        let (entries, skipped, kind) = if meta.is_dir() {
            let root = local.to_path_buf();
            let respect = options.respect_ignore_files;
            let walked = tokio::task::spawn_blocking(move || walk::walk(&root, respect))
                .await
                .map_err(|e| Error::Transfer(e.to_string()))??;
            let mut entries = vec![WalkEntry {
                path: local.to_path_buf(),
                rel: name.clone(),
                is_dir: true,
                size: 0,
                mode: 0,
                modified: None,
            }];
            entries.extend(walked.entries.into_iter().map(|mut e| {
                e.rel = format!("{name}/{}", e.rel);
                e
            }));
            (entries, walked.skipped, "folder")
        } else {
            (vec![walk::single(local, &name)?], vec![], "file")
        };

        // Host digests first: the guest checks every file against them.
        let mut declared = Vec::with_capacity(entries.len());
        let mut host_sha = std::collections::HashMap::new();
        for e in &entries {
            let sha = if e.is_dir {
                String::new()
            } else {
                let s = sha256_file(&e.path).await?;
                host_sha.insert(e.rel.clone(), s.clone());
                s
            };
            declared.push(pb::TransferEntry {
                relative_path: e.rel.clone(),
                directory: e.is_dir,
                size: e.size,
                sha256: sha,
                mode: e.mode & 0o777,
                modified_at: timestamp(e.modified),
            });
        }

        let transfer_id = format!("cua-send-{:016x}", rand::random::<u64>());
        let mut teleport = self.spacesd()?.teleport();
        let begun = teleport
            .begin_receive_files(pb::BeginReceiveFilesRequest {
                transfer_id: transfer_id.clone(),
                destination_subdir: options.subdir.clone(),
                entries: declared,
                ignore_patterns: vec![],
                honor_gitignore: false,
                conflict_policy: options.conflict.to_pb() as i32,
                ttl: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let result = self.send_accepted(&transfer_id, &begun, &entries).await;
        if let Err(e) = result {
            let _ = teleport
                .abort_receive_files(pb::AbortReceiveFilesRequest {
                    transfer_id: transfer_id.clone(),
                })
                .await;
            return Err(e);
        }
        let committed = match teleport
            .commit_receive_files(pb::CommitReceiveFilesRequest {
                transfer_id: transfer_id.clone(),
            })
            .await
        {
            Ok(r) => r.into_inner(),
            Err(status) => {
                let _ = teleport
                    .abort_receive_files(pb::AbortReceiveFilesRequest { transfer_id })
                    .await;
                return Err(Error::Transfer(format!(
                    "the Space refused to commit the transfer: {}",
                    cua_spacesd_client::Error::from(status)
                )));
            }
        };

        // Verify: every accepted file must have landed with our digest.
        let accepted_files: Vec<&pb::TransferEntry> =
            begun.accepted.iter().filter(|e| !e.directory).collect();
        let mut files = Vec::new();
        let mut bytes = 0;
        let mut unmatched: Vec<String> = Vec::new();
        for e in &accepted_files {
            let want = host_sha.get(&e.relative_path).cloned().unwrap_or_default();
            if committed.skipped.iter().any(|s| s == &e.relative_path) {
                continue;
            }
            match committed.files.iter().find(|f| {
                f.sha256.eq_ignore_ascii_case(&want)
                    && f.size == e.size
                    && !files.iter().any(|g: &SentFile| g.path == f.path)
            }) {
                Some(f) => {
                    bytes += f.size;
                    files.push(SentFile {
                        path: f.path.clone(),
                        size: f.size,
                        sha256: f.sha256.clone(),
                    });
                }
                None => unmatched.push(e.relative_path.clone()),
            }
        }
        if !unmatched.is_empty() {
            return Err(Error::Transfer(format!(
                "{} file(s) did not land intact in the Space: {}",
                unmatched.len(),
                unmatched.join(", ")
            )));
        }
        let mut all_skipped = skipped;
        all_skipped.extend(begun.ignored.iter().cloned());
        let dest = format!("{}/{}", committed.destination.trim_end_matches('/'), name);
        Ok(SendFileReport {
            source: local.display().to_string(),
            destination: committed.destination,
            dest: if kind == "file" {
                files.first().map(|f| f.path.clone()).unwrap_or(dest)
            } else {
                dest
            },
            kind,
            files,
            bytes,
            respect_ignorefiles: options.respect_ignore_files,
            skipped_count: all_skipped.len(),
            skipped_by_ignorefiles: all_skipped.into_iter().take(200).collect(),
            skipped_by_guest: committed.skipped,
            verified: true,
        })
    }

    async fn send_accepted(
        &self,
        transfer_id: &str,
        begun: &pb::BeginReceiveFilesResponse,
        entries: &[WalkEntry],
    ) -> Result<()> {
        let chunk = match begun.max_chunk_bytes as usize {
            0 => DEFAULT_SEND_CHUNK,
            n => n.min(MAX_SEND_CHUNK),
        };
        let mut teleport = self.spacesd()?.teleport();
        for (index, accepted) in begun.accepted.iter().enumerate() {
            if accepted.directory || accepted.size == 0 {
                continue;
            }
            let source = entries
                .iter()
                .find(|e| e.rel == accepted.relative_path)
                .ok_or_else(|| {
                    Error::Transfer(format!(
                        "the Space accepted {:?}, which was never declared",
                        accepted.relative_path
                    ))
                })?;
            let mut offset = begun
                .progress
                .iter()
                .find(|p| p.index as usize == index)
                .map(|p| p.received_bytes)
                .unwrap_or(0);
            let mut file = tokio::fs::File::open(&source.path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let mut buf = vec![0u8; chunk];
            // Bounded by the declared size: each pass advances `offset`.
            let max_chunks = accepted.size.div_ceil(chunk as u64) + 1;
            for _ in 0..max_chunks {
                if offset >= accepted.size {
                    break;
                }
                let want = ((accepted.size - offset) as usize).min(chunk);
                let mut filled = 0;
                while filled < want {
                    let n = file.read(&mut buf[filled..want]).await?;
                    if n == 0 {
                        return Err(Error::Transfer(format!(
                            "{} shrank while it was being sent",
                            source.path.display()
                        )));
                    }
                    filled += n;
                }
                let resp = teleport
                    .receive_files_chunk(pb::ReceiveFilesChunkRequest {
                        transfer_id: transfer_id.into(),
                        index: index as u32,
                        offset,
                        data: buf[..filled].to_vec(),
                    })
                    .await
                    .map_err(cua_spacesd_client::Error::from)?
                    .into_inner();
                offset = resp.received_bytes;
            }
            if offset != accepted.size {
                return Err(Error::Transfer(format!(
                    "{}: the Space acknowledged {offset} of {} bytes",
                    accepted.relative_path, accepted.size
                )));
            }
        }
        Ok(())
    }

    /// Copies a host file or folder to `dest` in the Space, through
    /// FilesystemService. An explicit `dest` is written as given (replacing
    /// a file there); with no `dest` it lands at `<home>/<name>`, or at the
    /// first free `<home>/<name> (n).ext` when that is taken.
    pub async fn upload(&self, local: &Path, dest: Option<&str>) -> Result<UploadReport> {
        let meta = tokio::fs::metadata(local)
            .await
            .map_err(|e| Error::invalid(format!("host path {}: {e}", local.display())))?;
        let name = file_name(local)?;
        let dest = match dest {
            Some(d) if !d.trim().is_empty() => d.trim_end_matches('/').to_string(),
            // No destination named: never replace something already in the
            // Space's home; take the first free `name (n).ext` instead.
            _ => {
                let home = self.home().await?;
                let env = self.spacesd()?;
                let mut chosen = None;
                for candidate in keep_both_names(&name, meta.is_dir()).take(1000) {
                    let path = format!("{home}/{candidate}");
                    match env.stat(&path).await {
                        Err(cua_spacesd_client::Error::PathNotFound(_)) => {
                            chosen = Some(path);
                            break;
                        }
                        Ok(_) => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
                chosen.ok_or_else(|| {
                    Error::invalid(format!(
                        "{home}/{name} and its numbered variants all exist; pass `dest`"
                    ))
                })?
            }
        };
        let opts = || UploadOptions {
            mode: pb::WriteMode::Overwrite,
            create_parents: true,
            ..Default::default()
        };
        if !meta.is_dir() {
            let res = self
                .spacesd()?
                .upload(&dest, PathBuf::from(local), opts())
                .await?;
            return Ok(UploadReport {
                source: local.display().to_string(),
                dest,
                kind: "file",
                files: 1,
                directories: 0,
                bytes: res.size,
                sha256: Some(res.sha256),
                verified: true,
            });
        }
        let root = local.to_path_buf();
        let walked = tokio::task::spawn_blocking(move || walk::walk(&root, false))
            .await
            .map_err(|e| Error::Transfer(e.to_string()))??;
        self.spacesd()?.make_dir(&dest).await?;
        let (mut files, mut dirs, mut bytes) = (0, 1, 0);
        for e in &walked.entries {
            let target = format!("{dest}/{}", e.rel);
            if e.is_dir {
                self.spacesd()?.make_dir(&target).await?;
                dirs += 1;
            } else {
                let res = self
                    .spacesd()?
                    .upload(&target, e.path.clone(), opts())
                    .await?;
                files += 1;
                bytes += res.size;
            }
        }
        Ok(UploadReport {
            source: local.display().to_string(),
            dest,
            kind: "folder",
            files,
            directories: dirs,
            bytes,
            sha256: None,
            verified: true,
        })
    }

    /// Copies a file or folder out of the Space into `dest_dir` on this host.
    /// Nothing on the host is overwritten: when `dest_dir/<name>` exists the
    /// copy lands at `<name> (1).ext` (or `(2)`, ...), and the report names
    /// the path used. To replace one exact host file, use
    /// `SpacesdClient::download_to_file`.
    pub async fn download(&self, remote: &str, dest_dir: &Path) -> Result<DownloadReport> {
        let windows = self.is_windows();
        let remote = if remote.len() > 1 {
            remote.trim_end_matches(|c| is_guest_sep(c, windows))
        } else {
            remote
        };
        let entry = self.spacesd()?.stat(remote).await?;
        let name = guest_file_name(remote, windows);
        tokio::fs::create_dir_all(dest_dir).await?;
        let is_dir = entry.r#type == pb::FileType::Directory as i32;
        if !is_dir {
            let out = keep_both_path(&dest_dir.join(name));
            let res = self
                .spacesd()?
                .download_to_file(remote, &out, DownloadOptions::default())
                .await?;
            return Ok(DownloadReport {
                source: remote.into(),
                dest: out.display().to_string(),
                kind: "file",
                files: 1,
                bytes: res.size,
                sha256: Some(res.sha256),
                verified: true,
            });
        }
        let root = keep_both_path(&dest_dir.join(name));
        tokio::fs::create_dir_all(&root).await?;
        let listing = self.spacesd()?.list_dir(remote, 64).await?;
        let (mut files, mut bytes) = (0, 0);
        for e in &listing {
            let Some(rel) = e
                .path
                .strip_prefix(remote)
                .and_then(|r| r.strip_prefix(|c| is_guest_sep(c, windows)))
            else {
                continue;
            };
            let parts: Vec<&str> = rel.split(|c| is_guest_sep(c, windows)).collect();
            if parts.iter().any(|s| *s == ".." || s.is_empty()) {
                continue;
            }
            let local = parts.iter().fold(root.clone(), |p, s| p.join(s));
            if e.r#type == pb::FileType::Directory as i32 {
                tokio::fs::create_dir_all(&local).await?;
            } else if e.r#type == pb::FileType::File as i32 {
                if let Some(parent) = local.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let res = self
                    .spacesd()?
                    .download_to_file(&e.path, &local, DownloadOptions::default())
                    .await?;
                files += 1;
                bytes += res.size;
            }
        }
        Ok(DownloadReport {
            source: remote.into(),
            dest: root.display().to_string(),
            kind: "folder",
            files,
            bytes,
            sha256: None,
            verified: true,
        })
    }
}

/// `name`, then `stem (1).ext`, `stem (2).ext`, ... (folders: `name (n)`).
fn keep_both_names(name: &str, is_dir: bool) -> impl Iterator<Item = String> + '_ {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if !is_dir && i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    std::iter::once(name.to_string()).chain((1u32..).map(move |n| format!("{stem} ({n}){ext}")))
}

/// `path` when nothing is there, else the first free `stem (n).ext` beside
/// it (directories get `name (n)`), so a download never replaces host data.
fn keep_both_path(path: &Path) -> PathBuf {
    let taken = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if !taken(path) {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let is_dir = std::fs::metadata(path).is_ok_and(|m| m.is_dir());
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match (is_dir, path.file_stem(), path.extension()) {
        (false, Some(stem), Some(ext)) if !stem.is_empty() => (
            stem.to_string_lossy().into_owned(),
            format!(".{}", ext.to_string_lossy()),
        ),
        _ => (file_name, String::new()),
    };
    for n in 1..10_000u32 {
        let candidate = parent.join(format!("{stem} ({n}){ext}"));
        if !taken(&candidate) {
            return candidate;
        }
    }
    parent.join(format!("{stem} ({:016x}){ext}", rand::random::<u64>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_named_by_the_guest_path_last_component() {
        assert_eq!(guest_file_name("/home/u/py/note.txt", false), "note.txt");
        assert_eq!(guest_file_name(r"C:\Users\u\py\note.txt", true), "note.txt");
        assert_eq!(guest_file_name("C:/Users/u/note.txt", true), "note.txt");
        // A backslash is part of a Linux file name.
        assert_eq!(guest_file_name(r"/tmp/a\b", false), r"a\b");
        assert_eq!(guest_file_name("/", false), "root");
        assert_eq!(guest_file_name("C:", true), "root");
    }

    #[test]
    fn downloads_never_reuse_an_existing_host_path() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("report.pdf");
        assert_eq!(keep_both_path(&f), f);
        std::fs::write(&f, b"mine").unwrap();
        assert_eq!(keep_both_path(&f), dir.path().join("report (1).pdf"));
        std::fs::write(dir.path().join("report (1).pdf"), b"x").unwrap();
        assert_eq!(keep_both_path(&f), dir.path().join("report (2).pdf"));

        let d = dir.path().join("proj.v2");
        std::fs::create_dir(&d).unwrap();
        assert_eq!(keep_both_path(&d), dir.path().join("proj.v2 (1)"));

        let dot = dir.path().join(".env");
        std::fs::write(&dot, b"x").unwrap();
        assert_eq!(keep_both_path(&dot), dir.path().join(".env (1)"));

        // A dangling symlink counts as taken: it is never written through.
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
            assert_eq!(keep_both_path(&link), dir.path().join("link (1).txt"));
        }
        assert_eq!(std::fs::read(&f).unwrap(), b"mine");
    }

    #[test]
    fn default_upload_names_never_reuse_the_original() {
        let n: Vec<_> = keep_both_names("report.pdf", false).take(3).collect();
        assert_eq!(n, ["report.pdf", "report (1).pdf", "report (2).pdf"]);
        let n: Vec<_> = keep_both_names("proj.v2", true).take(2).collect();
        assert_eq!(n, ["proj.v2", "proj.v2 (1)"]);
        let n: Vec<_> = keep_both_names(".env", false).take(2).collect();
        assert_eq!(n, [".env", ".env (1)"]);
    }

    #[test]
    fn sent_files_keep_both_unless_overwrite_is_requested() {
        assert_eq!(Conflict::default(), Conflict::Rename);
        assert_eq!(SendFileOptions::default().conflict, Conflict::Rename);
        assert_eq!(Conflict::Overwrite.to_pb(), pb::ConflictPolicy::Overwrite);
    }

    #[test]
    fn targets_normalize_to_a_downloads_subdir() {
        assert_eq!(downloads_subdir(None, None).unwrap(), "");
        assert_eq!(downloads_subdir(Some("~/Downloads"), None).unwrap(), "");
        assert_eq!(downloads_subdir(Some("~/Downloads/"), None).unwrap(), "");
        assert_eq!(
            downloads_subdir(Some("~/Downloads/in/box"), None).unwrap(),
            "in/box"
        );
        assert_eq!(downloads_subdir(Some("inbox"), None).unwrap(), "inbox");
        assert_eq!(
            downloads_subdir(Some("/home/cua/Downloads/x"), Some("/home/cua")).unwrap(),
            "x"
        );
        for bad in [
            "/etc",
            "~/Desktop",
            "~/DownloadsX",
            "../x",
            "a/../b",
            "C:\\x",
        ] {
            assert!(
                downloads_subdir(Some(bad), Some("/home/cua")).is_err(),
                "{bad}"
            );
        }
    }
}
