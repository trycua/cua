//! Durable Spaces: a Space on your machines that is created again on
//! another one, with the same name, when its machine goes away
//! (`SpaceCreate::durable`, `cua spaces create --on auto --durable`,
//! `create_space(durable=true)`).
//!
//! Whichever of your machines coordinates them keeps them (the coordinator
//! of the account's machines, which ships with Cua Spaces): this crate asks
//! it through [`PoolRequests`], which the cua daemon of Cua Spaces installs
//! ([`Spaces::set_pool`]). Without it a durable create fails and says so.
//!
//! A durable Space is otherwise a Space like any other: it is listed (with
//! [`SpaceInfo::durable`] set), found by its name, opened and used; deleting
//! it (by its name or its id) deletes it for good, wherever it is.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_sandbox_core::placement::On;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::{SpaceCreate, SpaceCreated, SpaceInfo, Spaces};

/// A request to whichever of the account's machines coordinates them:
/// `(status, body)` as it answered (HTTP-like status, JSON body).
pub trait PoolRequests: Send + Sync + 'static {
    /// One request (`durable_create`, `durable_get`, `durable_list`,
    /// `durable_delete`).
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = (u16, Value)> + Send + 'a>>;
}

/// How long a durable create waits for its first machine by default.
const READY_WITHIN: Duration = Duration::from_secs(15 * 60);

/// A durable Space's name: lower-case letters, digits, `-` and `.`, 48 at
/// most, starting with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

impl Spaces {
    /// Installs (or clears) what reaches the coordinator of the account's
    /// machines (the cua daemon of Cua Spaces does).
    pub fn set_pool(&self, pool: Option<Arc<dyn PoolRequests>>) {
        *self.inner.pool.write().expect("pool") = pool;
    }

    fn pool(&self) -> Result<Arc<dyn PoolRequests>> {
        self.inner
            .pool
            .read()
            .expect("pool")
            .clone()
            .ok_or_else(|| {
                Error::needs_cua_spaces("durable Spaces (kept on your machines by Cua Spaces)")
            })
    }

    async fn pool_call(&self, method: &str, params: Value) -> Result<Value> {
        let (status, body) = self.pool()?.request(method, params).await;
        if status == 200 {
            return Ok(body);
        }
        let msg = body["error"]
            .as_str()
            .unwrap_or("the coordinator of your machines failed")
            .to_string();
        Err(match status {
            400 => Error::InvalidArgument(msg),
            404 => Error::NotFound(msg),
            503 | 504 => Error::Timeout(msg),
            _ => Error::Agent(msg),
        })
    }

    /// Creates the durable Space `opts.name` (or returns it, when it exists
    /// with the same image and size) and waits until it runs on one of your
    /// machines (unless `opts.wait` is false).
    pub(crate) async fn create_durable(&self, opts: SpaceCreate) -> Result<SpaceCreated> {
        if !opts.volumes.is_empty() {
            return Err(Error::InvalidArgument(
                "a durable Space is created again by your machines when one goes away, with \
                 no client to serve a named volume to it: mount volumes in a Space that is \
                 not durable"
                    .into(),
            ));
        }
        match &opts.on {
            Some(On::Auto) => {}
            _ => {
                return Err(Error::InvalidArgument(
                    "a durable Space lives on your machines and moves when one goes away: \
                     create it with on=\"auto\" (`--on auto`)"
                        .into(),
                ));
            }
        }
        let name = opts
            .name
            .clone()
            .unwrap_or_else(|| format!("space-{}", crate::spaces::random_suffix()));
        if !valid_name(&name) {
            return Err(Error::InvalidArgument(format!(
                "{name:?}: a durable Space is named with lower-case letters, digits, `-` and \
                 `.` (48 at most)"
            )));
        }
        let spec = json!({
            "image": opts.image.clone().unwrap_or_default(),
            "kind": match opts.kind {
                cua_sandbox_core::placement::Kind::Auto => String::new(),
                k => k.as_str().to_string(),
            },
            "runtime": match &opts.runtime {
                cua_sandbox_core::placement::Runtime::Auto => String::new(),
                r => r.as_str().to_string(),
            },
            "cpus": opts.cpus.unwrap_or(0),
            "memory_mb": opts.memory_mb.unwrap_or(0),
            "headless": opts.headless,
        });
        let mut v = self
            .pool_call(
                "durable_create",
                json!({"name": name, "spec": spec, "wait": false}),
            )
            .await?;
        if opts.wait == Some(false) {
            return Ok(SpaceCreated::Starting(crate::spaces::PendingSpace {
                id: name,
                phase: "starting",
            }));
        }
        let deadline = Instant::now() + opts.timeout.unwrap_or(READY_WITHIN);
        loop {
            if v["state"] == "running" && v["space"].as_str().is_some_and(|s| !s.is_empty()) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "the durable Space {name} has no machine yet ({}); it keeps trying \
                     (`cua spaces ls`)",
                    v["error"]
                        .as_str()
                        .filter(|e| !e.is_empty())
                        .unwrap_or("waiting")
                )));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            v = self.pool_call("durable_get", json!({"name": name})).await?;
        }
        let info = self.durable_info(&v).await?;
        Ok(SpaceCreated::Ready {
            info,
            reused: false,
        })
    }

    /// The Space a durable view is now, registered and marked.
    async fn durable_info(&self, v: &Value) -> Result<SpaceInfo> {
        let name = v["name"].as_str().unwrap_or_default().to_string();
        let space = v["space"].as_str().unwrap_or_default().to_string();
        self.remember_durable(&name, &space);
        if space.starts_with("direct:") {
            // On a machine added by address: registered with its token.
            let token = v["token"].as_str().unwrap_or_default();
            self.add(&space, Some(token.to_string()), Some(name.clone()))
                .await?;
        } else {
            // On the relay: known once the directory lists it.
            let _ = self.relay_machines().await;
        }
        let mut info = self
            .list()?
            .into_iter()
            .find(|i| i.id == space)
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "the durable Space {name} runs as {space}, which does not show yet"
                ))
            })?;
        info.durable = name;
        Ok(info)
    }

    fn remember_durable(&self, name: &str, space: &str) {
        let mut g = self.inner.durable.lock().expect("durable");
        g.retain(|(n, _)| n != name);
        if !space.is_empty() {
            g.push((name.to_string(), space.to_string()));
        }
    }

    /// The durable Spaces as the coordinator keeps them (and this runtime
    /// remembers, so lists and names resolve without asking). Empty when
    /// there is no coordinator to ask.
    pub async fn durable_spaces(&self) -> Result<Vec<Value>> {
        if self.inner.pool.read().expect("pool").is_none() {
            return Ok(vec![]);
        }
        let v = self.pool_call("durable_list", Value::Null).await?;
        let all = v["spaces"].as_array().cloned().unwrap_or_default();
        *self.inner.durable.lock().expect("durable") = all
            .iter()
            .filter_map(|d| {
                let space = d["space"].as_str().filter(|s| !s.is_empty())?;
                Some((d["name"].as_str()?.to_string(), space.to_string()))
            })
            .collect();
        Ok(all)
    }

    /// The durable Space `key` names (its name, or the id it runs as now).
    pub(crate) fn durable_of(&self, key: &str) -> Option<(String, String)> {
        self.inner
            .durable
            .lock()
            .expect("durable")
            .iter()
            .find(|(n, s)| n == key || s == key)
            .cloned()
    }

    /// Marks the durable ones among `infos`.
    pub(crate) fn mark_durable(&self, infos: &mut [SpaceInfo]) {
        let g = self.inner.durable.lock().expect("durable");
        for i in infos {
            if let Some((n, _)) = g.iter().find(|(_, s)| *s == i.id) {
                i.durable = n.clone();
                // Shown by its durable name, whichever copy it is now.
                i.name = n.clone();
            }
        }
    }

    /// Deletes the durable Space `name` for good, wherever it runs.
    pub(crate) async fn delete_durable(&self, name: &str) -> Result<String> {
        self.pool_call("durable_delete", json!({"name": name}))
            .await?;
        let space = self.durable_of(name).map(|(_, s)| s);
        self.remember_durable(name, "");
        if let Some(s) = space {
            let _ = self.inner.registry.remove(&s);
            let _ = self.relay_machines().await;
        }
        Ok(format!(
            "Deleted the durable Space {name} (it is not created again)."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A coordinator that keeps one durable Space on `relay:space-1`.
    #[derive(Default)]
    struct FakePool {
        asked: Mutex<Vec<(String, Value)>>,
    }

    impl PoolRequests for FakePool {
        fn request<'a>(
            &'a self,
            method: &'a str,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = (u16, Value)> + Send + 'a>> {
            self.asked
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            let view = json!({"name": "research", "state": "running", "generation": 2,
                              "space": "relay:space-0123456789abcdef"});
            Box::pin(async move {
                match method {
                    "durable_list" => (200, json!({"spaces": [view]})),
                    "durable_delete" => (200, json!({"name": "research", "deleted": true})),
                    _ => (404, json!({"error": "no such request"})),
                }
            })
        }
    }

    fn spaces() -> (tempfile::TempDir, Spaces) {
        let dir = tempfile::tempdir().unwrap();
        let s = Spaces::builder().home(dir.path()).build();
        (dir, s)
    }

    #[tokio::test]
    async fn a_durable_space_needs_auto_and_the_coordinator() {
        let (_d, s) = spaces();
        let local = SpaceCreate {
            durable: true,
            ..Default::default()
        };
        let e = s.create(local).await.unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
        let auto = SpaceCreate {
            durable: true,
            on: Some(On::Auto),
            name: Some("research".into()),
            ..Default::default()
        };
        let e = s.create(auto.clone()).await.unwrap_err();
        assert!(matches!(e, Error::HostCapabilityMissing { .. }), "{e}");
        let bad = SpaceCreate {
            name: Some("Research".into()),
            ..auto
        };
        s.set_pool(Some(Arc::new(FakePool::default())));
        assert!(matches!(
            s.create(bad).await.unwrap_err(),
            Error::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn its_name_finds_it_and_deleting_it_deletes_it_for_good() {
        let (_d, s) = spaces();
        let pool = Arc::new(FakePool::default());
        s.set_pool(Some(pool.clone()));
        assert_eq!(s.durable_spaces().await.unwrap().len(), 1);
        assert_eq!(
            s.resolve("research").unwrap().to_string(),
            "relay:space-0123456789abcdef"
        );
        let msg = s.delete("research").await.unwrap();
        assert!(msg.contains("not created again"), "{msg}");
        // By its current id too: still the durable delete, never only the
        // copy (which the coordinator would create again).
        s.delete("relay:space-0123456789abcdef").await.unwrap();
        let deletes: Vec<Value> = pool
            .asked
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "durable_delete")
            .map(|(_, p)| p.clone())
            .collect();
        assert_eq!(deletes, vec![json!({"name": "research"}); 2]);
    }

    #[test]
    fn durable_names() {
        assert!(valid_name("research"));
        assert!(valid_name("build-01.mac"));
        assert!(!valid_name("Research"));
        assert!(!valid_name("-x"));
        assert!(!valid_name(&"a".repeat(49)));
    }
}
