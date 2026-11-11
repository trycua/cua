//! Small persisted collections of the Daytona gateway: volumes, secrets.
//! Files are written 0600 next to the gateway's other state.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::PathBuf;
use tokio::sync::Mutex;

/// A list kept in memory and saved to a JSON file after every change.
pub struct Persist<T> {
    path: Option<PathBuf>,
    pub items: Mutex<Vec<T>>,
}

impl<T: Serialize + DeserializeOwned + Clone> Persist<T> {
    pub fn open(path: Option<PathBuf>) -> Self {
        let items = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    pub async fn save(&self) {
        let Some(p) = &self.path else { return };
        let list = self.items.lock().await.clone();
        if let Err(e) = crate::tls::write_file(
            p,
            &serde_json::to_vec_pretty(&list).expect("serialize"),
            0o600,
        ) {
            tracing::warn!("compat: saving {}: {e}", p.display());
        }
    }
}

/// A Daytona volume: a folder of Cua Volume (`daytona-volumes/<id>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeEntry {
    pub id: String,
    pub name: String,
    pub key_id: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub last_used_ms: Option<u64>,
    /// `ready`, `pending_delete`, `deleting`, `deleted`, `error`.
    pub state: String,
    #[serde(default)]
    pub error_reason: Option<String>,
}

/// A Daytona secret. The value is write-only: it never leaves the gateway
/// except into a sandbox's environment when the sandbox opts in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEntry {
    pub id: String,
    pub name: String,
    pub key_id: String,
    pub value: String,
    pub placeholder: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

/// A Daytona warm pool and the Cua pool that serves it.
#[derive(Clone, Serialize, Deserialize)]
pub struct WarmPoolEntry {
    pub id: String,
    pub key_id: String,
    #[serde(default)]
    pub identity_daemon: Option<String>,
    #[serde(default)]
    pub identity_token: Option<String>,
    /// The Cua pool name.
    pub cua_pool: String,
    pub snapshot: String,
    pub snapshot_id: String,
    pub target: String,
    pub pool: u32,
    pub cpu: u32,
    pub mem: u32,
    pub disk: u32,
    pub os_user: String,
    #[serde(default)]
    pub error_reason: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

impl std::fmt::Debug for WarmPoolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarmPoolEntry")
            .field("id", &self.id)
            .field("key_id", &self.key_id)
            .field("identity_daemon", &self.identity_daemon)
            .field(
                "identity_token",
                &self.identity_token.as_ref().map(|_| "<redacted>"),
            )
            .field("cua_pool", &self.cua_pool)
            .field("snapshot", &self.snapshot)
            .field("snapshot_id", &self.snapshot_id)
            .field("target", &self.target)
            .field("pool", &self.pool)
            .field("cpu", &self.cpu)
            .field("mem", &self.mem)
            .field("disk", &self.disk)
            .field("os_user", &self.os_user)
            .field("error_reason", &self.error_reason)
            .field("created_ms", &self.created_ms)
            .field("updated_ms", &self.updated_ms)
            .finish()
    }
}

impl WarmPoolEntry {
    pub fn identity(&self) -> crate::backend::Identity {
        crate::backend::Identity {
            daemon: self.identity_daemon.clone(),
            token: self.identity_token.clone(),
        }
    }
}
