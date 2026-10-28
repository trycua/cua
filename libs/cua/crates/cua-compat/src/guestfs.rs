//! Guest file plumbing shared by the providers: streaming uploads into a
//! sandbox (cua-spacesd `WriteFile`, or the resumable chunk protocol over
//! gRPC-Web), ownership for non-root users, and a streaming
//! `multipart/form-data` reader.

use bytes::{Bytes, BytesMut};
use cua_spacesd_client::pb;
use cua_spacesd_client::transport::Transport;
use futures_util::{Stream, StreamExt};
use http::StatusCode;
use sha2::Digest;

/// A guest file error: the HTTP status it maps to and the message.
#[derive(Debug, Clone)]
pub struct FsError(pub StatusCode, pub String);

impl From<&tonic::Status> for FsError {
    fn from(s: &tonic::Status) -> Self {
        FsError(status_of(s), s.message().to_string())
    }
}

/// What an upload wrote.
#[derive(Debug, Clone)]
pub struct Written {
    pub name: String,
    pub path: String,
    pub entry: Option<pb::EntryInfo>,
}

pub fn status_of(s: &tonic::Status) -> StatusCode {
    match s.code() {
        tonic::Code::NotFound => StatusCode::NOT_FOUND,
        tonic::Code::InvalidArgument | tonic::Code::FailedPrecondition => StatusCode::BAD_REQUEST,
        tonic::Code::PermissionDenied => StatusCode::FORBIDDEN,
        tonic::Code::ResourceExhausted => StatusCode::INSUFFICIENT_STORAGE,
        tonic::Code::Unauthenticated => StatusCode::UNAUTHORIZED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The top-most ancestor of `path` (or `path` itself) that does not exist
/// yet, i.e. what a write with parents will create. `None`: it exists.
pub async fn first_missing(c: &cua_spacesd_client::SpacesdClient, path: &str) -> Option<String> {
    let exists = |p: String| async move {
        !matches!(
            c.filesystem()
                .stat(pb::StatRequest {
                    path: p,
                    no_follow_symlinks: true, ..Default::default()
                })
                .await,
            Err(ref s) if s.code() == tonic::Code::NotFound
        )
    };
    if exists(path.to_string()).await {
        return None;
    }
    let mut missing = path.to_string();
    let mut cur = path.to_string();
    while let Some((parent, _)) = cur.rsplit_once('/') {
        if parent.is_empty() || exists(parent.to_string()).await {
            break;
        }
        missing = parent.to_string();
        cur = parent.to_string();
    }
    Some(missing)
}

/// Gives `user` what this call created (from `from` down to `path`).
pub async fn chown_created(c: &cua_spacesd_client::SpacesdClient, user: &str, from: &str) {
    if user == "root" {
        return;
    }
    let cmd = cua_spacesd_client::Command::new("chown")
        .args(["-R", &format!("{user}:"), from])
        .user("root");
    match c.run(cmd).await {
        Ok(out) if out.success() => {}
        Ok(out) => tracing::warn!(
            "compat: chown {user} {from}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => tracing::warn!("compat: chown {user} {from}: {err}"),
    }
}

/// One file upload into the sandbox.
/// Copies `$1` to `$2` (through a dangling symlink too) with root's rights: creates `$2`, gives it
/// (and the directories this made) to `$3`, then writes it through the descriptor opened first (the order envd's
/// watchers see: create, chmod, write; reopening the root-owned file by name would
/// trip `fs.protected_regular` in a sticky directory such as /tmp).
const STAGED_COPY: &str = "d=$(dirname \"$2\"); top=\"\"; p=\"$d\"; \
    while [ ! -e \"$p\" ] && [ ! -L \"$p\" ]; do top=\"$p\"; p=$(dirname \"$p\"); done; \
    [ -d \"$p\" ] || { printf '%s' \"$p\" >&2; exit 30; }; \
    [ -d \"$2\" ] && { printf '%s' \"$2\" >&2; exit 31; }; \
    mkdir -p \"$d\" && { [ -e \"$2\" ] && command -v python3 >/dev/null && python3 -c 'import os,sys\nfor k in os.listxattr(sys.argv[1]):\n if k.startswith(\"user.e2b.\"): os.removexattr(sys.argv[1],k)' \"$2\" 2>/dev/null; true; } && exec 3> \"$2\" && chown \"$3\": \"$2\" && cat -- \"$1\" >&3 || exit 1; \
    if [ -n \"$top\" ] && [ \"$3\" != root ]; then chown -R \"$3\": \"$top\"; fi";

pub struct Writer<'a> {
    c: &'a cua_spacesd_client::SpacesdClient,
    /// Where the bytes go.
    path: String,
    /// Where the file ends up (differs from `path` when it is staged).
    dest: String,
    user: String,
    driver: String,
    created_from: Option<String>,
    sink: Sink,
}

enum Sink {
    /// `WriteFile` client stream (native gRPC).
    Stream {
        tx: tokio::sync::mpsc::Sender<pb::WriteFileRequest>,
        call: tokio::task::JoinHandle<Result<pb::WriteFileResponse, tonic::Status>>,
    },
    /// Resumable `BeginUpload` / `UploadChunk` / `CommitUpload` (gRPC-Web).
    Chunks {
        id: String,
        offset: u64,
        max: usize,
        sha: sha2::Sha256,
    },
}

const CHUNK: usize = 1 << 20;

impl<'a> Writer<'a> {
    /// A file at `path` written for `user`, through a cua-spacesd running as
    /// `driver`: written directly when the driver is that user; written
    /// and then given to `user` when the driver is root; otherwise staged
    /// in `/tmp` and moved into place with root's rights (`sudo -n`), then
    /// given to `user` with the directories it made, as the providers'
    /// own agents (running as root) do.
    pub async fn begin(
        c: &'a cua_spacesd_client::SpacesdClient,
        path: &str,
        user: &str,
        driver: &str,
    ) -> Result<Writer<'a>, FsError> {
        let staged = driver != "root";
        let created_from = if driver == "root" && !user.is_empty() && user != "root" {
            first_missing(c, path).await
        } else {
            None
        };
        let target = if staged {
            format!("/tmp/.cua-compat-{}", crate::auth::new_token())
        } else {
            path.to_string()
        };
        let header = pb::WriteFileHeader {
            path: target.clone(),
            mode: pb::WriteMode::Overwrite as i32,
            permissions: 0o644,
            create_parents: true,
            expected_size: 0,
            expected_sha256: String::new(),
            ..Default::default()
        };
        let sink = if c.transport() == Transport::Native {
            let (tx, rx) = tokio::sync::mpsc::channel::<pb::WriteFileRequest>(4);
            tx.send(pb::WriteFileRequest {
                message: Some(pb::write_file_request::Message::Header(header)),
            })
            .await
            .map_err(|_| FsError(StatusCode::INTERNAL_SERVER_ERROR, "upload closed".into()))?;
            let fs = c.filesystem();
            let call = tokio::spawn(async move {
                let mut fs = fs;
                fs.write_file(tokio_stream::wrappers::ReceiverStream::new(rx))
                    .await
                    .map(|r| r.into_inner())
            });
            Sink::Stream { tx, call }
        } else {
            let started = c
                .filesystem()
                .begin_upload(pb::BeginUploadRequest {
                    header: Some(header),
                    upload_id: String::new(),
                    ttl: None,
                })
                .await
                .map_err(|s| FsError::from(&s))?
                .into_inner();
            Sink::Chunks {
                id: started.upload_id,
                offset: 0,
                max: if started.max_chunk_bytes > 0 {
                    started.max_chunk_bytes as usize
                } else {
                    CHUNK
                },
                sha: sha2::Sha256::new(),
            }
        };
        Ok(Writer {
            c,
            path: target,
            dest: path.to_string(),
            user: user.to_string(),
            driver: driver.to_string(),
            created_from,
            sink,
        })
    }

    pub async fn write(&mut self, data: Bytes) -> Result<(), FsError> {
        match &mut self.sink {
            Sink::Stream { tx, .. } => {
                for piece in data.chunks(CHUNK) {
                    if tx
                        .send(pb::WriteFileRequest {
                            message: Some(pb::write_file_request::Message::Data(piece.to_vec())),
                        })
                        .await
                        .is_err()
                    {
                        // The call ended early; `finish` reports why.
                        return Ok(());
                    }
                }
                Ok(())
            }
            Sink::Chunks {
                id,
                offset,
                max,
                sha,
            } => {
                sha.update(&data);
                for piece in data.chunks(*max) {
                    let resp = self
                        .c
                        .filesystem()
                        .upload_chunk(pb::UploadChunkRequest {
                            upload_id: id.clone(),
                            offset: *offset,
                            data: piece.to_vec(),
                            ..Default::default()
                        })
                        .await
                        .map_err(|s| FsError::from(&s))?;
                    *offset = resp.into_inner().received_bytes;
                }
                Ok(())
            }
        }
    }

    pub async fn finish(self) -> Result<Written, FsError> {
        let entry = match self.sink {
            Sink::Stream { tx, call } => {
                drop(tx);
                let resp = call
                    .await
                    .map_err(|e| FsError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                    .map_err(|s| FsError::from(&s))?;
                resp.entry
            }
            Sink::Chunks { id, sha, .. } => {
                let resp = self
                    .c
                    .filesystem()
                    .commit_upload(pb::CommitUploadRequest {
                        upload_id: id,
                        sha256: hex::encode(sha.finalize()),
                    })
                    .await
                    .map_err(|s| FsError::from(&s))?
                    .into_inner();
                resp.entry
            }
        };
        if let Some(from) = &self.created_from {
            chown_created(self.c, &self.user, from).await;
        }
        let mut entry = entry;
        if self.path != self.dest {
            // As the providers' in-sandbox agents do: write with root's
            // rights, then give the file (and the directories this made)
            // to the user.
            let out = crate::guest::run_as(
                self.c,
                &self.driver,
                "root",
                STAGED_COPY,
                &[&self.path, &self.dest, &self.user],
            )
            .await;
            let _ = self
                .c
                .filesystem()
                .remove(pb::RemoveRequest {
                    path: self.path.clone(),
                    recursive: false,
                    missing_ok: true,
                    ..Default::default()
                })
                .await;
            match out {
                Ok(o) if o.success() => {}
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    return Err(match o.status.code {
                        Some(30) => FsError(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("error ensuring directories: path is a file: {err}"),
                        ),
                        Some(31) => FsError(
                            StatusCode::BAD_REQUEST,
                            format!("path is a directory: {err}"),
                        ),
                        _ if err.contains("File name too long") => FsError(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "error getting file info: stat {}: file name too long",
                                self.dest
                            ),
                        ),
                        _ if err.contains("Input/output error") => FsError(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "error writing file: write {}: input/output error",
                                self.dest
                            ),
                        ),
                        _ if err.contains("ermission denied") => {
                            FsError(StatusCode::FORBIDDEN, err)
                        }
                        _ if err.contains("o space") => {
                            FsError(StatusCode::INSUFFICIENT_STORAGE, err)
                        }
                        _ => FsError(StatusCode::INTERNAL_SERVER_ERROR, err),
                    });
                }
                Err(e) => return Err(FsError(StatusCode::BAD_GATEWAY, e)),
            }
            entry = self
                .c
                .filesystem()
                .stat(pb::StatRequest {
                    path: self.dest.clone(),
                    no_follow_symlinks: false,
                    ..Default::default()
                })
                .await
                .ok()
                .and_then(|r| r.into_inner().entry);
        }
        let name = entry
            .as_ref()
            .map(|e| e.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.dest.rsplit('/').next().unwrap_or_default().to_string());
        Ok(Written {
            name,
            path: self.dest,
            entry,
        })
    }
}

pub fn multipart_boundary(content_type: &str) -> Option<String> {
    let (mime, params) = content_type.split_once(';')?;
    if !mime.trim().eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    params.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("boundary")
            .then(|| v.trim().trim_matches('"').to_string())
    })
}

/// The `key=value` parameters of a header value (`form-data; name="a;b";
/// filename="x=y"`): keys lowercased, quoted values unquoted (`\\"` is a quote), `;` and `=` allowed inside quotes.
pub fn header_params(v: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = v.chars().peekable();
    // Skip the disposition type (`form-data`).
    for c in chars.by_ref() {
        if c == ';' {
            break;
        }
    }
    loop {
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c == ';' {
                break;
            }
            key.push(c);
            chars.next();
        }
        let mut val = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            if chars.peek() == Some(&'"') {
                chars.next();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        // Only `\"` is an escape: httpx and browsers send other
                        // backslashes (and percent-encode quotes) literally.
                        // (A trailing `\` before the closing quote stays literal.)
                        '\\' if chars.peek() == Some(&'"')
                            && chars.clone().skip(1).any(|c| c == '"') =>
                        {
                            chars.next();
                            val.push('"');
                        }
                        c => val.push(c),
                    }
                }
                // Anything up to the next `;` is ignored.
                while let Some(&c) = chars.peek() {
                    if c == ';' {
                        break;
                    }
                    chars.next();
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c == ';' {
                        break;
                    }
                    val.push(c);
                    chars.next();
                }
                val = val.trim().to_string();
            }
            let k = key.trim().to_ascii_lowercase();
            if !k.is_empty() {
                out.push((k, val));
            }
        }
        match chars.next() {
            Some(';') => continue,
            _ => break,
        }
    }
    out
}

/// One multipart part's headers.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: Option<String>,
    pub filename: Option<String>,
}

/// A streaming `multipart/form-data` reader: parts' bodies are read
/// chunk by chunk, never buffered whole.
pub struct Multipart<S> {
    body: S,
    buf: BytesMut,
    /// `\r\n--boundary`
    delim: Vec<u8>,
    state: MpState,
    eof: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum MpState {
    Preamble,
    Headers,
    Body,
    Done,
}

/// Most a part's headers may take.
const MAX_PART_HEADERS: usize = 16 << 10;

impl<S, E> Multipart<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    pub fn new(body: S, boundary: String) -> Self {
        Self {
            body,
            buf: BytesMut::new(),
            delim: format!("\r\n--{boundary}").into_bytes(),
            state: MpState::Preamble,
            eof: false,
        }
    }

    async fn fill(&mut self) -> Result<bool, String> {
        if self.eof {
            return Ok(false);
        }
        match self.body.next().await {
            Some(Ok(b)) => {
                self.buf.extend_from_slice(&b);
                Ok(true)
            }
            Some(Err(e)) => Err(e.to_string()),
            None => {
                self.eof = true;
                Ok(false)
            }
        }
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Advances to the next part; `None` after the last one.
    pub async fn next_part(&mut self) -> Result<Option<Part>, String> {
        loop {
            match self.state {
                MpState::Done => return Ok(None),
                MpState::Body => {
                    // Skip the rest of an unread body.
                    while self.read_chunk().await?.is_some() {}
                }
                MpState::Preamble => {
                    // The first delimiter may lack the leading CRLF.
                    let first = &self.delim[2..];
                    if let Some(i) = Self::find(&self.buf, first) {
                        let _ = self.buf.split_to(i + first.len());
                        self.state = MpState::Headers;
                    } else if !self.fill().await? {
                        return Err("multipart body has no boundary".into());
                    }
                }
                MpState::Headers => {
                    // After a delimiter: `--` ends the body, else CRLF and
                    // headers up to an empty line.
                    while self.buf.len() < 2 {
                        if !self.fill().await? {
                            return Err("truncated multipart body".into());
                        }
                    }
                    if &self.buf[..2] == b"--" {
                        self.state = MpState::Done;
                        return Ok(None);
                    }
                    let end = loop {
                        if let Some(i) = Self::find(&self.buf, b"\r\n\r\n") {
                            break i;
                        }
                        if self.buf.len() > MAX_PART_HEADERS {
                            return Err("multipart part headers too large".into());
                        }
                        if !self.fill().await? {
                            return Err("truncated multipart headers".into());
                        }
                    };
                    let head = self.buf.split_to(end + 4);
                    let text = String::from_utf8_lossy(&head);
                    let mut part = Part {
                        name: None,
                        filename: None,
                    };
                    for line in text.split("\r\n") {
                        let Some((k, v)) = line.split_once(':') else {
                            continue;
                        };
                        if k.trim().eq_ignore_ascii_case("content-disposition") {
                            for (pk, pv) in header_params(v) {
                                match pk.as_str() {
                                    "name" => part.name = Some(pv),
                                    "filename" => part.filename = Some(pv),
                                    _ => {}
                                }
                            }
                        }
                    }
                    self.state = MpState::Body;
                    return Ok(Some(part));
                }
            }
        }
    }

    /// The next piece of the current part's body; `None` at its end.
    pub async fn read_chunk(&mut self) -> Result<Option<Bytes>, String> {
        if self.state != MpState::Body {
            return Ok(None);
        }
        loop {
            if let Some(i) = Self::find(&self.buf, &self.delim) {
                if i > 0 {
                    return Ok(Some(self.buf.split_to(i).freeze()));
                }
                let _ = self.buf.split_to(self.delim.len());
                self.state = MpState::Headers;
                return Ok(None);
            }
            // Keep a tail that could be the start of the delimiter.
            let keep = self.delim.len() - 1;
            if self.buf.len() > keep {
                let n = self.buf.len() - keep;
                return Ok(Some(self.buf.split_to(n).freeze()));
            }
            if !self.fill().await? {
                return Err("truncated multipart body".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(body: &[u8], split: usize) -> Vec<(Option<String>, Vec<u8>)> {
        let chunks: Vec<Result<Bytes, std::io::Error>> = body
            .chunks(split)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        let mut mp = Multipart::new(futures_util::stream::iter(chunks), "XyZ".into());
        let mut out = Vec::new();
        while let Some(p) = mp.next_part().await.unwrap() {
            let mut data = Vec::new();
            while let Some(c) = mp.read_chunk().await.unwrap() {
                data.extend_from_slice(&c);
            }
            out.push((p.filename, data));
        }
        out
    }

    #[tokio::test]
    async fn parses_parts_across_any_chunking() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"/tmp/a.txt\"\r\nContent-Type: application/octet-stream\r\n\r\nh\xc3\xa9llo\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b\"\r\n\r\n\r\n--X\r\n--XyZ--\r\n";
        for split in [1, 2, 3, 7, 64, 4096] {
            let parts = parse(body, split).await;
            assert_eq!(
                parts,
                vec![
                    (Some("/tmp/a.txt".into()), "héllo".as_bytes().to_vec()),
                    (Some("b".into()), b"\r\n--X".to_vec()),
                ],
                "split {split}"
            );
        }
    }

    #[test]
    fn boundary_from_content_type() {
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=\"abc\"").as_deref(),
            Some("abc")
        );
        assert!(multipart_boundary("application/octet-stream").is_none());
    }
}
