//! Files: chunked, SHA-256-verified, resumable upload and download.
//!
//! Upload uses the native `WriteFile` client stream on HTTP/2 and the
//! resumable `BeginUpload` / `UploadChunk` / `CommitUpload` protocol on
//! gRPC-Web (or when [`UploadOptions::resumable`] is set, or after a native
//! stream breaks). Resumable uploads derive a deterministic upload id from
//! the destination and content hash, so an interrupted transfer — even from
//! a new process — continues from the server's `received_bytes`.

use crate::{
    client::SpacesdClient,
    error::{Error, Result},
    process::duration_pb,
    transport::Transport,
};
use bytes::Bytes;
use cua_proto::env::v1::{
    self as pb, read_file_response::Message as ReadMsg, write_file_request::Message as WriteMsg,
};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// What to upload.
#[derive(Clone, Debug)]
pub enum UploadSource {
    /// In-memory bytes.
    Bytes(Bytes),
    /// A local file, streamed.
    File(PathBuf),
}

impl From<Bytes> for UploadSource {
    fn from(b: Bytes) -> Self {
        UploadSource::Bytes(b)
    }
}

impl From<Vec<u8>> for UploadSource {
    fn from(b: Vec<u8>) -> Self {
        UploadSource::Bytes(b.into())
    }
}

impl From<&[u8]> for UploadSource {
    fn from(b: &[u8]) -> Self {
        UploadSource::Bytes(Bytes::copy_from_slice(b))
    }
}

impl From<&str> for UploadSource {
    fn from(s: &str) -> Self {
        UploadSource::Bytes(Bytes::copy_from_slice(s.as_bytes()))
    }
}

impl From<PathBuf> for UploadSource {
    fn from(p: PathBuf) -> Self {
        UploadSource::File(p)
    }
}

impl From<&Path> for UploadSource {
    fn from(p: &Path) -> Self {
        UploadSource::File(p.to_path_buf())
    }
}

impl UploadSource {
    async fn len(&self) -> Result<u64> {
        Ok(match self {
            UploadSource::Bytes(b) => b.len() as u64,
            UploadSource::File(p) => tokio::fs::metadata(p).await?.len(),
        })
    }

    async fn sha256(&self) -> Result<String> {
        match self {
            UploadSource::Bytes(b) => {
                let b = b.clone();
                Ok(
                    tokio::task::spawn_blocking(move || hex::encode(Sha256::digest(&b)))
                        .await
                        .map_err(|e| Error::Protocol(e.to_string()))?,
                )
            }
            UploadSource::File(p) => {
                let mut f = tokio::fs::File::open(p).await?;
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
        }
    }

    /// Reads `len` bytes at `offset`.
    async fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        match self {
            UploadSource::Bytes(b) => {
                let start = (offset as usize).min(b.len());
                let end = (start + len).min(b.len());
                Ok(b[start..end].to_vec())
            }
            UploadSource::File(p) => {
                let mut f = tokio::fs::File::open(p).await?;
                f.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut buf = vec![0u8; len];
                let mut filled = 0;
                while filled < len {
                    let n = f.read(&mut buf[filled..]).await?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                buf.truncate(filled);
                Ok(buf)
            }
        }
    }
}

/// Upload options.
#[derive(Clone, Debug, Default)]
pub struct UploadOptions {
    /// Existing-file behaviour.
    pub mode: pb::WriteMode,
    /// Permission bits for a new file (0 = 0o644).
    pub permissions: u32,
    /// Create missing parents.
    pub create_parents: bool,
    /// Chunk size (0 = server preferred, else 1 MiB).
    pub chunk_size: usize,
    /// Force (`Some(true)`) or forbid (`Some(false)`) the resumable protocol.
    /// `None` picks `WriteFile` on native gRPC and resumable on gRPC-Web.
    pub resumable: Option<bool>,
    /// Explicit upload id (resumable protocol). Default: derived from the
    /// path and SHA-256.
    pub upload_id: Option<String>,
    /// Partial-upload TTL on the server.
    pub ttl: Option<Duration>,
    /// Progress counter updated with bytes acknowledged by the server.
    pub progress: Option<Arc<AtomicU64>>,
}

/// Result of an upload.
#[derive(Clone, Debug)]
pub struct UploadResult {
    /// The written file.
    pub entry: Option<pb::EntryInfo>,
    /// Lowercase hex SHA-256 (verified against the server's).
    pub sha256: String,
    /// Bytes uploaded.
    pub size: u64,
    /// Whether the resumable protocol was used.
    pub resumable: bool,
    /// Number of times the transfer resumed after an interruption.
    pub resumes: u32,
}

/// Download options.
#[derive(Clone, Debug, Default)]
pub struct DownloadOptions {
    /// First byte.
    pub offset: u64,
    /// Max bytes (0 = to EOF).
    pub length: u64,
    /// Chunk size hint.
    pub chunk_size: u32,
    /// Progress counter (bytes received).
    pub progress: Option<Arc<AtomicU64>>,
}

/// Result of a download to a sink.
#[derive(Clone, Debug)]
pub struct DownloadResult {
    /// Bytes received.
    pub size: u64,
    /// SHA-256 of all bytes received.
    pub sha256: String,
    /// Number of times the transfer resumed.
    pub resumes: u32,
}

fn resumable_id(path: &str, sha: &str) -> String {
    let mut h = Sha256::new();
    h.update(path.as_bytes());
    h.update([0]);
    h.update(sha.as_bytes());
    format!("cua-{}", &hex::encode(h.finalize())[..32])
}

impl SpacesdClient {
    /// Uploads `source` to `path` (see the module docs for the protocol).
    pub async fn upload(
        &self,
        path: &str,
        source: impl Into<UploadSource>,
        options: UploadOptions,
    ) -> Result<UploadResult> {
        let source = source.into();
        let size = source.len().await?;
        let sha = source.sha256().await?;
        let (preferred, max) = self.chunk_limits().await;
        let chunk = if options.chunk_size == 0 {
            preferred
        } else {
            options.chunk_size.min(max)
        }
        .max(1);
        let resumable = options
            .resumable
            .unwrap_or(self.transport() == Transport::GrpcWeb);
        if resumable || self.transport() == Transport::GrpcWeb {
            return self
                .upload_resumable(path, &source, size, &sha, chunk, &options, 0)
                .await;
        }
        match self
            .upload_stream(path, &source, size, &sha, chunk, &options)
            .await
        {
            Ok(r) => Ok(r),
            // A broken stream cannot be resumed; continue with the resumable
            // protocol, which can.
            Err(e) if e.is_retryable() && options.resumable != Some(false) => {
                tracing::debug!(error = %e, "WriteFile interrupted; switching to resumable upload");
                if let Some(p) = &options.progress {
                    p.store(0, Ordering::Relaxed);
                }
                self.upload_resumable(path, &source, size, &sha, chunk, &options, 1)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    fn header(path: &str, size: u64, sha: &str, options: &UploadOptions) -> pb::WriteFileHeader {
        pb::WriteFileHeader {
            path: path.to_string(),
            mode: options.mode as i32,
            permissions: options.permissions,
            create_parents: options.create_parents,
            expected_size: size,
            expected_sha256: sha.to_string(),
        }
    }

    async fn upload_stream(
        &self,
        path: &str,
        source: &UploadSource,
        size: u64,
        sha: &str,
        chunk: usize,
        options: &UploadOptions,
    ) -> Result<UploadResult> {
        let header = Self::header(path, size, sha, options);
        let (tx, rx) = tokio::sync::mpsc::channel::<pb::WriteFileRequest>(4);
        let producer = {
            let source = source.clone();
            let progress = options.progress.clone();
            tokio::spawn(async move {
                if tx
                    .send(pb::WriteFileRequest {
                        message: Some(WriteMsg::Header(header)),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                let mut offset = 0u64;
                while offset < size {
                    let data = source.read_at(offset, chunk).await?;
                    if data.is_empty() {
                        break;
                    }
                    offset += data.len() as u64;
                    if tx
                        .send(pb::WriteFileRequest {
                            message: Some(WriteMsg::Data(data)),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if let Some(p) = &progress {
                        p.store(offset, Ordering::Relaxed);
                    }
                }
                Ok::<_, Error>(())
            })
        };
        let resp = self
            .filesystem()
            .write_file(tokio_stream::wrappers::ReceiverStream::new(rx))
            .await;
        let produced = producer.await.map_err(|e| Error::Protocol(e.to_string()))?;
        let resp = resp?.into_inner();
        produced?;
        verify(sha, &resp.sha256)?;
        Ok(UploadResult {
            entry: resp.entry,
            sha256: resp.sha256,
            size,
            resumable: false,
            resumes: 0,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_resumable(
        &self,
        path: &str,
        source: &UploadSource,
        size: u64,
        sha: &str,
        chunk: usize,
        options: &UploadOptions,
        mut resumes: u32,
    ) -> Result<UploadResult> {
        let upload_id = options
            .upload_id
            .clone()
            .unwrap_or_else(|| resumable_id(path, sha));
        let policy = self.retry_policy();
        let begin_req = pb::BeginUploadRequest {
            header: Some(Self::header(path, size, sha, options)),
            upload_id: upload_id.clone(),
            ttl: options.ttl.map(duration_pb),
        };
        let begin = |req: pb::BeginUploadRequest| async move {
            policy
                .run(|_| {
                    let req = req.clone();
                    async move { Ok(self.filesystem().begin_upload(req).await?.into_inner()) }
                })
                .await
        };
        let started = begin(begin_req.clone()).await?;
        let upload_id = started.upload_id.clone();
        let chunk = if started.max_chunk_bytes > 0 {
            chunk.min(started.max_chunk_bytes as usize)
        } else {
            chunk
        };
        let mut offset = started.received_bytes;
        if offset > 0 {
            resumes = resumes.max(1);
        }
        let mut failures = 0u32;
        while offset < size {
            let data = source.read_at(offset, chunk).await?;
            if data.is_empty() {
                return Err(Error::Protocol(format!(
                    "source ended at {offset} of {size} bytes"
                )));
            }
            let req = pb::UploadChunkRequest {
                upload_id: upload_id.clone(),
                offset,
                data,
            };
            match self.filesystem().upload_chunk(req).await {
                Ok(resp) => {
                    failures = 0;
                    offset = resp.into_inner().received_bytes;
                    if let Some(p) = &options.progress {
                        p.store(offset, Ordering::Relaxed);
                    }
                }
                Err(status) => {
                    let err = Error::from(status);
                    match err {
                        Error::OffsetMismatch {
                            expected_offset: Some(expected),
                            ..
                        } => {
                            offset = expected;
                        }
                        e if e.is_retryable() && failures + 1 < policy.max_attempts => {
                            failures += 1;
                            resumes += 1;
                            tracing::debug!(offset, error = %e, "upload interrupted; resuming");
                            tokio::time::sleep(policy.backoff(failures)).await;
                            // Ask the server where it is.
                            offset = begin(begin_req.clone()).await?.received_bytes;
                        }
                        e => return Err(e),
                    }
                }
            }
        }
        let committed = policy
            .run(|_| {
                let req = pb::CommitUploadRequest {
                    upload_id: upload_id.clone(),
                    sha256: sha.to_string(),
                };
                async move { Ok(self.filesystem().commit_upload(req).await?.into_inner()) }
            })
            .await?;
        verify(sha, &committed.sha256)?;
        Ok(UploadResult {
            entry: committed.entry,
            sha256: committed.sha256,
            size,
            resumable: true,
            resumes,
        })
    }

    /// Downloads `path` into memory, verifying SHA-256 per segment.
    pub async fn download(&self, path: &str) -> Result<Bytes> {
        let mut buf = Vec::new();
        self.download_with(path, DownloadOptions::default(), &mut buf)
            .await?;
        Ok(buf.into())
    }

    /// Downloads `path` to a local file. The bytes land in a fresh temp file
    /// next to `local` that is renamed over it only once the whole download
    /// verified, so a failed or interrupted download leaves any existing
    /// `local` untouched.
    pub async fn download_to_file(
        &self,
        path: &str,
        local: &Path,
        options: DownloadOptions,
    ) -> Result<DownloadResult> {
        let (tmp, mut f) = create_download_temp(local).await?;
        let written = async {
            let res = self.download_with(path, options, &mut f).await?;
            f.flush().await?;
            f.sync_all().await?;
            drop(f);
            tokio::fs::rename(&tmp, local).await?;
            Ok::<_, Error>(res)
        }
        .await;
        if written.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        written
    }

    /// Streams `path` into `sink`. On a dropped stream the download resumes
    /// at the next offset; each segment's SHA-256 is checked against the
    /// server's.
    pub async fn download_with<S: DownloadSink>(
        &self,
        path: &str,
        options: DownloadOptions,
        sink: &mut S,
    ) -> Result<DownloadResult> {
        let policy = self.retry_policy();
        let mut total = Sha256::new();
        let mut received = 0u64;
        let mut resumes = 0u32;
        let mut failures = 0u32;
        let end_limit = (options.length > 0).then(|| options.offset + options.length);
        loop {
            let offset = options.offset + received;
            let length = end_limit.map(|e| e - offset).unwrap_or(0);
            if end_limit.is_some() && length == 0 {
                break;
            }
            let req = pb::ReadFileRequest {
                path: path.to_string(),
                offset,
                length,
                chunk_size: options.chunk_size,
                compute_sha256: true,
            };
            let outcome: Result<bool> = async {
                let mut stream = self.filesystem().read_file(req).await?.into_inner();
                let mut segment = Sha256::new();
                while let Some(msg) = stream.message().await? {
                    match msg.message {
                        Some(ReadMsg::Entry(_)) | None => {}
                        Some(ReadMsg::Chunk(c)) => {
                            if c.offset != options.offset + received {
                                return Err(Error::Protocol(format!(
                                    "chunk at {} but expected {}",
                                    c.offset,
                                    options.offset + received
                                )));
                            }
                            segment.update(&c.data);
                            total.update(&c.data);
                            received += c.data.len() as u64;
                            if let Some(p) = &options.progress {
                                p.store(received, Ordering::Relaxed);
                            }
                            sink.write(c.data.into()).await?;
                        }
                        Some(ReadMsg::End(end)) => {
                            if !end.sha256.is_empty() {
                                verify(&end.sha256, &hex::encode(segment.finalize()))?;
                            }
                            return Ok(true);
                        }
                    }
                }
                Err(Error::Rpc(crate::error::ErrorDetails {
                    code: tonic::Code::Unavailable as i32,
                    message: "read stream ended without ReadFileEnd".into(),
                    metadata: Default::default(),
                }))
            }
            .await;
            match outcome {
                Ok(_) => break,
                Err(e) if e.is_retryable() && failures + 1 < policy.max_attempts => {
                    failures += 1;
                    resumes += 1;
                    tracing::debug!(received, error = %e, "download interrupted; resuming");
                    tokio::time::sleep(policy.backoff(failures)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(DownloadResult {
            size: received,
            sha256: hex::encode(total.finalize()),
            resumes,
        })
    }

    /// `Stat`.
    pub async fn stat(&self, path: &str) -> Result<pb::EntryInfo> {
        self.filesystem()
            .stat(pb::StatRequest {
                path: path.into(),
                no_follow_symlinks: false,
            })
            .await?
            .into_inner()
            .entry
            .ok_or_else(|| Error::Protocol("Stat without entry".into()))
    }

    /// `ListDir` (all pages).
    pub async fn list_dir(&self, path: &str, depth: u32) -> Result<Vec<pb::EntryInfo>> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let resp = self
                .filesystem()
                .list_dir(pb::ListDirRequest {
                    path: path.into(),
                    depth,
                    include_hidden: true,
                    page_size: 0,
                    page_token: token.clone(),
                })
                .await?
                .into_inner();
            out.extend(resp.entries);
            if resp.next_page_token.is_empty() {
                return Ok(out);
            }
            token = resp.next_page_token;
        }
    }

    /// `MakeDir` with parents.
    pub async fn make_dir(&self, path: &str) -> Result<pb::EntryInfo> {
        self.filesystem()
            .make_dir(pb::MakeDirRequest {
                path: path.into(),
                parents: true,
                mode: 0,
            })
            .await?
            .into_inner()
            .entry
            .ok_or_else(|| Error::Protocol("MakeDir without entry".into()))
    }

    /// `Remove`.
    pub async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.filesystem()
            .remove(pb::RemoveRequest {
                path: path.into(),
                recursive,
                missing_ok: false,
            })
            .await?;
        Ok(())
    }
}

/// A fresh, exclusively created temp file beside `local` (never an existing
/// file or symlink) for [`SpacesdClient::download_to_file`] to fill.
async fn create_download_temp(local: &Path) -> Result<(PathBuf, tokio::fs::File)> {
    let dir = match local.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    let mut last = None;
    for _ in 0..16 {
        let tmp = dir.join(format!(".{name}.{:016x}.part", rand::random::<u64>()));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await
        {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(last
        .unwrap_or_else(|| std::io::Error::other("no free temp name"))
        .into())
}

/// Destination of a download.
pub trait DownloadSink: Send {
    /// Appends one chunk.
    fn write(&mut self, chunk: Bytes) -> impl std::future::Future<Output = Result<()>> + Send;
}

impl DownloadSink for Vec<u8> {
    async fn write(&mut self, chunk: Bytes) -> Result<()> {
        self.extend_from_slice(&chunk);
        Ok(())
    }
}

impl DownloadSink for tokio::fs::File {
    async fn write(&mut self, chunk: Bytes) -> Result<()> {
        self.write_all(&chunk).await?;
        Ok(())
    }
}

fn verify(expected: &str, actual: &str) -> Result<()> {
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(Error::ChecksumMismatch {
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}
