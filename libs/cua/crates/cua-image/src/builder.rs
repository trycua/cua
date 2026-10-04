//! Local image builder.
//!
//! Boots a base image under `cua-vmm`, applies build steps through a
//! [`GuestExec`] (SSH for VMs, Docker exec for containers — the cua-spacesd
//! client can provide another implementation later), shuts down, and packs the
//! result as a KubeVirt containerDisk (VM builds) or a rootfs OCI image
//! (container builds), optionally pushing it.
//!
//! Step semantics follow `cua_sandbox/builder/executor.py`: `run` sources
//! `/etc/profile.d/cua-env.sh`, env vars land in that file and
//! `/etc/environment`, `apt_install` is non-interactive, `uv_install`
//! bootstraps uv when missing, `copy` writes local files into the guest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cua_vmm::exec::shell_quote;
use cua_vmm::{
    Arch, ExecRequest, GuestExec, ImageSource, Probe, Runtime, SshAccess, SshExec, StartSpec,
};

use crate::error::{ImageError, Result};
use crate::layout::PackedImage;
use crate::registry::RegistryClient;
use crate::spec::{ImageLayer, ImageResource, is_env_name};

/// One build step. A superset of [`ImageLayer`] covering the local-only
/// steps of the Python `Image` builder (`env`, `copy`, `expose`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildStep {
    Layer(ImageLayer),
    Env(BTreeMap<String, String>),
    /// Copy a host file into the guest.
    Copy {
        src: PathBuf,
        dst: String,
    },
    /// Record a port in the image config (no guest action).
    Expose(u16),
}

impl BuildStep {
    pub fn run(cmd: impl Into<String>) -> Self {
        BuildStep::Layer(ImageLayer::Run {
            command: cmd.into(),
        })
    }
    pub fn apt<I: IntoIterator<Item = S>, S: Into<String>>(pkgs: I) -> Self {
        BuildStep::Layer(ImageLayer::AptInstall {
            packages: pkgs.into_iter().map(Into::into).collect(),
        })
    }
}

/// Convert an Image resource's recipe into ordered steps (env first, as the
/// Python builder does, then layers, then exposed ports). Fleet `files`
/// reference server-side uploads and cannot be built locally.
pub fn steps_from_spec(r: &ImageResource) -> Result<Vec<BuildStep>> {
    r.validate()?;
    let recipe = &r.spec.recipe;
    if !recipe.files.is_empty() {
        return Err(ImageError::Spec(
            "recipe.files reference Fleet uploads; use BuildStep::Copy for local builds".into(),
        ));
    }
    let mut steps = Vec::new();
    if !recipe.env.is_empty() {
        steps.push(BuildStep::Env(recipe.env.clone()));
    }
    steps.extend(recipe.layers.iter().cloned().map(BuildStep::Layer));
    steps.extend(recipe.ports.iter().copied().map(BuildStep::Expose));
    Ok(steps)
}

fn quote_all(pkgs: &[String]) -> String {
    pkgs.iter()
        .map(|p| shell_quote(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render a step to a guest shell script. `sudo` is `""` when already root.
pub fn render(step: &BuildStep, sudo: &str) -> Result<Option<String>> {
    Ok(Some(match step {
        BuildStep::Layer(ImageLayer::Run { command }) => {
            let inner = shell_quote(&format!(
                ". /etc/profile.d/cua-env.sh 2>/dev/null; {command}"
            ));
            format!(
                "if command -v bash >/dev/null 2>&1; then {sudo}bash -c {inner}; else {sudo}sh -c {inner}; fi"
            )
        }
        BuildStep::Layer(ImageLayer::AptInstall { packages }) => format!(
            "{sudo}env DEBIAN_FRONTEND=noninteractive apt-get update -qq && \
             {sudo}env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends {}",
            quote_all(packages)
        ),
        BuildStep::Layer(ImageLayer::PipInstall { packages }) => {
            format!(
                "{sudo}python3 -m pip install --break-system-packages {}",
                quote_all(packages)
            )
        }
        BuildStep::Layer(ImageLayer::UvInstall { packages }) => format!(
            "export PATH=\"/usr/local/bin:$HOME/.local/bin:$PATH\"; \
             command -v uv >/dev/null 2>&1 || (curl -LsSf https://astral.sh/uv/install.sh | {sudo}env UV_INSTALL_DIR=/usr/local/bin sh) && \
             {sudo}env PATH=\"$PATH\" uv pip install --system --break-system-packages {}",
            quote_all(packages)
        ),
        BuildStep::Layer(ImageLayer::AppInstall { app_id }) => {
            let dir = std::env::var_os("CUA_APPS_DIR")
                .map(PathBuf::from)
                .ok_or_else(|| ImageError::Build {
                    step: format!("app_install {app_id}"),
                    detail:
                        "local app installs need the cua-sandbox-apps catalog; set CUA_APPS_DIR"
                            .into(),
                })?;
            let script = std::fs::read_to_string(dir.join(app_id).join("linux").join("install.sh"))
                .map_err(|e| ImageError::Build {
                    step: format!("app_install {app_id}"),
                    detail: e.to_string(),
                })?;
            format!("{sudo}bash -c {}", shell_quote(&script))
        }
        BuildStep::Env(vars) => {
            let mut s = format!(
                "printf '#!/bin/sh\\n' | {sudo}tee -a /etc/profile.d/cua-env.sh >/dev/null"
            );
            for (k, v) in vars {
                if !is_env_name(k) {
                    return Err(ImageError::Spec(format!("unsafe env var name '{k}'")));
                }
                let q = shell_quote(v);
                s.push_str(&format!(
                    " && printf 'export {k}=%s\\n' {qq} | {sudo}tee -a /etc/profile.d/cua-env.sh >/dev/null \
                     && printf '{k}=%s\\n' {q} | {sudo}tee -a /etc/environment >/dev/null",
                    qq = shell_quote(&shell_quote(v)),
                ));
            }
            s
        }
        BuildStep::Copy { .. } | BuildStep::Expose(_) => return Ok(None),
    }))
}

/// Apply steps through any [`GuestExec`]. `log` receives progress lines.
pub async fn apply_steps(
    exec: &dyn GuestExec,
    steps: &[BuildStep],
    log: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    let sudo = if exec.is_root().await? {
        ""
    } else {
        "sudo -n "
    };
    for (i, step) in steps.iter().enumerate() {
        let label = format!("step {}/{} {:?}", i + 1, steps.len(), step);
        log(&label);
        let t = Instant::now();
        match step {
            BuildStep::Copy { src, dst } => {
                let data = std::fs::read(src).map_err(|e| ImageError::Build {
                    step: label.clone(),
                    detail: format!("{}: {e}", src.display()),
                })?;
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::metadata(src)?.permissions().mode() & 0o7777
                };
                #[cfg(not(unix))]
                let mode = 0o644;
                exec.put_file(dst, &data, mode)
                    .await
                    .map_err(|e| ImageError::Build {
                        step: label.clone(),
                        detail: e.to_string(),
                    })?;
            }
            other => {
                if let Some(script) = render(other, sudo)? {
                    let out = exec
                        .exec(ExecRequest::sh(script).timeout(Duration::from_secs(3600)))
                        .await?;
                    if !out.success() {
                        return Err(ImageError::Build {
                            step: label,
                            detail: format!("exit {}: {}", out.exit_code, out.stderr_str().trim()),
                        });
                    }
                }
            }
        }
        log(&format!("  done in {:.1}s", t.elapsed().as_secs_f64()));
    }
    Ok(())
}

/// Outputs of a build.
#[derive(Debug, Default)]
pub struct BuildOutput {
    /// Flattened, compressed qcow2 (VM builds).
    pub disk: Option<PathBuf>,
    /// Packed image (containerDisk for VMs, rootfs for containers).
    pub image: Option<PackedImage>,
    /// Pushed `reference → manifest digest`.
    pub pushed: Option<(String, String)>,
    pub timings: Vec<(String, f64)>,
}

/// Common build options.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub arch: Arch,
    pub cpus: u32,
    pub memory_mb: u64,
    /// Grow the build disk to this size (VM builds).
    pub disk_size_gb: Option<u32>,
    /// Where packed layers and the flattened disk go.
    pub out_dir: PathBuf,
    /// Push the packed image here.
    pub push: Option<String>,
    /// Boot/SSH timeout for VM builds.
    pub boot_timeout: Duration,
}

impl BuildOptions {
    pub fn new(out_dir: impl Into<PathBuf>) -> Self {
        Self {
            arch: Arch::host(),
            cpus: 4,
            memory_mb: 4096,
            disk_size_gb: None,
            out_dir: out_dir.into(),
            push: None,
            boot_timeout: Duration::from_secs(600),
        }
    }
}

struct Clock {
    start: Instant,
    last: Instant,
    laps: Vec<(String, f64)>,
}

impl Clock {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
            laps: vec![],
        }
    }
    fn lap(&mut self, what: &str) {
        let now = Instant::now();
        let s = (now - self.last).as_secs_f64();
        tracing::info!("{what}: {s:.1}s");
        self.laps.push((what.to_string(), s));
        self.last = now;
    }
    fn finish(mut self) -> Vec<(String, f64)> {
        self.laps
            .push(("total".into(), self.start.elapsed().as_secs_f64()));
        self.laps
    }
}

/// Build a VM image: boot `base` (a local disk; resolve OCI refs with
/// [`crate::containerdisk::pull`] first) under `qemu`, apply `steps` over SSH,
/// power off, flatten + compress, pack as a containerDisk and optionally push.
pub async fn build_vm(
    qemu: &cua_vmm::qemu::QemuRuntime,
    base: &Path,
    steps: &[BuildStep],
    opts: &BuildOptions,
) -> Result<BuildOutput> {
    let mut clock = Clock::new();
    // The build VM's overlay, the flattened disk and the packed layer.
    cua_vmm::disk::ensure_space(
        &opts.out_dir,
        std::fs::metadata(base)
            .map(|m| m.len())
            .unwrap_or(0)
            .saturating_mul(2)
            .saturating_add(cua_vmm::disk::BUILD_ESTIMATE),
        "build a VM image",
    )
    .map_err(cua_vmm::VmmError::from)?;
    std::fs::create_dir_all(&opts.out_dir)?;
    let key = cua_vmm::cloudinit::ensure_ssh_key(
        &cua_vmm::host::cua_home().join("build").join("id_ed25519"),
    )
    .await?;
    let name = format!(
        "cua-build-{:x}",
        std::process::id() as u64 * 7919 + clock.start.elapsed().as_nanos() as u64 % 7919
    );
    let mut spec = StartSpec::new(&name, ImageSource::disk(base))
        .arch(opts.arch)
        .cpus(opts.cpus)
        .memory_mb(opts.memory_mb)
        .ssh(SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        })
        .probe(Probe::tcp(22))
        .ready_timeout(opts.boot_timeout);
    spec.disk_size_gb = opts.disk_size_gb;

    let result: Result<BuildOutput> = async {
        let inst = qemu.start(&spec).await?;
        let ssh = SshExec::from_endpoint(inst.endpoints.ssh.as_ref().expect("ssh requested"));
        ssh.wait_until_ready(opts.boot_timeout).await?;
        clock.lap("boot + ssh");
        // Let first-boot cloud-init finish (apt locks, growpart).
        let _ = ssh
            .exec(ExecRequest::sh("command -v cloud-init >/dev/null && sudo -n cloud-init status --wait >/dev/null 2>&1; true")
                .timeout(opts.boot_timeout))
            .await;
        clock.lap("cloud-init settle");
        let log = |s: &str| tracing::info!(build = %name, "{s}");
        apply_steps(&ssh, steps, &log).await?;
        clock.lap("apply steps");
        // Generalise: next boot is a new cloud-init instance, and the build
        // key is not left authorised in the image.
        let _ = ssh
            .exec(ExecRequest::sh(
                "sudo -n cloud-init clean --logs --seed >/dev/null 2>&1; sudo -n rm -f /etc/ssh/ssh_host_*; \
                 rm -f ~/.ssh/authorized_keys; sync",
            ))
            .await;
        qemu.stop(&name).await?;
        clock.lap("shutdown");
        let st = qemu.load(&name)?;
        let disk = opts.out_dir.join(format!("disk-{}.qcow2", opts.arch.oci()));
        let _ = std::fs::remove_file(&disk);
        cua_vmm::qemu::img::convert(&st.disk, &disk, true, None).await?;
        clock.lap("flatten + compress (qemu-img convert -c)");
        let (d, arch, out) = (disk.clone(), opts.arch.oci().to_string(), opts.out_dir.join(format!("containerdisk-{}", opts.arch.oci())));
        let image = tokio::task::spawn_blocking(move || crate::containerdisk::pack(&d, &arch, &out))
            .await
            .map_err(|e| ImageError::Build { step: "pack".into(), detail: e.to_string() })??;
        clock.lap("pack containerDisk");
        let mut output = BuildOutput { disk: Some(disk), image: Some(image), ..Default::default() };
        if let Some(reference) = &opts.push {
            let client = RegistryClient::default();
            let desc = output.image.as_ref().unwrap().push(&client, reference).await?;
            output.pushed = Some((reference.clone(), desc.digest));
            clock.lap("push");
        }
        Ok(output)
    }
    .await;
    let _ = qemu.delete(&name).await;
    let mut out = result?;
    out.timings = clock.finish();
    Ok(out)
}

/// Build a container rootfs image: run `base` with a sleep command under the
/// container backend (gVisor when available), apply `steps` over Docker exec,
/// export the rootfs (`tar` via exec, which also sees gVisor's overlay), pack
/// it as a single-layer OCI image keeping the base image's config, and
/// optionally push it. Also imports the result into the engine as
/// `cua-vmm/checkpoint:<tag>` when `engine_tag` is set.
pub async fn build_container(
    rt: &cua_vmm::container::ContainerRuntime,
    base: &str,
    steps: &[BuildStep],
    opts: &BuildOptions,
    engine_tag: Option<&str>,
) -> Result<BuildOutput> {
    let mut clock = Clock::new();
    cua_vmm::disk::ensure_space(
        &opts.out_dir,
        cua_vmm::disk::BUILD_ESTIMATE,
        "build a container image",
    )
    .map_err(cua_vmm::VmmError::from)?;
    std::fs::create_dir_all(&opts.out_dir)?;
    let name = format!("cua-build-ctr-{:x}", std::process::id());
    let platform = format!("linux/{}", opts.arch.oci());
    rt.ensure_image(base, Some(&platform)).await?;
    let base_cfg = rt
        .docker()
        .inspect_image(base)
        .await
        .map_err(|e| ImageError::Build {
            step: "inspect base".into(),
            detail: e.to_string(),
        })?
        .config
        .unwrap_or_default();
    let spec = StartSpec::new(&name, ImageSource::oci(base))
        .arch(opts.arch)
        .cpus(opts.cpus)
        .memory_mb(opts.memory_mb)
        .command([
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 3600 & wait; done",
        ])
        .label(cua_vmm::container::LABEL_KIND, "build");
    let result: Result<BuildOutput> = async {
        rt.start(&spec).await?;
        clock.lap("start build container");
        let exec = rt.exec_handle(&name);
        let log = |s: &str| tracing::info!(build = %name, "{s}");
        apply_steps(&exec, steps, &log).await?;
        clock.lap("apply steps");

        let mut exposed: Vec<u16> = base_cfg
            .exposed_ports
            .iter()
            .flatten()
            .filter_map(|p| p.split('/').next()?.parse().ok())
            .collect();
        exposed.extend(steps.iter().filter_map(|s| {
            if let BuildStep::Expose(p) = s {
                Some(*p)
            } else {
                None
            }
        }));
        let mut env = base_cfg.env.clone().unwrap_or_default();
        for s in steps {
            if let BuildStep::Env(vars) = s {
                env.extend(vars.iter().map(|(k, v)| format!("{k}={v}")));
            }
        }
        let cfg = crate::rootfs::RootfsConfig {
            cmd: base_cfg.cmd.clone(),
            env,
            exposed_ports: exposed,
            working_dir: base_cfg.working_dir.clone().filter(|s| !s.is_empty()),
            user: base_cfg.user.clone().filter(|s| !s.is_empty()),
        };

        // Stream the rootfs out through exec into a tar file, then pack.
        let tar_path = opts.out_dir.join("rootfs.tar");
        let (mut stream, exec_id) = exec
            .stream_stdout("cd / && exec tar -cf - $(ls -A / | grep -vxE 'proc|sys|dev')")
            .await?;
        {
            use futures::StreamExt;
            use tokio::io::AsyncWriteExt;
            let mut f = tokio::io::BufWriter::new(tokio::fs::File::create(&tar_path).await?);
            while let Some(chunk) = stream.next().await {
                f.write_all(&chunk?).await?;
            }
            f.flush().await?;
        }
        let code = rt
            .docker()
            .inspect_exec(&exec_id)
            .await
            .ok()
            .and_then(|i| i.exit_code)
            .unwrap_or(-1);
        if code != 0 && code != 1 {
            return Err(ImageError::Build {
                step: "export rootfs".into(),
                detail: format!("tar exited {code}"),
            });
        }
        clock.lap("export rootfs");
        let (tp, arch, out) = (
            tar_path.clone(),
            opts.arch.oci().to_string(),
            opts.out_dir.join(format!("rootfs-{}", opts.arch.oci())),
        );
        let image = tokio::task::spawn_blocking(move || {
            crate::rootfs::pack_tar(std::fs::File::open(&tp)?, &arch, &cfg, &out)
        })
        .await
        .map_err(|e| ImageError::Build {
            step: "pack".into(),
            detail: e.to_string(),
        })??;
        let _ = std::fs::remove_file(&tar_path);
        clock.lap("pack rootfs image");
        if let Some(tag) = engine_tag {
            rt.checkpoint(&name, tag).await?;
            clock.lap("import into engine");
        }
        let mut output = BuildOutput {
            image: Some(image),
            ..Default::default()
        };
        if let Some(reference) = &opts.push {
            let client = RegistryClient::default();
            let desc = output
                .image
                .as_ref()
                .unwrap()
                .push(&client, reference)
                .await?;
            output.pushed = Some((reference.clone(), desc.digest));
            clock.lap("push");
        }
        Ok(output)
    }
    .await;
    let _ = rt.delete(&name).await;
    let mut out = result?;
    out.timings = clock.finish();
    Ok(out)
}

/// Build a container image into the local engine as `repo:tag`: run
/// `base` with a sleep command under the container backend (gVisor when
/// available), apply `steps` over Docker exec, and import the rootfs (`tar`
/// through exec, so gVisor's overlay is included) with the base image's
/// config plus the steps' environment and exposed ports. Nothing is packed
/// or pushed; the result runs as `container:<repo>:<tag>`. `creds` pull a
/// private base. Returns the timings.
pub async fn build_container_into_engine(
    rt: &cua_vmm::container::ContainerRuntime,
    base: &str,
    steps: &[BuildStep],
    opts: &BuildOptions,
    repo: &str,
    tag: &str,
    creds: Option<&cua_vmm::RegistryCredentials>,
) -> Result<Vec<(String, f64)>> {
    let mut clock = Clock::new();
    let name = format!(
        "cua-build-{}",
        tag.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(40)
            .collect::<String>()
    );
    let platform = format!("linux/{}", opts.arch.oci());
    rt.ensure_image_with(base, Some(&platform), creds).await?;
    let base_cfg = rt
        .docker()
        .inspect_image(base)
        .await
        .map_err(|e| ImageError::Build {
            step: "inspect base".into(),
            detail: e.to_string(),
        })?
        .config
        .unwrap_or_default();
    // A leftover build container of the same image (a crashed build).
    let _ = rt.delete(&name).await;
    // The exported rootfs streams into the engine as a new image.
    cua_vmm::disk::ensure_space(
        &cua_vmm::host::cua_home(),
        cua_vmm::disk::BUILD_ESTIMATE,
        &format!("build {repo}:{tag}"),
    )
    .map_err(cua_vmm::VmmError::from)?;
    let spec = StartSpec::new(&name, ImageSource::oci(base))
        .arch(opts.arch)
        .cpus(opts.cpus)
        .memory_mb(opts.memory_mb)
        .command([
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 3600 & wait; done",
        ])
        .label(cua_vmm::container::LABEL_KIND, "build");
    let result: Result<()> = async {
        rt.start(&spec).await?;
        clock.lap("start build container");
        let exec = rt.exec_handle(&name);
        let log = |s: &str| tracing::info!(build = %name, "{s}");
        apply_steps(&exec, steps, &log).await?;
        clock.lap("apply steps");
        let json = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
        let mut changes = Vec::new();
        let mut env: Vec<(String, String)> = base_cfg
            .env
            .iter()
            .flatten()
            .filter_map(|e| {
                e.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        for s in steps {
            if let BuildStep::Env(vars) = s {
                env.extend(vars.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        for (k, v) in env {
            changes.push(format!(
                "ENV {k}={}",
                serde_json::to_string(&v).unwrap_or_default()
            ));
        }
        if let Some(ep) = base_cfg.entrypoint.as_ref().filter(|v| !v.is_empty()) {
            changes.push(format!("ENTRYPOINT {}", json(ep)));
        }
        if let Some(cmd) = base_cfg.cmd.as_ref().filter(|v| !v.is_empty()) {
            changes.push(format!("CMD {}", json(cmd)));
        }
        if let Some(wd) = base_cfg.working_dir.as_ref().filter(|s| !s.is_empty()) {
            changes.push(format!("WORKDIR {wd}"));
        }
        if let Some(u) = base_cfg.user.as_ref().filter(|s| !s.is_empty()) {
            changes.push(format!("USER {u}"));
        }
        let mut exposed: Vec<String> = base_cfg.exposed_ports.iter().flatten().cloned().collect();
        exposed.extend(steps.iter().filter_map(|s| match s {
            BuildStep::Expose(p) => Some(format!("{p}/tcp")),
            _ => None,
        }));
        exposed.sort();
        exposed.dedup();
        changes.extend(exposed.into_iter().map(|p| format!("EXPOSE {p}")));
        rt.import_rootfs(&name, repo, tag, changes).await?;
        clock.lap("import into engine");
        Ok(())
    }
    .await;
    let _ = rt.delete(&name).await;
    result?;
    Ok(clock.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ImageRecipe, ImageResource};

    #[test]
    fn spec_maps_to_ordered_steps() {
        let mut recipe = ImageRecipe {
            distro: "ubuntu".into(),
            version: "24.04".into(),
            layers: vec![
                ImageLayer::AptInstall {
                    packages: vec!["curl".into()],
                },
                ImageLayer::Run {
                    command: "true".into(),
                },
            ],
            ports: vec![8080],
            ..Default::default()
        };
        recipe.env.insert("A".into(), "b c".into());
        let steps = steps_from_spec(&ImageResource::new("x", "y", recipe)).unwrap();
        assert!(matches!(steps[0], BuildStep::Env(_)));
        assert!(matches!(
            steps[1],
            BuildStep::Layer(ImageLayer::AptInstall { .. })
        ));
        assert_eq!(steps.last(), Some(&BuildStep::Expose(8080)));
    }

    #[test]
    fn rendering_quotes_and_sudo() {
        let s = render(&BuildStep::apt(["curl", "evil; rm -rf /"]), "sudo -n ")
            .unwrap()
            .unwrap();
        assert!(s.contains("'evil; rm -rf /'"));
        assert!(s.contains("sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install"));
        let s = render(&BuildStep::run("echo \"$HOME\" 'x'"), "")
            .unwrap()
            .unwrap();
        assert!(s.starts_with("if command -v bash"));
        assert!(!s.contains("sudo"));
        let mut env = BTreeMap::new();
        env.insert("BAD-NAME".to_string(), "x".to_string());
        assert!(render(&BuildStep::Env(env), "").is_err());
        assert!(render(&BuildStep::Expose(1), "").unwrap().is_none());
    }

    /// Apply every step kind against the host shell through PrefixExec, with
    /// commands pointed at a temp dir instead of /etc — exercises quoting and
    /// the env/profile/copy logic end to end without a VM.
    #[tokio::test]
    async fn env_and_run_steps_execute_in_a_real_shell() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().display().to_string();
        // Rewrite /etc paths into the temp dir for this hermetic test.
        struct Rooted {
            inner: cua_vmm::PrefixExec,
            root: String,
        }
        #[async_trait::async_trait]
        impl GuestExec for Rooted {
            async fn exec(&self, mut req: ExecRequest) -> cua_vmm::Result<cua_vmm::ExecOutput> {
                req.script = req.script.replace("/etc/", &format!("{}/etc/", self.root));
                self.inner.exec(req).await
            }
            async fn is_root(&self) -> cua_vmm::Result<bool> {
                Ok(true)
            }
        }
        std::fs::create_dir_all(d.path().join("etc/profile.d")).unwrap();
        let exec = Rooted {
            inner: cua_vmm::PrefixExec {
                program: "/bin/sh".into(),
                prefix: vec!["-c".into()],
            },
            root,
        };
        let mut env = BTreeMap::new();
        env.insert("GREETING".to_string(), "it's \"quoted\" $HOME".to_string());
        let src = d.path().join("src.txt");
        std::fs::write(&src, b"payload").unwrap();
        let steps = vec![
            BuildStep::Env(env),
            BuildStep::run(format!(
                "printf '%s' \"$GREETING\" > {}/out.txt",
                d.path().display()
            )),
            BuildStep::Copy {
                src,
                dst: format!("{}/copied/deep/file.txt", d.path().display()),
            },
        ];
        apply_steps(&exec, &steps, &|_| {}).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("out.txt")).unwrap(),
            "it's \"quoted\" $HOME"
        );
        let envfile = std::fs::read_to_string(d.path().join("etc/environment")).unwrap();
        assert!(envfile.contains("GREETING=it's"));
        assert_eq!(
            std::fs::read(d.path().join("copied/deep/file.txt")).unwrap(),
            b"payload"
        );
        // A failing step reports which step failed.
        let err = apply_steps(&exec, &[BuildStep::run("exit 3")], &|_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exit 3"), "{err}");
    }
}
