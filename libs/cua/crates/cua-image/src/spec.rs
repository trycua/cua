//! `images.cua.ai/v1alpha1` Image resource, the Rust source of truth for the
//! schema published at `libs/python/cua-sandbox/schemas/image-v1alpha1.schema.json`
//! (JSON Schema generated with `schemars`, the same pattern as the Fleet SDK's
//! sdk-schema → CRD generation). A test checks the generated schema stays
//! compatible with the checked-in Python schema.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ImageError, Result};

pub const API_VERSION: &str = "images.cua.ai/v1alpha1";
pub const KIND: &str = "Image";

const DNS_LABEL: &str = "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$";
const SHA256: &str = "^sha256:[0-9a-f]{64}$";
const REGISTRY_SECRET: &str = "^cua-registry-[a-z0-9]([-a-z0-9]*[a-z0-9])?$";
/// Largest `sizeBytes` a file reference may carry: 2^53 - 1, so the value
/// survives every JSON client (images.cua.ai CRD, trycua/cua#3839).
pub const MAX_FILE_SIZE_BYTES: u64 = 9_007_199_254_740_991;

/// `apiVersion` (single allowed value).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiVersion {
    #[default]
    #[serde(rename = "images.cua.ai/v1alpha1")]
    V1alpha1,
}

/// `kind` (single allowed value).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ResourceKind {
    #[default]
    Image,
}

/// The full Image resource.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageResource")]
pub struct ImageResource {
    pub api_version: ApiVersion,
    pub kind: ResourceKind,
    pub metadata: ImageObjectMeta,
    pub spec: ImageSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ImageStatus>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageObjectMeta")]
pub struct ImageObjectMeta {
    /// Image name: a DNS label.
    #[schemars(length(min = 1, max = 63), regex(pattern = DNS_LABEL))]
    pub name: String,
    /// Namespace the image belongs to.
    #[schemars(length(min = 1, max = 63), regex(pattern = DNS_LABEL))]
    pub namespace: String,
    /// Labels on the Image resource.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("maxProperties" = 64, "additionalProperties" = {"type": "string", "maxLength": 4096}))]
    pub labels: BTreeMap<String, String>,
    /// Annotations on the Image resource.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("maxProperties" = 64, "additionalProperties" = {"type": "string", "maxLength": 4096}))]
    pub annotations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageSpec")]
pub struct ImageSpec {
    /// What to build.
    pub recipe: ImageRecipe,
    /// Build resources and limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<ImageBuildOptions>,
    /// Free-form metadata kept with the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ImageUserMetadata>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageBuildOptions")]
pub struct ImageBuildOptions {
    /// Disk size of the build machine (default `40Gi`).
    #[serde(default = "default_disk_size")]
    #[schemars(regex(pattern = "^[1-9][0-9]*(Gi|Ti)$"))]
    pub disk_size: String,
    /// Build budget in seconds (default 7200).
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 60, max = 86400))]
    pub timeout_seconds: u32,
}

impl Default for ImageBuildOptions {
    fn default() -> Self {
        Self {
            disk_size: default_disk_size(),
            timeout_seconds: default_timeout(),
        }
    }
}

fn default_disk_size() -> String {
    "40Gi".into()
}
fn default_timeout() -> u32 {
    7200
}

impl ImageBuildOptions {
    /// `diskSize` in GiB.
    pub fn disk_size_gb(&self) -> Result<u32> {
        let s = &self.disk_size;
        let (n, mul) = if let Some(n) = s.strip_suffix("Gi") {
            (n, 1)
        } else if let Some(n) = s.strip_suffix("Ti") {
            (n, 1024)
        } else {
            return Err(ImageError::Spec(format!(
                "diskSize '{s}' must end in Gi or Ti"
            )));
        };
        n.parse::<u32>()
            .ok()
            .and_then(|v| v.checked_mul(mul))
            .ok_or_else(|| ImageError::Spec(format!("bad diskSize '{s}'")))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageUserMetadata")]
pub struct ImageUserMetadata {
    /// Tags, as key-value pairs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("maxProperties" = 32, "additionalProperties" = {"type": "string", "maxLength": 256}))]
    pub tags: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecipeOsType {
    #[default]
    Linux,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecipeKind {
    #[default]
    Vm,
    /// An OCI rootfs (docker / gVisor) built on the registry image `from`.
    Container,
}

/// What to build.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageRecipe")]
pub struct ImageRecipe {
    /// Guest OS family.
    pub os_type: RecipeOsType,
    /// Distribution of the base, for example `ubuntu`.
    #[schemars(length(min = 1, max = 64))]
    pub distro: String,
    /// Distribution version, for example `24.04`.
    #[schemars(length(min = 1, max = 64))]
    pub version: String,
    /// `vm`: a VM disk image. `container`: an OCI rootfs built on `from`.
    pub kind: RecipeKind,
    /// Registry base image (`kind: container`): layers are built on top of
    /// it and the result is an OCI rootfs.
    #[serde(default, rename = "from", skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub from: Option<String>,
    /// A `cua-registry-*` pull Secret in this namespace for a private base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 253), regex(pattern = REGISTRY_SECRET))]
    pub from_pull_secret: Option<String>,
    /// Build steps, run in order.
    #[schemars(length(max = 128))]
    pub layers: Vec<ImageLayer>,
    /// Environment variables of the image.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("maxProperties" = 128, "additionalProperties" = {"type": "string", "maxLength": 8192}))]
    pub env: BTreeMap<String, String>,
    /// Uploaded files copied into the image.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = 128))]
    pub files: Vec<ImageFile>,
    /// Ports the image exposes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = 32), inner(range(min = 1)), extend("uniqueItems" = true))]
    pub ports: Vec<u16>,
}

/// One build step. Serialised as `{"type": "<kind>", ...}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(title = "ImageLayer")]
pub enum ImageLayer {
    AptInstall {
        #[schemars(length(min = 1, max = 256), inner(length(min = 1, max = 256)))]
        packages: Vec<String>,
    },
    PipInstall {
        #[schemars(length(min = 1, max = 256), inner(length(min = 1, max = 256)))]
        packages: Vec<String>,
    },
    UvInstall {
        #[schemars(length(min = 1, max = 256), inner(length(min = 1, max = 256)))]
        packages: Vec<String>,
    },
    AppInstall {
        #[serde(rename = "appId")]
        #[schemars(length(min = 1, max = 256))]
        app_id: String,
    },
    Run {
        #[schemars(length(min = 1, max = 16384))]
        command: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageFile")]
pub struct ImageFile {
    /// The uploaded file.
    pub source: ImageFileReference,
    /// Absolute path in the image.
    #[schemars(length(max = 4096), regex(pattern = "^/.*"))]
    pub destination: String,
}

/// A file uploaded to Fleet (`uploads/<namespace>/<id>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageFileReference")]
pub struct ImageFileReference {
    #[schemars(
        length(min = 1, max = 2048),
        regex(pattern = "^uploads/[a-z0-9]([-a-z0-9]*[a-z0-9])?/[A-Za-z0-9_-]+$")
    )]
    /// Upload reference from the image upload API (`uploads/<namespace>/<id>`).
    pub reference: String,
    /// SHA-256 digest of the file.
    #[schemars(regex(pattern = SHA256))]
    pub digest: String,
    /// Size in bytes.
    #[schemars(range(max = 9_007_199_254_740_991u64))]
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImagePhase {
    Pending,
    Validating,
    Building,
    #[serde(rename = "PushingOCI")]
    PushingOci,
    Importing,
    Quiescing,
    Snapshotting,
    Ready,
    Failed,
    Cancelling,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageStatus")]
pub struct ImageStatus {
    /// Build phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<ImagePhase>,
    /// The spec generation this status describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<u64>,
    /// Digest of the recipe that was built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = SHA256))]
    pub recipe_digest: Option<String>,
    /// Identifier of the build run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 63))]
    pub build_identity: Option<String>,
    /// What the build produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<ImageArtifacts>,
    /// Where the build log is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<ImageLogs>,
    /// Kubernetes-style conditions of the build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<ImageCondition>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageArtifacts")]
pub struct ImageArtifacts {
    /// The built OCI image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci: Option<ImageOciArtifact>,
    /// The built VM disk, as a volume snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_snapshot: Option<ImageVolumeSnapshotArtifact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageOciArtifact")]
pub struct ImageOciArtifact {
    /// Image reference.
    #[schemars(length(min = 1, max = 2048))]
    pub reference: String,
    /// Image digest.
    #[schemars(regex(pattern = SHA256))]
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageVolumeSnapshotArtifact")]
pub struct ImageVolumeSnapshotArtifact {
    /// Namespace of the snapshot.
    #[schemars(length(min = 1, max = 63))]
    pub namespace: String,
    /// Name of the snapshot.
    #[schemars(length(min = 1, max = 253))]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageLogs")]
pub struct ImageLogs {
    /// Pod that streams the log while the build runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 253))]
    pub live_pod_name: Option<String>,
    /// Where the log is kept after the build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 2048))]
    pub retained_reference: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ConditionStatus {
    True,
    False,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(title = "ImageCondition")]
pub struct ImageCondition {
    /// Condition type.
    #[serde(rename = "type")]
    #[schemars(length(min = 1, max = 64))]
    pub type_: String,
    /// `True`, `False` or `Unknown`.
    pub status: ConditionStatus,
    /// Machine-readable reason.
    #[schemars(length(min = 1, max = 128))]
    pub reason: String,
    /// Human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 32768))]
    pub message: Option<String>,
    /// When the condition last changed (RFC 3339).
    #[schemars(extend("format" = "date-time"))]
    pub last_transition_time: String,
    /// The spec generation the condition describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<u64>,
}

impl ImageResource {
    /// A minimal resource for `recipe`.
    pub fn new(name: &str, namespace: &str, recipe: ImageRecipe) -> Self {
        Self {
            api_version: ApiVersion::V1alpha1,
            kind: ResourceKind::Image,
            metadata: ImageObjectMeta {
                name: name.into(),
                namespace: namespace.into(),
                ..Default::default()
            },
            spec: ImageSpec {
                recipe,
                build: None,
                metadata: None,
            },
            status: None,
        }
    }

    /// The JSON Schema for this resource (draft 2020-12, `Option` fields
    /// non-nullable: absent is the only way to omit them, as in the Python
    /// schema).
    pub fn json_schema() -> serde_json::Value {
        let schema = schemars::generate::SchemaSettings::draft2020_12()
            .into_generator()
            .into_root_schema_for::<Self>();
        let mut v = serde_json::to_value(schema).expect("schema serialises");
        strip_nullable(&mut v);
        v["$id"] = serde_json::json!("https://cua.ai/schemas/images.cua.ai/v1alpha1/image.json");
        v
    }

    /// Parse and validate the parts serde cannot express.
    pub fn from_json(raw: &str) -> Result<Self> {
        let r: Self = serde_json::from_str(raw).map_err(|e| ImageError::Spec(e.to_string()))?;
        r.validate()?;
        Ok(r)
    }

    pub fn validate(&self) -> Result<()> {
        for (what, v) in [
            ("metadata.name", &self.metadata.name),
            ("metadata.namespace", &self.metadata.namespace),
        ] {
            if !is_dns_label(v) {
                return Err(ImageError::Spec(format!(
                    "{what} '{v}' must be a DNS label ({DNS_LABEL}, max 63)"
                )));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for p in &self.spec.recipe.ports {
            if *p == 0 || !seen.insert(*p) {
                return Err(ImageError::Spec(format!(
                    "ports must be unique and in 1..=65535 (got {p})"
                )));
            }
        }
        for k in self.spec.recipe.env.keys() {
            if !is_env_name(k) {
                return Err(ImageError::Spec(format!("unsafe env var name '{k}'")));
            }
        }
        for f in self.spec.recipe.files.iter() {
            if f.source.size_bytes > MAX_FILE_SIZE_BYTES {
                return Err(ImageError::Spec(format!(
                    "files[].source.sizeBytes must be at most {MAX_FILE_SIZE_BYTES}"
                )));
            }
        }
        if let Some(b) = &self.spec.build {
            b.disk_size_gb()?;
        }
        let r = &self.spec.recipe;
        match (r.kind, &r.from) {
            (RecipeKind::Container, None) => {
                return Err(ImageError::Spec(
                    "kind: container needs `from` (the registry base image)".into(),
                ));
            }
            (RecipeKind::Vm, Some(_)) => {
                return Err(ImageError::Spec(
                    "`from` builds an OCI rootfs: set kind: container (a VM recipe starts \
                     from an attested base disk)"
                        .into(),
                ));
            }
            _ => {}
        }
        if r.from
            .as_deref()
            .is_some_and(|f| f.trim().is_empty() || f.chars().any(char::is_whitespace))
        {
            return Err(ImageError::Spec("`from` must be an image reference".into()));
        }
        if let Some(sec) = &r.from_pull_secret {
            let ok =
                sec.strip_prefix("cua-registry-").is_some_and(is_dns_label) && sec.len() <= 253;
            if !ok || r.from.is_none() {
                return Err(ImageError::Spec(format!(
                    "fromPullSecret '{sec}' must name a cua-registry-* Secret and go with `from`"
                )));
            }
        }
        if r.kind == RecipeKind::Container
            && r.layers
                .iter()
                .any(|l| matches!(l, ImageLayer::AppInstall { .. }))
        {
            return Err(ImageError::Spec(
                "app_install layers are VM-only; a container recipe cannot use them".into(),
            ));
        }
        Ok(())
    }

    /// `sha256:` digest of the canonical recipe JSON (the `recipeDigest`
    /// status field and the local build cache key).
    pub fn recipe_digest(&self) -> String {
        let canonical =
            serde_json::to_vec(&serde_json::to_value(&self.spec.recipe).expect("serialisable"))
                .expect("serialisable");
        crate::digest::sha256_bytes(&canonical)
    }
}

/// `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, at most 63 chars.
pub fn is_dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_env_name(k: &str) -> bool {
    let mut c = k.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

/// Remove `null` from `type` arrays and `anyOf: [X, {type: null}]` wrappers.
fn strip_nullable(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Array(types)) = map.get_mut("type") {
                types.retain(|t| t != "null");
                if types.len() == 1 {
                    let only = types[0].clone();
                    map.insert("type".into(), only);
                }
            }
            if let Some(serde_json::Value::Array(any)) = map.get("anyOf").cloned() {
                let non_null: Vec<_> = any
                    .iter()
                    .filter(|s| s.get("type") != Some(&"null".into()))
                    .cloned()
                    .collect();
                if non_null.len() == 1 && any.len() == 2 {
                    map.remove("anyOf");
                    if let serde_json::Value::Object(inner) = &non_null[0] {
                        for (k, val) in inner {
                            map.entry(k.clone()).or_insert(val.clone());
                        }
                    }
                }
            }
            for (_, child) in map.iter_mut() {
                strip_nullable(child);
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip_nullable),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn python_schema() -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../python/cua-sandbox/schemas/image-v1alpha1.schema.json");
        serde_json::from_slice(&std::fs::read(p).expect("python schema present")).unwrap()
    }

    /// Resolve `$ref` (local `#/$defs/...`) against `root`.
    fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> &'a Value {
        while let Some(r) = node.get("$ref").and_then(Value::as_str) {
            let ptr = r.trim_start_matches('#');
            node = root
                .pointer(ptr)
                .unwrap_or_else(|| panic!("dangling ref {r}"));
        }
        node
    }

    /// Every variant of a node (itself plus oneOf/anyOf/allOf members), resolved.
    fn variants<'a>(root: &'a Value, node: &'a Value) -> Vec<&'a Value> {
        let node = resolve(root, node);
        let mut out = vec![node];
        for key in ["oneOf", "anyOf", "allOf"] {
            if let Some(Value::Array(a)) = node.get(key) {
                for s in a {
                    out.extend(variants(root, s));
                }
            }
        }
        out
    }

    fn allowed_values(root: &Value, node: &Value) -> Option<Vec<Value>> {
        let mut vals = Vec::new();
        for v in variants(root, node) {
            if let Some(c) = v.get("const") {
                vals.push(c.clone());
            }
            if let Some(Value::Array(e)) = v.get("enum") {
                vals.extend(e.iter().cloned());
            }
        }
        (!vals.is_empty()).then_some(vals)
    }

    /// Structural compatibility: every property the Python schema defines
    /// exists in ours with the same required-ness, closed objects stay closed,
    /// and enums/consts accept the same values.
    fn compat(
        py_root: &Value,
        py: &Value,
        our_root: &Value,
        ours: &Value,
        path: &str,
        errs: &mut Vec<String>,
    ) {
        let py = resolve(py_root, py);
        let our_vs = variants(our_root, ours);
        if let Some(pv) = allowed_values(py_root, py) {
            match allowed_values(our_root, ours) {
                Some(ov) => {
                    for v in &pv {
                        if !ov.contains(v) {
                            errs.push(format!(
                                "{path}: value {v} allowed by python but not by rust"
                            ));
                        }
                    }
                    for v in &ov {
                        if v.is_string() && !pv.contains(v) {
                            errs.push(format!(
                                "{path}: value {v} allowed by rust but not by python"
                            ));
                        }
                    }
                }
                None => errs.push(format!(
                    "{path}: python restricts values to {pv:?}, rust does not"
                )),
            }
        }
        if let Some(Value::Object(pprops)) = py.get("properties") {
            // Property schemas from every variant; the same key in several
            // variants (a tag) is compared as the union of its variants.
            let mut per_key: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            let mut orequired: Vec<Value> = Vec::new();
            for v in &our_vs {
                if let Some(Value::Object(p)) = v.get("properties") {
                    for (k, s) in p {
                        per_key.entry(k.clone()).or_default().push(s.clone());
                    }
                }
                if let Some(Value::Array(r)) = v.get("required") {
                    orequired.extend(r.iter().cloned());
                }
            }
            let oprops: serde_json::Map<String, Value> = per_key
                .into_iter()
                .map(|(k, mut v)| {
                    (
                        k,
                        if v.len() == 1 {
                            v.remove(0)
                        } else {
                            json!({ "anyOf": v })
                        },
                    )
                })
                .collect();
            let preq: Vec<Value> = py
                .get("required")
                .and_then(|r| r.as_array().cloned())
                .unwrap_or_default();
            // Tagged unions (ImageLayer) spread `required` over variants; compare
            // plain objects strictly.
            let is_union = our_vs.len() > 1;
            for (k, ps) in pprops {
                let sub = format!("{path}.{k}");
                match oprops.get(k) {
                    None => errs.push(format!("{sub}: missing in rust schema")),
                    Some(os) => {
                        let (pr, or) = (preq.contains(&json!(k)), orequired.contains(&json!(k)));
                        if !is_union && pr != or {
                            errs.push(format!("{sub}: required python={pr} rust={or}"));
                        }
                        compat(py_root, ps, our_root, os, &sub, errs);
                    }
                }
            }
            for k in oprops.keys() {
                if !pprops.contains_key(k) && k != "type" {
                    errs.push(format!("{path}.{k}: extra property in rust schema"));
                }
            }
            if py.get("additionalProperties") == Some(&json!(false))
                && !our_vs
                    .iter()
                    .any(|v| v.get("additionalProperties") == Some(&json!(false)))
            {
                errs.push(format!(
                    "{path}: python is closed (additionalProperties: false), rust is open"
                ));
            }
        }
        if let Some(pi) = py.get("items") {
            match our_vs.iter().find_map(|v| v.get("items")) {
                Some(oi) => compat(py_root, pi, our_root, oi, &format!("{path}[]"), errs),
                None => errs.push(format!("{path}: python has items, rust does not")),
            }
        }
    }

    #[test]
    fn generated_schema_is_compatible_with_python_schema() {
        let py = python_schema();
        let ours = ImageResource::json_schema();
        let mut errs = Vec::new();
        compat(&py, &py, &ours, &ours, "$", &mut errs);
        assert!(errs.is_empty(), "schema drift:\n  {}", errs.join("\n  "));
    }

    /// Drift gate for the checked-in generated schema
    /// (`schema/image-v1alpha1.schema.json`). Regenerate with
    /// `CUA_UPDATE_SCHEMA=1 cargo test -p cua-image generated_schema_file`.
    #[test]
    fn generated_schema_file_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schema/image-v1alpha1.schema.json");
        let fresh = serde_json::to_string_pretty(&ImageResource::json_schema()).unwrap() + "\n";
        if std::env::var("CUA_UPDATE_SCHEMA").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &fresh).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            on_disk, fresh,
            "schema drift: run CUA_UPDATE_SCHEMA=1 cargo test -p cua-image generated_schema_file"
        );
    }

    fn sample() -> Value {
        json!({
            "apiVersion": "images.cua.ai/v1alpha1",
            "kind": "Image",
            "metadata": {"name": "my-image", "namespace": "team-a", "labels": {"a": "b"}},
            "spec": {
                "recipe": {
                    "osType": "linux", "distro": "ubuntu", "version": "24.04", "kind": "vm",
                    "layers": [
                        {"type": "apt_install", "packages": ["curl", "git"]},
                        {"type": "uv_install", "packages": ["requests"]},
                        {"type": "app_install", "appId": "vscode"},
                        {"type": "run", "command": "echo hi > /etc/motd"}
                    ],
                    "env": {"FOO": "bar"},
                    "ports": [8080, 3211],
                    "files": [{
                        "source": {"reference": "uploads/team-a/abc_123", "digest": format!("sha256:{}", "a".repeat(64)), "sizeBytes": 12},
                        "destination": "/opt/x"
                    }]
                },
                "build": {"diskSize": "64Gi", "timeoutSeconds": 3600},
                "metadata": {"tags": {"owner": "me"}}
            },
            "status": {
                "phase": "PushingOCI",
                "artifacts": {"oci": {"reference": "ghcr.io/x/y:1", "digest": format!("sha256:{}", "b".repeat(64))}},
                "conditions": [{"type": "Ready", "status": "False", "reason": "Building", "lastTransitionTime": "2026-09-22T00:00:00Z"}]
            }
        })
    }

    #[test]
    fn file_sizes_stay_json_safe() {
        let mut doc = sample();
        doc["spec"]["recipe"]["files"][0]["source"]["sizeBytes"] = json!(MAX_FILE_SIZE_BYTES);
        let raw = serde_json::to_string(&doc).unwrap();
        assert!(ImageResource::from_json(&raw).is_ok());
        doc["spec"]["recipe"]["files"][0]["source"]["sizeBytes"] = json!(MAX_FILE_SIZE_BYTES + 1);
        let raw = serde_json::to_string(&doc).unwrap();
        let err = ImageResource::from_json(&raw).unwrap_err().to_string();
        assert!(err.contains("sizeBytes"), "{err}");
        let py = jsonschema::validator_for(&python_schema()).unwrap();
        assert!(!py.is_valid(&doc), "the CRD schema rejects it too");
    }

    #[test]
    fn documents_agree_under_both_schemas() {
        let py = jsonschema::validator_for(&python_schema()).unwrap();
        let ours = jsonschema::validator_for(&ImageResource::json_schema()).unwrap();
        let good = sample();
        assert!(py.is_valid(&good), "sample must satisfy the python schema");
        assert!(
            ours.is_valid(&good),
            "sample must satisfy the rust schema: {:?}",
            ours.validate(&good).err()
        );

        // Serde round trip is lossless and still valid under the python schema.
        let parsed: ImageResource = serde_json::from_value(good.clone()).unwrap();
        parsed.validate().unwrap();
        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back, good);
        assert!(py.is_valid(&back));

        // Documents both schemas reject.
        let mut bads = Vec::new();
        let mut b = good.clone();
        b["spec"]["recipe"]["layers"][0] =
            json!({"type": "apt_install", "command": "x", "packages": ["a"]});
        bads.push(("layer mixing fields", b));
        let mut b = good.clone();
        b["spec"]["recipe"]["layers"][0] = json!({"type": "brew_install", "packages": ["a"]});
        bads.push(("unknown layer type", b));
        let mut b = good.clone();
        b["metadata"]["name"] = json!("Bad_Name");
        bads.push(("bad name", b));
        let mut b = good.clone();
        b["spec"]["recipe"]["osType"] = json!("windows");
        bads.push(("windows os", b));
        let mut b = good.clone();
        b["spec"]["recipe"]["extra"] = json!(1);
        bads.push(("unknown field", b));
        let mut b = good.clone();
        b["spec"]["build"]["diskSize"] = json!("40G");
        bads.push(("bad disk size", b));
        let mut b = good.clone();
        b["apiVersion"] = json!("images.cua.ai/v1");
        bads.push(("bad apiVersion", b));
        for (what, doc) in bads {
            assert!(!py.is_valid(&doc), "python accepted {what}");
            assert!(!ours.is_valid(&doc), "rust schema accepted {what}");
            let rust = serde_json::from_value::<ImageResource>(doc.clone())
                .map_err(|e| ImageError::Spec(e.to_string()))
                .and_then(|r| r.validate());
            assert!(rust.is_err(), "ImageResource accepted {what}");
        }
    }

    #[test]
    fn container_recipes_build_from_a_registry_base() {
        let py = jsonschema::validator_for(&python_schema()).unwrap();
        let ours = jsonschema::validator_for(&ImageResource::json_schema()).unwrap();
        let doc = json!({
            "apiVersion": "images.cua.ai/v1alpha1", "kind": "Image",
            "metadata": {"name": "cua-b-1", "namespace": "cua-build-x"},
            "spec": {"recipe": {
                "osType": "linux", "distro": "registry", "version": "3.12-slim",
                "kind": "container", "from": "python:3.12-slim",
                "fromPullSecret": "cua-registry-abc",
                "layers": [{"type": "pip_install", "packages": ["mcp"]}]}}
        });
        assert!(py.is_valid(&doc) && ours.is_valid(&doc));
        let r: ImageResource = serde_json::from_value(doc.clone()).unwrap();
        r.validate().unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap(), doc, "lossless");
        let reject = |f: &dyn Fn(&mut Value)| {
            let mut d = doc.clone();
            f(&mut d);
            serde_json::from_value::<ImageResource>(d)
                .map_err(|e| ImageError::Spec(e.to_string()))
                .and_then(|r| r.validate())
                .is_err()
        };
        assert!(reject(&|d| {
            d["spec"]["recipe"].as_object_mut().unwrap().remove("from");
        }));
        assert!(reject(&|d| d["spec"]["recipe"]["kind"] = json!("vm")));
        assert!(reject(
            &|d| d["spec"]["recipe"]["fromPullSecret"] = json!("ecr-credentials")
        ));
        assert!(reject(
            &|d| d["spec"]["recipe"]["layers"] = json!([{"type": "app_install", "appId": "x"}])
        ));
        // Old VM recipes are unchanged.
        let vm = sample();
        let r: ImageResource = serde_json::from_value(vm.clone()).unwrap();
        r.validate().unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap(), vm);
    }

    #[test]
    fn helpers() {
        let r = ImageResource::new(
            "x",
            "y",
            ImageRecipe {
                distro: "ubuntu".into(),
                version: "24.04".into(),
                layers: vec![ImageLayer::Run {
                    command: "true".into(),
                }],
                ..Default::default()
            },
        );
        assert!(r.recipe_digest().starts_with("sha256:"));
        assert_eq!(ImageBuildOptions::default().disk_size_gb().unwrap(), 40);
        assert_eq!(
            ImageBuildOptions {
                disk_size: "2Ti".into(),
                timeout_seconds: 60
            }
            .disk_size_gb()
            .unwrap(),
            2048
        );
        assert!(is_env_name("_A1") && !is_env_name("1A") && !is_env_name("A-B"));
    }
}
