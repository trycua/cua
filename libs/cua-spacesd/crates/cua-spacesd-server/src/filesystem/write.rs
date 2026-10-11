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

use super::paths::entry_info;

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
}

/// An in-progress write. Data goes to a temporary sibling (except in append
/// mode) that is renamed into place by [`AtomicWriter::finish`].
pub struct AtomicWriter {
    target: WriteTarget,
    tmp: Option<PathBuf>,
    file: tokio::fs::File,
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
        let file = options
            .open(&path)
            .await
            .map_err(|e| io_status(&e, &path))?;
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
        if let Some(tmp) = self.tmp.take() {
            drop(self.file);
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
        let meta = tokio::fs::metadata(&dest)
            .await
            .map_err(|e| io_status(&e, &dest))?;
        Ok((entry_info(&dest, &meta), digest))
    }

    /// Discards the temporary file.
    pub async fn abort(self) {
        drop(self.file);
        if let Some(tmp) = self.tmp {
            let _ = tokio::fs::remove_file(tmp).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dest: PathBuf, mode: WriteMode) -> WriteTarget {
        WriteTarget {
            dest,
            mode,
            permissions: 0,
            create_parents: true,
        }
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
}
