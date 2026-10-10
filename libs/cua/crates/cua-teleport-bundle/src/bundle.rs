// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The [`SessionBundle`] on-wire format.
//!
//! A `SessionBundle` is a streamed tar archive whose **first** entry is
//! `manifest.json` (a serialized [`BundleHeader`]) followed by the file
//! entries the header describes. [`BundleWriter`] builds one and [`BundleReader`]
//! consumes one, verifying every entry's SHA-256 against the header and
//! enforcing a configurable total-size guard.

use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};

use crate::error::{Result, TeleportError};
use crate::types::TransferScope;

/// Marker for the `SessionBundle` on-wire format: a streamed tar archive
/// whose first entry is `manifest.json` (a [`BundleHeader`]) followed by the
/// checksummed file entries it declares. Produced by [`BundleWriter`] and
/// consumed by [`BundleReader`]; uploaded in chunks through
/// `cua.env.v1.TeleportService/ImportSession`.
pub struct SessionBundle;

impl SessionBundle {
    /// Recommended HTTP content type for the streamed tar body.
    pub const CONTENT_TYPE: &'static str = "application/x-tar";
}

/// Name of the mandatory first tar entry.
pub const MANIFEST_ENTRY: &str = "manifest.json";

/// Default maximum total (declared and materialized) bundle payload size.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A single file described in the bundle header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleEntry {
    /// Path relative to the destination root; forward slashes, no `..`.
    pub rel_path: String,
    /// Unix file mode (permission bits).
    pub mode: u32,
    pub byte_len: u64,
    /// Lowercase hex SHA-256 of the entry's bytes.
    pub sha256: String,
}

/// The `manifest.json` payload that opens every bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleHeader {
    pub provider_id: String,
    pub app_display_name: String,
    pub scope: TransferScope,
    pub entries: Vec<BundleEntry>,
}

impl BundleHeader {
    /// Sum of the declared entry byte lengths.
    pub fn declared_total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.byte_len).sum()
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Reject absolute paths, parent traversal, and empty components so an entry
/// cannot escape the destination root on import.
fn validate_rel_path(rel_path: &str) -> Result<()> {
    if rel_path.is_empty()
        || rel_path.starts_with('/')
        || rel_path.starts_with('\\')
        || rel_path.contains(':')
    {
        return Err(TeleportError::InvalidEntryPath(rel_path.to_string()));
    }
    for component in rel_path.split(['/', '\\']) {
        if component.is_empty() || component == "." || component == ".." {
            return Err(TeleportError::InvalidEntryPath(rel_path.to_string()));
        }
    }
    Ok(())
}

fn write_tar_file<W: Write>(
    builder: &mut Builder<W>,
    name: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(mode);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    builder
        .append_data(&mut header, name, Cursor::new(bytes))
        .map_err(TeleportError::Io)
}

/// Builds a [`SessionBundle`] tar stream. Entries are buffered in memory so
/// the header (which must be the first tar entry) can enumerate every file
/// with its checksum before any file bytes are written.
pub struct BundleWriter<W: Write> {
    inner: Option<W>,
    provider_id: String,
    app_display_name: String,
    scope: TransferScope,
    entries: Vec<BundleEntry>,
    payloads: Vec<Vec<u8>>,
    total_bytes: u64,
    max_total_bytes: u64,
}

impl<W: Write> BundleWriter<W> {
    /// Create a writer with the default size guard.
    pub fn new(
        writer: W,
        provider_id: impl Into<String>,
        app_display_name: impl Into<String>,
        scope: TransferScope,
    ) -> Self {
        Self::with_limit(
            writer,
            provider_id,
            app_display_name,
            scope,
            DEFAULT_MAX_TOTAL_BYTES,
        )
    }

    /// Create a writer with an explicit total-size guard.
    pub fn with_limit(
        writer: W,
        provider_id: impl Into<String>,
        app_display_name: impl Into<String>,
        scope: TransferScope,
        max_total_bytes: u64,
    ) -> Self {
        Self {
            inner: Some(writer),
            provider_id: provider_id.into(),
            app_display_name: app_display_name.into(),
            scope,
            entries: Vec::new(),
            payloads: Vec::new(),
            total_bytes: 0,
            max_total_bytes,
        }
    }

    /// Add an entry from an in-memory byte slice.
    pub fn add_bytes(
        &mut self,
        rel_path: impl Into<String>,
        mode: u32,
        bytes: &[u8],
    ) -> Result<()> {
        let rel_path = rel_path.into();
        validate_rel_path(&rel_path)?;
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        if self.total_bytes > self.max_total_bytes {
            return Err(TeleportError::BundleTooLarge {
                limit_bytes: self.max_total_bytes,
            });
        }
        self.entries.push(BundleEntry {
            rel_path,
            mode,
            byte_len: bytes.len() as u64,
            sha256: hex_sha256(bytes),
        });
        self.payloads.push(bytes.to_vec());
        Ok(())
    }

    /// Add an entry by reading a source to end. The source is buffered.
    pub fn add_reader(
        &mut self,
        rel_path: impl Into<String>,
        mode: u32,
        reader: &mut dyn Read,
    ) -> Result<()> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        self.add_bytes(rel_path, mode, &bytes)
    }

    /// Write the manifest and all buffered entries, returning the writer.
    pub fn finish(mut self) -> Result<W> {
        let writer = self.inner.take().expect("BundleWriter already finished");
        let header = BundleHeader {
            provider_id: std::mem::take(&mut self.provider_id),
            app_display_name: std::mem::take(&mut self.app_display_name),
            scope: self.scope,
            entries: std::mem::take(&mut self.entries),
        };
        let manifest = serde_json::to_vec(&header)?;

        let mut builder = Builder::new(writer);
        write_tar_file(&mut builder, MANIFEST_ENTRY, 0o644, &manifest)?;
        for (entry, payload) in header.entries.iter().zip(self.payloads.iter()) {
            write_tar_file(&mut builder, &entry.rel_path, entry.mode, payload)?;
        }
        let writer = builder.into_inner().map_err(TeleportError::Io)?;
        Ok(writer)
    }
}

/// A verified entry yielded by [`BundleReader`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEntry {
    pub rel_path: String,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

/// Reads and verifies a [`SessionBundle`] tar stream. Construct with
/// [`BundleReader::open`], which reads and returns the [`BundleHeader`], then
/// iterate [`BundleReader::next_entry`] to obtain checksum-verified files.
///
/// The `tar` crate's entry iterator borrows the archive, so a struct cannot
/// hold both. The bundle is therefore drained into memory at open time
/// (bounded by the size guard) and entries are verified lazily as they are
/// yielded.
pub struct BundleReader {
    header: BundleHeader,
    remaining: std::collections::VecDeque<RawEntry>,
    index: usize,
    seen_bytes: u64,
}

struct RawEntry {
    rel_path: String,
    mode: u32,
    bytes: Vec<u8>,
}

impl BundleReader {
    /// Open a bundle with the default size guard, reading its header.
    pub fn open<R: Read>(reader: R) -> Result<Self> {
        Self::open_with_limit(reader, DEFAULT_MAX_TOTAL_BYTES)
    }

    /// Open a bundle with an explicit total-size guard, reading its header.
    pub fn open_with_limit<R: Read>(reader: R, max_total_bytes: u64) -> Result<Self> {
        let mut archive = tar::Archive::new(reader);
        let mut entries = archive.entries().map_err(TeleportError::Io)?;

        let mut first = entries
            .next()
            .ok_or_else(|| TeleportError::InvalidBundle("empty archive".into()))?
            .map_err(TeleportError::Io)?;
        let first_path = first
            .path()
            .map_err(TeleportError::Io)?
            .to_string_lossy()
            .into_owned();
        if first_path != MANIFEST_ENTRY {
            return Err(TeleportError::InvalidBundle(format!(
                "first entry is {first_path:?}, expected {MANIFEST_ENTRY:?}"
            )));
        }
        let mut manifest_bytes = Vec::new();
        first.read_to_end(&mut manifest_bytes)?;
        let header: BundleHeader = serde_json::from_slice(&manifest_bytes)?;

        if header.declared_total_bytes() > max_total_bytes {
            return Err(TeleportError::BundleTooLarge {
                limit_bytes: max_total_bytes,
            });
        }

        let mut remaining = std::collections::VecDeque::new();
        let mut seen: u64 = 0;
        for entry in entries {
            let mut entry = entry.map_err(TeleportError::Io)?;
            let rel_path = entry
                .path()
                .map_err(TeleportError::Io)?
                .to_string_lossy()
                .into_owned();
            let mode = entry.header().mode().unwrap_or(0o644);
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            seen = seen.saturating_add(bytes.len() as u64);
            if seen > max_total_bytes {
                return Err(TeleportError::BundleTooLarge {
                    limit_bytes: max_total_bytes,
                });
            }
            remaining.push_back(RawEntry {
                rel_path,
                mode,
                bytes,
            });
        }

        Ok(Self {
            header,
            remaining,
            index: 0,
            seen_bytes: seen,
        })
    }

    /// The bundle header read at open time.
    pub fn header(&self) -> &BundleHeader {
        &self.header
    }

    /// Total materialized payload bytes buffered from the stream.
    pub fn seen_bytes(&self) -> u64 {
        self.seen_bytes
    }

    /// Yield the next checksum-verified entry, or `None` at end of stream.
    pub fn next_entry(&mut self) -> Result<Option<VerifiedEntry>> {
        let Some(entry) = self.remaining.pop_front() else {
            return Ok(None);
        };
        let declared = self.header.entries.get(self.index).ok_or_else(|| {
            TeleportError::InvalidBundle(format!(
                "archive has more entries than the manifest declares (at {})",
                entry.rel_path
            ))
        })?;
        self.index += 1;
        validate_rel_path(&entry.rel_path)?;
        if entry.rel_path != declared.rel_path {
            return Err(TeleportError::InvalidBundle(format!(
                "entry {:?} does not match manifest entry {:?}",
                entry.rel_path, declared.rel_path
            )));
        }
        let actual = hex_sha256(&entry.bytes);
        if actual != declared.sha256 {
            return Err(TeleportError::ChecksumMismatch {
                rel_path: entry.rel_path.clone(),
            });
        }
        Ok(Some(VerifiedEntry {
            rel_path: entry.rel_path,
            mode: entry.mode,
            bytes: entry.bytes,
        }))
    }

    /// Convenience: drain all remaining verified entries.
    pub fn read_all(mut self) -> Result<Vec<VerifiedEntry>> {
        let mut out = Vec::new();
        while let Some(entry) = self.next_entry()? {
            out.push(entry);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sample(max: u64) -> Vec<u8> {
        let mut writer = BundleWriter::with_limit(
            Vec::new(),
            "test.provider",
            "Test App",
            TransferScope::FullProfile,
            max,
        );
        writer
            .add_bytes("tabs.json", 0o644, b"[\"https://a\"]")
            .unwrap();
        writer
            .add_bytes("profile/Preferences", 0o600, b"{\"k\":1}")
            .unwrap();
        writer.finish().unwrap()
    }

    #[test]
    fn round_trip_verifies_sha256() {
        let bytes = write_sample(DEFAULT_MAX_TOTAL_BYTES);
        let mut reader = BundleReader::open(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.header().provider_id, "test.provider");
        assert_eq!(reader.header().scope, TransferScope::FullProfile);
        assert_eq!(reader.header().entries.len(), 2);

        let first = reader.next_entry().unwrap().unwrap();
        assert_eq!(first.rel_path, "tabs.json");
        assert_eq!(first.bytes, b"[\"https://a\"]");
        let second = reader.next_entry().unwrap().unwrap();
        assert_eq!(second.rel_path, "profile/Preferences");
        assert_eq!(second.mode & 0o777, 0o600);
        assert!(reader.next_entry().unwrap().is_none());
    }

    #[test]
    fn tampered_entry_fails_checksum() {
        // Build a bundle whose manifest declares a hash that will not match
        // the payload once a byte is flipped in the file entry.
        let mut writer = BundleWriter::new(
            Vec::new(),
            "test.provider",
            "Test App",
            TransferScope::TabsOnly,
        );
        writer
            .add_bytes("tabs.json", 0o644, b"hello world")
            .unwrap();
        let good = writer.finish().unwrap();

        // Flip a byte inside the tar payload region (after the two 512-byte
        // headers). The manifest entry comes first, so tampering the last
        // bytes of the archive hits the file payload, not the manifest.
        let mut tampered = good.clone();
        // Find the "hello world" payload and corrupt it.
        let needle = b"hello world";
        let pos = tampered
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("payload present");
        tampered[pos] ^= 0xff;

        let mut reader = BundleReader::open(Cursor::new(tampered)).unwrap();
        let err = reader.next_entry().unwrap_err();
        assert!(
            matches!(err, TeleportError::ChecksumMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn oversize_guard_trips_on_write() {
        let mut writer = BundleWriter::with_limit(
            Vec::new(),
            "test.provider",
            "Test App",
            TransferScope::TabsOnly,
            8,
        );
        let err = writer
            .add_bytes("big", 0o644, b"way too many bytes")
            .unwrap_err();
        assert!(
            matches!(err, TeleportError::BundleTooLarge { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn oversize_guard_trips_on_read() {
        let bytes = write_sample(DEFAULT_MAX_TOTAL_BYTES);
        let result = BundleReader::open_with_limit(Cursor::new(bytes), 4).map(|_| ());
        assert!(
            matches!(result, Err(TeleportError::BundleTooLarge { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn rejects_unsafe_paths() {
        let mut writer = BundleWriter::new(Vec::new(), "p", "A", TransferScope::TabsOnly);
        assert!(matches!(
            writer.add_bytes("../escape", 0o644, b"x").unwrap_err(),
            TeleportError::InvalidEntryPath(_)
        ));
        assert!(matches!(
            writer.add_bytes("/abs", 0o644, b"x").unwrap_err(),
            TeleportError::InvalidEntryPath(_)
        ));
    }

    #[test]
    fn missing_manifest_first_entry_is_invalid() {
        // A plain tar without a manifest first entry must be rejected.
        let mut builder = Builder::new(Vec::new());
        write_tar_file(&mut builder, "not-a-manifest", 0o644, b"x").unwrap();
        let raw = builder.into_inner().unwrap();
        let result = BundleReader::open(Cursor::new(raw)).map(|_| ());
        assert!(
            matches!(result, Err(TeleportError::InvalidBundle(_))),
            "{result:?}"
        );
    }
}
