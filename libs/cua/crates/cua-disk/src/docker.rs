//! The container-engine view the accounting and garbage collection need,
//! behind a trait so tests run against a fake engine.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

pub use cua_vmm::container::{
    LABEL_CREATED, LABEL_EPHEMERAL, LABEL_HOME, LABEL_KIND, LABEL_MANAGED,
};

/// Legacy sandbox label (containers created before `ai.cua.managed`).
pub const LABEL_SANDBOX: &str = "cua.sandbox";
/// Repository prefix of every image the container backend writes.
pub const MANAGED_REPO_PREFIX: &str = "cua-vmm/";

/// One engine image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerImage {
    /// `sha256:...`.
    pub id: String,
    /// `repo:tag` names.
    pub tags: Vec<String>,
    /// Config labels.
    pub labels: HashMap<String, String>,
    /// Size in bytes (shared layers counted in full).
    pub size: u64,
    /// Unix seconds.
    pub created: u64,
}

/// One engine container (running or not).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerContainer {
    /// Container id.
    pub id: String,
    /// Name (without the leading `/`).
    pub name: String,
    /// Image id it runs.
    pub image_id: String,
    /// Labels.
    pub labels: HashMap<String, String>,
    /// `running`, `exited`, ...
    pub state: String,
    /// Bytes written in its layer.
    pub size_rw: u64,
    /// Unix seconds.
    pub created: u64,
    /// Named volumes it mounts.
    pub volumes: Vec<String>,
}

/// One engine volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerVolume {
    /// Name.
    pub name: String,
    /// Labels.
    pub labels: HashMap<String, String>,
}

/// Whether an object carries `ai.cua.managed=true`.
pub fn is_managed(labels: &HashMap<String, String>) -> bool {
    labels.get(LABEL_MANAGED).is_some_and(|v| v == "true")
}

/// The engine operations accounting and GC use. Removal never forces: an
/// image a container still uses is refused by the engine as well.
#[async_trait]
pub trait DockerApi: Send + Sync {
    /// Every image.
    async fn images(&self) -> Result<Vec<DockerImage>, String>;
    /// Every container, with its writable-layer size.
    async fn containers(&self) -> Result<Vec<DockerContainer>, String>;
    /// Volumes labelled `ai.cua.managed=true`.
    async fn managed_volumes(&self) -> Result<Vec<DockerVolume>, String>;
    /// Removes an image by reference or id (untags a reference first; an
    /// image with other tags stays). Never forced.
    async fn remove_image(&self, reference: &str) -> Result<(), String>;
    /// Removes a volume (never forced).
    async fn remove_volume(&self, name: &str) -> Result<(), String>;
}

/// The engine the SDK would use (discovered like the docker CLI), when it
/// answers within a few seconds. Never starts an engine.
pub async fn connect() -> Option<Arc<dyn DockerApi>> {
    let fut = cua_vmm::container::ContainerRuntime::connect(
        cua_vmm::container::ContainerConfig::default(),
    );
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(rt)) => Some(Arc::new(Engine(rt.docker().clone()))),
        _ => None,
    }
}

/// [`DockerApi`] over the Docker Engine API.
pub struct Engine(pub bollard::Docker);

fn secs(v: i64) -> u64 {
    v.max(0) as u64
}

#[async_trait]
impl DockerApi for Engine {
    async fn images(&self) -> Result<Vec<DockerImage>, String> {
        use bollard::query_parameters::ListImagesOptions;
        let rows = self
            .0
            .list_images(Some(ListImagesOptions {
                all: false,
                ..Default::default()
            }))
            .await
            .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(|i| DockerImage {
                id: i.id,
                tags: i
                    .repo_tags
                    .into_iter()
                    .filter(|t| t != "<none>:<none>")
                    .collect(),
                labels: i.labels,
                size: secs(i.size),
                created: secs(i.created),
            })
            .collect())
    }

    async fn containers(&self) -> Result<Vec<DockerContainer>, String> {
        use bollard::query_parameters::ListContainersOptions;
        let rows = self
            .0
            .list_containers(Some(ListContainersOptions {
                all: true,
                size: true,
                ..Default::default()
            }))
            .await
            .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(|c| DockerContainer {
                id: c.id.unwrap_or_default(),
                name: c
                    .names
                    .and_then(|n| n.first().map(|s| s.trim_start_matches('/').to_string()))
                    .unwrap_or_default(),
                image_id: c.image_id.unwrap_or_default(),
                labels: c.labels.unwrap_or_default(),
                state: c
                    .state
                    .map(|s| format!("{s:?}").to_lowercase())
                    .unwrap_or_default(),
                size_rw: c.size_rw.map(secs).unwrap_or(0),
                created: c.created.map(secs).unwrap_or(0),
                volumes: c
                    .mounts
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|m| m.name)
                    .collect(),
            })
            .collect())
    }

    async fn managed_volumes(&self) -> Result<Vec<DockerVolume>, String> {
        use bollard::query_parameters::ListVolumesOptions;
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec![format!("{LABEL_MANAGED}=true")]);
        let r = self
            .0
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(filters),
            }))
            .await
            .map_err(|e| e.to_string())?;
        Ok(r.volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| DockerVolume {
                name: v.name,
                labels: v.labels,
            })
            .collect())
    }

    async fn remove_image(&self, reference: &str) -> Result<(), String> {
        use bollard::query_parameters::RemoveImageOptions;
        self.0
            .remove_image(
                reference,
                Some(RemoveImageOptions {
                    force: false,
                    noprune: false,
                    ..Default::default()
                }),
                None,
            )
            .await
            .map(drop)
            .map_err(|e| e.to_string())
    }

    async fn remove_volume(&self, name: &str) -> Result<(), String> {
        use bollard::query_parameters::RemoveVolumeOptions;
        self.0
            .remove_volume(name, Some(RemoveVolumeOptions { force: false }))
            .await
            .map_err(|e| e.to_string())
    }
}
