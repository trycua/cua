//! A booted, prepared base sandbox per shape, forked for each create.
//!
//! A Cua VM takes seconds to boot (and a cold first image pull far longer),
//! while E2B's create takes a fraction of a second and its SDKs give up on
//! a create after 60 s. So the first create of a shape (default image, size,
//! kind, template variant) boots one sandbox, prepares it as a create
//! would (default user, umask, the code-interpreter server), saves its
//! memory and disk as a checkpoint, and deletes it; this and every later
//! create of the shape is a fork of that checkpoint, about a second
//! whatever the concurrency. A fork is a new machine (its own token, keys,
//! machine id and random state), and is made before any user code runs.
//!
//! Nothing here is visible to a client: the checkpoints are named
//! `e2b-golden-<hash>` (hidden from `list_snapshots`), and a create that
//! cannot use one (a fork that fails, a shape that is not forkable, a build
//! that failed recently) boots cold as it always did.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};

use crate::backend::{CreateSpec, Identity, Placement, Source};

use super::E2b;

/// Prefix of the checkpoint names; hidden from the snapshot list.
pub const PREFIX: &str = "e2b-golden-";
/// A shape whose build failed is booted cold for this long.
const COOLDOWN: Duration = Duration::from_secs(120);
/// At most this many shapes are kept (each holds a memory image).
const MAX_SHAPES: usize = 4;

#[derive(Clone)]
enum Entry {
    Building(Arc<Notify>),
    Ready(String),
    Failed(Instant),
}

#[derive(Default)]
pub struct Golden {
    shapes: Mutex<HashMap<String, Entry>>,
}

/// What differs between bases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Base,
    CodeInterpreter,
}

pub fn is_golden(checkpoint: &str) -> bool {
    checkpoint.starts_with(PREFIX)
}

fn shape_key(who: &Identity, variant: Variant, p: &Placement) -> String {
    format!(
        "{}|{}|{variant:?}|{:?}|{:?}|{:?}|{:?}|{}",
        who.daemon.as_deref().unwrap_or(""),
        who.token.as_deref().unwrap_or(""),
        p.on,
        p.kind,
        p.cpus,
        p.memory_mb,
        env!("CARGO_PKG_VERSION"),
    )
}

fn checkpoint_name(key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    cua_image::canonical::canonical_image_or_linux("linux").hash(&mut h);
    format!("{PREFIX}{:016x}", h.finish())
}

impl Golden {
    /// The checkpoint to fork a new sandbox of this shape from, building it
    /// if this is the first (others wait for that build); `None` boots cold.
    pub async fn checkpoint_for(
        &self,
        e: &E2b,
        who: &Identity,
        variant: Variant,
        p: &Placement,
    ) -> Option<String> {
        if !e.backend.forks_fast(p) {
            return None;
        }
        let key = shape_key(who, variant, p);
        loop {
            let notify = {
                let mut shapes = self.shapes.lock().await;
                match shapes.get(&key).cloned() {
                    Some(Entry::Ready(cp)) => return Some(cp),
                    Some(Entry::Failed(at)) if at.elapsed() < COOLDOWN => return None,
                    Some(Entry::Building(n)) => Some(n),
                    other => {
                        let live = shapes
                            .values()
                            .filter(|v| !matches!(v, Entry::Failed(_)))
                            .count();
                        if other.is_none() && live >= MAX_SHAPES {
                            return None;
                        }
                        shapes.insert(key.clone(), Entry::Building(Arc::new(Notify::new())));
                        None
                    }
                }
            };
            match notify {
                Some(n) => {
                    // Waiting for another create's build; re-check after.
                    let _ = tokio::time::timeout(Duration::from_secs(600), n.notified()).await;
                }
                None => return self.build(e, who, variant, p, key).await,
            }
        }
    }

    async fn build(
        &self,
        e: &E2b,
        who: &Identity,
        variant: Variant,
        p: &Placement,
        key: String,
    ) -> Option<String> {
        let name = checkpoint_name(&key);
        let result = Self::boot_and_save(e, who, variant, p, &name).await;
        let mut shapes = self.shapes.lock().await;
        let notify = match shapes.get(&key) {
            Some(Entry::Building(n)) => Some(n.clone()),
            _ => None,
        };
        let out = match result {
            Ok(()) => {
                shapes.insert(key, Entry::Ready(name.clone()));
                Some(name)
            }
            Err(err) => {
                tracing::warn!("compat: building the base sandbox failed, creating cold: {err}");
                shapes.insert(key, Entry::Failed(Instant::now()));
                None
            }
        };
        drop(shapes);
        if let Some(n) = notify {
            n.notify_waiters();
        }
        out
    }

    async fn boot_and_save(
        e: &E2b,
        who: &Identity,
        variant: Variant,
        p: &Placement,
        cp: &str,
    ) -> Result<(), String> {
        // A checkpoint of this name from an earlier run of the gateway is
        // reused when the daemon still has it.
        if e.backend
            .checkpoints(who)
            .await
            .map_err(|x| x.to_string())?
            .iter()
            .any(|c| c.name == cp)
        {
            return Ok(());
        }
        let sandbox = format!("{cp}-boot");
        let created = e
            .backend
            .create(
                who,
                CreateSpec {
                    name: sandbox.clone(),
                    source: Source::Image(None),
                    env: Default::default(),
                    internet: true,
                    place: p.clone(),
                },
            )
            .await
            .map_err(|x| x.to_string())?;
        // The boot sandbox must not outlive a gateway that dies mid-build
        // (measured: it did, with no lifetime): core ends it on its own.
        let _ = e
            .backend
            .set_lifetime(who, &created.name, 1800, "delete")
            .await;
        let out = async {
            let c = e
                .backend
                .spacesd(who, &created.name)
                .await
                .map_err(|x| x.to_string())?;
            let driver = e.drivers.of(&format!("golden-{cp}"), &c).await;
            super::api::ensure_default_user(e, &c, &driver, &e.cfg.default_user).await;
            if variant == Variant::CodeInterpreter {
                super::code_interpreter::provision(e, &c, &driver).await;
            }
            e.drivers.forget(&format!("golden-{cp}")).await;
            e.backend
                .checkpoint(who, &created.name, cp)
                .await
                .map_err(|x| x.to_string())
        }
        .await;
        let _ = e.backend.delete(who, &created.name).await;
        out
    }

    /// Drops a checkpoint that could not be forked (it is gone or stale);
    /// the next create builds a new one.
    pub async fn forget(&self, e: &E2b, who: &Identity, checkpoint: &str) {
        self.shapes
            .lock()
            .await
            .retain(|_, v| !matches!(v, Entry::Ready(c) if c == checkpoint));
        let _ = e.backend.delete_checkpoint(who, checkpoint).await;
    }
}
