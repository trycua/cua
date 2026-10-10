// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host's end of a Space's guest mount.
//!
//! [`GuestServers`] serves one Space's view of the volume (a [`Vfs`] for the
//! Space's principal, or for the agent running in it) on loopback, in both
//! shapes a guest mounts: the NFSv3 server (macOS guests, feature `nfs`) and
//! the file operations protocol ([`crate::remote`], Linux guests). cua-spacesd
//! relays the guest's connections as tunnel streams naming the target `nfs`
//! or `fs`; [`GuestServers::dial`] answers them. Both listeners bind
//! 127.0.0.1 only.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::vfs::{FsOps, Vfs};
use crate::{Error, Result};

/// Loopback servers of one Space's view.
pub struct GuestServers {
    vfs: Arc<Vfs>,
    fs_addr: SocketAddr,
    fs_task: tokio::task::JoinHandle<()>,
    #[cfg(feature = "nfs")]
    nfs: Option<crate::nfs::NfsServer>,
}

impl std::fmt::Debug for GuestServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestServers")
            .field("fs", &self.fs_addr)
            .finish()
    }
}

impl GuestServers {
    /// Serves `vfs` for a guest.
    pub async fn start(vfs: Arc<Vfs>) -> Result<GuestServers> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| Error::Backend(format!("volume listener: {e}")))?;
        let fs_addr = listener
            .local_addr()
            .map_err(|e| Error::Backend(e.to_string()))?;
        let ops: Arc<dyn FsOps> = vfs.clone();
        let fs_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let _ = stream.set_nodelay(true);
                tokio::spawn(crate::remote::serve(stream, ops.clone()));
            }
        });
        Ok(GuestServers {
            #[cfg(feature = "nfs")]
            nfs: Some(crate::nfs::NfsServer::start_for_guest(vfs.clone()).await?),
            vfs,
            fs_addr,
            fs_task,
        })
    }

    /// The Space's view.
    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// Where a stream to `target` (`nfs` or `fs`) connects.
    pub fn target(&self, target: &str) -> Option<SocketAddr> {
        match target {
            "fs" => Some(self.fs_addr),
            #[cfg(feature = "nfs")]
            "nfs" => self
                .nfs
                .as_ref()
                .map(|n| SocketAddr::from(([127, 0, 0, 1], n.port()))),
            _ => None,
        }
    }

    /// A dial function for [`cua_hotspot::peer::serve_egress`]: the guest's
    /// streams name a target, never an address.
    pub fn dialer(
        self: &Arc<Self>,
    ) -> impl Fn(
        String,
        u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<tokio::net::TcpStream>> + Send>,
    > + Send
    + Sync
    + 'static {
        let me = self.clone();
        move |target: String, _port: u16| {
            let addr = me.target(&target);
            Box::pin(async move {
                let addr = addr.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("no volume server {target:?}"),
                    )
                })?;
                let s = tokio::net::TcpStream::connect(addr).await?;
                let _ = s.set_nodelay(true);
                Ok(s)
            })
        }
    }

    /// Lands pending writes and stops serving.
    pub async fn stop(self) -> Result<()> {
        self.fs_task.abort();
        #[cfg(feature = "nfs")]
        if let Some(n) = self.nfs {
            return n.stop().await;
        }
        self.vfs.flush_all().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::ROOT;
    use crate::{Context, Drive};

    #[tokio::test]
    async fn guests_reach_only_named_targets() {
        let dir = tempfile::tempdir().unwrap();
        let drive = Drive::open_local(dir.path());
        let vfs = Vfs::new(
            &drive,
            Context::space("local:lab"),
            None,
            None,
            &dir.path().join("m"),
        )
        .unwrap();
        let g = Arc::new(GuestServers::start(vfs).await.unwrap());
        let dial = g.dialer();
        assert!(dial("127.0.0.1".into(), 22).await.is_err());
        let s = dial("fs".into(), 0).await.unwrap();
        let fs = crate::remote::RemoteFs::new(s);
        let names: Vec<String> = fs
            .readdir(ROOT)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(names, ["public", "spaces"]);
        #[cfg(feature = "nfs")]
        assert!(dial("nfs".into(), 0).await.is_ok());
    }
}
