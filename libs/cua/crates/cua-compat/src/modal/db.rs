//! The gateway's objects: apps, secrets, volumes, images, mounts,
//! sandboxes, snapshots, dicts and queues, persisted as one 0600 JSON file
//! (`$CUA_HOME/compat/modal/objects.json`). Every object is owned by the
//! API key that made it; a key sees only its own.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub type Tags = BTreeMap<String, String>;

pub fn now_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub environment: String,
    pub description: String,
    pub created_at: f64,
    /// An ephemeral app (`app.run()`), stopped when its client leaves.
    pub ephemeral: bool,
    #[serde(default)]
    pub stopped: bool,
    #[serde(default)]
    pub tags: Tags,
    /// Last client heartbeat (ephemeral apps).
    #[serde(default)]
    pub heartbeat_at: f64,
    /// The layout `AppPublish` recorded: function tag -> id, class tag -> id.
    #[serde(default)]
    pub function_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub class_ids: BTreeMap<String, String>,
    /// Deployment name and time (`App.deploy`).
    #[serde(default)]
    pub deployed_at: f64,
    #[serde(default)]
    pub deployment_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub environment: String,
    pub created_at: f64,
    pub app_id: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub environment: String,
    pub created_at: f64,
    /// Modal volume version (1 or 2).
    pub version: i32,
    /// Directory name under the volume store.
    pub store: String,
    /// Commit counter (`VolumeCommit`).
    #[serde(default)]
    pub commits: u64,
    /// Last heartbeat of an ephemeral volume's client.
    #[serde(default)]
    pub heartbeat_at: f64,
    /// Made with `Volume.ephemeral()`: deleted when its client goes away.
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub created_at: f64,
    pub files: Vec<MountFileRec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountFileRec {
    pub filename: String,
    pub sha256: String,
    pub size: u64,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvRec {
    pub name: String,
    pub key_id: String,
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub environment: String,
    pub created_at: f64,
    pub app_id: Option<String>,
    /// `(key, value)`, both hex-encoded bytes, in insertion order.
    #[serde(default)]
    pub items: Vec<(String, String)>,
    #[serde(default)]
    pub heartbeat_at: f64,
    #[serde(default)]
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRec {
    pub id: String,
    pub key_id: String,
    pub name: Option<String>,
    pub environment: String,
    pub created_at: f64,
    pub app_id: Option<String>,
    /// partition key (hex) -> `(entry id, value hex)` in order
    #[serde(default)]
    pub partitions: BTreeMap<String, Vec<(u64, String)>>,
    #[serde(default)]
    pub next_entry: u64,
    #[serde(default)]
    pub heartbeat_at: f64,
    #[serde(default)]
    pub ephemeral: bool,
}

/// A Function: the client's `Function` message (prost bytes, base64) and
/// what the gateway tracks around it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionRec {
    pub id: String,
    pub key_id: String,
    pub app_id: String,
    /// The function's tag in the app (`square`, `Cls.method`).
    pub name: String,
    /// `FunctionCreate` happened (precreated records have no definition).
    #[serde(default)]
    pub created: bool,
    /// `api.Function`, prost-encoded then base64.
    #[serde(default)]
    pub definition: String,
    /// `api.FunctionData` the same way (a create with ranked GPU functions).
    #[serde(default)]
    pub data: String,
    pub created_at: f64,
    /// For a bound copy (`FunctionBindParams`): the unbound function and the
    /// serialized parameters.
    #[serde(default)]
    pub base_function_id: Option<String>,
    #[serde(default)]
    pub bound_params: Option<String>,
    /// Autoscaler settings the client updated (`update_autoscaler`).
    #[serde(default)]
    pub min_containers: Option<u32>,
    #[serde(default)]
    pub max_containers: Option<u32>,
    #[serde(default)]
    pub buffer_containers: Option<u32>,
    #[serde(default)]
    pub scaledown_window: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassRec {
    pub id: String,
    pub key_id: String,
    pub app_id: String,
    pub name: String,
    /// `(method name, function id)`, with the service function first.
    pub methods: Vec<(String, String)>,
    pub created_at: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Data {
    #[serde(default)]
    pub functions: BTreeMap<String, FunctionRec>,
    #[serde(default)]
    pub classes: BTreeMap<String, ClassRec>,
    #[serde(default)]
    pub apps: BTreeMap<String, AppRec>,
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretRec>,
    #[serde(default)]
    pub volumes: BTreeMap<String, VolumeRec>,
    #[serde(default)]
    pub mounts: BTreeMap<String, MountRec>,
    #[serde(default)]
    pub images: BTreeMap<String, super::image::ImageRec>,
    #[serde(default)]
    pub sandboxes: BTreeMap<String, super::sandbox::SandboxRec>,
    #[serde(default)]
    pub snapshots: BTreeMap<String, super::sandbox::SnapshotRec>,
    #[serde(default)]
    pub dicts: BTreeMap<String, DictRec>,
    #[serde(default)]
    pub queues: BTreeMap<String, QueueRec>,
    #[serde(default)]
    pub environments: BTreeMap<String, EnvRec>,
    /// `Image.publish` names: `<key id>/<tag>` -> image id.
    #[serde(default)]
    pub image_tags: BTreeMap<String, String>,
}

/// The persisted objects. Critical sections are short. The file (atomic,
/// 0600) is written behind: a writer marks the state dirty and a thread
/// saves it at most every 25 ms (and once more when the `Db` is dropped), so
/// a burst of writes costs one save however large the state has grown.
pub struct Db {
    path: Option<PathBuf>,
    data: std::sync::Arc<Mutex<Data>>,
    flush: std::sync::Arc<Flush>,
    flusher: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct Flush {
    state: Mutex<(bool, bool)>, // (dirty, stop)
    cv: std::sync::Condvar,
}

fn save(path: &std::path::Path, data: &Mutex<Data>) {
    let body = serde_json::to_vec(&*data.lock().expect("db lock")).expect("objects");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = crate::tls::write_file(path, &body, 0o600) {
        tracing::warn!("compat: saving {}: {e}", path.display());
    }
}

impl Db {
    /// Loads `path` (missing: empty). `None`: in memory (tests).
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Db> {
        let data = match &path {
            Some(p) => match std::fs::read(p) {
                Ok(b) => serde_json::from_slice(&b).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("{}: {e}", p.display()),
                    )
                })?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
                Err(e) => return Err(e),
            },
            None => Data::default(),
        };
        let data = std::sync::Arc::new(Mutex::new(data));
        let flush = std::sync::Arc::new(Flush::default());
        let flusher = path.clone().map(|p| {
            let (data, flush) = (data.clone(), flush.clone());
            std::thread::spawn(move || {
                loop {
                    let stop = {
                        let mut st = flush.state.lock().expect("flush");
                        while !st.0 && !st.1 {
                            st = flush.cv.wait(st).expect("flush");
                        }
                        let stop = st.1;
                        if !stop {
                            // Let a burst of writes land before saving.
                            drop(st);
                            std::thread::sleep(std::time::Duration::from_millis(25));
                            st = flush.state.lock().expect("flush");
                        }
                        let dirty = std::mem::take(&mut st.0);
                        drop(st);
                        if dirty {
                            save(&p, &data);
                        }
                        stop
                    };
                    if stop {
                        return;
                    }
                }
            })
        });
        Ok(Db {
            path,
            data,
            flush,
            flusher: Mutex::new(flusher),
        })
    }

    pub fn read<R>(&self, f: impl FnOnce(&Data) -> R) -> R {
        f(&self.data.lock().expect("db lock"))
    }

    /// Mutates; the save follows shortly.
    pub fn write<R>(&self, f: impl FnOnce(&mut Data) -> R) -> R {
        let r = f(&mut self.data.lock().expect("db lock"));
        if self.path.is_some() {
            self.flush.state.lock().expect("flush").0 = true;
            self.flush.cv.notify_one();
        }
        r
    }

    /// Saves now (what a graceful stop and the tests use).
    pub fn flush(&self) {
        if let Some(p) = &self.path {
            self.flush.state.lock().expect("flush").0 = false;
            save(p, &self.data);
        }
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if let Some(h) = self.flusher.lock().expect("flusher").take() {
            self.flush.state.lock().expect("flush").1 = true;
            self.flush.cv.notify_one();
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> EnvRec {
        EnvRec {
            name: name.into(),
            key_id: "k".into(),
            created_at: 1.0,
        }
    }

    #[test]
    fn writes_are_saved_behind_and_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.json");
        let db = Db::open(Some(path.clone())).unwrap();
        for i in 0..200 {
            db.write(|d| {
                d.environments
                    .insert(format!("k/{i}"), env(&format!("e{i}")));
            });
        }
        // Saved soon after the burst, without a flush.
        let mut saved = false;
        for _ in 0..100 {
            if std::fs::read(&path).is_ok_and(|b| b.len() > 100) {
                saved = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(saved, "the state was never saved");
        db.write(|d| {
            d.environments.insert("k/last".into(), env("last"));
        });
        drop(db); // a final save
        let again = Db::open(Some(path)).unwrap();
        assert_eq!(again.read(|d| d.environments.len()), 201);
        assert!(again.read(|d| d.environments.contains_key("k/last")));
    }

    #[test]
    fn flush_saves_now() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("objects.json");
        let db = Db::open(Some(path.clone())).unwrap();
        db.write(|d| {
            d.environments.insert("k/x".into(), env("x"));
        });
        db.flush();
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(on_disk["environments"]["k/x"].is_object());
    }
}
