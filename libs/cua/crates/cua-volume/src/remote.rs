// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The filesystem operations over a byte stream: how a Space's guest mounts
//! its view of the volume without holding keys or reaching the store.
//!
//! The host serves one [`FsOps`] (a [`crate::vfs::Vfs`] for that Space's
//! principal, so the access rules, audit and secret scanner apply on the
//! host) with [`serve`]; the guest's mount adapter talks to it through
//! [`RemoteFs`]. In a Space the stream is a tunnel stream carried by
//! cua-spacesd's `/volume` socket, which the host opened; the guest never
//! has a network path to the host.
//!
//! Frames, both directions:
//!
//! ```text
//! [len: u32 BE][json_len: u32 BE][json][data]     len = 4 + json_len + data
//! ```
//!
//! A request's json is a [`Request`]; a reply's is a [`Reply`] with the
//! same `id`. `data` carries the bytes of a `write` request and of a `read`
//! reply. Requests are answered concurrently and may complete out of order.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::vfs::{Attr, FsOps, errno, error_from_errno, libc_errno};
use crate::{Error, Result};

/// Largest frame accepted (a read or write carries at most 1 MiB of data
/// from the kernel; the rest is headroom).
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
/// Requests a server works on at once per connection.
const CONCURRENCY: usize = 64;

/// One operation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ino: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub parent: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub len: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub to_parent: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to_name: String,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// The answer to one [`Request`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u64,
    /// 0 on success, else an errno ([`crate::vfs::libc_errno`]).
    #[serde(default)]
    pub errno: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<Attr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<(String, Attr)>>,
    #[serde(default)]
    pub eof: bool,
}

async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    json: &[u8],
    data: &[u8],
) -> std::io::Result<()> {
    let len = 4 + json.len() + data.len();
    let mut head = Vec::with_capacity(8 + json.len());
    head.extend_from_slice(&(len as u32).to_be_bytes());
    head.extend_from_slice(&(json.len() as u32).to_be_bytes());
    head.extend_from_slice(json);
    w.write_all(&head).await?;
    if !data.is_empty() {
        w.write_all(data).await?;
    }
    w.flush().await
}

async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if !(4..=MAX_FRAME).contains(&len) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let json_len = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if 4 + json_len > len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "json longer than its frame",
        ));
    }
    let data = body.split_off(4 + json_len);
    body.drain(..4);
    Ok(Some((body, data)))
}

async fn answer(fs: &dyn FsOps, q: &Request, data: Vec<u8>) -> (Reply, Vec<u8>) {
    let mut r = Reply {
        id: q.id,
        ..Default::default()
    };
    let res: Result<Vec<u8>> = async {
        match q.op.as_str() {
            "getattr" => r.attr = Some(fs.getattr(q.ino).await?),
            "lookup" => r.attr = Some(fs.lookup(q.parent, &q.name).await?),
            "readdir" => r.entries = Some(fs.readdir(q.ino).await?),
            "read" => {
                let (bytes, eof) = fs.read(q.ino, q.offset, q.len.min(4 << 20)).await?;
                r.eof = eof;
                return Ok(bytes);
            }
            "write" => r.attr = Some(fs.write(q.ino, q.offset, &data).await?),
            "create" => r.attr = Some(fs.create(q.parent, &q.name).await?),
            "truncate" => r.attr = Some(fs.truncate(q.ino, q.len).await?),
            "mkdir" => r.attr = Some(fs.mkdir(q.parent, &q.name).await?),
            "unlink" => fs.unlink(q.parent, &q.name).await?,
            "rmdir" => fs.rmdir(q.parent, &q.name).await?,
            "rename" => {
                fs.rename(q.parent, &q.name, q.to_parent, &q.to_name)
                    .await?
            }
            "flush" => fs.flush_ino(q.ino).await?,
            "flush_all" => fs.flush_all().await?,
            other => return Err(Error::Invalid(format!("unknown operation {other:?}"))),
        }
        Ok(vec![])
    }
    .await;
    match res {
        Ok(bytes) => (r, bytes),
        Err(e) => {
            r.errno = errno(&e);
            r.message = e.to_string();
            (r, vec![])
        }
    }
}

/// Serves `fs` on one stream until it closes. Every request runs as its
/// own task (bounded), so a slow read never holds up the others.
pub async fn serve<S>(stream: S, fs: Arc<dyn FsOps>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::channel::<(Vec<u8>, Vec<u8>)>(CONCURRENCY);
    let out = tokio::spawn(async move {
        while let Some((json, data)) = rx.recv().await {
            if write_frame(&mut writer, &json, &data).await.is_err() {
                break;
            }
        }
    });
    let sem = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let result = loop {
        let (json, data) = match read_frame(&mut reader).await {
            Ok(Some(f)) => f,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        let q: Request = match serde_json::from_slice(&json) {
            Ok(q) => q,
            Err(e) => break Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        };
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break Ok(());
        };
        let (fs, tx) = (fs.clone(), tx.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let (reply, bytes) = answer(fs.as_ref(), &q, data).await;
            if let Ok(json) = serde_json::to_vec(&reply) {
                let _ = tx.send((json, bytes)).await;
            }
        });
    };
    drop(tx);
    let _ = out.await;
    result
}

type Pending = HashMap<u64, oneshot::Sender<(Reply, Vec<u8>)>>;

/// The client side: [`FsOps`] over a stream to a [`serve`]r.
pub struct RemoteFs {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: Arc<std::sync::Mutex<Pending>>,
    next: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for RemoteFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteFs")
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl RemoteFs {
    /// A client over `stream`. Calls fail with an I/O error once the stream
    /// closes.
    pub fn new<S>(stream: S) -> Arc<RemoteFs>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut reader, writer) = tokio::io::split(stream);
        let pending: Arc<std::sync::Mutex<Pending>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (p, c) = (pending.clone(), closed.clone());
        tokio::spawn(async move {
            while let Ok(Some((json, data))) = read_frame(&mut reader).await {
                let Ok(reply) = serde_json::from_slice::<Reply>(&json) else {
                    break;
                };
                if let Some(tx) = p.lock().unwrap().remove(&reply.id) {
                    let _ = tx.send((reply, data));
                }
            }
            c.store(true, Ordering::SeqCst);
            // Every caller still waiting learns the stream is gone.
            p.lock().unwrap().clear();
        });
        Arc::new(RemoteFs {
            writer: Mutex::new(Box::new(writer)),
            pending,
            next: AtomicU64::new(1),
            closed,
        })
    }

    /// Whether the stream has closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn call(&self, mut q: Request, data: &[u8]) -> Result<(Reply, Vec<u8>)> {
        if self.is_closed() {
            return Err(Error::Backend("the volume's host is not connected".into()));
        }
        q.id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(q.id, tx);
        let json = serde_json::to_vec(&q)?;
        {
            let mut w = self.writer.lock().await;
            if let Err(e) = write_frame(&mut *w, &json, data).await {
                self.pending.lock().unwrap().remove(&q.id);
                return Err(Error::Backend(format!("the volume's host: {e}")));
            }
        }
        let (reply, bytes) = rx
            .await
            .map_err(|_| Error::Backend("the volume's host went away".into()))?;
        if reply.errno != 0 {
            return Err(error_from_errno(reply.errno, reply.message));
        }
        Ok((reply, bytes))
    }

    async fn attr(&self, q: Request, data: &[u8]) -> Result<Attr> {
        self.call(q, data)
            .await?
            .0
            .attr
            .ok_or_else(|| Error::Backend("reply without attributes".into()))
    }
}

fn req(op: &str) -> Request {
    Request {
        op: op.into(),
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl FsOps for RemoteFs {
    async fn getattr(&self, ino: u64) -> Result<Attr> {
        self.attr(
            Request {
                ino,
                ..req("getattr")
            },
            &[],
        )
        .await
    }
    async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
        self.attr(
            Request {
                parent,
                name: name.into(),
                ..req("lookup")
            },
            &[],
        )
        .await
    }
    async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>> {
        Ok(self
            .call(
                Request {
                    ino,
                    ..req("readdir")
                },
                &[],
            )
            .await?
            .0
            .entries
            .unwrap_or_default())
    }
    async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
        let (r, bytes) = self
            .call(
                Request {
                    ino,
                    offset,
                    len,
                    ..req("read")
                },
                &[],
            )
            .await?;
        Ok((bytes, r.eof))
    }
    async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr> {
        self.attr(
            Request {
                ino,
                offset,
                ..req("write")
            },
            data,
        )
        .await
    }
    async fn create(&self, parent: u64, name: &str) -> Result<Attr> {
        self.attr(
            Request {
                parent,
                name: name.into(),
                ..req("create")
            },
            &[],
        )
        .await
    }
    async fn truncate(&self, ino: u64, size: u64) -> Result<Attr> {
        self.attr(
            Request {
                ino,
                len: size,
                ..req("truncate")
            },
            &[],
        )
        .await
    }
    async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr> {
        self.attr(
            Request {
                parent,
                name: name.into(),
                ..req("mkdir")
            },
            &[],
        )
        .await
    }
    async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
        self.call(
            Request {
                parent,
                name: name.into(),
                ..req("unlink")
            },
            &[],
        )
        .await
        .map(|_| ())
    }
    async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
        self.call(
            Request {
                parent,
                name: name.into(),
                ..req("rmdir")
            },
            &[],
        )
        .await
        .map(|_| ())
    }
    async fn rename(&self, from_dir: u64, from: &str, to_dir: u64, to: &str) -> Result<()> {
        self.call(
            Request {
                parent: from_dir,
                name: from.into(),
                to_parent: to_dir,
                to_name: to.into(),
                ..req("rename")
            },
            &[],
        )
        .await
        .map(|_| ())
    }
    async fn flush_ino(&self, ino: u64) -> Result<()> {
        self.call(
            Request {
                ino,
                ..req("flush")
            },
            &[],
        )
        .await
        .map(|_| ())
    }
    async fn flush_all(&self) -> Result<()> {
        self.call(req("flush_all"), &[]).await.map(|_| ())
    }
}

/// The errno a caller sees when the host is gone.
pub const DISCONNECTED: i32 = libc_errno::EIO;

/// Most bytes [`WriteBack`] holds for one file before sending them.
pub const WRITE_BACK_BYTES: usize = 1 << 20;

/// Sequential writes gathered into larger ones before they cross to the
/// host: a guest kernel may hand a FUSE filesystem small writes, and each
/// is a round trip. Held bytes go when they reach [`WRITE_BACK_BYTES`], when
/// a write is not contiguous, before any other operation on the file (and
/// before listings and namespace changes, so no stale size is seen), on
/// close ([`FsOps::sync_writes`]), fsync and unmount. A write that fails
/// after it was acknowledged fails the file's next close or fsync, as with
/// any write-back cache.
pub struct WriteBack {
    inner: Arc<dyn FsOps>,
    held: Mutex<HashMap<u64, Held>>,
    failed: std::sync::Mutex<HashMap<u64, (i32, String)>>,
}

struct Held {
    offset: u64,
    data: Vec<u8>,
}

impl WriteBack {
    pub fn new(inner: Arc<dyn FsOps>) -> Arc<WriteBack> {
        Arc::new(WriteBack {
            inner,
            held: Mutex::new(HashMap::new()),
            failed: std::sync::Mutex::new(HashMap::new()),
        })
    }

    async fn send(&self, ino: u64, h: Held) -> Result<()> {
        match self.inner.write(ino, h.offset, &h.data).await {
            Ok(_) => Ok(()),
            Err(e) => {
                self.failed
                    .lock()
                    .unwrap()
                    .insert(ino, (errno(&e), e.to_string()));
                Err(e)
            }
        }
    }

    async fn drain(&self, ino: u64) -> Result<()> {
        let h = self.held.lock().await.remove(&ino);
        match h {
            Some(h) => self.send(ino, h).await,
            None => Ok(()),
        }
    }

    async fn drain_all(&self) -> Result<()> {
        let all: Vec<(u64, Held)> = self.held.lock().await.drain().collect();
        let mut first = Ok(());
        for (ino, h) in all {
            if let Err(e) = self.send(ino, h).await
                && first.is_ok()
            {
                first = Err(e);
            }
        }
        first
    }

    /// A write to `ino` that failed after it was acknowledged, once.
    fn take_failed(&self, ino: u64) -> Result<()> {
        match self.failed.lock().unwrap().remove(&ino) {
            Some((code, msg)) => Err(error_from_errno(code, msg)),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl FsOps for WriteBack {
    async fn getattr(&self, ino: u64) -> Result<Attr> {
        self.drain(ino).await?;
        self.inner.getattr(ino).await
    }
    async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
        self.drain_all().await?;
        self.inner.lookup(parent, name).await
    }
    async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>> {
        self.drain_all().await?;
        self.inner.readdir(ino).await
    }
    async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
        self.drain(ino).await?;
        self.inner.read(ino, offset, len).await
    }
    async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr> {
        let end = offset + data.len() as u64;
        let acknowledged = Attr {
            ino,
            is_dir: false,
            size: end,
            mtime_ms: crate::now_ms(),
            writable: true,
        };
        let mut held = self.held.lock().await;
        if let Some(h) = held.get_mut(&ino)
            && h.offset + h.data.len() as u64 == offset
            && h.data.len() + data.len() <= WRITE_BACK_BYTES
        {
            h.data.extend_from_slice(data);
            return Ok(acknowledged);
        }
        let before = held.remove(&ino);
        if data.len() >= WRITE_BACK_BYTES {
            drop(held);
            if let Some(b) = before {
                self.send(ino, b).await?;
            }
            return self.inner.write(ino, offset, data).await;
        }
        held.insert(
            ino,
            Held {
                offset,
                data: data.to_vec(),
            },
        );
        drop(held);
        if let Some(b) = before {
            self.send(ino, b).await?;
        }
        Ok(acknowledged)
    }
    async fn create(&self, parent: u64, name: &str) -> Result<Attr> {
        self.drain_all().await?;
        self.inner.create(parent, name).await
    }
    async fn truncate(&self, ino: u64, size: u64) -> Result<Attr> {
        self.drain(ino).await?;
        self.inner.truncate(ino, size).await
    }
    async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr> {
        self.drain_all().await?;
        self.inner.mkdir(parent, name).await
    }
    async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
        self.drain_all().await?;
        self.inner.unlink(parent, name).await
    }
    async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
        self.drain_all().await?;
        self.inner.rmdir(parent, name).await
    }
    async fn rename(&self, from_dir: u64, from: &str, to_dir: u64, to: &str) -> Result<()> {
        self.drain_all().await?;
        self.inner.rename(from_dir, from, to_dir, to).await
    }
    async fn flush_ino(&self, ino: u64) -> Result<()> {
        self.drain(ino).await?;
        self.take_failed(ino)?;
        self.inner.flush_ino(ino).await
    }
    async fn flush_all(&self) -> Result<()> {
        let drained = self.drain_all().await;
        let flushed = self.inner.flush_all().await;
        drained.and(flushed)
    }
    async fn sync_writes(&self, ino: u64) -> Result<()> {
        self.drain(ino).await?;
        self.take_failed(ino)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{ROOT, Vfs};
    use crate::{Condition, Context, Drive};

    #[tokio::test]
    async fn a_space_sees_its_view_through_the_stream() {
        let dir = tempfile::tempdir().unwrap();
        let drive = Drive::open_local(dir.path());
        drive
            .session(Context::user())
            .write("public/rules.md", b"be kind".to_vec(), Condition::None)
            .await
            .unwrap();
        let big: Vec<u8> = (0..(3u32 << 20)).map(|i| (i % 251) as u8).collect();
        drive
            .session(Context::user())
            .write("public/big.bin", big.clone(), Condition::None)
            .await
            .unwrap();
        // The host serves the Space's own view (not the user's).
        let vfs = Vfs::new(
            &drive,
            Context::space("local:lab"),
            None,
            None,
            &dir.path().join("m"),
        )
        .unwrap();
        let (host, guest) = tokio::io::duplex(1 << 20);
        tokio::spawn(serve(host, vfs.clone() as Arc<dyn FsOps>));
        let fs = RemoteFs::new(guest);
        let names: Vec<String> = fs
            .readdir(ROOT)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(names, ["public", "spaces"]);
        let public = fs.lookup(ROOT, "public").await.unwrap();
        let rules = fs.lookup(public.ino, "rules.md").await.unwrap();
        assert_eq!(
            fs.read(rules.ino, 0, 100).await.unwrap(),
            (b"be kind".to_vec(), true)
        );
        // Parallel ranged reads of a large file, answered out of order.
        let b = fs.lookup(public.ino, "big.bin").await.unwrap();
        let mut tasks = vec![];
        for i in 0..6u64 {
            let fs = fs.clone();
            tasks.push(tokio::spawn(async move {
                fs.read(b.ino, i * 500_000, 65536).await.unwrap().0
            }));
        }
        for (i, t) in tasks.into_iter().enumerate() {
            let got = t.await.unwrap();
            let at = i * 500_000;
            assert_eq!(got, &big[at..at + 65536]);
        }
        // public/ is read-only for a Space; its own folder is not.
        let e = fs.create(public.ino, "x.md").await.unwrap_err();
        assert_eq!(e.tag(), "forbidden");
        let spaces = fs.lookup(ROOT, "spaces").await.unwrap();
        let mine = fs.lookup(spaces.ino, "local-lab").await.unwrap();
        let f = fs.create(mine.ino, "out.txt").await.unwrap();
        fs.write(f.ino, 0, b"from the guest").await.unwrap();
        fs.flush_ino(f.ino).await.unwrap();
        assert_eq!(
            drive
                .session(Context::user())
                .read("spaces/local-lab/out.txt", None)
                .await
                .unwrap()
                .0,
            b"from the guest"
        );
        // Errors keep their kind across the wire.
        assert_eq!(
            fs.lookup(ROOT, "nope").await.unwrap_err().tag(),
            "not_found"
        );
        assert_eq!(
            fs.rmdir(spaces.ino, "local-lab").await.unwrap_err().tag(),
            "precondition_failed"
        );
    }

    /// Counts the writes that reach the host.
    struct Count(Arc<dyn FsOps>, AtomicU64);

    #[async_trait::async_trait]
    impl FsOps for Count {
        async fn getattr(&self, ino: u64) -> Result<Attr> {
            self.0.getattr(ino).await
        }
        async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
            self.0.lookup(parent, name).await
        }
        async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>> {
            self.0.readdir(ino).await
        }
        async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
            self.0.read(ino, offset, len).await
        }
        async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.write(ino, offset, data).await
        }
        async fn create(&self, parent: u64, name: &str) -> Result<Attr> {
            self.0.create(parent, name).await
        }
        async fn truncate(&self, ino: u64, size: u64) -> Result<Attr> {
            self.0.truncate(ino, size).await
        }
        async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr> {
            self.0.mkdir(parent, name).await
        }
        async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
            self.0.unlink(parent, name).await
        }
        async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
            self.0.rmdir(parent, name).await
        }
        async fn rename(&self, a: u64, b: &str, c: u64, d: &str) -> Result<()> {
            self.0.rename(a, b, c, d).await
        }
        async fn flush_ino(&self, ino: u64) -> Result<()> {
            self.0.flush_ino(ino).await
        }
        async fn flush_all(&self) -> Result<()> {
            self.0.flush_all().await
        }
    }

    #[tokio::test]
    async fn small_sequential_writes_cross_as_large_ones() {
        let dir = tempfile::tempdir().unwrap();
        let drive = Drive::open_local(dir.path());
        let vfs = Vfs::new(&drive, Context::user(), None, None, &dir.path().join("m")).unwrap();
        let count = Arc::new(Count(vfs, AtomicU64::new(0)));
        let wb = WriteBack::new(count.clone());
        let public = wb.lookup(ROOT, "public").await.unwrap();
        let f = wb.create(public.ino, "big.bin").await.unwrap();
        // 3 MiB in 4 KiB writes: three round trips, not 768.
        let chunk = vec![7u8; 4096];
        for i in 0..768u64 {
            wb.write(f.ino, i * 4096, &chunk).await.unwrap();
        }
        // A read sees every byte (held ones go first).
        let (tail, _) = wb.read(f.ino, 3 * 1048576 - 4096, 4096).await.unwrap();
        assert_eq!(tail, chunk);
        assert_eq!(count.1.load(Ordering::SeqCst), 3);
        // A gap sends what is held; close lands it.
        wb.write(f.ino, 10 << 20, b"end").await.unwrap();
        wb.write(f.ino, 0, b"start").await.unwrap();
        wb.sync_writes(f.ino).await.unwrap();
        wb.flush_ino(f.ino).await.unwrap();
        let (got, _) = drive
            .session(Context::user())
            .read("public/big.bin", None)
            .await
            .unwrap();
        assert_eq!(got.len(), (10 << 20) + 3);
        assert_eq!(&got[..5], b"start");
        assert_eq!(&got[10 << 20..], b"end");
    }

    #[tokio::test]
    async fn a_write_refused_after_it_was_held_fails_the_close() {
        let dir = tempfile::tempdir().unwrap();
        let drive = Drive::open_local(dir.path());
        drive
            .session(Context::user())
            .write("public/ro.md", b"x".to_vec(), Condition::None)
            .await
            .unwrap();
        // A Space may read public/ but not write it.
        let vfs = Vfs::new(
            &drive,
            Context::space("local:lab"),
            None,
            None,
            &dir.path().join("m"),
        )
        .unwrap();
        let wb = WriteBack::new(vfs);
        let public = wb.lookup(ROOT, "public").await.unwrap();
        let f = wb.lookup(public.ino, "ro.md").await.unwrap();
        // Held, so acknowledged...
        wb.write(f.ino, 0, b"y").await.unwrap();
        // ...and the close reports the refusal.
        let e = wb.sync_writes(f.ino).await.unwrap_err();
        assert!(matches!(e, Error::Forbidden(_)), "{e:?}");
    }

    #[tokio::test]
    async fn calls_fail_cleanly_when_the_host_goes_away() {
        let (host, guest) = tokio::io::duplex(4096);
        let fs = RemoteFs::new(guest);
        drop(host);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let e = fs.getattr(ROOT).await.unwrap_err();
        assert_eq!(e.tag(), "backend");
        assert!(fs.is_closed());
    }
}
