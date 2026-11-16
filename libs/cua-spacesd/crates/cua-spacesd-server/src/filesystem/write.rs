// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Atomic, hashed file writes shared by `WriteFile`, resumable uploads, the
//! `/files` PUT route and teleport file transfers.

use std::path::{Path, PathBuf};

use cua_proto::env::v1::{EntryInfo, ErrorReason, WriteMode};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tonic::{Code, Status};

use crate::error::{io_status, status, StatusBuilder};
use crate::util::{hex_digest, random_id};

use super::offworker::OffWorker;
use super::paths::entry_info;
use super::xattr::{self, Attributes};

/// Parameters of one write.
#[derive(Debug, Clone)]
pub struct WriteTarget {
    /// Final path.
    pub dest: PathBuf,
    /// Existing-file policy.
    pub mode: WriteMode,
    /// Permission bits for a new file (0 = 0o644, or the replaced file's).
    pub permissions: u32,
    /// Create missing parent directories.
    pub create_parents: bool,
    /// Extended attributes to store with the file, and the namespace the
    /// published entry reports them under.
    pub attributes: Attributes,
}

impl WriteTarget {
    /// A target with no metadata.
    pub fn new(dest: PathBuf, mode: WriteMode, permissions: u32, create_parents: bool) -> Self {
        Self {
            dest,
            mode,
            permissions,
            create_parents,
            attributes: Attributes {
                namespace: xattr::DEFAULT_NAMESPACE.to_owned(),
                values: Default::default(),
            },
        }
    }
}

/// An in-progress write. Data goes to a temporary sibling (except in append
/// mode) that is renamed into place by [`AtomicWriter::finish`].
pub struct AtomicWriter {
    target: WriteTarget,
    tmp: Option<PathBuf>,
    file: OffWorker<tokio::fs::File>,
    hasher: Sha256,
    written: u64,
}

fn parent_of(dest: &Path) -> PathBuf {
    dest.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Precondition checks shared by every writer.
pub async fn prepare(target: &WriteTarget) -> Result<(), Status> {
    let parent = parent_of(&target.dest);
    match tokio::fs::metadata(&parent).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::NotADirectory,
                format!("{}: not a directory", parent.display()),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && target.create_parents => {
            tokio::fs::create_dir_all(&parent)
                .await
                .map_err(|e| io_status(&e, &parent))?;
        }
        Err(e) => return Err(io_status(&e, &parent)),
    }
    match tokio::fs::metadata(&target.dest).await {
        Ok(meta) if meta.is_dir() => Err(status(
            Code::FailedPrecondition,
            ErrorReason::IsADirectory,
            format!("{}: is a directory", target.dest.display()),
        )),
        Ok(_) if target.mode == WriteMode::CreateNew => Err(StatusBuilder::new(
            Code::AlreadyExists,
            ErrorReason::PathExists,
            format!("{} already exists", target.dest.display()),
        )
        .meta("path", target.dest.display())
        .build()),
        _ => Ok(()),
    }
}

/// Looks-ups of a just-published file before giving up on them.
const STAT_RETRIES: u32 = 5;
const STAT_RETRY_WAIT: std::time::Duration = std::time::Duration::from_millis(40);

/// The metadata of a file that was just renamed into place.
///
/// On a network mount (the Cua Volume's NFS mount on macOS) the client can
/// answer "not found" for the new name for a moment after the rename, though
/// the file is there. The write already succeeded, so a miss is retried a few
/// times and then answered from the file's own metadata, rather than failing
/// a write whose data is on disk.
async fn stat_published<F, Fut>(
    mut stat: F,
    fallback: Option<std::fs::Metadata>,
    retries: u32,
    wait: std::time::Duration,
) -> std::io::Result<std::fs::Metadata>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<std::fs::Metadata>>,
{
    let mut attempt = 0;
    loop {
        match stat().await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if attempt >= retries {
                    return fallback.ok_or(e);
                }
                attempt += 1;
                tokio::time::sleep(wait).await;
            }
            other => return other,
        }
    }
}

impl AtomicWriter {
    /// Validates the target and opens the temporary file.
    pub async fn create(target: WriteTarget) -> Result<Self, Status> {
        prepare(&target).await?;
        let mut options = tokio::fs::OpenOptions::new();
        let (path, tmp) = if target.mode == WriteMode::Append {
            options.append(true).create(true);
            (target.dest.clone(), None)
        } else {
            let name = target
                .dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let tmp = parent_of(&target.dest).join(format!(".{name}.cua-{}.part", random_id(6)));
            options.write(true).create_new(true);
            (tmp.clone(), Some(tmp))
        };
        #[cfg(unix)]
        options.mode(if target.permissions == 0 {
            0o644
        } else {
            target.permissions
        });
        let file = OffWorker::new(
            options
                .open(&path)
                .await
                .map_err(|e| io_status(&e, &path))?,
        );
        if !target.attributes.values.is_empty() {
            #[cfg(unix)]
            let stored = {
                use std::os::fd::AsRawFd as _;
                let (fd, attributes) = (file.as_raw_fd(), target.attributes.clone());
                tokio::task::spawn_blocking(move || xattr::set_on_fd(fd, &attributes))
                    .await
                    .unwrap_or_else(|e| Err(crate::error::internal(e.to_string())))
            };
            #[cfg(not(unix))]
            let stored = xattr::set_on_fd(0, &target.attributes);
            if let Err(error) = stored {
                file.close().await;
                if let Some(tmp) = &tmp {
                    let _ = tokio::fs::remove_file(tmp).await;
                }
                return Err(error);
            }
        }
        Ok(Self {
            target,
            tmp,
            file,
            hasher: Sha256::new(),
            written: 0,
        })
    }

    /// Bytes written so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Appends bytes.
    pub async fn write(&mut self, data: &[u8]) -> Result<(), Status> {
        let shown = self.tmp.clone().unwrap_or_else(|| self.target.dest.clone());
        self.file
            .write_all(data)
            .await
            .map_err(|e| io_status(&e, &shown))?;
        self.hasher.update(data);
        self.written += data.len() as u64;
        Ok(())
    }

    /// Hex SHA-256 of everything written so far (does not consume).
    pub fn sha256_so_far(&self) -> String {
        hex_digest(self.hasher.clone().finalize())
    }

    /// Verifies size and digest, then publishes atomically. On a mismatch
    /// the data is discarded.
    pub async fn finish(
        mut self,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(EntryInfo, String), Status> {
        let dest = self.target.dest.clone();
        self.file.flush().await.map_err(|e| io_status(&e, &dest))?;
        self.file
            .sync_all()
            .await
            .map_err(|e| io_status(&e, &dest))?;
        let digest = hex_digest(self.hasher.clone().finalize());
        let written = self.written;
        if expected_size != 0 && expected_size != written {
            self.abort().await;
            return Err(StatusBuilder::new(
                Code::FailedPrecondition,
                ErrorReason::ChecksumMismatch,
                format!("expected {expected_size} bytes, received {written}"),
            )
            .meta("path", dest.display())
            .build());
        }
        if !expected_sha256.is_empty() && !expected_sha256.eq_ignore_ascii_case(&digest) {
            self.abort().await;
            return Err(StatusBuilder::new(
                Code::FailedPrecondition,
                ErrorReason::ChecksumMismatch,
                format!("sha256 mismatch: expected {expected_sha256}, got {digest}"),
            )
            .meta("path", dest.display())
            .build());
        }
        // The open file's own metadata, taken before it is renamed: what the
        // entry reports if the destination cannot be looked up right after.
        let published_meta = self.file.metadata().await.ok();
        // Closed on a blocking thread (the destination can be on the Cua
        // Volume), before the rename.
        self.file.close_in_place().await;
        if let Some(tmp) = self.tmp.take() {
            let published = match self.target.mode {
                WriteMode::CreateNew => {
                    // link(2) fails if the destination appeared meanwhile.
                    let result = tokio::fs::hard_link(&tmp, &dest).await;
                    let _ = tokio::fs::remove_file(&tmp).await;
                    result
                }
                _ => {
                    #[cfg(unix)]
                    if self.target.permissions == 0 {
                        // Keep the replaced file's permissions.
                        if let Ok(meta) = tokio::fs::metadata(&dest).await {
                            let _ = tokio::fs::set_permissions(&tmp, meta.permissions()).await;
                        }
                    }
                    let result = tokio::fs::rename(&tmp, &dest).await;
                    if result.is_err() {
                        let _ = tokio::fs::remove_file(&tmp).await;
                    }
                    result
                }
            };
            published.map_err(|e| io_status(&e, &dest))?;
        }
        let meta = stat_published(
            || tokio::fs::metadata(&dest),
            published_meta,
            STAT_RETRIES,
            STAT_RETRY_WAIT,
        )
        .await
        .map_err(|e| io_status(&e, &dest))?;
        let mut entry = entry_info(&dest, &meta);
        let (read_path, namespace) = (dest.clone(), self.target.attributes.namespace.clone());
        entry.metadata =
            tokio::task::spawn_blocking(move || xattr::read(&read_path, &namespace, true))
                .await
                .unwrap_or_default();
        Ok((entry, digest))
    }

    /// Discards the temporary file.
    pub async fn abort(mut self) {
        self.file.close_in_place().await;
        if let Some(tmp) = self.tmp {
            let _ = tokio::fs::remove_file(tmp).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dest: PathBuf, mode: WriteMode) -> WriteTarget {
        WriteTarget::new(dest, mode, 0, true)
    }

    #[tokio::test]
    async fn a_just_published_file_that_is_not_found_yet_is_retried_then_answered() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("f");
        std::fs::write(&real, b"x").unwrap();
        let meta = std::fs::metadata(&real).unwrap();
        let wait = std::time::Duration::from_millis(1);
        let missing = || std::io::Error::from(std::io::ErrorKind::NotFound);
        // A miss that clears is retried.
        let calls = std::cell::Cell::new(0);
        let got = stat_published(
            || {
                calls.set(calls.get() + 1);
                let r = if calls.get() < 3 {
                    Err(missing())
                } else {
                    Ok(meta.clone())
                };
                async move { r }
            },
            None,
            5,
            wait,
        )
        .await;
        assert!(got.is_ok());
        assert_eq!(calls.get(), 3);
        // A miss that stays is answered from the file's own metadata.
        let got = stat_published(|| async { Err(missing()) }, Some(meta.clone()), 2, wait).await;
        assert_eq!(got.unwrap().len(), 1);
        // With nothing to fall back on, the error is still the error.
        let got = stat_published(|| async { Err(missing()) }, None, 2, wait).await;
        assert_eq!(got.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        // Other errors are not retried.
        let calls = std::cell::Cell::new(0);
        let got = stat_published(
            || {
                calls.set(calls.get() + 1);
                async { Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)) }
            },
            Some(meta),
            5,
            wait,
        )
        .await;
        assert_eq!(
            got.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn writes_atomically_and_verifies_sha() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub/f.bin");
        let mut w = AtomicWriter::create(target(dest.clone(), WriteMode::Overwrite))
            .await
            .unwrap();
        w.write(b"hello ").await.unwrap();
        assert!(!dest.exists(), "not visible before commit");
        w.write(b"world").await.unwrap();
        let sha = hex_digest(Sha256::digest(b"hello world"));
        let (entry, digest) = w.finish(11, &sha).await.unwrap();
        assert_eq!(digest, sha);
        assert_eq!(entry.size, 11);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn sha_mismatch_discards_everything() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");
        let mut w = AtomicWriter::create(target(dest.clone(), WriteMode::Overwrite))
            .await
            .unwrap();
        w.write(b"data").await.unwrap();
        let error = w.finish(0, &"0".repeat(64)).await.unwrap_err();
        assert_eq!(
            crate::error::error_info(&error).unwrap().reason,
            ErrorReason::ChecksumMismatch as i32
        );
        assert!(!dest.exists());
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "temp removed"
        );
    }

    #[tokio::test]
    async fn create_new_refuses_existing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f");
        std::fs::write(&dest, b"x").unwrap();
        let error = AtomicWriter::create(target(dest, WriteMode::CreateNew))
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), Code::AlreadyExists);
    }

    #[tokio::test]
    async fn append_mode_appends() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("log");
        std::fs::write(&dest, b"a").unwrap();
        let mut w = AtomicWriter::create(target(dest.clone(), WriteMode::Append))
            .await
            .unwrap();
        w.write(b"b").await.unwrap();
        w.finish(0, "").await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"ab");
    }

    #[tokio::test]
    async fn metadata_is_published_with_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let mut t = target(dest.clone(), WriteMode::Overwrite);
        t.attributes = Attributes::from_request(
            "user.e2b.",
            &[("k".to_owned(), "v".to_owned())].into_iter().collect(),
        )
        .unwrap();
        let mut w = match AtomicWriter::create(t).await {
            Ok(w) => w,
            // A file system without user.* attributes (some tmpfs).
            Err(e) if e.code() == Code::FailedPrecondition => return,
            Err(e) => panic!("{e}"),
        };
        w.write(b"x").await.unwrap();
        let (entry, _) = w.finish(0, "").await.unwrap();
        assert_eq!(entry.metadata.get("k").map(String::as_str), Some("v"));
        assert_eq!(
            xattr::read(&dest, "user.e2b.", true)
                .get("k")
                .map(String::as_str),
            Some("v")
        );
    }
}
