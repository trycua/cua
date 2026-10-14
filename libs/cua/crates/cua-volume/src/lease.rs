// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One writer per agent home.
//!
//! Harnesses keep SQLite files and memory notes in their home, and two runs
//! writing one home would interleave them. The lease is an object at
//! `agents/<agent>/.cua-lease` taken with a create-only (or
//! compare-and-swap) write, so exactly one holder wins even across machines
//! on an S3 backend. It expires on its own (a crashed holder never locks an
//! agent out for good) and is renewed while the run lives.

use serde::{Deserialize, Serialize};

use crate::backend::Condition;
use crate::{Drive, Error, Result, now_ms, path};

/// The lease object's name inside an agent home.
pub const LEASE_NAME: &str = ".cua-lease";

/// Who holds a home, and until when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseInfo {
    /// A stable id for the holder (for example `local:work/run-1a2b3c4d`).
    pub holder: String,
    pub acquired_ms: u64,
    pub expires_ms: u64,
}

/// A held lease. Renew it before `expires_ms`; release it when done.
#[derive(Clone, Debug)]
pub struct Lease {
    pub agent: String,
    pub info: LeaseInfo,
    etag: String,
}

fn key(agent: &str) -> Result<String> {
    Ok(format!("{}{LEASE_NAME}", path::agent_home(agent)?))
}

impl Lease {
    /// Takes the home of `agent` for `holder` for `ttl_ms`. The same holder
    /// may take it again (a restarted run); anyone else waits for expiry.
    pub async fn acquire(drive: &Drive, agent: &str, holder: &str, ttl_ms: u64) -> Result<Lease> {
        let k = key(agent)?;
        let b = drive.backend();
        let now = now_ms();
        let info = LeaseInfo {
            holder: holder.into(),
            acquired_ms: now,
            expires_ms: now + ttl_ms,
        };
        let cond = match b.head(&k).await? {
            None => Condition::IfNoneMatch,
            Some(m) => {
                let (bytes, _) = b.get(&k, None).await?;
                let cur: LeaseInfo = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Backend(format!("lease: {e}")))?;
                if cur.holder != holder && cur.expires_ms > now {
                    return Err(Error::LeaseHeld {
                        holder: cur.holder,
                        expires_ms: cur.expires_ms,
                    });
                }
                Condition::IfMatch(m.etag)
            }
        };
        let m = b
            .put(&k, serde_json::to_vec(&info)?, cond)
            .await
            .map_err(|e| match e {
                Error::Precondition(_) => Error::LeaseHeld {
                    holder: "another writer (it just took the lease)".into(),
                    expires_ms: now + ttl_ms,
                },
                other => other,
            })?;
        let _ = drive.audit().append(
            &format!("agent:{agent}"),
            "lease",
            &k,
            &format!("acquired by {holder}"),
        );
        Ok(Lease {
            agent: agent.into(),
            info,
            etag: m.etag,
        })
    }

    /// Extends the lease; fails if someone else took it (after expiry).
    pub async fn renew(&mut self, drive: &Drive, ttl_ms: u64) -> Result<()> {
        let k = key(&self.agent)?;
        let mut info = self.info.clone();
        info.expires_ms = now_ms() + ttl_ms;
        let m = drive
            .backend()
            .put(
                &k,
                serde_json::to_vec(&info)?,
                Condition::IfMatch(self.etag.clone()),
            )
            .await
            .map_err(|e| match e {
                Error::Precondition(_) => Error::LeaseHeld {
                    holder: "another writer".into(),
                    expires_ms: 0,
                },
                other => other,
            })?;
        self.info = info;
        self.etag = m.etag;
        Ok(())
    }

    /// Gives the home back (no-op if someone else already took it).
    pub async fn release(self, drive: &Drive) -> Result<()> {
        let k = key(&self.agent)?;
        match drive
            .backend()
            .delete(&k, Condition::IfMatch(self.etag))
            .await
        {
            Ok(()) | Err(Error::Precondition(_)) | Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let _ = drive.audit().append(
            &format!("agent:{}", self.agent),
            "lease",
            &k,
            &format!("released by {}", self.info.holder),
        );
        Ok(())
    }

    /// Who holds `agent`'s home right now, if anyone.
    pub async fn holder(drive: &Drive, agent: &str) -> Result<Option<LeaseInfo>> {
        let k = key(agent)?;
        match drive.backend().get(&k, None).await {
            Ok((bytes, _)) => {
                let info: LeaseInfo = serde_json::from_slice(&bytes)?;
                Ok((info.expires_ms > now_ms()).then_some(info))
            }
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_writer_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::open_local(dir.path());
        let mut a = Lease::acquire(&d, "ada", "local:a/run-1", 60_000)
            .await
            .unwrap();
        let e = Lease::acquire(&d, "ada", "cloud:b/run-2", 60_000)
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "lease_held");
        // The same holder may take it again (a restarted run).
        Lease::acquire(&d, "ada", "local:a/run-1", 60_000)
            .await
            .unwrap();
        // ...which makes the old handle stale.
        assert_eq!(a.renew(&d, 60_000).await.unwrap_err().tag(), "lease_held");
        let b = Lease::acquire(&d, "ada", "local:a/run-1", 60_000)
            .await
            .unwrap();
        assert_eq!(
            Lease::holder(&d, "ada").await.unwrap().unwrap().holder,
            "local:a/run-1"
        );
        b.release(&d).await.unwrap();
        assert!(Lease::holder(&d, "ada").await.unwrap().is_none());
        Lease::acquire(&d, "ada", "cloud:b/run-2", 60_000)
            .await
            .unwrap();
        // Expired leases are taken over.
        let _c = Lease::acquire(&d, "bob", "x", 0).await.unwrap();
        Lease::acquire(&d, "bob", "y", 60_000).await.unwrap();
    }
}
