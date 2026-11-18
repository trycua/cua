// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Moving big objects between machines in blocks.
//!
//! The receiver pulls. It asks the sender for the SHA-256 of every 1 MiB
//! block of a version, keeps the blocks it already has (from a partial file
//! of an earlier try, or from its own previous version of the key at the
//! same offsets), fetches the rest a few at a time, checks each block's
//! hash on arrival, and checks the whole object's hash before it is used.
//! So a link that drops and returns costs the blocks in flight, not the file,
//! and a file that only changed at its end costs its end.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::task::JoinSet;

use super::link::Link;
use super::node::Inner;
use super::wire::{Msg, Src};
use crate::backend::ObjectMeta;
use crate::{Error, Result, sha256_hex};

#[cfg(unix)]
fn read_at(f: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

fn read_block(path: &std::path::Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    if offset >= size {
        return Ok(vec![]);
    }
    let want = len.min((size - offset) as usize);
    let mut buf = vec![0u8; want];
    let mut done = 0;
    while done < want {
        let n = read_at(&f, &mut buf[done..], offset + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    buf.truncate(done);
    Ok(buf)
}

fn io_err(e: std::io::Error) -> Error {
    if e.raw_os_error() == Some(28) {
        Error::Backend("the disk is full".into())
    } else {
        Error::from(e)
    }
}

impl Inner {
    fn src_path(&self, src: &Src) -> Result<PathBuf> {
        match src {
            Src::Data { key, version } => self.store.version_file(key, version),
            Src::Spool { key, version } => self.spool.version_file(key, version),
        }
    }

    async fn hashes_of(&self, path: PathBuf, block: u64) -> Result<(Arc<Vec<String>>, u64)> {
        let ck = format!("{}:{block}", path.display());
        if let Some(h) = self.hashes.lock().expect("hashes").get(&ck).cloned() {
            let size = std::fs::metadata(&path)?.len();
            return Ok((h, size));
        }
        let p = path.clone();
        let (hashes, size) = tokio::task::spawn_blocking(move || -> Result<(Vec<String>, u64)> {
            use std::io::Read as _;
            let mut f = std::fs::File::open(&p)?;
            let size = f.metadata()?.len();
            let mut out = vec![];
            let mut buf = vec![0u8; block as usize];
            loop {
                let mut n = 0;
                while n < buf.len() {
                    let r = f.read(&mut buf[n..])?;
                    if r == 0 {
                        break;
                    }
                    n += r;
                }
                if n == 0 {
                    break;
                }
                out.push(sha256_hex(&buf[..n]));
                if n < buf.len() {
                    break;
                }
            }
            Ok((out, size))
        })
        .await
        .map_err(|e| Error::Backend(e.to_string()))??;
        let arc = Arc::new(hashes);
        let mut c = self.hashes.lock().expect("hashes");
        if c.len() > 32
            && let Some(k) = c.keys().next().cloned()
        {
            c.remove(&k);
        }
        c.insert(ck, arc.clone());
        Ok((arc, size))
    }

    pub(super) async fn handle_hashes(&self, src: Src, block: u64, from: u64, count: u64) -> Msg {
        let r = async {
            let path = self.src_path(&src)?;
            let (all, size) = self.hashes_of(path, block).await?;
            let from = (from as usize).min(all.len());
            let to = from.saturating_add(count as usize).min(all.len());
            Ok::<_, Error>(Msg::HashesAre {
                hashes: all[from..to].to_vec(),
                size,
            })
        }
        .await;
        r.unwrap_or_else(|e| Msg::Error {
            kind: e.tag().into(),
            message: e.to_string(),
        })
    }

    pub(super) async fn handle_block(&self, src: Src, block: u64, idx: u64) -> Result<Vec<u8>> {
        let path = self.src_path(&src)?;
        tokio::task::spawn_blocking(move || read_block(&path, idx * block, block as usize))
            .await
            .map_err(|e| Error::Backend(e.to_string()))?
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotFound("that version is not here".into()),
                _ => Error::from(e),
            })
    }

    /// Fetches `meta` from `link`'s machine into a file in the incoming
    /// directory, verified, and returns its path. `seed` is this machine's
    /// own previous version of the key, whose unchanged blocks are reused.
    pub(super) async fn pull(
        self: &Arc<Self>,
        link: &Arc<Link>,
        src: Src,
        meta: &ObjectMeta,
        seed: Option<&ObjectMeta>,
    ) -> Result<PathBuf> {
        let block = self.cfg.block;
        let size = meta.size;
        let nblocks = size.div_ceil(block);
        let inc = self.dir.join("incoming");
        std::fs::create_dir_all(&inc)?;
        let stem = format!("{}-{}", meta.etag, size);
        let part = inc.join(format!("{stem}.part"));
        let havef = inc.join(format!("{stem}.have"));
        cua_home::guard_write(&part)?;
        // What an earlier attempt left.
        let mut have: Vec<bool> = match (std::fs::read(&havef), part.is_file()) {
            (Ok(b), true) if b.len() as u64 == nblocks => b.iter().map(|x| *x == 1).collect(),
            _ => vec![false; nblocks as usize],
        };
        if !part.is_file() {
            have = vec![false; nblocks as usize];
        }
        {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&part)
                .map_err(io_err)?;
            f.set_len(size).map_err(io_err)?;
        }
        // The hashes, in pages.
        let mut hashes: Vec<String> = Vec::with_capacity(nblocks as usize);
        let to = self.cfg.data_timeout;
        while (hashes.len() as u64) < nblocks {
            let from = hashes.len() as u64;
            let (m, _) = link
                .call_strict(
                    Msg::Hashes {
                        src: src.clone(),
                        block,
                        from,
                        count: 4096,
                    },
                    &[],
                    to,
                )
                .await
                .map_err(|e| Error::Backend(e.to_string()))?;
            match m {
                Msg::HashesAre { hashes: h, size: s } => {
                    if s != size || h.is_empty() {
                        return Err(Error::Backend(
                            "the sender's version is not the one announced".into(),
                        ));
                    }
                    hashes.extend(h);
                }
                Msg::Error { kind, message } => {
                    return Err(super::wire::Rep::into_error(&kind, message));
                }
                _ => return Err(Error::Backend("unexpected reply".into())),
            }
        }
        // Blocks this machine already holds from its own previous version.
        let seed_path = seed.and_then(|h| self.store.version_file(&h.key, &h.version).ok());
        let mut need: Vec<u64> = vec![];
        for idx in 0..nblocks {
            if have[idx as usize] {
                continue;
            }
            if let Some(sp) = &seed_path
                && let Ok(b) = read_block(sp, idx * block, block as usize)
                && !b.is_empty()
                && sha256_hex(&b) == hashes[idx as usize]
            {
                write_at(&part, idx * block, &b).map_err(io_err)?;
                have[idx as usize] = true;
                continue;
            }
            need.push(idx);
        }
        persist_have(&havef, &have);
        // The rest, a window at a time.
        let hashes = Arc::new(hashes);
        let mut set: JoinSet<Result<(u64, Vec<u8>)>> = JoinSet::new();
        let mut next = 0usize;
        let mut since_persist = 0;
        let window = self.cfg.window.max(1);
        let mut failed: Option<Error> = None;
        while next < need.len() || !set.is_empty() {
            while failed.is_none() && next < need.len() && set.len() < window {
                let idx = need[next];
                next += 1;
                let (link, src, hashes) = (link.clone(), src.clone(), hashes.clone());
                set.spawn(async move {
                    let (m, bytes) = link
                        .call_strict(Msg::Block { src, block, idx }, &[], to)
                        .await
                        .map_err(|e| Error::Backend(e.to_string()))?;
                    match m {
                        Msg::BlockIs => {}
                        Msg::Error { kind, message } => {
                            return Err(super::wire::Rep::into_error(&kind, message));
                        }
                        _ => return Err(Error::Backend("unexpected reply".into())),
                    }
                    if sha256_hex(&bytes) != hashes[idx as usize] {
                        return Err(Error::Backend(format!("block {idx} arrived damaged")));
                    }
                    Ok((idx, bytes))
                });
            }
            let Some(joined) = set.join_next().await else {
                break;
            };
            match joined {
                Ok(Ok((idx, bytes))) => {
                    write_at(&part, idx * block, &bytes).map_err(io_err)?;
                    have[idx as usize] = true;
                    since_persist += 1;
                    if since_persist >= 16 {
                        since_persist = 0;
                        persist_have(&havef, &have);
                    }
                }
                Ok(Err(e)) => failed = Some(e),
                Err(e) => failed = Some(Error::Backend(e.to_string())),
            }
        }
        persist_have(&havef, &have);
        if let Some(e) = failed {
            return Err(e);
        }
        // The whole object, once.
        let p = part.clone();
        let etag = meta.etag.clone();
        let ok = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
            use sha2::Digest as _;
            use std::io::Read as _;
            let mut f = std::fs::File::open(&p)?;
            let mut h = sha2::Sha256::new();
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex::encode(h.finalize()) == etag)
        })
        .await
        .map_err(|e| Error::Backend(e.to_string()))?
        .map_err(io_err)?;
        if !ok {
            let _ = std::fs::remove_file(&part);
            let _ = std::fs::remove_file(&havef);
            return Err(Error::Backend(
                "the object did not match its hash; it will be fetched again".into(),
            ));
        }
        let _ = std::fs::remove_file(&havef);
        Ok(part)
    }
}

fn write_at(path: &std::path::Path, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().write(true).open(path)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::write_all_at(&f, bytes, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < bytes.len() {
            done += std::os::windows::fs::FileExt::seek_write(
                &f,
                &bytes[done..],
                offset + done as u64,
            )?;
        }
        Ok(())
    }
}

fn persist_have(path: &std::path::Path, have: &[bool]) {
    let bytes: Vec<u8> = have.iter().map(|b| u8::from(*b)).collect();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}
