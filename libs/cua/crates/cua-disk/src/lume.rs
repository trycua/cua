//! The Lume view accounting and GC use, behind a trait for tests.

use std::sync::Arc;

use async_trait::async_trait;

/// One Lume VM.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LumeVm {
    /// Name.
    pub name: String,
    /// `running`, `stopped`, `pulling`, ...
    pub status: String,
    /// Allocated bytes Lume reports (APFS clones share blocks, so the sum
    /// over clones overstates what they take together).
    pub allocated: u64,
}

/// Lume operations accounting and GC use.
#[async_trait]
pub trait LumeApi: Send + Sync {
    /// Every VM Lume knows.
    async fn vms(&self) -> Result<Vec<LumeVm>, String>;
    /// Deletes a VM.
    async fn delete(&self, name: &str) -> Result<(), String>;
}

/// The local `lume serve`, when it already answers (macOS only). Never
/// starts it.
pub async fn connect() -> Option<Arc<dyn LumeApi>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let client = cua_vmm::lume::LumeClient::new(cua_vmm::lume::LumeConfig::default().url);
    client
        .reachable()
        .await
        .then(|| Arc::new(Serve(client)) as Arc<dyn LumeApi>)
}

/// [`LumeApi`] over `lume serve`.
pub struct Serve(pub cua_vmm::lume::LumeClient);

#[async_trait]
impl LumeApi for Serve {
    async fn vms(&self) -> Result<Vec<LumeVm>, String> {
        Ok(self
            .0
            .list()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|v| LumeVm {
                allocated: v
                    .disk_size
                    .get("allocated")
                    .and_then(|a| a.as_u64())
                    .unwrap_or(0),
                name: v.name,
                status: v.status,
            })
            .collect())
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        self.0.delete(name).await.map_err(|e| e.to_string())
    }
}
