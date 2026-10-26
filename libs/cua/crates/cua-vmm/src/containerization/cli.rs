//! The `container` CLI's arguments and output: pure functions, so they are
//! tested without the CLI.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::types::{StartSpec, Status};

/// Label: a sandbox this backend made.
pub const LABEL_BACKEND: &str = "cua.vmm.backend";
/// Label: the sandbox name.
pub const LABEL_NAME: &str = "cua.sandbox.name";
/// Label: the guest ports the sandbox serves (`3211,8080`), reached at the
/// guest's own address.
pub const LABEL_PORTS: &str = "ai.cua.ports";

/// Whether `name` is a valid container id (`container` limits ids to a DNS
/// label: letters, digits, `_`, `.`, `-`, at most 63, not starting with a
/// separator).
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (2..=63).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `/dev/shm` for a sandbox: half its memory, between 64 MiB and 2 GiB
/// (browsers keep renderer buffers there), as the container backend sizes it.
pub fn shm_mib(memory_mb: u64) -> u64 {
    (memory_mb / 2).clamp(64, 2048)
}

/// `container run` for `spec`: detached, named, sized, labelled, the
/// environment from `env_file` (kept off the command line: it carries the
/// Spaces token), and `command` replacing the image's ENTRYPOINT and CMD as
/// the container backend does. Ports are not published: the host reaches
/// the guest at its own address, as it reaches a Lume VM (`container`'s
/// loopback forwarder runs in a launchd helper that macOS's Local Network
/// privacy keeps from reaching the guest).
///
/// `inject_dir`: the host directory holding the injected cua-spacesd
/// ([`crate::inject`]), bound read-only at [`crate::inject::GUEST_DIR`]; the
/// supervisor it runs starts as root.
pub fn run_args(
    spec: &StartSpec,
    image: &str,
    env_file: Option<&str>,
    labels: &BTreeMap<String, String>,
    inject_dir: Option<&str>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "run".into(),
        "--detach".into(),
        "--progress".into(),
        "none".into(),
        "--name".into(),
        spec.name.clone(),
        "--cpus".into(),
        spec.cpus.max(1).to_string(),
        "--memory".into(),
        format!("{}M", spec.memory_mb.max(256)),
        "--shm-size".into(),
        format!("{}M", shm_mib(spec.memory_mb)),
        // The guest kernel is the Space's own: cua-spacesd mounts Cua
        // Volume (FUSE) with it, as it does under gVisor.
        "--cap-add".into(),
        "CAP_SYS_ADMIN".into(),
    ];
    if let Some(arch) = spec.arch {
        a.push("--arch".into());
        a.push(arch.oci().into());
    }
    for (k, v) in labels {
        a.push("--label".into());
        a.push(format!("{k}={v}"));
    }
    if let Some(f) = env_file {
        a.push("--env-file".into());
        a.push(f.into());
    }
    if let Some(dir) = inject_dir {
        a.push("--user".into());
        a.push("0".into());
        a.push("--mount".into());
        a.push(format!(
            "type=bind,source={dir},target={},readonly",
            crate::inject::GUEST_DIR
        ));
    }
    let command = spec.command.as_ref().filter(|c| !c.is_empty());
    if let Some(c) = command {
        a.push("--entrypoint".into());
        a.push(c[0].clone());
    }
    a.push(image.into());
    if let Some(c) = command {
        a.extend(c[1..].iter().cloned());
    }
    a
}

/// The run config of an image in `container image inspect --format json`
/// output (one image or a list): the config of the variant for `arch`
/// (`arm64`/`amd64`), else of the first one.
pub fn parse_run_config(stdout: &str, arch: &str) -> Option<crate::OciRunConfig> {
    let v: Value = serde_json::from_str(stdout).ok()?;
    let image = match &v {
        Value::Array(a) => a.first()?,
        other => other,
    };
    let variants = image.get("variants")?.as_array()?;
    let pick = variants
        .iter()
        .find(|x| {
            x.pointer("/config/architecture").and_then(Value::as_str) == Some(arch)
                || x.pointer("/platform/architecture").and_then(Value::as_str) == Some(arch)
        })
        .or_else(|| variants.first())?;
    let c = pick.pointer("/config/config")?;
    let strings = |k: &str| -> Vec<String> {
        c.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let text = |k: &str| {
        c.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Some(crate::OciRunConfig {
        entrypoint: strings("Entrypoint"),
        cmd: strings("Cmd"),
        env: strings("Env"),
        working_dir: text("WorkingDir"),
        user: text("User"),
    })
}

/// `name` as a Linux host name: letters, digits and `-`, at most 63
/// characters, never empty.
pub fn hostname(name: &str) -> String {
    let h: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(63)
        .collect();
    let h = h.trim_matches('-');
    if h.is_empty() {
        "space".into()
    } else {
        h.to_string()
    }
}

/// An env file (`KEY=value` per line) for `env`. Values with a newline
/// cannot be expressed in one; they are refused.
pub fn env_file(env: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    for (k, v) in env {
        if k.is_empty() || k.contains('=') || k.contains('\n') {
            return Err(format!("invalid environment variable name {k:?}"));
        }
        if v.contains('\n') {
            return Err(format!(
                "environment variable {k} has a newline, which `container` cannot pass"
            ));
        }
        out.push_str(&format!("{k}={v}\n"));
    }
    Ok(out)
}

/// What `container system status --format json` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Services {
    /// Running, from this application root and install root.
    Running {
        app_root: String,
        install_root: String,
    },
    /// Not running (or not registered with launchd).
    Stopped,
}

/// Parses `container system status --format json` (it exits 1, with
/// `{"status":"not running"}` or `"unregistered"`, when the services are
/// down).
pub fn parse_services(stdout: &str) -> Option<Services> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    match v.get("status")?.as_str()? {
        "running" => {
            let paths = v.get("paths");
            let get = |k: &str| {
                paths
                    .and_then(|p| p.get(k))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Some(Services::Running {
                app_root: get("appRoot"),
                install_root: get("installRoot"),
            })
        }
        _ => Some(Services::Stopped),
    }
}

/// Whether two directory paths name the same directory (ignoring a
/// trailing `/`, which `container` adds).
pub fn same_dir(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_end_matches('/'), b.trim_end_matches('/'));
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

/// One container from `container list --format json` / `inspect`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    /// `running`, `stopped`, `stopping` or `unknown`.
    pub state: String,
    pub image: String,
    pub labels: BTreeMap<String, String>,
    /// Guest port → host port (TCP).
    pub ports: BTreeMap<u16, u16>,
    /// The address published ports listen on (`127.0.0.1`).
    pub host_address: Option<String>,
    /// The guest's own address on the host's network.
    pub ip: Option<String>,
    /// The guest's gateway (the host, as the guest reaches it).
    pub gateway: Option<String>,
    /// The guest ports the sandbox serves ([`LABEL_PORTS`]).
    pub served: Vec<u16>,
}

impl ContainerInfo {
    pub fn status(&self) -> Status {
        match self.state.as_str() {
            "running" => Status::Running,
            "stopped" => Status::Stopped,
            // Stopping: no longer serving, not yet stopped.
            "stopping" => Status::Stopped,
            other => Status::Unknown(other.to_string()),
        }
    }

    /// Made by this backend.
    pub fn ours(&self) -> bool {
        self.labels.get(LABEL_BACKEND).map(String::as_str) == Some("containerization")
    }
}

/// An address as `container` encodes it (a string, or `{"value": ...}`),
/// without a prefix length.
fn address(v: Option<&Value>) -> Option<String> {
    let v = v?;
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o
            .values()
            .find_map(|x| x.as_str().map(str::to_string))
            .unwrap_or_default(),
        _ => return None,
    };
    let s = s.split('/').next().unwrap_or_default().trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn container(v: &Value) -> Option<ContainerInfo> {
    let cfg = v.get("configuration")?;
    let id = cfg.get("id")?.as_str()?.to_string();
    let status = v.get("status");
    let state = status
        .and_then(|s| s.get("state").or(Some(s)))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let labels: BTreeMap<String, String> = cfg
        .get("labels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut ports = BTreeMap::new();
    let mut host_address = None;
    for p in cfg
        .get("publishedPorts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if p.get("proto").and_then(Value::as_str).unwrap_or("tcp") != "tcp" {
            continue;
        }
        let (Some(g), Some(h)) = (
            p.get("containerPort").and_then(Value::as_u64),
            p.get("hostPort").and_then(Value::as_u64),
        ) else {
            continue;
        };
        let count = p.get("count").and_then(Value::as_u64).unwrap_or(1).max(1);
        for i in 0..count {
            if let (Ok(g), Ok(h)) = (u16::try_from(g + i), u16::try_from(h + i)) {
                ports.insert(g, h);
            }
        }
        host_address = host_address.or_else(|| address(p.get("hostAddress")));
    }
    let net = status
        .and_then(|s| s.get("networks"))
        .or_else(|| v.get("networks"))
        .and_then(Value::as_array)
        .and_then(|n| n.first());
    let served = labels
        .get(LABEL_PORTS)
        .map(|p| p.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    Some(ContainerInfo {
        served,
        id,
        state,
        image: cfg
            .get("image")
            .and_then(|i| i.get("reference"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        labels,
        ports,
        host_address,
        ip: net.and_then(|n| address(n.get("ipv4Address").or(n.get("address")))),
        gateway: net.and_then(|n| address(n.get("ipv4Gateway").or(n.get("gateway")))),
    })
}

/// Parses `container list --format json` or `container inspect` output.
pub fn parse_containers(stdout: &str) -> Result<Vec<ContainerInfo>, serde_json::Error> {
    let t = stdout.trim();
    if t.is_empty() {
        return Ok(vec![]);
    }
    let v: Value = serde_json::from_str(t)?;
    Ok(match v {
        Value::Array(items) => items.iter().filter_map(container).collect(),
        other => container(&other).into_iter().collect(),
    })
}

/// Whether a failed command's stderr says the container does not exist.
pub fn not_found(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("not found") || s.contains("notfound") || s.contains("no such container")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ImageSource, StartSpec};

    #[test]
    fn names() {
        assert!(valid_name("space-1.a_b"));
        assert!(!valid_name("-x"));
        assert!(!valid_name("a"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(&"a".repeat(64)));
    }

    #[test]
    fn run_args_mirror_the_container_backend() {
        let mut spec = StartSpec::new("sb1", ImageSource::oci("ghcr.io/trycua/linux:24.04-slim"))
            .cpus(2)
            .memory_mb(4096);
        spec.command = Some(vec!["/bin/sh".into(), "-c".into(), "sleep 1".into()]);
        let labels = BTreeMap::from([("ai.cua.managed".to_string(), "true".to_string())]);
        let a = run_args(
            &spec,
            "ghcr.io/trycua/linux@sha256:ab",
            Some("/tmp/env"),
            &labels,
            None,
        );
        let s = a.join(" ");
        assert!(
            s.starts_with("run --detach --progress none --name sb1 --cpus 2 --memory 4096M"),
            "{s}"
        );
        assert!(s.contains("--shm-size 2048M"), "{s}");
        assert!(s.contains("--label ai.cua.managed=true"), "{s}");
        assert!(s.contains("--env-file /tmp/env"), "{s}");
        assert!(!s.contains("--publish"), "{s}");
        assert!(
            s.ends_with("--entrypoint /bin/sh ghcr.io/trycua/linux@sha256:ab -c sleep 1"),
            "{s}"
        );
        // No command: the image's own ENTRYPOINT/CMD.
        spec.command = Some(vec![]);
        let a = run_args(&spec, "img", None, &BTreeMap::new(), None);
        assert_eq!(a.last().map(String::as_str), Some("img"));
        assert!(!a.contains(&"--entrypoint".to_string()));
        assert!(!a.contains(&"--env-file".to_string()));
        assert!(!a.contains(&"--mount".to_string()));
    }

    #[test]
    fn an_injected_cua_spacesd_is_bound_read_only_and_runs_as_root() {
        let mut spec = StartSpec::new("sb1", ImageSource::oci("alpine:3.20"));
        spec.command = Some(vec![
            crate::inject::GUEST_PATH.into(),
            "supervise".into(),
            "--on-exit".into(),
            "serve".into(),
        ]);
        let a = run_args(&spec, "alpine:3.20", None, &BTreeMap::new(), Some("/h/sd"));
        let s = a.join(" ");
        assert!(s.contains("--user 0"), "{s}");
        assert!(
            s.contains("--mount type=bind,source=/h/sd,target=/.cua/bin,readonly"),
            "{s}"
        );
        assert!(
            s.ends_with("--entrypoint /.cua/bin/cua-spacesd alpine:3.20 supervise --on-exit serve"),
            "{s}"
        );
    }

    #[test]
    fn the_run_config_comes_from_the_variant_of_the_architecture() {
        let out = r#"[{"id":"abc","variants":[
          {"config":{"architecture":"amd64","config":{"Cmd":["x"]}}},
          {"config":{"architecture":"arm64","config":{
            "Entrypoint":["/entry.sh"],"Cmd":["serve"],"User":"app","WorkingDir":"/srv",
            "Env":["A=1"]}}}]}]"#;
        let c = parse_run_config(out, "arm64").unwrap();
        assert_eq!(c.entrypoint, ["/entry.sh"]);
        assert_eq!(c.cmd, ["serve"]);
        assert_eq!(c.user.as_deref(), Some("app"));
        assert_eq!(c.working_dir.as_deref(), Some("/srv"));
        assert_eq!(c.env, ["A=1"]);
        assert_eq!(parse_run_config(out, "amd64").unwrap().cmd, ["x"]);
        assert!(parse_run_config("not json", "arm64").is_none());
        assert!(parse_run_config("[]", "arm64").is_none());
    }

    #[test]
    fn env_files() {
        let env = BTreeMap::from([
            ("CUA_ENV_TOKEN".to_string(), "tok=1 two".to_string()),
            ("A".to_string(), String::new()),
        ]);
        assert_eq!(env_file(&env).unwrap(), "A=\nCUA_ENV_TOKEN=tok=1 two\n");
        let bad = BTreeMap::from([("A".to_string(), "x\ny".to_string())]);
        assert!(env_file(&bad).is_err());
    }

    #[test]
    fn services() {
        let up = r#"{"client":{"version":"1.5.0"},"paths":{"appRoot":"/u/.cua/data/","installRoot":"/u/.cua/install/"},"status":"running"}"#;
        assert_eq!(
            parse_services(up),
            Some(Services::Running {
                app_root: "/u/.cua/data/".into(),
                install_root: "/u/.cua/install/".into()
            })
        );
        assert_eq!(
            parse_services(r#"{"status":"not running"}"#),
            Some(Services::Stopped)
        );
        assert_eq!(
            parse_services(r#"{"status":"unregistered"}"#),
            Some(Services::Stopped)
        );
        assert_eq!(parse_services("apiserver is not running"), None);
        assert!(same_dir("/u/.cua/data/", "/u/.cua/data"));
        assert!(!same_dir("/u/.cua/data", "/u/other"));
    }

    #[test]
    fn containers() {
        let ls = r#"[{"configuration":{"id":"sb1","image":{"reference":"ghcr.io/trycua/linux:24.04-slim"},
            "labels":{"cua.vmm.backend":"containerization","ai.cua.managed":"true","ai.cua.ports":"3211,8080"},
            "publishedPorts":[{"hostAddress":"127.0.0.1","hostPort":40001,"containerPort":3211,"proto":"tcp","count":1},
                              {"hostAddress":"127.0.0.1","hostPort":40002,"containerPort":53,"proto":"udp"}]},
            "status":{"state":"running","networks":[{"network":"default","hostname":"sb1",
              "ipv4Address":"192.168.64.5/24","ipv4Gateway":"192.168.64.1"}]}},
           {"configuration":{"id":"theirs","labels":{}},"status":{"state":"stopped","networks":[]}}]"#;
        let c = parse_containers(ls).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, "sb1");
        assert!(c[0].ours());
        assert_eq!(c[0].status(), Status::Running);
        assert_eq!(c[0].ports, BTreeMap::from([(3211, 40001)]));
        assert_eq!(c[0].host_address.as_deref(), Some("127.0.0.1"));
        assert_eq!(c[0].ip.as_deref(), Some("192.168.64.5"));
        assert_eq!(c[0].gateway.as_deref(), Some("192.168.64.1"));
        assert_eq!(c[0].served, [3211, 8080]);
        assert!(!c[1].ours());
        assert_eq!(c[1].status(), Status::Stopped);
        assert!(parse_containers("").unwrap().is_empty());
        assert!(parse_containers("[]").unwrap().is_empty());
        assert!(not_found("Error: container not found: sb9"));
    }
}
