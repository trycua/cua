//! Modal images, built in Cua Spaces.
//!
//! The client describes an image as layers: a Dockerfile fragment
//! (`FROM base`, `RUN ...`, `COPY ...`), the layers it builds on
//! (`base_images`), files to copy (`context_files`, a context mount) and
//! secrets for the build. The gateway turns each layer into a Cua
//! checkpoint:
//!
//! 1. A Space is made from the parent layer's checkpoint (or, for the first
//!    layer, from the Cua image its `FROM` maps to).
//! 2. The layer's instructions run in it as root through cua-spacesd
//!    (`RUN` in a shell, `COPY` of context files, `ENV`/`WORKDIR`/`USER`
//!    tracked as image configuration).
//! 3. The Space is checkpointed as `modal-<spec hash>` and deleted.
//!
//! A sandbox started from the image is a fork of that checkpoint. Builds are
//! cached by the hash of the layer's spec and its parent's, so the same
//! `pip_install` costs a build once. Concurrent requests for one layer share
//! the build.
//!
//! `FROM` mapping: Cua images (`ghcr.io/trycua/...`, anything in
//! `images.json`) are used as they are; `python:<version>[-slim...]` becomes
//! Cua's Linux image with that Python (uv-managed, on `PATH` as `python`,
//! `python3` and `pip`); `ubuntu:*` and `debian:*` become Cua's Linux image;
//! anything else is an `ImageBuildError` that says so, because an arbitrary
//! OCI image does not carry cua-spacesd.

use crate::backend::{CreateSpec, Placement, Source};
use crate::modal::Modal;
use crate::modal::db::{Tags, now_s};
use crate::modal::dockerfile::{self, Exec, Instr};
use cua_spacesd_client::{Command, ProcessEvent, SpacesdClient};
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Bump to invalidate every cached layer when the build logic changes.
pub const RECIPE_VERSION: u32 = 2;

pub type BuildFuture = Shared<BoxFuture<'static, Result<Arc<Built>, String>>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CtxFile {
    pub filename: String,
    pub sha256: String,
}

/// What the client sent for one layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Spec {
    /// `docker_tag` -> image id of the layers this one builds on.
    pub base_images: Vec<(String, String)>,
    pub commands: Vec<String>,
    pub context_files: Vec<CtxFile>,
    pub context_mount_id: String,
    pub secret_ids: Vec<String>,
    pub registry_secret_id: String,
    pub build_args: BTreeMap<String, String>,
    pub builder_version: String,
    pub gpu: String,
    /// `Image.run_function`: the Function that builds the layer, its source
    /// and globals (the client's hash material), and its input.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build_function_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build_function_def: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build_function_globals_sha: String,
    /// The Function's input (`FunctionInput`, encoded, in base64).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build_function_input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", content = "error", rename_all = "snake_case")]
pub enum Status {
    Pending,
    Building,
    Built,
    Failed(String),
}

/// What a built layer is: the checkpoint (or Cua image) a sandbox starts
/// from and the configuration the layer ended with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Built {
    /// The checkpoint holding the layer's filesystem.
    pub checkpoint: Option<String>,
    /// The Cua image to start from when there is no checkpoint (no layer
    /// changed the base).
    pub image: Option<String>,
    /// `FROM scratch`: nothing to run.
    pub scratch: bool,
    pub env: BTreeMap<String, String>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub cmd: Option<Vec<String>>,
    pub entrypoint: Option<Vec<String>>,
    pub shell: Option<Vec<String>>,
    pub python: Option<String>,
    pub exposed: Vec<String>,
    /// A filesystem snapshot: start fresh from the disk, not the memory.
    #[serde(default)]
    pub reboot: bool,
    /// A directory snapshot: a tar (SHA-256 in the blob store) of the
    /// directory, mounted into a sandbox with `mount_image`.
    #[serde(default)]
    pub dir_tar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRec {
    pub id: String,
    pub key_id: String,
    pub created_at: f64,
    pub spec: Spec,
    pub hash: String,
    pub status: Status,
    #[serde(default)]
    pub built: Option<Built>,
    #[serde(default)]
    pub tags: Tags,
    /// When a snapshot image expires (`ttl`); `None`: never.
    #[serde(default)]
    pub expires_at: Option<f64>,
}

/// The cache key of a layer: its spec, its parents' keys and the recipe
/// version. Secret and mount contents are part of it.
pub fn spec_hash(spec: &Spec, parents: &[String], extra: &[String]) -> String {
    let mut h = sha2::Sha256::new();
    let v = serde_json::json!({
        "recipe": RECIPE_VERSION,
        // The preludes are part of what a layer contains.
        "preludes": hex::encode(sha2::Sha256::digest(format!("{}{}", plain_distro_script(), python_script("3")))),
        "commands": spec.commands,
        "context": spec.context_files,
        "mount": spec.context_mount_id,
        "args": spec.build_args,
        "parents": parents,
        "extra": extra,
        "gpu": spec.gpu,
        "build_function": [spec.build_function_def, spec.build_function_globals_sha, spec.build_function_input],
    });
    h.update(v.to_string().as_bytes());
    hex::encode(h.finalize())
}

/// A layer's build log: lines, a change signal, and the outcome.
pub struct BuildLog {
    lines: Mutex<Vec<String>>,
    tx: watch::Sender<usize>,
    outcome: Mutex<Option<Result<(), String>>>,
}

impl BuildLog {
    pub fn new() -> Arc<BuildLog> {
        let (tx, _) = watch::channel(0);
        Arc::new(BuildLog {
            lines: Mutex::new(Vec::new()),
            tx,
            outcome: Mutex::new(None),
        })
    }

    pub fn push(&self, line: impl Into<String>) {
        let mut l = self.lines.lock().expect("log");
        l.push(line.into());
        let _ = self.tx.send(l.len());
    }

    pub fn finish(&self, r: Result<(), String>) {
        *self.outcome.lock().expect("log") = Some(r);
        let _ = self.tx.send(usize::MAX);
    }

    pub fn outcome(&self) -> Option<Result<(), String>> {
        self.outcome.lock().expect("log").clone()
    }

    pub fn lines_from(&self, from: usize) -> Vec<String> {
        let l = self.lines.lock().expect("log");
        l.get(from..).map(<[String]>::to_vec).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.lock().expect("log").is_empty()
    }

    pub fn subscribe(&self) -> watch::Receiver<usize> {
        self.tx.subscribe()
    }
}

/// How a `FROM` reference maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Base {
    /// A Cua image that carries cua-spacesd.
    Cua(String),
    /// Cua's Linux image, with this Python on `PATH` (`python:<v>`).
    Python(String),
    /// Cua's Linux image (`ubuntu:*`, `debian:*`).
    Distro,
    /// Cua's Linux image with micromamba (`mambaorg/micromamba:<v>`, what
    /// `Image.micromamba` starts from).
    Micromamba(String),
    Scratch,
}

/// What a stock `ubuntu`/`debian`/`python` image has for apt: the distribution's
/// own sources only.
const PLAIN_DISTRO_HEAD: &str = r#"for f in /etc/apt/sources.list.d/*; do
  case "$f" in */ubuntu.sources|*/debian.sources) ;; *) rm -f -- "$f" ;; esac
done
rm -f /var/lib/apt/lists/*docker* /var/lib/apt/lists/*mozilla* /var/lib/apt/lists/*launchpad* 2>/dev/null || true
"#;

/// A VM disk whose cloud-init could not name the release wrote `UNAVAILABLE` suites; name them.
pub const FIX_APT_SOURCES: &str = r#"# A VM disk's cloud-init could not name the release when it wrote the sources: suites read UNAVAILABLE.
if grep -qs UNAVAILABLE /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list; then
  C="$(. /etc/os-release 2>/dev/null; echo "${VERSION_CODENAME:-}")"
  if [ -z "$C" ]; then
    case "$(. /etc/os-release 2>/dev/null; echo "${VERSION_ID:-}")" in
      26.04) C=resolute;; 25.10) C=questing;; 25.04) C=plucky;; 24.10) C=oracular;; 24.04) C=noble;; 22.04) C=jammy;; 20.04) C=focal;;
    esac
  fi
  [ -z "$C" ] || sed -i "s/UNAVAILABLE/$C/g" /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list 2>/dev/null || true
fi
"#;

/// The prelude of a stock-distribution base.
fn plain_distro_script() -> String {
    format!("{PLAIN_DISTRO_HEAD}{FIX_APT_SOURCES}exit 0\n")
}

/// `pickle.dumps(((), {}), protocol=4)`: the arguments of a build function
/// given none.
const NO_ARGS_PICKLE: &[u8] = b"\x80\x04\x95\x06\x00\x00\x00\x00\x00\x00\x00)}\x94\x86\x94.";

/// The pseudo App a layer's build logs belong to, and its task.
pub fn build_app_id(image_id: &str) -> String {
    format!("ap-ib-{image_id}")
}

pub fn build_task_id(image_id: &str) -> String {
    format!("ta-ib-{image_id}")
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Maps a `FROM` reference (see the module docs).
pub fn classify_base(
    reference: &str,
    map: &std::collections::HashMap<String, String>,
) -> Result<Base, String> {
    let r = reference.trim();
    if r.eq_ignore_ascii_case("scratch") {
        return Ok(Base::Scratch);
    }
    if let Some(t) = map.get(r) {
        return Ok(Base::Cua(t.clone()));
    }
    if r.starts_with("ghcr.io/trycua/") {
        return Ok(Base::Cua(r.to_string()));
    }
    let short = r
        .strip_prefix("docker.io/library/")
        .or_else(|| r.strip_prefix("index.docker.io/library/"))
        .or_else(|| r.strip_prefix("docker.io/"))
        .unwrap_or(r);
    let (name, tag) = short.split_once(':').unwrap_or((short, "latest"));
    let name = name.split('@').next().unwrap_or(name);
    match name {
        "python" => Ok(Base::Python(python_version(tag))),
        "ubuntu" | "debian" => Ok(Base::Distro),
        "mambaorg/micromamba" => Ok(Base::Micromamba(python_version(tag))),
        _ => Err(format!(
            "the base image {r:?} cannot run on Cua Spaces: an arbitrary container image does not carry cua-spacesd. Use a Cua image (`ghcr.io/trycua/...`), `python:<version>`, `ubuntu:<version>` or `debian:<version>`, or map {r:?} to a Cua image in images.json in the gateway directory"
        )),
    }
}

/// The Python version in a `python:` tag: `3.12.7-slim-bookworm` is
/// `3.12.7`; a tag without a version (`slim`, `latest`) is `3`.
pub fn python_version(tag: &str) -> String {
    let v: String = tag
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v = v.trim_matches('.').to_string();
    if v.is_empty() { "3".into() } else { v }
}

/// The Python version of Modal's `python-build-standalone.<release>.<version>-gnu`
/// mount name (`3.11.5`; free-threaded builds end in `t`).
pub fn standalone_python_version(mount_name: &str) -> Option<String> {
    let rest = mount_name.strip_prefix("python-build-standalone.")?;
    let (_, v) = rest.split_once('.')?;
    let v = v.strip_suffix("-gnu").unwrap_or(v);
    let ok = !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 't');
    ok.then(|| v.to_string())
}

/// Shell for installing Python `v` with uv and putting it first on PATH.
pub fn python_script(v: &str) -> String {
    let minor: String = {
        let parts: Vec<&str> = v.split('.').collect();
        match parts.as_slice() {
            [a, b, ..] => format!("{a}.{b}"),
            [a] => a.to_string(),
            [] => "3".into(),
        }
    };
    format!(
        r#"set -e
V='{v}'
# A python:* image's pip installs freely; distribution Pythons mark themselves externally managed.
rm -f /usr/lib/python3*/EXTERNALLY-MANAGED 2>/dev/null || true
if [ -x /usr/local/bin/python ] && /usr/local/bin/python -c "import sys; v='{minor}'.split('.'); sys.exit(0 if all(a == str(b) for a, b in zip(v, sys.version_info[:len(v)])) else 1)" 2>/dev/null; then
  # A distribution's python3 (apt-managed pip) is not what a python:* image has: install uv's.
  case "$(readlink -f /usr/local/bin/python)" in /usr/bin/*|/bin/*) ;; *) exit 0 ;; esac
fi
export UV_PYTHON_INSTALL_DIR=/opt/uv-python
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
fi
uv python install "$V"
P="$(uv python find --managed-python "$V")"
# What a python:* image has: the interpreter lives under /usr/local, so pip's console scripts
# (`jupyter`, `uvicorn`, ...) land in /usr/local/bin, on PATH. Copy the build's tree there.
ROOT="$(dirname "$(dirname "$(readlink -f "$P")")")"
cp -a "$ROOT"/. /usr/local/
find /usr/local/lib -maxdepth 2 -name EXTERNALLY-MANAGED -delete 2>/dev/null || true
B=/usr/local/bin
[ -e $B/python{minor} ] || ln -sf "$P" $B/python{minor}
for n in python python3; do ln -sf $B/python{minor} $B/$n; done
$B/python{minor} -m ensurepip --upgrade >/dev/null 2>&1 || true
[ -e $B/pip3 ] && ln -sf $B/pip3 $B/pip || true
[ -e $B/pip ] || printf '#!/bin/sh\nexec %s -m pip "$@"\n' $B/python{minor} > $B/pip && chmod +x $B/pip
python --version
"#
    )
}

/// Shell for what `mambaorg/micromamba` carries: the `micromamba` binary
/// and the scripts Modal's micromamba images build with (`SHELL
/// ["/usr/local/bin/_dockerfile_shell.sh"]`, which runs a command with the
/// base environment active).
pub fn micromamba_script(v: &str) -> String {
    let ver = if v.is_empty() || v == "3" {
        "latest".to_string()
    } else {
        v.to_string()
    };
    format!(
        r#"set -e
if ! command -v micromamba >/dev/null 2>&1; then
  case "$(uname -m)" in x86_64) P=linux-64;; aarch64|arm64) P=linux-aarch64;; *) echo "no micromamba build for $(uname -m)" >&2; exit 1;; esac
  curl -fLs "https://micro.mamba.pm/api/micromamba/$P/{ver}" | tar -xj -C /usr/local bin/micromamba
fi
mkdir -p /opt/conda /usr/local/bin
cat > /usr/local/bin/_activate_current_env.sh <<'EOS'
eval "$(micromamba shell hook --shell=bash)"
micromamba activate "${{ENV_NAME:-base}}"
EOS
cat > /usr/local/bin/_dockerfile_shell.sh <<'EOS'
#!/bin/bash
if [ "${{MAMBA_DOCKERFILE_ACTIVATE:-0}}" = 1 ]; then source /usr/local/bin/_activate_current_env.sh; fi
exec /bin/bash -c "$*"
EOS
chmod 0755 /usr/local/bin/_dockerfile_shell.sh
"#
    )
}

fn log_line(log: &BuildLog, s: &str) {
    for l in s.lines() {
        log.push(l.to_string());
    }
}

impl Modal {
    pub fn image(&self, id: &str) -> Option<ImageRec> {
        self.db.read(|d| d.images.get(id).cloned())
    }

    /// Spec of one `Image` message; context file data goes to the blob
    /// store. Fails when a base image id is unknown.
    pub fn spec_of(
        &self,
        img: &crate::modal::pb::api::Image,
        builder_version: &str,
        build_function_id: &str,
    ) -> Result<Spec, String> {
        let mut context_files = Vec::new();
        for f in &img.context_files {
            let sha = self.blobs.put_sha(None, &f.data)?;
            context_files.push(CtxFile {
                filename: f.filename.clone(),
                sha256: sha,
            });
        }
        Ok(Spec {
            base_images: img
                .base_images
                .iter()
                .map(|b| (b.docker_tag.clone(), b.image_id.clone()))
                .collect(),
            commands: img.dockerfile_commands.clone(),
            context_files,
            context_mount_id: img.context_mount_id.clone(),
            secret_ids: img.secret_ids.clone(),
            registry_secret_id: img
                .image_registry_config
                .as_ref()
                .map(|c| c.secret_id.clone())
                .unwrap_or_default(),
            build_args: img
                .build_args
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            builder_version: builder_version.to_string(),
            gpu: img
                .gpu_config
                .as_ref()
                .map(|g| g.gpu_type.clone())
                .unwrap_or_default(),
            build_function_id: build_function_id.to_string(),
            build_function_def: img
                .build_function
                .as_ref()
                .map(|b| b.definition.clone())
                .unwrap_or_else(|| img.build_function_def.clone()),
            build_function_globals_sha: {
                let g = img
                    .build_function
                    .as_ref()
                    .map(|b| b.globals.clone())
                    .unwrap_or_else(|| img.build_function_globals.clone());
                if g.is_empty() {
                    String::new()
                } else {
                    hex::encode(sha2::Sha256::digest(&g))
                }
            },
            build_function_input: img
                .build_function
                .as_ref()
                .and_then(|b| b.input.as_ref())
                .map(|i| {
                    use base64::Engine;
                    use prost::Message;
                    base64::engine::general_purpose::STANDARD.encode(i.encode_to_vec())
                })
                .unwrap_or_default(),
        })
    }

    /// The hash of a spec given the layers it builds on.
    pub fn hash_of(&self, key_id: &str, spec: &Spec) -> String {
        let parents: Vec<String> = spec
            .base_images
            .iter()
            .map(|(tag, id)| {
                format!(
                    "{tag}={}",
                    self.image(id).map(|r| r.hash).unwrap_or_else(|| id.clone())
                )
            })
            .collect();
        // Mount files and secrets are part of what the layer contains.
        let mut extra = vec![key_id.to_string()];
        // A layer built on a stock distribution base holds whatever Cua image
        // that base maps to: another default image is another layer.
        extra.push(format!("base:{}", self.base_image()));
        if let Some(m) = self
            .db
            .read(|d| d.mounts.get(&spec.context_mount_id).cloned())
        {
            extra.push(format!("mountname:{:?}", m.name));
            for f in &m.files {
                extra.push(format!("mount:{}:{}:{:?}", f.filename, f.sha256, f.mode));
            }
        }
        for sid in &spec.secret_ids {
            if let Some(s) = self.db.read(|d| d.secrets.get(sid).cloned()) {
                extra.push(format!(
                    "secret:{}",
                    hex::encode(sha2::Sha256::digest(
                        serde_json::to_vec(&s.env).unwrap_or_default()
                    ))
                ));
            }
        }
        spec_hash(spec, &parents, &extra)
    }

    /// Builds (or finds built) the image `id`: a shared future per layer.
    pub fn ensure_image(
        self: &Arc<Self>,
        id: String,
    ) -> BoxFuture<'static, Result<Arc<Built>, String>> {
        let me = self.clone();
        async move {
            let rec = me
                .image(&id)
                .ok_or_else(|| format!("image {id} not found"))?;
            if rec.status == Status::Built
                && let Some(b) = &rec.built
                && me.layer_present(&rec.key_id, b).await
            {
                return Ok(Arc::new(b.clone()));
            }
            let fut = {
                let mut builds = me.live.builds.lock().await;
                if let Some(f) = builds.get(&rec.hash) {
                    f.clone()
                } else {
                    let m2 = me.clone();
                    let id2 = id.clone();
                    let hash = rec.hash.clone();
                    let f: BuildFuture = async move {
                        let r = m2.build_image(&id2).await.map(Arc::new);
                        m2.live.builds.lock().await.remove(&hash);
                        r
                    }
                    .boxed()
                    .shared();
                    builds.insert(rec.hash.clone(), f.clone());
                    f
                }
            };
            fut.await
        }
        .boxed()
    }

    /// Whether a built layer's checkpoint still exists.
    async fn layer_present(&self, key_id: &str, b: &Built) -> bool {
        let Some(cp) = &b.checkpoint else { return true };
        let who = self.identity_of_key(key_id).await;
        match self.backend.checkpoints(&who).await {
            Ok(list) => list.iter().any(|c| &c.name == cp),
            // Cannot tell: assume it is there and let the fork say.
            Err(_) => true,
        }
    }

    pub async fn identity_of_key(&self, key_id: &str) -> crate::backend::Identity {
        let keys = self.keys.read().await;
        keys.keys
            .iter()
            .find(|k| k.id() == key_id)
            .map(|k| crate::backend::Identity {
                daemon: k.daemon.clone(),
                token: k.token.clone(),
            })
            .unwrap_or_default()
    }

    fn set_status(&self, id: &str, status: Status, built: Option<Built>) {
        self.db.write(|d| {
            if let Some(r) = d.images.get_mut(id) {
                r.status = status;
                if built.is_some() {
                    r.built = built;
                }
            }
        });
    }

    /// The build's output as the logs of the layer's pseudo builder App, which
    /// is how the client reads it (`Image.logs`, `ImageBuildChainGet`).
    fn record_build_log(&self, id: &str, log: &Arc<BuildLog>, started_ns: u64) {
        let app = build_app_id(id);
        let task = build_task_id(id);
        let mut g = self.app_logs.lock().expect("app logs");
        let v = g.entry(app.clone()).or_default();
        v.clear();
        for (i, line) in log.lines_from(0).into_iter().enumerate() {
            v.push(crate::modal::containers::AppLogLine {
                // A microsecond apart: the client reads times as float seconds.
                at_ns: started_ns + 2_000 + 1_000 * i as u64,
                fd: 0,
                text: format!("{line}\n"),
                function_id: String::new(),
                task_id: task.clone(),
            });
        }
        let last = v.last().map_or(started_ns, |l| l.at_ns);
        self.build_spans
            .lock()
            .expect("build spans")
            .insert(id.to_string(), (started_ns, now_ns().max(last + 2_000)));
    }

    pub async fn build_log(&self, id: &str) -> Arc<BuildLog> {
        self.live
            .build_logs
            .lock()
            .await
            .entry(id.to_string())
            .or_insert_with(BuildLog::new)
            .clone()
    }

    async fn build_image(self: &Arc<Self>, id: &str) -> Result<Built, String> {
        let log = self.build_log(id).await;
        self.set_status(id, Status::Building, None);
        let started = now_ns();
        let r = self.build_layer(id, &log).await;
        self.record_build_log(id, &log, started);
        match &r {
            Ok(b) => {
                self.set_status(id, Status::Built, Some(b.clone()));
                log.finish(Ok(()));
            }
            Err(e) => {
                log.push(format!("Build failed: {e}"));
                self.set_status(id, Status::Failed(e.clone()), None);
                log.finish(Err(e.clone()));
            }
        }
        r
    }

    async fn build_layer(self: &Arc<Self>, id: &str, log: &Arc<BuildLog>) -> Result<Built, String> {
        let rec = self.image(id).ok_or("image vanished")?;
        let spec = rec.spec.clone();
        if !spec.build_function_id.is_empty() {
            return self.build_with_function(&rec, &spec, log).await;
        }
        let text = spec.commands.join("\n");
        let instrs = dockerfile::parse(&text).map_err(|e| e.to_string())?;
        let Some(Instr::From { reference, .. }) = instrs.first() else {
            return Err("an image layer must start with FROM".into());
        };
        let stages = instrs
            .iter()
            .filter(|i| matches!(i, Instr::From { .. }))
            .count();
        if stages > 1 {
            return Err("multi-stage Dockerfiles are not supported in image layers yet".into());
        }
        // The starting point.
        let mut state = Built::default();
        let (mut source, mut checkpoint_in): (Option<Source>, Option<String>) = (None, None);
        let mut python_prelude: Option<String> = None;
        let mut mamba_prelude: Option<String> = None;
        let mut plain_distro = false;
        if let Some((_, parent_id)) = spec.base_images.iter().find(|(t, _)| t == reference) {
            let parent = self.ensure_image(parent_id.clone()).await?;
            state = (*parent).clone();
            match (&parent.checkpoint, &parent.image) {
                (Some(cp), _) => {
                    source = Some(Source::Checkpoint(cp.clone()));
                    checkpoint_in = Some(cp.clone());
                }
                (None, Some(img)) => source = Some(Source::Image(Some(img.clone()))),
                (None, None) if parent.scratch => {}
                (None, None) => {
                    source = Some(Source::Image(Some(self.base_image())));
                }
            }
        } else {
            match classify_base(reference, &self.cfg.image_map)? {
                Base::Cua(img) => {
                    state.image = Some(img.clone());
                    source = Some(Source::Image(Some(img)));
                }
                Base::Python(v) => {
                    plain_distro = true;
                    state.python = Some(v.clone());
                    python_prelude = Some(v);
                    source = Some(Source::Image(Some(self.base_image())));
                }
                Base::Distro => {
                    plain_distro = true;
                    source = Some(Source::Image(Some(self.base_image())))
                }
                Base::Micromamba(v) => {
                    plain_distro = true;
                    mamba_prelude = Some(v);
                    source = Some(Source::Image(Some(self.base_image())));
                    // What the micromamba image sets.
                    state
                        .env
                        .insert("MAMBA_ROOT_PREFIX".into(), "/opt/conda".into());
                    state
                        .env
                        .insert("MAMBA_EXE".into(), "/usr/local/bin/micromamba".into());
                    state.env.insert("ENV_NAME".into(), "base".into());
                    state.env.insert(
                        "PATH".into(),
                        "/opt/conda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                    );
                }
                Base::Scratch => state.scratch = true,
            }
        }
        // Does anything here change the filesystem?
        let mutates = python_prelude.is_some()
            || mamba_prelude.is_some()
            || plain_distro
            || instrs
                .iter()
                .any(|i| matches!(i, Instr::Run(_) | Instr::Copy(_)));
        let mut envs: BTreeMap<String, String> = state.env.clone();
        let mut args: BTreeMap<String, String> = BTreeMap::new();
        let mut declared: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut workdir = state.workdir.clone().unwrap_or_else(|| "/".into());
        let mut user = state.user.clone().unwrap_or_default();
        let mut shell = state
            .shell
            .clone()
            .unwrap_or_else(|| vec!["/bin/sh".into(), "-c".into()]);

        // The build environment: secrets as environment variables.
        let mut secret_env = BTreeMap::new();
        for sid in &spec.secret_ids {
            if let Some(s) = self.db.read(|d| d.secrets.get(sid).cloned()) {
                secret_env.extend(s.env);
            }
        }

        let mut space: Option<BuildSpace> = None;
        if mutates {
            let src = source
                .clone()
                .ok_or("this layer has nothing to run in (FROM scratch)")?;
            log.push(format!(
                "Creating the build Space from {}",
                match &src {
                    Source::Checkpoint(c) => format!("checkpoint {c}"),
                    Source::Image(Some(i)) => format!("image {i}"),
                    Source::Image(None) => "the default Cua image".into(),
                }
            ));
            space = Some(self.open_build_space(&rec.key_id, id, src).await?);
        }
        let total = instrs.len();
        let result: Result<(), String> = async {
            if plain_distro && let Some(sp) = &space {
                // A stock distribution image has only the distribution's own
                // package sources; the Cua image also carries third-party ones
                // (whose outage fails `apt-get update`), so they are dropped.
                sp.run(
                    &["/bin/sh".into(), "-c".into(), plain_distro_script()],
                    &BTreeMap::new(),
                    "/",
                    "root",
                    log,
                )
                .await
                .map_err(|e| format!("preparing the base image failed (exit {e})"))?;
            }
            if let (Some(v), Some(sp)) = (&python_prelude, &space) {
                log.push(format!("Installing Python {v}"));
                sp.run(
                    &["/bin/sh".into(), "-c".into(), python_script(v)],
                    &BTreeMap::new(),
                    "/",
                    "root",
                    log,
                )
                .await
                .map_err(|e| format!("installing Python {v} failed: {e}"))?;
            }
            if let (Some(v), Some(sp)) = (&mamba_prelude, &space) {
                log.push(format!("Installing micromamba {v}"));
                sp.run(
                    &["/bin/sh".into(), "-c".into(), micromamba_script(v)],
                    &BTreeMap::new(),
                    "/",
                    "root",
                    log,
                )
                .await
                .map_err(|e| format!("installing micromamba {v} failed: {e}"))?;
            }
            for (n, ins) in instrs.iter().enumerate() {
                let step = n + 1;
                match ins {
                    Instr::From { reference, .. } => {
                        log.push(format!("Step {step}/{total} : FROM {reference}"));
                    }
                    Instr::Arg { name, default } => {
                        log.push(format!("Step {step}/{total} : ARG {name}"));
                        declared.insert(name.clone(), default.clone());
                        if let Some(v) = spec.build_args.get(name).cloned().or(default.clone()) {
                            args.insert(name.clone(), v);
                        }
                    }
                    Instr::Env(kv) => {
                        log.push(format!("Step {step}/{total} : ENV"));
                        for (k, v) in kv {
                            let scope = scope(&envs, &args);
                            envs.insert(k.clone(), dockerfile::expand(v, &scope));
                        }
                    }
                    Instr::Workdir(w) => {
                        log.push(format!("Step {step}/{total} : WORKDIR {w}"));
                        let w = dockerfile::expand(w, &scope(&envs, &args));
                        workdir = if w.starts_with('/') {
                            w
                        } else {
                            format!("{}/{}", workdir.trim_end_matches('/'), w)
                        };
                        if let Some(sp) = &space {
                            sp.run(
                                &[
                                    "/bin/sh".into(),
                                    "-c".into(),
                                    format!("mkdir -p -- '{}'", workdir.replace('\'', "'\\''")),
                                ],
                                &BTreeMap::new(),
                                "/",
                                "root",
                                log,
                            )
                            .await?;
                        }
                    }
                    Instr::User(u) => {
                        log.push(format!("Step {step}/{total} : USER {u}"));
                        user = dockerfile::expand(u, &scope(&envs, &args));
                    }
                    Instr::Shell(s) => {
                        log.push(format!("Step {step}/{total} : SHELL"));
                        shell = s.clone();
                    }
                    Instr::Cmd(e) => {
                        log.push(format!("Step {step}/{total} : CMD"));
                        state.cmd = Some(argv_of(e, &shell));
                    }
                    Instr::Entrypoint(e) => {
                        log.push(format!("Step {step}/{total} : ENTRYPOINT"));
                        state.entrypoint = Some(argv_of(e, &shell));
                        // A new ENTRYPOINT resets the CMD, as in Docker.
                        state.cmd = None;
                    }
                    Instr::Expose(p) => {
                        log.push(format!("Step {step}/{total} : EXPOSE {}", p.join(" ")));
                        state.exposed.extend(p.iter().cloned());
                    }
                    Instr::Label(_)
                    | Instr::Volume(_)
                    | Instr::StopSignal(_)
                    | Instr::Healthcheck(_) => {
                        log.push(format!("Step {step}/{total} : (recorded)"));
                    }
                    Instr::Run(e) => {
                        let sp = space.as_ref().ok_or("no build Space")?;
                        let mut env = envs.clone();
                        env.extend(args.clone());
                        env.extend(secret_env.clone());
                        let argv = match e {
                            Exec::Shell(line) => {
                                log.push(format!("Step {step}/{total} : RUN {line}"));
                                let mut a = shell.clone();
                                a.push(line.clone());
                                a
                            }
                            Exec::Argv(v) => {
                                log.push(format!("Step {step}/{total} : RUN {v:?}"));
                                v.clone()
                            }
                        };
                        let who = if user.is_empty() {
                            "root"
                        } else {
                            user.as_str()
                        };
                        sp.run(&argv, &env, &workdir, who, log)
                            .await
                            .map_err(|code| {
                                let all = log.lines_from(0);
                                let tail = all[all.len().saturating_sub(12)..].join("\n");
                                format!(
                                    "The command '{}' returned a non-zero code: {code}\n{tail}",
                                    argv.join(" ")
                                )
                            })?;
                    }
                    Instr::Copy(c) => {
                        let sp = space.as_ref().ok_or("no build Space")?;
                        log.push(format!(
                            "Step {step}/{total} : {} {} {}",
                            if c.add { "ADD" } else { "COPY" },
                            c.sources.join(" "),
                            c.dest
                        ));
                        self.copy_into(sp, &spec, c, &envs, &args, &workdir, log)
                            .await?;
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            if let Some(sp) = space {
                sp.discard().await;
            }
            return Err(e);
        }
        state.env = envs;
        state.workdir = (workdir != "/").then_some(workdir).or(state.workdir);
        state.user = (!user.is_empty()).then_some(user);
        state.shell = Some(shell);
        if let Some(sp) = space {
            let cp = format!("modal-{}", &rec.hash[..20]);
            log.push(format!("Saving the layer as checkpoint {cp}"));
            sp.checkpoint_as(&cp).await?;
            sp.discard().await;
            state.checkpoint = Some(cp);
            state.image = None;
        } else {
            state.checkpoint = checkpoint_in;
        }
        log.push("Image build succeeded");
        Ok(state)
    }

    pub async fn open_build_space(
        self: &Arc<Self>,
        key_id: &str,
        image_id: &str,
        source: Source,
    ) -> Result<BuildSpace, String> {
        let who = self.identity_of_key(key_id).await;
        let name = format!("modal-build-{}", image_id.to_ascii_lowercase());
        let created = self
            .backend
            .create(
                &who,
                CreateSpec {
                    name,
                    source,
                    env: Default::default(),
                    internet: true,
                    place: Placement::default(),
                },
            )
            .await
            .map_err(|e| format!("could not start a build Space: {e}"))?;
        let client = self
            .backend
            .spacesd(&who, &created.name)
            .await
            .map_err(|e| format!("could not reach the build Space: {e}"))?;
        let driver = self.live.drivers.of(&created.name, &client).await;
        Ok(BuildSpace {
            dry_run: self.cfg.dry_run_builds,
            m: self.clone(),
            who,
            name: created.name,
            client,
            driver,
        })
    }
}

fn scope(
    env: &BTreeMap<String, String>,
    args: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut s = args.clone();
    s.extend(env.clone());
    s
}

fn argv_of(e: &Exec, shell: &[String]) -> Vec<String> {
    match e {
        Exec::Argv(v) => v.clone(),
        Exec::Shell(line) => {
            let mut a = shell.to_vec();
            a.push(line.clone());
            a
        }
    }
}

/// A Space an image layer is being built in.
pub struct BuildSpace {
    /// Hermetic tests over mock Spaces: commands are logged, not run.
    dry_run: bool,
    m: Arc<Modal>,
    who: crate::backend::Identity,
    pub name: String,
    pub client: SpacesdClient,
    driver: String,
}

impl BuildSpace {
    /// Runs `argv` as `user` in `cwd`, streaming output to `log`; `Err`
    /// carries the exit code (or the reason).
    pub async fn run(
        &self,
        argv: &[String],
        env: &BTreeMap<String, String>,
        cwd: &str,
        user: &str,
        log: &Arc<BuildLog>,
    ) -> Result<(), String> {
        if self.dry_run {
            log.push(format!("(dry run) {}", argv.join(" ")));
            return Ok(());
        }
        let (program, args, u, cwd) = crate::guest::as_user_in(
            &self.driver,
            user,
            argv[0].clone(),
            argv[1..].to_vec(),
            cwd.to_string(),
        );
        let mut cmd = Command::new(program).args(args);
        for (k, v) in env {
            cmd = cmd.env(k, v);
        }
        if !u.is_empty() {
            cmd = cmd.user(u);
        }
        if !cwd.is_empty() {
            cmd = cmd.cwd(cwd);
        }
        let mut h = self.client.spawn(cmd).await.map_err(|e| e.to_string())?;
        let mut pending = String::new();
        loop {
            match h.next_event().await.map_err(|e| e.to_string())? {
                Some(
                    ProcessEvent::Stdout { data, .. }
                    | ProcessEvent::Stderr { data, .. }
                    | ProcessEvent::Pty { data, .. },
                ) => {
                    pending.push_str(&String::from_utf8_lossy(&data));
                    while let Some(i) = pending.find('\n') {
                        let line: String = pending.drain(..=i).collect();
                        log.push(line.trim_end_matches(['\n', '\r']).to_string());
                    }
                }
                Some(ProcessEvent::Exit(st)) => {
                    if !pending.is_empty() {
                        log_line(log, &pending);
                    }
                    return match (st.code, st.error) {
                        (Some(0), _) => Ok(()),
                        (Some(c), _) => Err(c.to_string()),
                        (None, Some(e)) => Err(e),
                        (None, None) => Err(format!("killed by {:?}", st.signal)),
                    };
                }
                None => return Err("the build process vanished".into()),
            }
        }
    }

    pub async fn checkpoint_as(&self, cp: &str) -> Result<(), String> {
        // A leftover from an earlier, interrupted build of the same layer.
        let _ = self.m.backend.delete_checkpoint(&self.who, cp).await;
        self.m
            .checkpoint_retry(&self.who, &self.name, cp)
            .await
            .map_err(|e| format!("could not save the layer: {e}"))
    }

    pub async fn discard(&self) {
        let _ = self.m.backend.delete(&self.who, &self.name).await;
        self.m.live.drivers.forget(&self.name).await;
    }

    pub fn driver(&self) -> &str {
        &self.driver
    }
}

impl Modal {
    /// `Image.run_function`: runs the Function in a container made from the
    /// parent layer and takes the container's disk as the layer. The Function
    /// is the one the client registered (`is_builder_function`); the input is
    /// the arguments `run_function` was given.
    async fn build_with_function(
        self: &Arc<Self>,
        rec: &ImageRec,
        spec: &Spec,
        log: &Arc<BuildLog>,
    ) -> Result<Built, String> {
        use crate::modal::pb::api as pb;
        let parent_id = spec
            .base_images
            .first()
            .map(|(_, id)| id.clone())
            .ok_or("a build function needs the image it runs on")?;
        let parent = self.ensure_image(parent_id).await?;
        let f = self
            .function(&spec.build_function_id)
            .filter(|f| f.key_id == rec.key_id && f.created)
            .ok_or_else(|| {
                format!(
                    "the build function {} was not created",
                    spec.build_function_id
                )
            })?;
        let def = self.function_def(&f);
        let caller = self
            .caller_of_key(&rec.key_id)
            .await
            .ok_or("the key of this image is gone")?;
        log.push(format!(
            "Running the build function {} in a container",
            def.function_name
        ));
        let input = if spec.build_function_input.is_empty() {
            pb::FunctionInput {
                args_oneof: Some(pb::function_input::ArgsOneof::Args(
                    NO_ARGS_PICKLE.to_vec().into(),
                )),
                data_format: pb::DataFormat::Pickle as i32,
                ..Default::default()
            }
        } else {
            use base64::Engine;
            use prost::Message;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(&spec.build_function_input)
                .map_err(|e| format!("the build function's arguments: {e}"))?;
            pb::FunctionInput::decode(raw.as_slice())
                .map_err(|e| format!("the build function's arguments: {e}"))?
        };
        let call = self.new_call(&caller, &f, 0);
        self.add_inputs(
            &caller,
            &f.id,
            &call,
            &[pb::FunctionPutInputsItem {
                idx: 0,
                input: Some(input),
                ..Default::default()
            }],
        )
        .map_err(|e| e.message().to_string())?;
        let wait =
            std::time::Duration::from_secs(u64::from(def.timeout_secs).clamp(60, 3600) + 300);
        let out = self
            .wait_outputs(&caller, &call, None, "", 1, wait, false)
            .await
            .map_err(|e| e.message().to_string())?;
        let item = out
            .outputs
            .into_iter()
            .next()
            .ok_or("the build function did not finish in time")?;
        let result = item.result.unwrap_or_default();
        if result.status != 1 {
            let why = if result.exception.is_empty() {
                "it failed".to_string()
            } else {
                result.exception
            };
            self.stop_build_container(&f.id, &item.task_id);
            return Err(format!(
                "the build function {} failed: {why}",
                def.function_name
            ));
        }
        // The container that ran it holds the layer's disk.
        let space = {
            let g = self.fns.lock().expect("fns");
            g.queues
                .get(&f.id)
                .and_then(|q| q.containers.get(&item.task_id))
                .and_then(|c| c.space.clone())
        };
        let mut built = (*parent).clone();
        if let Some((who, name)) = space {
            let cp = format!("modal-{}", &rec.hash[..20.min(rec.hash.len())]);
            let _ = self.backend.delete_checkpoint(&who, &cp).await;
            // The runtime's own files are not part of the layer.
            if let Ok(client) = self.backend.spacesd(&who, &name).await {
                let driver = self.live.drivers.of(&name, &client).await;
                let _ =
                    crate::guest::run_as(&client, &driver, "root", "rm -rf /__modal", &[]).await;
            }
            self.checkpoint_retry(&who, &name, &cp)
                .await
                .map_err(|e| format!("could not save the layer: {e}"))?;
            built.checkpoint = Some(cp);
            built.image = None;
            built.reboot = false;
        } else {
            log.push("(test runtime: no Space to snapshot; the layer is its parent)".to_string());
        }
        self.stop_build_container(&f.id, &item.task_id);
        log.push("Build function finished");
        Ok(built)
    }

    /// Ends the one-use container of a build function.
    fn stop_build_container(&self, function_id: &str, task_id: &str) {
        {
            let mut g = self.fns.lock().expect("fns");
            if let Some(c) = g
                .queues
                .get_mut(function_id)
                .and_then(|q| q.containers.get_mut(task_id))
            {
                c.stop = true;
            }
        }
        self.fn_changed();
    }

    /// The caller (key entry) behind a key id.
    pub async fn caller_of_key(&self, key_id: &str) -> Option<crate::modal::Caller> {
        self.keys
            .read()
            .await
            .keys
            .iter()
            .find(|k| k.id() == key_id)
            .cloned()
            .map(|k| crate::modal::Caller(k, false))
    }

    /// `COPY --from=ghcr.io/astral-sh/uv:<tag> /uv <dest>`: how Modal's
    /// `uv_pip_install` and `uv_sync` put `uv` in an image. The image holds
    /// only the binary, so the same release is installed from Astral's
    /// installer into the destination. Any other `--from` source is refused.
    async fn copy_from_uv(
        &self,
        sp: &BuildSpace,
        from: &str,
        c: &dockerfile::Copy,
        envs: &BTreeMap<String, String>,
        args: &BTreeMap<String, String>,
        log: &Arc<BuildLog>,
    ) -> Result<(), String> {
        let (repo, tag) = from.split_once(':').unwrap_or((from, "latest"));
        let srcs: Vec<&str> = c.sources.iter().map(String::as_str).collect();
        if repo != "ghcr.io/astral-sh/uv" || !matches!(srcs.as_slice(), ["/uv"] | ["/uvx"]) {
            return Err(format!(
                "COPY --from={from} needs a multi-stage build; only the uv binary (`COPY --from=ghcr.io/astral-sh/uv:<tag> /uv <dest>`, what uv_pip_install writes) is supported in image layers"
            ));
        }
        if !tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
        {
            return Err(format!("the uv tag {tag:?} is not a version"));
        }
        let sc = scope(envs, args);
        let dest = dockerfile::expand(&c.dest, &sc);
        let url = if tag == "latest" {
            "https://astral.sh/uv/install.sh".to_string()
        } else {
            format!("https://astral.sh/uv/{tag}/install.sh")
        };
        let bin = &srcs[0][1..];
        let quoted = format!("'{}'", dest.replace('\'', "'\\''"));
        let script = format!(
            "set -e; T=$(mktemp -d); curl -LsSf {url} | env UV_UNMANAGED_INSTALL=\"$T\" sh >/dev/null; \
             D={quoted}; case \"$D\" in */) mkdir -p \"$D\"; D=\"$D{bin}\";; *) mkdir -p \"$(dirname \"$D\")\";; esac; \
             cp -f \"$T/{bin}\" \"$D\"; chmod 0755 \"$D\"; rm -rf \"$T\"",
        );
        sp.run(
            &["/bin/sh".into(), "-c".into(), script],
            &BTreeMap::new(),
            "/",
            "root",
            log,
        )
        .await
    }

    /// `COPY` / `ADD` of context files and mount files into the build Space.
    #[allow(clippy::too_many_arguments)]
    async fn copy_into(
        &self,
        sp: &BuildSpace,
        spec: &Spec,
        c: &dockerfile::Copy,
        envs: &BTreeMap<String, String>,
        args: &BTreeMap<String, String>,
        workdir: &str,
        log: &Arc<BuildLog>,
    ) -> Result<(), String> {
        if let Some(from) = &c.from {
            return self.copy_from_uv(sp, from, c, envs, args, log).await;
        }
        // `add_python`: `COPY /python/. /usr/local` from Modal's global
        // python-build-standalone mount, which holds no files here. The same
        // Python version is installed with uv (into /usr/local/bin).
        if let Some(v) = self
            .db
            .read(|d| d.mounts.get(&spec.context_mount_id).cloned())
            .and_then(|m| m.name)
            .and_then(|n| standalone_python_version(&n))
            && c.sources.iter().all(|s| {
                s.trim_end_matches('/') == "/python/." || s == "/python/" || s == "/python"
            })
        {
            log.push(format!("Installing Python {v} (python-build-standalone)"));
            return sp
                .run(
                    &["/bin/sh".into(), "-c".into(), python_script(&v)],
                    &BTreeMap::new(),
                    "/",
                    "root",
                    log,
                )
                .await;
        }
        let sc = scope(envs, args);
        let dest = dockerfile::expand(&c.dest, &sc);
        let dest_abs = if dest.starts_with('/') {
            dest.clone()
        } else {
            format!("{}/{}", workdir.trim_end_matches('/'), dest)
        };
        // The files available: the layer's context files, then the mount's.
        let mut files: Vec<(String, String, Option<u32>)> = spec
            .context_files
            .iter()
            .map(|f| {
                (
                    f.filename.trim_start_matches('/').to_string(),
                    f.sha256.clone(),
                    None,
                )
            })
            .collect();
        if let Some(m) = self
            .db
            .read(|d| d.mounts.get(&spec.context_mount_id).cloned())
        {
            files.extend(m.files.iter().map(|f| {
                (
                    f.filename.trim_start_matches('/').to_string(),
                    f.sha256.clone(),
                    f.mode,
                )
            }));
        }
        let mut copied = 0usize;
        let multi = c.sources.len() > 1;
        for src in &c.sources {
            let src = dockerfile::expand(src, &sc);
            let pat = src
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();
            let pat = if pat.is_empty() || pat == "." {
                String::new()
            } else {
                pat
            };
            let glob = globset::Glob::new(&pat)
                .map_err(|e| format!("bad COPY source {src:?}: {e}"))?
                .compile_matcher();
            let mut hits: Vec<&(String, String, Option<u32>)> = Vec::new();
            for f in &files {
                let under = pat.is_empty()
                    || f.0 == pat
                    || f.0.starts_with(&format!("{}/", pat.trim_end_matches('/')));
                if under || glob.is_match(&f.0) {
                    hits.push(f);
                }
            }
            if hits.is_empty() {
                return Err(format!(
                    "COPY failed: no source files were specified or found for {src:?}"
                ));
            }
            // A directory (or a glob matching several files) copies its
            // contents into `dest`; a single file lands at `dest`.
            let single_file = hits.len() == 1 && hits[0].0 == pat;
            for (name, sha, mode) in hits {
                let target = if single_file && !dest.ends_with('/') && !multi {
                    dest_abs.clone()
                } else if single_file {
                    format!(
                        "{}/{}",
                        dest_abs.trim_end_matches('/'),
                        name.rsplit('/').next().unwrap_or(name)
                    )
                } else {
                    let rel = if pat.is_empty() {
                        name.as_str()
                    } else {
                        name.strip_prefix(pat.trim_end_matches('/'))
                            .map(|r| r.trim_start_matches('/'))
                            .unwrap_or(name.as_str())
                    };
                    format!("{}/{}", dest_abs.trim_end_matches('/'), rel)
                };
                let data = self.blobs.get_sha(sha).ok_or_else(|| {
                    format!("context file {name} is missing from the gateway's store")
                })?;
                let perms = mode.unwrap_or(0o644) & 0o7777;
                sp.client
                    .upload(
                        &target,
                        data,
                        cua_spacesd_client::UploadOptions {
                            create_parents: true,
                            permissions: perms,
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|e| format!("COPY to {target} failed: {e}"))?;
                copied += 1;
            }
        }
        if let Some(own) = &c.chown {
            let own = dockerfile::expand(own, &sc);
            let _ = sp
                .run(
                    &[
                        "/bin/sh".into(),
                        "-c".into(),
                        format!("chown -R {own} '{}'", dest_abs.replace('\'', "'\\''")),
                    ],
                    &BTreeMap::new(),
                    "/",
                    "root",
                    log,
                )
                .await;
        }
        log.push(format!("Copied {copied} file(s)"));
        let _ = now_s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn standalone_mount_names() {
        assert_eq!(
            standalone_python_version("python-build-standalone.20230826.3.11.5-gnu").as_deref(),
            Some("3.11.5")
        );
        assert_eq!(
            standalone_python_version("python-build-standalone.20241008.3.13.0t-gnu").as_deref(),
            Some("3.13.0t")
        );
        assert_eq!(standalone_python_version("modal-client-mount-1.6.2"), None);
    }

    #[test]
    fn maps_bases() {
        let m = HashMap::from([("nvcr.io/x:1".to_string(), "my-image".to_string())]);
        assert_eq!(
            classify_base("python:3.12-slim-bookworm", &m),
            Ok(Base::Python("3.12".into()))
        );
        assert_eq!(
            classify_base("python:3.11.9", &m),
            Ok(Base::Python("3.11.9".into()))
        );
        assert_eq!(
            classify_base("python:slim", &m),
            Ok(Base::Python("3".into()))
        );
        assert_eq!(
            classify_base("docker.io/library/python:3.10", &m),
            Ok(Base::Python("3.10".into()))
        );
        assert_eq!(classify_base("ubuntu:22.04", &m), Ok(Base::Distro));
        assert_eq!(classify_base("debian:bookworm", &m), Ok(Base::Distro));
        assert_eq!(
            classify_base("ghcr.io/trycua/linux:24.04", &m),
            Ok(Base::Cua("ghcr.io/trycua/linux:24.04".into()))
        );
        assert_eq!(
            classify_base("nvcr.io/x:1", &m),
            Ok(Base::Cua("my-image".into()))
        );
        assert_eq!(classify_base("scratch", &m), Ok(Base::Scratch));
        let e = classify_base("nvcr.io/nvidia/pytorch:22.12-py3", &m).unwrap_err();
        assert!(e.contains("cua-spacesd"), "{e}");
    }

    #[test]
    fn python_versions_parse() {
        assert_eq!(python_version("3.12.7-slim-bookworm"), "3.12.7");
        assert_eq!(python_version("3.13t"), "3.13");
        assert_eq!(python_version("latest"), "3");
        assert!(python_script("3.12").contains("uv python install"));
    }

    /// pip's console scripts (`jupyter`, `uvicorn`) must land on PATH, as in a
    /// python:* image: the interpreter tree is installed under /usr/local.
    #[test]
    fn the_python_prelude_installs_the_interpreter_under_usr_local() {
        let s = python_script("3.12.10");
        assert!(s.contains("cp -a \"$ROOT\"/. /usr/local/"));
        assert!(s.contains("B=/usr/local/bin"));
        assert!(s.contains("python3.12"));
    }

    #[test]
    fn hashes_depend_on_everything_that_changes_the_image() {
        let s = Spec {
            commands: vec!["FROM base".into(), "RUN a".into()],
            ..Default::default()
        };
        let a = spec_hash(&s, &["p1".into()], &[]);
        assert_eq!(a, spec_hash(&s, &["p1".into()], &[]));
        assert_ne!(a, spec_hash(&s, &["p2".into()], &[]));
        let mut t = s.clone();
        t.commands.push("RUN b".into());
        assert_ne!(a, spec_hash(&t, &["p1".into()], &[]));
        t = s.clone();
        t.build_args.insert("X".into(), "1".into());
        assert_ne!(a, spec_hash(&t, &["p1".into()], &[]));
        assert_ne!(a, spec_hash(&s, &["p1".into()], &["secret:x".into()]));
    }
}
