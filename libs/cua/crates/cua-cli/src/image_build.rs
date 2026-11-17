//! `cua images build|publish|promote`: build a libs/images sandbox image
//! definition and publish it (the CLI form of `libs/images/build.sh`, used by
//! the image CD workflows).
//!
//! An image definition is a directory with a `Dockerfile` (the guest tree) and
//! usually an `image.json` (repository, platforms, outputs, services, claims),
//! inside a tree that has `common/` (the VM layer and the disk builder), the
//! build context. From one definition:
//!
//! | Output | Result | How |
//! |---|---|---|
//! | `rootfs` | `<repo>:docker-<tag>-<arch>` | `docker buildx build` |
//! | `disk` | `<out>/<name>/<arch>/disk.img` | VM layer (`vm.Dockerfile` or `common/vm/Dockerfile`) exported as a tar into `common/disk-builder` |
//! | `containerdisk` | `<repo>:<tag>-<arch>` | [`cua_image::containerdisk::pack`], pushed with the cua-image client |
//!
//! The rootfs is pushed only when `image.json` lists a `rootfs` output (a
//! VM-only image such as omarchy builds it as an intermediate).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cua_image::publish::{self, PublishRecord, PublishSpec};
use cua_image::{RegistryClient, containerdisk};
use cua_sdk::CuaError;
use serde_json::{Value, json};

fn bad(msg: impl Into<String>) -> CuaError {
    CuaError::InvalidArgument(msg.into())
}

fn failed(msg: impl Into<String>) -> CuaError {
    CuaError::Internal(msg.into())
}

/// Options of `cua images build`.
#[derive(Debug, Clone)]
pub struct BuildOpts {
    pub dir: PathBuf,
    pub platforms: Option<String>,
    pub tag: String,
    pub repo: Option<String>,
    pub outputs: String,
    pub build_args: Vec<String>,
    pub build_contexts: Vec<String>,
    pub labels: Vec<String>,
    pub out: Option<PathBuf>,
    pub disk_size: String,
    /// Dockerfile stage to build (`--target`, e.g. a tier: `slim`, `full`);
    /// `None` builds the Dockerfile's last stage. Disks of a target go to
    /// `<out>/<name>-<target>/<arch>` so tiers never overwrite each other.
    pub target: Option<String>,
    pub push: bool,
    pub dry_run: bool,
}

/// A loaded image definition.
#[derive(Debug, Clone)]
pub struct Definition {
    /// Directory of the definition.
    pub dir: PathBuf,
    /// The tree holding `common/` (build context).
    pub root: PathBuf,
    pub name: String,
    pub json: Value,
}

impl Definition {
    pub fn load(dir: &Path) -> Result<Self, CuaError> {
        let renamed;
        let dir = match legacy_dir(dir) {
            Some(new) => {
                eprintln!(
                    "{} was renamed to {}; building that",
                    dir.display(),
                    new.display()
                );
                renamed = new;
                renamed.as_path()
            }
            None => dir,
        };
        let dir = dir
            .canonicalize()
            .map_err(|e| bad(format!("{}: {e}", dir.display())))?;
        if !dir.join("Dockerfile").is_file() {
            return Err(bad(format!("{} has no Dockerfile", dir.display())));
        }
        let root = dir
            .ancestors()
            .skip(1)
            .find(|a| a.join("common/disk-builder/Dockerfile").is_file())
            .ok_or_else(|| {
                bad(format!(
                    "{} is not inside an images tree (no common/disk-builder above it)",
                    dir.display()
                ))
            })?
            .to_path_buf();
        let json = match std::fs::read_to_string(dir.join("image.json")) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| bad(format!("image.json: {e}")))?,
            Err(_) => Value::Null,
        };
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_string();
        Ok(Self {
            dir,
            root,
            name,
            json,
        })
    }

    /// `image.json` `repository`, else `cua-e2e-local/<name>`.
    pub fn repo(&self, over: Option<&str>) -> String {
        over.map(str::to_string)
            .or_else(|| self.json["repository"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("cua-e2e-local/{}", self.name))
    }

    /// `linux/<arch>` platforms the definition supports.
    pub fn platforms(&self) -> Vec<String> {
        self.json["platforms"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the image runs cua-spacesd (a cua-spacesd service in image.json).
    pub fn spacesd(&self) -> bool {
        self.json["services"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["component"] == "cua-spacesd"))
    }

    /// Outputs the definition publishes (`rootfs`, `containerdisk`).
    pub fn publishes(&self, output: &str) -> bool {
        match self.json["outputs"].as_object() {
            Some(o) => o.contains_key(output),
            None => true,
        }
    }

    /// The directory name disks go under: `<name>`, or `<name>-<target>`.
    pub fn out_name(&self, target: Option<&str>) -> String {
        match target {
            Some(t) => format!("{}-{t}", self.name),
            None => self.name.clone(),
        }
    }

    pub fn vm_dockerfile(&self) -> PathBuf {
        let own = self.json["vm_dockerfile"]
            .as_str()
            .map(|f| self.dir.join(f))
            .unwrap_or_else(|| self.dir.join("vm.Dockerfile"));
        if own.is_file() {
            own
        } else {
            self.root.join("common/vm/Dockerfile")
        }
    }
}

/// Image directories renamed to the `<os>` convention; the old path is
/// accepted for one release.
const RENAMED_DIRS: &[(&str, &str)] = &[("cua-desktop-linux", "linux")];

/// The new directory for a pre-rename `dir` that no longer exists.
fn legacy_dir(dir: &Path) -> Option<PathBuf> {
    if dir.exists() {
        return None;
    }
    let name = dir.file_name()?.to_str()?;
    let (_, new) = RENAMED_DIRS.iter().find(|(old, _)| *old == name)?;
    let new = dir.with_file_name(new);
    new.is_dir().then_some(new)
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

/// One step of a build plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Run(Vec<String>),
    /// `a | b`.
    Pipe(Vec<String>, Vec<String>),
    /// Compare the rootfs labels with its /etc/cua-image/manifest.json.
    CheckLabels(String),
    /// Pack a disk as a containerDisk, then push it or load it into docker.
    Pack {
        disk: PathBuf,
        arch: String,
        reference: String,
        push: bool,
    },
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The build as a list of steps (what `--dry-run` prints).
pub fn plan(def: &Definition, o: &BuildOpts) -> Result<Vec<Step>, CuaError> {
    let outputs: Vec<&str> = o
        .outputs
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect();
    for out in &outputs {
        if !["rootfs", "disk", "containerdisk"].contains(out) {
            return Err(bad(format!(
                "unknown output {out}; expected rootfs, disk, containerdisk"
            )));
        }
    }
    let want =
        |x: &str| outputs.contains(&x) || (x == "disk" && outputs.contains(&"containerdisk"));
    let supported = def.platforms();
    let platforms: Vec<String> = match &o.platforms {
        Some(p) => p.split(',').map(|x| x.trim().to_string()).collect(),
        None => {
            let host = format!("linux/{}", host_arch());
            if supported.is_empty() || supported.contains(&host) {
                vec![host]
            } else {
                vec![supported[0].clone()]
            }
        }
    };
    for p in &platforms {
        if !p.starts_with("linux/") {
            return Err(bad(format!("platform {p}: expected linux/<arch>")));
        }
        if !supported.is_empty() && !supported.contains(p) {
            return Err(bad(format!(
                "{} does not support {p} (image.json platforms: {})",
                def.name,
                supported.join(", ")
            )));
        }
    }
    if let Some(t) = o.target.as_deref()
        && (t.is_empty()
            || !t
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'))
    {
        return Err(bad(format!(
            "target {t:?}: expected a Dockerfile stage name"
        )));
    }
    let repo = def.repo(o.repo.as_deref());
    let ctx = def.root.display().to_string();
    let out_dir = o
        .out
        .clone()
        .or_else(|| std::env::var_os("CUA_IMAGES_OUT").map(PathBuf::from))
        .unwrap_or_else(|| dirs_home().join(".cache/cua-images"));

    let spacesd = def.spacesd() && !o.build_args.iter().any(|a| a == "CUA_SPACESD_SOURCE=none");
    let mut build_args: Vec<String> = Vec::new();
    for a in &o.build_args {
        build_args.extend(["--build-arg".into(), a.clone()]);
    }
    for c in &o.build_contexts {
        build_args.extend(["--build-context".into(), c.clone()]);
    }
    if !def.json.is_null() {
        let rev = git_head(&def.root).unwrap_or_default();
        build_args.extend([
            "--build-arg".into(),
            format!("CUA_IMAGE_SOURCE_REVISION={rev}"),
        ]);
    }
    let mut labels: Vec<String> = Vec::new();
    for l in [
        "org.opencontainers.image.source=https://github.com/trycua/cua".to_string(),
        "ai.cua.image.variant=rootfs".into(),
        "ai.cua.image.os=linux".into(),
        format!("ai.cua.spacesd={spacesd}"),
        format!("ai.cua.env-driver={spacesd}"),
    ]
    .into_iter()
    .chain(o.labels.iter().cloned())
    {
        labels.extend(["--label".into(), l]);
    }

    let mut steps = Vec::new();
    for platform in &platforms {
        let arch = platform.trim_start_matches("linux/").to_string();
        let rootfs_ref = format!("{repo}:docker-{}-{arch}", o.tag);
        let mut build = s(&[
            "docker",
            "buildx",
            "build",
            "--platform",
            platform,
            "--load",
            "-f",
        ]);
        build.push(def.dir.join("Dockerfile").display().to_string());
        if let Some(t) = &o.target {
            build.extend(["--target".into(), t.clone()]);
        }
        build.extend(build_args.iter().cloned());
        build.extend(labels.iter().cloned());
        build.extend(["-t".into(), rootfs_ref.clone(), ctx.clone()]);
        steps.push(Step::Run(build));
        steps.push(Step::CheckLabels(rootfs_ref.clone()));
        if o.push && want("rootfs") && def.publishes("rootfs") {
            steps.push(Step::Run(s(&["docker", "push", &rootfs_ref])));
        }
        if want("disk") {
            let disk_dir = out_dir.join(def.out_name(o.target.as_deref())).join(&arch);
            let builder = format!("cua-e2e-local/disk-builder:{arch}");
            steps.push(Step::Run(vec![
                "docker".into(),
                "buildx".into(),
                "build".into(),
                "--platform".into(),
                platform.clone(),
                "--load".into(),
                "-t".into(),
                builder.clone(),
                def.root.join("common/disk-builder").display().to_string(),
            ]));
            let mut vm = s(&["docker", "buildx", "build", "--platform", platform, "-f"]);
            vm.push(def.vm_dockerfile().display().to_string());
            vm.extend(["--build-arg".into(), format!("BASE_IMAGE={rootfs_ref}")]);
            vm.extend(["--output".into(), "type=tar,dest=-".into(), ctx.clone()]);
            let make = vec![
                "docker".into(),
                "run".into(),
                "--rm".into(),
                "-i".into(),
                "--platform".into(),
                platform.clone(),
                "--memory=4g".into(),
                "--memory-swap=4g".into(),
                "-e".into(),
                format!("DISK_SIZE={}", o.disk_size),
                "-e".into(),
                format!(
                    "QCOW2_COMPRESS={}",
                    std::env::var("QCOW2_COMPRESS").unwrap_or_else(|_| "1".into())
                ),
                "-v".into(),
                format!("{}:/out", disk_dir.display()),
                builder,
                "-".into(),
                "/out/disk.img".into(),
            ];
            steps.push(Step::Pipe(vm, make));
            if want("containerdisk") {
                steps.push(Step::Pack {
                    disk: disk_dir.join("disk.img"),
                    arch: arch.clone(),
                    reference: format!("{repo}:{}-{arch}", o.tag),
                    push: o.push && def.publishes("containerdisk"),
                });
            }
        }
    }
    Ok(steps)
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn git_head(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn show(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.contains(' ') || a.is_empty() {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn run(argv: &[String]) -> Result<(), CuaError> {
    eprintln!("+ {}", show(argv));
    let st = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|e| failed(format!("{}: {e}", argv[0])))?;
    if st.success() {
        Ok(())
    } else {
        Err(failed(format!("{} failed ({st})", show(argv))))
    }
}

fn pipe(a: &[String], b: &[String]) -> Result<(), CuaError> {
    eprintln!("+ {} | {}", show(a), show(b));
    let mut first = Command::new(&a[0])
        .args(&a[1..])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| failed(format!("{}: {e}", a[0])))?;
    let stdout = first.stdout.take().expect("piped stdout");
    let second = Command::new(&b[0])
        .args(&b[1..])
        .stdin(Stdio::from(stdout))
        .status();
    let first = first.wait();
    let second = second.map_err(|e| failed(format!("{}: {e}", b[0])))?;
    let first = first.map_err(|e| failed(format!("{}: {e}", a[0])))?;
    if !first.success() {
        return Err(failed(format!("{} failed ({first})", show(a))));
    }
    if !second.success() {
        return Err(failed(format!("{} failed ({second})", show(b))));
    }
    Ok(())
}

fn check_labels(def: &Definition, reference: &str) -> Result<(), CuaError> {
    let manifest = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "cat",
            reference,
            "/etc/cua-image/manifest.json",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| failed(format!("docker: {e}")))?;
    if !manifest.status.success() {
        return Ok(()); // no manifest: nothing to compare
    }
    let labels = Command::new("docker")
        .args(["inspect", "-f", "{{json .Config.Labels}}", reference])
        .output()
        .map_err(|e| failed(format!("docker: {e}")))?;
    let tmp = std::env::temp_dir().join(format!("cua-image-manifest-{}.json", std::process::id()));
    std::fs::write(&tmp, &manifest.stdout).map_err(|e| failed(e.to_string()))?;
    let r = run(&[
        "python3".into(),
        def.root
            .join("common/tools/cua-image-manifest")
            .display()
            .to_string(),
        "check-labels".into(),
        "--manifest".into(),
        tmp.display().to_string(),
        "--labels".into(),
        String::from_utf8_lossy(&labels.stdout).trim().to_string(),
    ]);
    let _ = std::fs::remove_file(&tmp);
    r
}

async fn pack(disk: &Path, arch: &str, reference: &str, push: bool) -> Result<Value, CuaError> {
    let work = disk
        .parent()
        .unwrap_or(Path::new("."))
        .join("containerdisk");
    let _ = std::fs::remove_dir_all(&work);
    let t0 = std::time::Instant::now();
    let img = containerdisk::pack(disk, arch, &work.join("blobs"))
        .map_err(|e| failed(format!("pack {}: {e}", disk.display())))?;
    let mut rec = json!({"reference": reference, "arch": arch, "disk": disk.display().to_string()});
    if push {
        let d = img
            .push(&RegistryClient::new(vec![]), reference)
            .await
            .map_err(|e| failed(format!("push {reference}: {e}")))?;
        eprintln!(
            "pushed {reference}@{} ({:.1}s)",
            d.digest,
            t0.elapsed().as_secs_f32()
        );
        rec["digest"] = json!(d.digest);
    } else {
        // A local tag: the OCI layout loaded into docker.
        let layout = work.join("layout");
        img.write_oci_layout(&layout)
            .map_err(|e| failed(format!("oci layout: {e}")))?;
        let tar = Command::new("tar")
            .arg("-C")
            .arg(&layout)
            .args(["-cf", "-", "."])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| failed(format!("tar: {e}")))?;
        let out = Command::new("docker")
            .args(["load", "-q"])
            .stdin(Stdio::from(tar.stdout.expect("piped")))
            .output()
            .map_err(|e| failed(format!("docker load: {e}")))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let id = text
            .lines()
            .find_map(|l| {
                l.strip_prefix("Loaded image ID: ")
                    .or_else(|| l.strip_prefix("Loaded image: "))
            })
            .ok_or_else(|| failed(format!("docker load: {text}")))?
            .trim()
            .to_string();
        run(&["docker".into(), "tag".into(), id, reference.into()])?;
        eprintln!("loaded {reference} ({:.1}s)", t0.elapsed().as_secs_f32());
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(rec)
}

/// `cua images build <DIR>`.
pub async fn build(o: &BuildOpts) -> Result<Value, CuaError> {
    let def = Definition::load(&o.dir)?;
    let steps = plan(&def, o)?;
    if o.dry_run {
        let lines: Vec<Value> = steps.iter().map(step_text).map(Value::String).collect();
        return Ok(json!({"image": def.name, "dry_run": true, "steps": lines}));
    }
    let mut packed = Vec::new();
    for step in &steps {
        match step {
            Step::Run(argv) => run(argv)?,
            Step::Pipe(a, b) => {
                if let Some(dir) = b.iter().find_map(|x| x.strip_suffix(":/out")) {
                    std::fs::create_dir_all(dir).map_err(|e| failed(format!("{dir}: {e}")))?;
                }
                pipe(a, b)?
            }
            Step::CheckLabels(r) => check_labels(&def, r)?,
            Step::Pack {
                disk,
                arch,
                reference,
                push,
            } => packed.push(pack(disk, arch, reference, *push).await?),
        }
    }
    Ok(
        json!({"image": def.name, "repo": def.repo(o.repo.as_deref()), "tag": o.tag, "containerdisks": packed}),
    )
}

fn step_text(step: &Step) -> String {
    match step {
        Step::Run(a) => show(a),
        Step::Pipe(a, b) => format!("{} | {}", show(a), show(b)),
        Step::CheckLabels(r) => format!("check-labels {r}"),
        Step::Pack {
            disk,
            arch,
            reference,
            push,
        } => format!(
            "pack {} ({arch}) -> {reference}{}",
            disk.display(),
            if *push { " (push)" } else { " (docker load)" }
        ),
    }
}

/// `cua images pack <DIR>`: pack disks an earlier `cua images build
/// --outputs disk` wrote (the ones the doctor checked) as containerDisks
/// `<repo>:<tag>-<arch>`, and push or load them.
pub async fn pack_cmd(o: &BuildOpts) -> Result<Value, CuaError> {
    let def = Definition::load(&o.dir)?;
    let plan_opts = BuildOpts {
        outputs: "containerdisk".into(),
        ..o.clone()
    };
    let mut packed = Vec::new();
    for step in plan(&def, &plan_opts)? {
        if let Step::Pack {
            disk,
            arch,
            reference,
            push,
        } = step
        {
            if o.dry_run {
                packed.push(json!({"reference": reference, "arch": arch, "disk": disk.display().to_string(), "push": push}));
                continue;
            }
            if !disk.is_file() {
                return Err(bad(format!(
                    "{} does not exist; run `cua images build {} --outputs disk` first",
                    disk.display(),
                    o.dir.display()
                )));
            }
            if o.push && !push {
                return Err(bad(format!(
                    "{} does not publish a containerdisk output",
                    def.name
                )));
            }
            packed.push(pack(&disk, &arch, &reference, push).await?);
        }
    }
    Ok(
        json!({"image": def.name, "repo": def.repo(o.repo.as_deref()), "tag": o.tag, "containerdisks": packed}),
    )
}

/// Options of `cua images publish`.
#[derive(Debug, Clone)]
pub struct PublishOpts {
    pub dir: PathBuf,
    pub tag: String,
    pub series: String,
    pub stamp: Option<String>,
    pub repo: Option<String>,
    pub platforms: Option<String>,
    pub annotations: Vec<String>,
    /// `VARIANT/ARCH:KEY=VALUE` annotations on one child descriptor.
    pub descriptor_annotations: Vec<String>,
    pub dry_run: bool,
}

/// Annotations per child descriptor, keyed by (variant, arch).
type DescriptorAnnotations = BTreeMap<(String, String), BTreeMap<String, String>>;

/// Parse `VARIANT/ARCH:KEY=VALUE` into the publish spec's map.
fn descriptor_annotations(raw: &[String]) -> Result<DescriptorAnnotations, CuaError> {
    let mut out = DescriptorAnnotations::new();
    for a in raw {
        let err = || {
            bad(format!(
                "descriptor annotation {a}: expected VARIANT/ARCH:KEY=VALUE"
            ))
        };
        let (target, kv) = a.split_once(':').ok_or_else(err)?;
        let (variant, arch) = target.split_once('/').ok_or_else(err)?;
        let (k, v) = kv.split_once('=').ok_or_else(err)?;
        if !["rootfs", "containerdisk"].contains(&variant) || k.is_empty() {
            return Err(err());
        }
        out.entry((variant.into(), arch.into()))
            .or_default()
            .insert(k.into(), v.into());
    }
    Ok(out)
}

fn default_stamp(root: &Path) -> String {
    let sha = git_head(root).unwrap_or_default();
    let date = chrono::Utc::now().format("%Y%m%d");
    format!("{date}-{}", &sha[..sha.len().min(7)])
}

/// The publish spec for a definition's per-arch build tag.
pub fn publish_spec(def: &Definition, o: &PublishOpts) -> Result<PublishSpec, CuaError> {
    let repo = def.repo(o.repo.as_deref());
    let platforms: Vec<String> = match &o.platforms {
        Some(p) => p.split(',').map(|x| x.trim().to_string()).collect(),
        None => def.platforms(),
    };
    if platforms.is_empty() {
        return Err(bad(
            "no platforms: pass --platform or list them in image.json",
        ));
    }
    let arches: Vec<String> = platforms
        .iter()
        .map(|p| p.trim_start_matches("linux/").to_string())
        .collect();
    let mut annotations = BTreeMap::new();
    annotations.insert(
        "org.opencontainers.image.source".into(),
        "https://github.com/trycua/cua".into(),
    );
    if let Some(rev) = git_head(&def.root) {
        annotations.insert("org.opencontainers.image.revision".into(), rev);
    }
    annotations.insert("org.opencontainers.image.version".into(), o.series.clone());
    for a in &o.annotations {
        let (k, v) = a
            .split_once('=')
            .ok_or_else(|| bad(format!("annotation {a}: expected KEY=VALUE")))?;
        annotations.insert(k.into(), v.into());
    }
    Ok(PublishSpec {
        repo: repo.clone(),
        series: o.series.clone(),
        stamp: o.stamp.clone().unwrap_or_else(|| default_stamp(&def.root)),
        containerdisks: arches
            .iter()
            .map(|a| (a.clone(), format!("{repo}:{}-{a}", o.tag)))
            .collect(),
        rootfs: if def.publishes("rootfs") {
            arches
                .iter()
                .map(|a| (a.clone(), format!("{repo}:docker-{}-{a}", o.tag)))
                .collect()
        } else {
            vec![]
        },
        os: "linux".into(),
        spacesd: def.spacesd(),
        annotations,
        descriptor_annotations: descriptor_annotations(&o.descriptor_annotations)?,
    })
}

/// `cua images publish <DIR>`: immutable pins only.
pub async fn publish_cmd(o: &PublishOpts) -> Result<PublishRecord, CuaError> {
    let def = Definition::load(&o.dir)?;
    let spec = publish_spec(&def, o)?;
    publish::publish(&RegistryClient::new(vec![]), &spec, o.dry_run)
        .await
        .map_err(|e| failed(format!("publish: {e}")))
}

/// `cua images promote <RECORD>`: move the moving tags to the pins.
pub async fn promote_cmd(record: &Path, dry_run: bool) -> Result<Vec<String>, CuaError> {
    let text =
        std::fs::read_to_string(record).map_err(|e| bad(format!("{}: {e}", record.display())))?;
    let rec: PublishRecord =
        serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", record.display())))?;
    publish::promote(&RegistryClient::new(vec![]), &rec, dry_run)
        .await
        .map_err(|e| failed(format!("promote: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn images_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../images")
    }

    /// A step's text with host path separators as `/` (Windows joins with
    /// `\\`; the rendered commands are otherwise the same).
    fn text_of(step: &Step) -> String {
        step_text(step).replace('\\', "/")
    }

    fn opts(dir: &str) -> BuildOpts {
        BuildOpts {
            dir: images_dir().join(dir),
            platforms: None,
            tag: "t1".into(),
            repo: None,
            outputs: "rootfs,disk,containerdisk".into(),
            build_args: vec![],
            build_contexts: vec![],
            labels: vec![],
            out: Some("/tmp/out".into()),
            disk_size: "20G".into(),
            target: None,
            push: true,
            dry_run: true,
        }
    }

    #[test]
    fn omarchy_is_a_vm_only_amd64_image_with_its_own_vm_layer() {
        let def = Definition::load(&images_dir().join("omarchy")).unwrap();
        assert_eq!(def.repo(None), "ghcr.io/trycua/omarchy");
        assert!(def.spacesd());
        assert!(!def.publishes("rootfs"));
        assert!(def.publishes("containerdisk"));
        assert!(def.vm_dockerfile().ends_with("omarchy/vm.Dockerfile"));
        let steps = plan(&def, &opts("omarchy")).unwrap();
        let text: Vec<String> = steps.iter().map(text_of).collect();
        // Defaults to the only platform it supports, even on an arm64 host.
        assert!(text[0].contains("--platform linux/amd64"), "{}", text[0]);
        assert!(text[0].contains("ai.cua.spacesd=true"));
        assert!(text[0].contains(&format!("-t {}:docker-t1-amd64", def.repo(None))));
        // The intermediate rootfs is never pushed.
        assert!(
            !text.iter().any(|t| t.starts_with("docker push")),
            "{text:#?}"
        );
        assert!(
            text.iter()
                .any(|t| t.contains("omarchy/vm.Dockerfile") && t.contains("| docker run"))
        );
        assert!(
            text.last()
                .unwrap()
                .contains(&format!("-> {}:t1-amd64 (push)", def.repo(None)))
        );
        let mut o = opts("omarchy");
        o.platforms = Some("linux/arm64".into());
        assert!(plan(&def, &o).is_err());
    }

    #[test]
    fn linux_matches_build_sh() {
        let def = Definition::load(&images_dir().join("linux")).unwrap();
        assert_eq!(def.name, "linux");
        assert_eq!(def.repo(None), "ghcr.io/trycua/linux");
        assert!(def.publishes("rootfs") && def.publishes("containerdisk"));
        let mut o = opts("linux");
        o.platforms = Some("linux/arm64".into());
        o.repo = Some("cua-e2e-local/desktop".into());
        o.build_args = vec!["CUA_SPACESD_SOURCE=none".into()];
        let text: Vec<String> = plan(&def, &o).unwrap().iter().map(text_of).collect();
        assert!(text[0].contains("ai.cua.spacesd=false"), "{}", text[0]);
        assert!(text[0].contains("CUA_IMAGE_SOURCE_REVISION="));
        assert!(
            text.iter()
                .any(|t| t == "docker push cua-e2e-local/desktop:docker-t1-arm64")
        );
        assert!(text.iter().any(|t| t.contains("common/vm/Dockerfile")));
    }

    #[test]
    fn target_selects_a_stage_and_its_own_disk_dir() {
        let def = Definition::load(&images_dir().join("omarchy")).unwrap();
        let mut o = opts("omarchy");
        o.target = Some("slim".into());
        let text: Vec<String> = plan(&def, &o).unwrap().iter().map(text_of).collect();
        assert!(text[0].contains("--target slim"), "{}", text[0]);
        assert!(
            text.iter()
                .any(|t| t.contains("/tmp/out/omarchy-slim/amd64:/out")),
            "{text:#?}"
        );
        assert!(
            text.last()
                .unwrap()
                .contains("/tmp/out/omarchy-slim/amd64/disk.img")
        );
        let untargeted: Vec<String> = plan(&def, &opts("omarchy"))
            .unwrap()
            .iter()
            .map(text_of)
            .collect();
        assert!(!untargeted[0].contains("--target"));
        assert!(
            untargeted
                .iter()
                .any(|t| t.contains("/tmp/out/omarchy/amd64:/out"))
        );
        for bad in ["", "a b", "x;rm"] {
            o.target = Some(bad.into());
            assert!(plan(&def, &o).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn publish_spec_for_a_vm_only_image() {
        let def = Definition::load(&images_dir().join("omarchy")).unwrap();
        let spec = publish_spec(
            &def,
            &PublishOpts {
                dir: def.dir.clone(),
                tag: "build-abc".into(),
                series: "edge".into(),
                stamp: Some("20260925-abcdef1".into()),
                repo: None,
                platforms: None,
                annotations: vec!["ai.cua.doctor.status=pass".into()],
                descriptor_annotations: vec![
                    "containerdisk/amd64:ai.cua.doctor.status=pass".into(),
                ],
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(
            spec.containerdisks,
            vec![(
                "amd64".to_string(),
                format!("{}:build-abc-amd64", def.repo(None))
            )]
        );
        assert!(spec.rootfs.is_empty());
        assert!(spec.spacesd);
        assert_eq!(spec.annotations["ai.cua.doctor.status"], "pass");
        assert_eq!(
            spec.descriptor_annotations[&("containerdisk".to_string(), "amd64".to_string())]["ai.cua.doctor.status"],
            "pass"
        );
        assert!(descriptor_annotations(&["rootfs:k=v".into()]).is_err());
        assert!(descriptor_annotations(&["iso/amd64:k=v".into()]).is_err());
    }

    #[test]
    fn the_pre_rename_linux_dir_still_loads() {
        let def = Definition::load(&images_dir().join("cua-desktop-linux")).unwrap();
        assert_eq!(def.name, "linux");
        assert!(legacy_dir(&images_dir().join("nope")).is_none());
    }

    #[test]
    fn publish_spec_for_linux_links_rootfs_and_disk() {
        let def = Definition::load(&images_dir().join("linux")).unwrap();
        let spec = publish_spec(
            &def,
            &PublishOpts {
                dir: def.dir.clone(),
                tag: "ci-abc".into(),
                series: "24.04".into(),
                stamp: Some("20260925-abcdef1".into()),
                repo: None,
                platforms: None,
                annotations: vec![],
                descriptor_annotations: vec![],
                dry_run: true,
            },
        )
        .unwrap();
        let repo = "ghcr.io/trycua/linux";
        assert_eq!(spec.repo, repo);
        assert_eq!(
            spec.rootfs,
            vec![
                ("amd64".to_string(), format!("{repo}:docker-ci-abc-amd64")),
                ("arm64".to_string(), format!("{repo}:docker-ci-abc-arm64")),
            ]
        );
        assert_eq!(
            spec.containerdisks,
            vec![
                ("amd64".to_string(), format!("{repo}:ci-abc-amd64")),
                ("arm64".to_string(), format!("{repo}:ci-abc-arm64")),
            ]
        );
        let tags = cua_image::publish::series_tags(&spec.series, &spec.stamp);
        assert_eq!(tags.primary_pin, "24.04-20260925-abcdef1");
        assert_eq!(tags.containerdisk_pin, "24.04-disk-20260925-abcdef1");
    }

    #[test]
    fn rejects_non_definitions() {
        assert!(Definition::load(&images_dir().join("common")).is_err());
        assert!(
            plan(
                &Definition::load(&images_dir().join("omarchy")).unwrap(),
                &BuildOpts {
                    outputs: "iso".into(),
                    ..opts("omarchy")
                }
            )
            .is_err()
        );
    }
}
