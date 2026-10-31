// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Atomic, hashed file writes shared by `WriteFile`, resumable uploads, the
//! `/files` PUT route and teleport file transfers.

use std::path::{Path, PathBuf};

use cua_proto::env::v1::{EntryInfo, ErrorReason, WriteMode};
use sha2::{Digest, Sha256};
use std::io::SeekFrom;

use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tonic::{Code, Status};

use crate::error::{io_status, status, StatusBuilder};
use crate::util::{hex_digest, random_id};

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
        if !target.attributes.values.is_empty() {
            #[cfg(unix)]
            let stored = {
                use std::os::fd::AsRawFd as _;
                xattr::set_on_fd(file.as_raw_fd(), &target.attributes)
            };
            #[cfg(not(unix))]
            let stored = xattr::set_on_fd(0, &target.attributes);
            if let Err(error) = stored {
                drop(file);
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
    ///
    /// A `tokio::fs::File` write is buffered and flushed on a background
    /// thread, so an I/O error from one `write_all` can surface only on the
    /// next call, and a failing write can still leave part of its bytes on
    /// disk. If that went unnoticed, `self.written` and the running hash
    /// would count bytes the file never durably held, and `finish` would
    /// publish a short or duplicated file while reporting the expected
    /// size and checksum.
    ///
    /// So each write is flushed here, where its own error is charged to it,
    /// and on any error the file is cut back to the last fully written
    /// offset. A resumed upload then resends from `written` onto a clean
    /// boundary instead of past a partial tail.
    pub async fn write(&mut self, data: &[u8]) -> Result<(), Status> {
        let shown = self.tmp.clone().unwrap_or_else(|| self.target.dest.clone());
        if let Err(e) = self.write_flushed(data).await {
            self.restore_to_written().await;
            return Err(io_status(&e, &shown));
        }
        self.hasher.update(data);
        self.written += data.len() as u64;
        Ok(())
    }

    async fn write_flushed(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.file.write_all(data).await?;
        self.file.flush().await
    }

    /// Drop any bytes written past `self.written` after a failed write, so
    /// the file on disk holds exactly the data the counter and hash cover.
    /// Append mode writes straight to the destination, which already holds
    /// prior content, so it cannot be safely rewound and is left alone.
    async fn restore_to_written(&mut self) {
        if self.tmp.is_none() {
            return;
        }
        let _ = self.file.set_len(self.written).await;
        let _ = self.file.seek(SeekFrom::Start(self.written)).await;
    }

    /// On-disk length of the file bytes this writer produced, or `None` in
    /// append mode, where the destination also holds content from before
    /// this session and a length comparison would not be meaningful.
    async fn durable_len(&self) -> std::io::Result<Option<u64>> {
        match &self.tmp {
            Some(tmp) => Ok(Some(tokio::fs::metadata(tmp).await?.len())),
            None => Ok(None),
        }
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
        // Defence in depth: the bytes the hash and counter cover must be the
        // bytes on disk. If a buffered write was lost without surfacing an
        // error, refuse to publish rather than commit a file whose contents
        // do not match the checksum we are about to report as verified.
        let written = self.written;
        let on_disk = self.durable_len().await.map_err(|e| io_status(&e, &dest))?;
        if let Some(on_disk) = on_disk {
            if on_disk != written {
                self.abort().await;
                return Err(status(
                    Code::Internal,
                    ErrorReason::Unspecified,
                    format!(
                        "upload was not durably written: {written} bytes accepted,                          {on_disk} bytes on disk; the transfer was discarded, retry it"
                    ),
                ));
            }
        }
        let digest = hex_digest(self.hasher.clone().finalize());
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
        let mut entry = entry_info(&dest, &meta);
        entry.metadata = xattr::read(&dest, &self.target.attributes.namespace, true);
        Ok((entry, digest))
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
        WriteTarget::new(dest, mode, 0, true)
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
    async fn finish_refuses_a_file_shorter_on_disk_than_counted() {
        // The headline bug: a buffered write can be lost without surfacing an
        // error, leaving the file short while the counter and hash still cover
        // the full data. Reproduce that end state by truncating the temp file
        // behind the writer, then confirm finish refuses instead of publishing
        // a file whose bytes do not match the checksum it would report.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");
        let mut w = AtomicWriter::create(target(dest.clone(), WriteMode::Overwrite))
            .await
            .unwrap();
        w.write(&vec![b'a'; 1024]).await.unwrap();
        let tmp = w.tmp.clone().expect("overwrite uses a temp file");
        // Lose the last 24 bytes, as a dropped buffered write would.
        let short = std::fs::OpenOptions::new().write(true).open(&tmp).unwrap();
        short.set_len(1000).unwrap();
        drop(short);

        let sha = hex_digest(Sha256::digest(&vec![b'a'; 1024]));
        let error = w.finish(1024, &sha).await.unwrap_err();
        assert_eq!(error.code(), Code::Internal, "{error:?}");
        assert!(!dest.exists(), "a short file must not be published");
        assert!(!tmp.exists(), "the temp file is discarded");
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
