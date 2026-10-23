//! `Sandboxes.bake`: a world checkpoint of a Docker Compose project, so
//! forks start with its containers already running (`cua sb bake` in the
//! CLI; `sandboxes().bake(...)` in every SDK language).
//!
//! create (a local `vm` Space, by default of the docker tier,
//! `ghcr.io/trycua/linux:24.04-docker-disk`) -> dockerd ready -> upload the
//! project (and `docker save` tarballs) -> `docker compose up --wait` -> the
//! stack check (+ `health` commands) -> `run` commands -> live memory
//! checkpoint -> (`verify_fork`: fork it once and check the stack is still
//! live) -> delete the bake Space.
//!
//! Every guest step runs as root through the Space's cua-spacesd (with
//! passwordless sudo when cua-spacesd runs as the desktop user).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Sandbox, Sandboxes, SpacesdClient, run};
use crate::{CuaError, Result, SpacesdCommand};

/// Where a bake keeps its project in the guest, under `<dest>/<project>`.
pub const BAKE_DEFAULT_DEST: &str = "/opt/cua-bake";

/// Options for [`Sandboxes::bake`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BakeOptions {
    /// The Docker Compose project directory on this machine (holds
    /// compose.yaml or docker-compose.yml, and whatever it builds or
    /// mounts).
    pub project_dir: String,
    /// The checkpoint to save (`Sandboxes.fork(checkpoint, name)` starts
    /// from it).
    pub checkpoint: String,
    /// The image: a reference or a local disk path. `None`: the docker tier
    /// (`Image.linux(tier="docker")`, ghcr.io/trycua/linux:24.04-docker-disk).
    #[uniffi(default = None)]
    pub image: Option<String>,
    /// The bake Space's name. `None`: `bake-<checkpoint>`.
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// vCPUs of the bake Space (forks get the same).
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// Memory of the bake Space in MiB (forks get the same).
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Compose files, relative to the project. Empty: compose's own lookup.
    #[uniffi(default = [])]
    pub compose_files: Vec<String>,
    /// The compose project name. `None`: the directory name.
    #[uniffi(default = None)]
    pub project_name: Option<String>,
    /// Where the project lands in the guest. `None`:
    /// `/opt/cua-bake/<project>`.
    #[uniffi(default = None)]
    pub dest: Option<String>,
    /// `docker save` tarballs on this machine to `docker load` before
    /// `compose up`, for images the guest cannot pull.
    #[uniffi(default = [])]
    pub image_tarballs: Vec<String>,
    /// Guest shell commands, run as root in the project directory, that
    /// must exit 0 before the checkpoint (each retried until
    /// `health_timeout_s`). `compose up --wait` already waits for the
    /// services' healthchecks.
    #[uniffi(default = [])]
    pub health: Vec<String>,
    /// Budget for `compose up --wait` and each health command, seconds.
    #[uniffi(default = 600)]
    pub health_timeout_s: u32,
    /// Guest shell commands, run as root in the project directory once the
    /// stack is healthy and before the checkpoint: state forks should start
    /// with.
    #[uniffi(default = [])]
    pub run: Vec<String>,
    /// Fork the checkpoint once, check the stack is live in the fork, then
    /// delete the fork.
    #[uniffi(default = false)]
    pub verify_fork: bool,
    /// Keep the bake Space after the checkpoint (default: delete it).
    #[uniffi(default = false)]
    pub keep: bool,
}

impl BakeOptions {
    /// Options for `project_dir` -> `checkpoint`, everything else default.
    pub fn new(project_dir: impl Into<String>, checkpoint: impl Into<String>) -> Self {
        Self {
            project_dir: project_dir.into(),
            checkpoint: checkpoint.into(),
            image: None,
            name: None,
            cpus: None,
            memory_mb: None,
            compose_files: vec![],
            project_name: None,
            dest: None,
            image_tarballs: vec![],
            health: vec![],
            health_timeout_s: 600,
            run: vec![],
            verify_fork: false,
            keep: false,
        }
    }
}

/// The fork [`BakeOptions::verify_fork`] made and checked.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BakeForkCheck {
    /// The fork's name (deleted after the check).
    pub fork: String,
    /// Seconds from the fork request until it was ready.
    pub fork_s: f64,
    /// Seconds the stack check and health commands took in the fork.
    pub check_s: f64,
}

/// What [`Sandboxes::bake`] did and how long each step took (seconds).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BakeSummary {
    /// The checkpoint saved.
    pub checkpoint: String,
    /// The engine holding it (`qemu`, ...).
    pub runtime: String,
    /// The bake Space (deleted unless `keep`).
    pub space: String,
    /// The image it ran.
    pub image: String,
    /// The compose project name.
    pub project: String,
    /// The project directory in the guest.
    pub dest: String,
    /// When dockerd became ready, seconds after the guest kernel booted.
    pub dockerd_ready_s: Option<f64>,
    /// Creating the Space until its cua-spacesd answered.
    pub create_s: f64,
    /// The packed project's size.
    pub upload_bytes: u64,
    /// Uploading the project and loading the tarballs.
    pub upload_s: f64,
    /// `docker compose up --wait`.
    pub compose_up_s: f64,
    /// The stack check and health commands.
    pub health_s: f64,
    /// The checkpoint.
    pub checkpoint_s: f64,
    /// The whole bake.
    pub total_s: f64,
    /// One line per container: name, state or health, restarts.
    pub containers: Vec<String>,
    /// The fork check, with `verify_fork`.
    pub verify_fork: Option<BakeForkCheck>,
}

/// The project name compose would derive: lowercase letters, digits, `-`
/// and `_`, starting with a letter or digit.
pub fn bake_project_name(dir: &Path) -> String {
    let raw = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
        .or_else(|| dir.file_name().map(|f| f.to_string_lossy().to_string()))
        .unwrap_or_default();
    let s: String = raw
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let s = s.trim_start_matches(['-', '_']).to_string();
    if s.is_empty() { "world".into() } else { s }
}

/// POSIX single-quoting.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,@%+".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `docker compose -p P [-f F]...` for the project.
pub fn compose_cmd(project: &str, files: &[String]) -> String {
    let mut c = format!("docker compose -p {}", sh_quote(project));
    for f in files {
        c.push_str(" -f ");
        c.push_str(&sh_quote(f));
    }
    c
}

/// A shell check that the compose stack is live: dockerd answers, every
/// service of the config has a running container, and every container is
/// `healthy` (with a healthcheck) or `running` (without) and has not
/// restarted. Prints `STACK_LIVE` or `STACK_DEAD`, then one line per
/// container.
pub fn stack_check_script(compose: &str) -> String {
    format!(
        r#"docker info >/dev/null 2>&1 || {{ echo STACK_DEAD; echo dockerd-down; exit 0; }}
want="$({compose} config --services | sort)"
have="$({compose} ps --services --filter status=running | sort)"
ids="$({compose} ps -q)"
states="$(for id in $ids; do docker inspect --format '{{{{.Name}}}} {{{{if .State.Health}}}}{{{{.State.Health.Status}}}}{{{{else}}}}{{{{.State.Status}}}}{{{{end}}}} restarts={{{{.RestartCount}}}}' "$id"; done)"
bad="$(printf '%s\n' "$states" | awk 'NF && ($2 != "healthy" && $2 != "running" || $3 != "restarts=0")')"
if [ -n "$want" ] && [ "$want" = "$have" ] && [ -z "$bad" ]; then echo STACK_LIVE; else echo STACK_DEAD; echo "want: $want"; echo "running: $have"; fi
printf '%s\n' "$states""#
    )
}

/// Whether [`stack_check_script`]'s output says live.
pub fn stack_is_live(stdout: &str) -> bool {
    stdout.lines().next().map(str::trim) == Some("STACK_LIVE")
}

/// The seconds-after-boot of `docker-ready`'s line (`dockerd ready 3.21s
/// after boot`).
pub fn parse_dockerd_ready(stdout: &str) -> Option<f64> {
    stdout.lines().find_map(|l| {
        l.trim()
            .strip_prefix("dockerd ready ")?
            .split_whitespace()
            .next()?
            .strip_suffix('s')?
            .parse()
            .ok()
    })
}

/// The project as a gzipped tar (paths relative to the project; `.git`
/// left out), in memory.
pub fn pack_project(dir: &Path) -> std::io::Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let abs = dir.join(&rel);
        let mut entries: Vec<_> =
            std::fs::read_dir(&abs)?.collect::<std::result::Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            if name == ".git" {
                continue;
            }
            let r = rel.join(&name);
            if e.file_type()?.is_dir() {
                tar.append_dir(&r, e.path())?;
                stack.push(r);
            } else {
                tar.append_path_with_name(e.path(), &r)?;
            }
        }
    }
    tar.into_inner()?.finish()
}

/// Runs `$0` as root: directly when cua-spacesd runs as root, else through
/// passwordless sudo (the image's desktop user has it; cua-spacesd runs as
/// that user and cannot switch users itself).
const AS_ROOT: &str =
    r#"if [ "$(id -u)" = 0 ]; then exec /bin/sh -c "$0"; else exec sudo -n /bin/sh -c "$0"; fi"#;

fn secs(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 100.0).round() / 100.0
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut t = lines[lines.len().saturating_sub(30)..].join("\n");
    if !t.is_empty() {
        t.push('\n');
    }
    t
}

/// One guest shell command as root; its stdout, or an error with its output.
async fn root_sh(env: &SpacesdClient, what: &str, line: &str, timeout_s: u32) -> Result<String> {
    let o = env
        .run(SpacesdCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), AS_ROOT.into(), line.into()],
            env: Default::default(),
            cwd: None,
            user: None,
            timeout_ms: (timeout_s > 0).then(|| timeout_s.saturating_mul(1000)),
            tag: None,
            stdin: false,
            pty: None,
            request_id: None,
            epoch: None,
            scrollback_bytes: None,
            kill_on_disconnect: false,
        })
        .await?;
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    if !o.exit.success {
        let why = if o.exit.timed_out {
            format!("ran out of time after {timeout_s}s")
        } else {
            match (&o.exit.code, &o.exit.signal, &o.exit.error) {
                (Some(c), _, _) => format!("exit {c}"),
                (None, Some(sig), _) => format!("killed by signal {sig:?}"),
                (None, None, Some(e)) => format!("supervisor error: {e}"),
                _ => "no exit status".to_string(),
            }
        };
        return Err(CuaError::Internal(format!(
            "bake: {what} failed ({why}):\n{}{}",
            tail(&stdout),
            tail(&String::from_utf8_lossy(&o.stderr))
        )));
    }
    Ok(stdout)
}

/// Retries `line` until it exits 0 or `budget_s` is spent.
async fn until_ok(env: &SpacesdClient, what: &str, line: &str, budget_s: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(budget_s.into());
    loop {
        let left = deadline.saturating_duration_since(Instant::now()).as_secs() as u32;
        match root_sh(env, what, line, left.clamp(5, 120)).await {
            Ok(_) => return Ok(()),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
}

/// The stack check, then each health command, in `dir`. Returns the
/// container lines.
async fn check_stack(
    env: &SpacesdClient,
    dir: &str,
    compose: &str,
    health: &[String],
    budget_s: u32,
) -> Result<Vec<String>> {
    let out = root_sh(
        env,
        "the stack check",
        &format!("cd {} && {}", sh_quote(dir), stack_check_script(compose)),
        120,
    )
    .await?;
    if !stack_is_live(&out) {
        return Err(CuaError::Internal(format!(
            "bake: the compose stack is not live:\n{}",
            tail(&out)
        )));
    }
    for h in health {
        until_ok(
            env,
            &format!("health command {h:?}"),
            &format!("cd {} && {h}", sh_quote(dir)),
            budget_s,
        )
        .await?;
    }
    Ok(out
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect())
}

/// Checks what a bake can check before it starts a VM.
fn validate(o: &BakeOptions) -> Result<()> {
    if o.checkpoint.trim().is_empty() {
        return Err(CuaError::InvalidArgument(
            "bake: checkpoint is empty".into(),
        ));
    }
    if !Path::new(&o.project_dir).is_dir() {
        return Err(CuaError::NotFound(format!(
            "{}: not a directory (the compose project)",
            o.project_dir
        )));
    }
    for t in &o.image_tarballs {
        if !Path::new(t).is_file() {
            return Err(CuaError::NotFound(format!("{t}: no such image tarball")));
        }
    }
    Ok(())
}

#[uniffi::export]
impl Sandboxes {
    /// Bakes a Docker Compose project into a checkpoint that forks start
    /// from with its containers already running: a local VM (by default
    /// the docker tier, dockerd up at boot), the project uploaded, `docker
    /// compose up --wait`, the stack checked (every service running or
    /// healthy, none restarted) and `health` commands passed, `run`
    /// commands, then a live memory checkpoint. The bake Space is deleted
    /// afterwards unless `keep`. Fork it with `fork(checkpoint, name)`.
    pub async fn bake(&self, options: BakeOptions) -> Result<BakeSummary> {
        validate(&options)?;
        let sbx = Sandboxes {
            backend: self.backend.clone(),
        };
        run(async move { bake(sbx, options).await }).await
    }
}

/// The bake Space: a local VM of `image` that is a fork source (guest RAM a
/// shared file, so the checkpoint clones it in milliseconds and every fork
/// shares the world's pages), ready once cua-spacesd listens.
fn bake_create_options(o: &BakeOptions, image: &str, name: &str) -> super::SandboxCreateOptions {
    let mut c = super::SandboxCreateOptions::auto(image.to_string());
    c.on = Some("local".into());
    c.kind = Some("vm".into());
    c.os = Some("linux".into());
    c.name = Some(name.to_string());
    c.cpus = o.cpus;
    c.memory_mb = o.memory_mb;
    c.forkable = Some(true);
    c.wait_for = vec![super::ReadinessProbe {
        port: cua_proto::SPACESD_DEFAULT_PORT,
        http_path: None,
        http_status: None,
        service: None,
    }];
    c
}

async fn bake(sbx: Sandboxes, o: BakeOptions) -> Result<BakeSummary> {
    let total = Instant::now();
    let dir = PathBuf::from(&o.project_dir);
    let project = o
        .project_name
        .clone()
        .unwrap_or_else(|| bake_project_name(&dir));
    let dest = o
        .dest
        .clone()
        .unwrap_or_else(|| format!("{BAKE_DEFAULT_DEST}/{project}"));
    let name = o
        .name
        .clone()
        .unwrap_or_else(|| format!("bake-{}", o.checkpoint));
    let compose = compose_cmd(&project, &o.compose_files);
    let image = match &o.image {
        Some(i) => i.clone(),
        None => cua_image::canonical::tier_image("linux", None, Some("docker"))
            .map_err(super::image::image_error)?,
    };
    let c = bake_create_options(&o, &image, &name);
    tracing::info!(space = %name, %image, "bake: creating the bake Space");
    let t = Instant::now();
    let sb = sbx.create(c).await?;
    let create_s = secs(t);
    let r = bake_in(&sbx, &o, &name, &project, &dest, &compose, &sb).await;
    if !o.keep
        && let Err(e) = sbx.delete(name.clone()).await
    {
        tracing::warn!(space = %name, "bake: could not delete the bake Space: {e}");
    }
    let mut s = r?;
    s.image = image;
    s.create_s = create_s;
    s.total_s = secs(total);
    Ok(s)
}

async fn bake_in(
    sbx: &Sandboxes,
    o: &BakeOptions,
    name: &str,
    project: &str,
    dest: &str,
    compose: &str,
    sb: &Arc<Sandbox>,
) -> Result<BakeSummary> {
    let env = sb.spacesd(None).await?;

    // dockerd.
    let ready = root_sh(
        &env,
        "waiting for dockerd (is this a docker-tier image?)",
        "command -v docker-ready >/dev/null || { echo 'no docker-ready: not a docker-tier image' >&2; exit 3; }; docker-ready 180",
        200,
    )
    .await?;
    let dockerd_ready_s = parse_dockerd_ready(&ready);
    tracing::info!("bake: {}", ready.trim());

    // The project (and image tarballs).
    let t = Instant::now();
    let tgz = pack_project(Path::new(&o.project_dir))
        .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", o.project_dir)))?;
    let upload_bytes = tgz.len() as u64;
    let staged = format!("/tmp/cua-bake-{project}.tgz");
    env.upload(staged.clone(), tgz, None).await?;
    root_sh(
        &env,
        "unpacking the project",
        &format!(
            "set -e; rm -rf {d}; mkdir -p {d}; tar -xzf {s} -C {d}; rm -f {s}",
            d = sh_quote(dest),
            s = sh_quote(&staged)
        ),
        300,
    )
    .await?;
    for (i, tarball) in o.image_tarballs.iter().enumerate() {
        let guest = format!("/tmp/cua-bake-load-{i}.tar");
        env.upload_file(tarball.clone(), guest.clone(), None)
            .await?;
        root_sh(
            &env,
            &format!("docker load {tarball}"),
            &format!("docker load -i {g} && rm -f {g}", g = sh_quote(&guest)),
            900,
        )
        .await?;
    }
    let upload_s = secs(t);

    // compose up --wait.
    let t = Instant::now();
    let compose_out = root_sh(
        &env,
        "docker compose up",
        &format!(
            "cd {} && {compose} up -d --wait --wait-timeout {} 2>&1",
            sh_quote(dest),
            o.health_timeout_s
        ),
        o.health_timeout_s.saturating_add(900),
    )
    .await;
    match compose_out {
        Ok(_) => {}
        Err(CuaError::Internal(msg)) => {
            // Best effort: what the guest saw, so a failure explains itself.
            let diag = root_sh(
            &env,
            "diagnostics",
            "echo '--- free -m'; free -m; echo '--- df'; df -h / /var/lib/docker; echo '--- docker.service'; \
             systemctl is-active docker containerd; journalctl -u docker -n 25 --no-pager; \
             echo '--- dmesg'; dmesg | tail -25",
            30,
        )
        .await
        .unwrap_or_else(|e| format!("(no guest diagnostics: {e})"));
            return Err(CuaError::Internal(format!("{msg}\nguest state:\n{diag}")));
        }
        Err(e) => return Err(e),
    }
    let compose_up_s = secs(t);
    tracing::info!("bake: compose up --wait {compose_up_s}s");

    // Health.
    let t = Instant::now();
    let containers = check_stack(&env, dest, compose, &o.health, o.health_timeout_s).await?;
    let health_s = secs(t);

    // run.
    for r in &o.run {
        root_sh(
            &env,
            &format!("run command {r:?}"),
            &format!("cd {} && {r}", sh_quote(dest)),
            900,
        )
        .await?;
    }

    // The checkpoint: memory, devices and disk, the containers running.
    // Pinned: rollouts fork it for as long as it exists, so pruning and
    // golden-snapshot eviction keep it.
    let t = Instant::now();
    let ck = sbx
        .checkpoint(name.to_string(), o.checkpoint.clone(), true)
        .await?;
    let checkpoint_s = secs(t);
    tracing::info!(checkpoint = %ck.name, "bake: checkpoint saved in {checkpoint_s}s");

    // verify_fork.
    let verify_fork = if o.verify_fork {
        let fork = format!("{}-verify", o.checkpoint);
        let t = Instant::now();
        let f = sbx.fork(o.checkpoint.clone(), fork.clone()).await?;
        let fork_s = secs(t);
        let r = async {
            let env = f.spacesd(None).await?;
            let t = Instant::now();
            check_stack(&env, dest, compose, &o.health, 60).await?;
            Ok::<_, CuaError>(secs(t))
        }
        .await;
        if let Err(e) = sbx.delete(fork.clone()).await {
            tracing::warn!(%fork, "bake: could not delete the verify fork: {e}");
        }
        Some(BakeForkCheck {
            fork,
            fork_s,
            check_s: r?,
        })
    } else {
        None
    };

    Ok(BakeSummary {
        checkpoint: ck.name,
        runtime: ck.runtime_type,
        space: name.to_string(),
        image: String::new(),
        project: project.to_string(),
        dest: dest.to_string(),
        dockerd_ready_s,
        create_s: 0.0,
        upload_bytes,
        upload_s,
        compose_up_s,
        health_s,
        checkpoint_s,
        total_s: 0.0,
        containers,
        verify_fork,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bake_space_is_a_local_forkable_vm() {
        let o = BakeOptions {
            project_dir: "/tmp/world".into(),
            checkpoint: "world-v1".into(),
            image: None,
            name: None,
            cpus: Some(2),
            memory_mb: None,
            compose_files: vec![],
            project_name: None,
            dest: None,
            image_tarballs: vec![],
            health: vec![],
            health_timeout_s: 0,
            run: vec![],
            verify_fork: false,
            keep: false,
        };
        let c = bake_create_options(
            &o,
            "ghcr.io/trycua/linux:24.04-docker-disk",
            "bake-world-v1",
        );
        assert_eq!(c.on.as_deref(), Some("local"));
        assert_eq!(c.kind.as_deref(), Some("vm"));
        assert_eq!(c.forkable, Some(true), "forks must share the bake VM's RAM");
        assert_eq!(c.name.as_deref(), Some("bake-world-v1"));
        assert_eq!(c.cpus, Some(2));
    }

    #[test]
    fn quoting_and_compose_command() {
        assert_eq!(sh_quote("apex"), "apex");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(
            compose_cmd("apex", &["docker-compose.yaml".into(), "x y.yaml".into()]),
            "docker compose -p apex -f docker-compose.yaml -f 'x y.yaml'"
        );
        assert_eq!(compose_cmd("w", &[]), "docker compose -p w");
    }

    #[test]
    fn project_names_follow_compose() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("My World.v2");
        std::fs::create_dir(&p).unwrap();
        assert_eq!(bake_project_name(&p), "myworldv2");
        let q = d.path().join("--__");
        std::fs::create_dir(&q).unwrap();
        assert_eq!(bake_project_name(&q), "world");
    }

    #[test]
    fn ready_line_and_stack_output() {
        assert_eq!(
            parse_dockerd_ready("dockerd ready 3.21s after boot\n"),
            Some(3.21)
        );
        assert_eq!(parse_dockerd_ready("nope"), None);
        assert!(stack_is_live("STACK_LIVE\n/agent running restarts=0\n"));
        assert!(!stack_is_live("STACK_DEAD\nwant: a\n"));
        assert!(!stack_is_live(""));
    }

    /// `AS_ROOT` hands the line over intact, quotes and all (sudo itself
    /// swapped out: tests never escalate).
    #[cfg(unix)]
    #[test]
    fn as_root_runs_the_line_verbatim() {
        assert!(AS_ROOT.contains("exec sudo -n /bin/sh -c \"$0\""));
        let script = AS_ROOT.replace("sudo -n ", "");
        let line = r#"printf '%s|' "a b" 'c"d'; echo $((1+2))"#;
        let o = std::process::Command::new("/bin/sh")
            .args(["-c", &script, line])
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout), "a b|c\"d|3\n");
    }

    /// The stack check against a fake `docker`: live only when every
    /// service runs and every container is healthy or running with no
    /// restarts.
    #[cfg(unix)]
    #[test]
    fn stack_check_script_decides_on_states() {
        use std::os::unix::fs::PermissionsExt as _;
        let run = |services: &str, running: &str, states: &str| {
            let d = tempfile::tempdir().unwrap();
            let fake = d.path().join("docker");
            let (s1, s2) = states.split_once('|').unwrap();
            std::fs::write(
                &fake,
                format!(
                    "#!/bin/sh\ncase \"$*\" in\n  info) exit 0 ;;\n  *config\\ --services*) printf '{services}' ;;\n  *ps\\ --services*) printf '{running}' ;;\n  *ps\\ -q*) echo id1; echo id2 ;;\n  inspect*) case \"$*\" in *id1*) echo '{s1}' ;; *) echo '{s2}' ;; esac ;;\nesac\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            let o = std::process::Command::new("sh")
                .arg("-c")
                .arg(stack_check_script(&compose_cmd("apex", &[])))
                .env("PATH", format!("{}:/usr/bin:/bin", d.path().display()))
                .output()
                .unwrap();
            stack_is_live(&String::from_utf8_lossy(&o.stdout))
        };
        let both = "agent\\nmcp\\n";
        assert!(run(
            both,
            both,
            "/agent running restarts=0|/mcp healthy restarts=0"
        ));
        assert!(!run(
            both,
            "agent\\n",
            "/agent running restarts=0|/mcp healthy restarts=0"
        ));
        assert!(!run(
            both,
            both,
            "/agent running restarts=0|/mcp unhealthy restarts=0"
        ));
        assert!(!run(
            both,
            both,
            "/agent running restarts=1|/mcp healthy restarts=0"
        ));
        assert!(!run(
            both,
            both,
            "/agent running restarts=0|/mcp starting restarts=0"
        ));
    }

    #[test]
    fn packs_the_project_without_git() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("compose.yaml"), "services: {}\n").unwrap();
        std::fs::create_dir_all(d.path().join("mcp/src")).unwrap();
        std::fs::write(d.path().join("mcp/src/server.py"), "print(1)\n").unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::write(d.path().join(".git/HEAD"), "ref\n").unwrap();
        let tgz = pack_project(d.path()).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&tgz[..]));
        let mut names: Vec<String> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["compose.yaml", "mcp", "mcp/src", "mcp/src/server.py"]
        );
    }

    /// Validation runs before any VM: a missing project or tarball is
    /// NotFound, an empty checkpoint InvalidArgument.
    #[test]
    fn validates_before_creating() {
        let d = tempfile::tempdir().unwrap();
        let ok = BakeOptions::new(d.path().display().to_string(), "w1");
        assert!(validate(&ok).is_ok());
        let missing = BakeOptions::new("/nonexistent/cua-bake-project", "w1");
        assert!(matches!(validate(&missing), Err(CuaError::NotFound(_))));
        let mut tarball = ok.clone();
        tarball.image_tarballs = vec!["/nonexistent/image.tar".into()];
        assert!(matches!(validate(&tarball), Err(CuaError::NotFound(_))));
        let empty = BakeOptions::new(d.path().display().to_string(), " ");
        assert!(matches!(
            validate(&empty),
            Err(CuaError::InvalidArgument(_))
        ));
    }
}
