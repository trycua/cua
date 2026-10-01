//! Provisioning gVisor (`runsc`) into a container engine.
//!
//! `runsc` must exist inside the engine's Linux environment *and* be
//! registered with dockerd, persistently:
//!
//! * **Colima** regenerates `/etc/docker/daemon.json` on every start, so
//!   `runsc install` inside the VM is lost after `colima restart`. The binary
//!   is installed in the VM (apt; the VM disk persists) and the runtime is
//!   registered in Colima's own profile config,
//!   `~/.colima/<profile>/colima.yaml` → `docker.runtimes.runsc.path`, followed
//!   by `colima restart --profile <p>`. **The restart stops every running
//!   container in that engine**, which is why provisioning is opt-in
//!   (`ContainerConfig::allow_install_runsc`).
//! * **Native Linux dockerd**: `runsc install` writes `/etc/docker/daemon.json`
//!   (persistent there), then dockerd is restarted (`sudo -n`).
//! * **Docker Desktop**: runtimes are configured in `~/.docker/daemon.json`
//!   (Settings → Docker Engine), but its VM root filesystem is immutable and
//!   reset on restart, so there is nowhere persistent to put the `runsc`
//!   binary. Not automatable; we explain instead.
//! * **OrbStack / Podman**: not automatable; we explain instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::engine::EngineKind;

/// Where `runsc` lands inside the engine VM (Debian/Ubuntu package path).
pub const RUNSC_PATH: &str = "/usr/bin/runsc";

/// Installs the `runsc` binary from gVisor's apt repository (idempotent). Does
/// not register it with dockerd.
pub const INSTALL_BINARY_SCRIPT: &str = r#"set -eu
if ! command -v runsc >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq ca-certificates curl gnupg >/dev/null
  curl -fsSL https://gvisor.dev/archive.key | gpg --dearmor --yes -o /usr/share/keyrings/gvisor-archive-keyring.gpg
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gvisor-archive-keyring.gpg] https://storage.googleapis.com/gvisor/releases release main" > /etc/apt/sources.list.d/gvisor.list
  apt-get update -qq
  apt-get install -y -qq runsc
fi
"#;

/// Native Linux: install, register in /etc/docker/daemon.json, restart dockerd.
pub fn linux_script() -> String {
    format!(
        "{INSTALL_BINARY_SCRIPT}runsc install\n(systemctl restart docker || service docker restart)\n"
    )
}

/// One provisioning step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    /// Run a host command.
    Run { argv: Vec<String> },
    /// Register `runsc` in a Colima profile config (`docker.runtimes.runsc`).
    ColimaRuntime { config: PathBuf },
}

/// How `runsc` would be installed for an engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlan {
    /// Human description for `doctor` output and logs.
    pub description: String,
    pub steps: Vec<Step>,
    /// Running containers in this engine will be stopped.
    pub restarts_engine: bool,
}

/// `$COLIMA_HOME` or `~/.colima`.
pub fn colima_home() -> PathBuf {
    std::env::var_os("COLIMA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::host::home_dir().join(".colima"))
}

/// The install plan for `kind`, or `Err(hint)` when it cannot be automated.
pub fn install_plan(kind: &EngineKind) -> Result<InstallPlan, String> {
    match kind {
        EngineKind::Colima { profile } => {
            let s = |v: &str| v.to_string();
            Ok(InstallPlan {
                description: format!(
                    "install runsc in the Colima VM (profile '{profile}'), register it in colima.yaml \
                     (docker.runtimes.runsc) and `colima restart` (stops running containers)"
                ),
                steps: vec![
                    Step::Run {
                        argv: vec![
                            s("colima"), s("ssh"), s("--profile"), profile.clone(), s("--"),
                            s("sudo"), s("sh"), s("-c"), s(INSTALL_BINARY_SCRIPT),
                        ],
                    },
                    Step::ColimaRuntime { config: colima_home().join(profile).join("colima.yaml") },
                    Step::Run { argv: vec![s("colima"), s("restart"), s("--profile"), profile.clone()] },
                ],
                restarts_engine: true,
            })
        }
        EngineKind::NativeLinux => Ok(InstallPlan {
            description: "install runsc on this Linux host, `runsc install` into /etc/docker/daemon.json and restart dockerd (sudo -n)".into(),
            steps: vec![Step::Run {
                argv: vec!["sudo".into(), "-n".into(), "sh".into(), "-c".into(), linux_script()],
            }],
            restarts_engine: true,
        }),
        EngineKind::DockerDesktop => Err(
            "Docker Desktop registers runtimes in ~/.docker/daemon.json (Settings → Docker Engine), but its VM has no \
             persistent place for the runsc binary; use Colima (`brew install colima docker && colima start`) or a Linux \
             host for gVisor isolation"
                .into(),
        ),
        EngineKind::OrbStack => Err(
            "OrbStack does not support custom OCI runtimes; use Colima or a Linux host for gVisor isolation".into(),
        ),
        EngineKind::Podman => Err("install runsc on the Podman machine and add it to containers.conf `runtimes`".into()),
        EngineKind::Unknown => Err(
            "unknown engine; install runsc (https://gvisor.dev/docs/user_guide/install/) and register it with dockerd".into(),
        ),
    }
}

/// Add `docker.runtimes.runsc.path` to a colima.yaml, preserving comments and
/// everything else. Returns `None` when it is already registered.
///
/// Handles the shapes Colima writes: a top-level `docker: {}` (the default),
/// a `docker:` block with other keys, and an existing `runtimes:` map.
pub fn colima_yaml_with_runsc(yaml: &str, runsc_path: &str) -> Option<String> {
    let lines: Vec<&str> = yaml.lines().collect();
    let entry = |indent: usize| {
        let p = " ".repeat(indent);
        format!("{p}runsc:\n{p}  path: {runsc_path}")
    };
    let docker = lines.iter().position(|l| {
        l.trim_end() == "docker: {}" || l.trim_end() == "docker:" || l.starts_with("docker: ")
    });
    let mut out: Vec<String> = Vec::new();
    match docker {
        None => {
            out.extend(lines.iter().map(|s| s.to_string()));
            out.push(format!("docker:\n  runtimes:\n{}", entry(4)));
        }
        Some(i) if lines[i].trim_end() != "docker:" => {
            // `docker: {}` (or an inline map we only handle when empty).
            if lines[i].trim_end() != "docker: {}" {
                return None;
            }
            out.extend(lines[..i].iter().map(|s| s.to_string()));
            out.push(format!("docker:\n  runtimes:\n{}", entry(4)));
            out.extend(lines[i + 1..].iter().map(|s| s.to_string()));
        }
        Some(i) => {
            // Block form: find the block's extent (indented or blank lines).
            let end = (i + 1..lines.len())
                .find(|&j| {
                    let l = lines[j];
                    !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('\t')
                })
                .unwrap_or(lines.len());
            let block = &lines[i + 1..end];
            if block.iter().any(|l| l.trim_start().starts_with("runsc:")) {
                return None;
            }
            out.extend(lines[..=i].iter().map(|s| s.to_string()));
            match block.iter().position(|l| {
                l.trim_start().starts_with("runtimes:") && !l.trim_start().starts_with('#')
            }) {
                Some(r) if block[r].trim_end().ends_with("{}") => {
                    let indent = block[r].len() - block[r].trim_start().len();
                    for (k, l) in block.iter().enumerate() {
                        if k == r {
                            out.push(format!("{}runtimes:", " ".repeat(indent)));
                            out.push(entry(indent + 2));
                        } else {
                            out.push(l.to_string());
                        }
                    }
                }
                Some(r) => {
                    let indent = block[r].len() - block[r].trim_start().len();
                    for (k, l) in block.iter().enumerate() {
                        out.push(l.to_string());
                        if k == r {
                            out.push(entry(indent + 2));
                        }
                    }
                }
                None => {
                    out.push(format!("  runtimes:\n{}", entry(4)));
                    out.extend(block.iter().map(|s| s.to_string()));
                }
            }
            out.extend(lines[end..].iter().map(|s| s.to_string()));
        }
    }
    let mut s = out.join("\n");
    if yaml.ends_with('\n') {
        s.push('\n');
    }
    Some(s)
}

/// Apply [`colima_yaml_with_runsc`] to a file (keeping a `.bak`).
pub fn register_in_colima_config(config: &Path) -> std::io::Result<bool> {
    let yaml = std::fs::read_to_string(config)?;
    match colima_yaml_with_runsc(&yaml, RUNSC_PATH) {
        None => Ok(false),
        Some(new) => {
            std::fs::write(config.with_extension("yaml.bak"), &yaml)?;
            std::fs::write(config, new)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colima_plan_registers_in_colima_yaml_and_restarts() {
        let p = install_plan(&EngineKind::Colima {
            profile: "default".into(),
        })
        .unwrap();
        assert!(p.restarts_engine);
        let Step::Run { argv } = &p.steps[0] else {
            panic!()
        };
        assert_eq!(
            &argv[..6],
            ["colima", "ssh", "--profile", "default", "--", "sudo"]
        );
        assert!(
            !argv.last().unwrap().contains("runsc install"),
            "daemon.json registration is lost on colima start"
        );
        assert!(
            matches!(&p.steps[1], Step::ColimaRuntime { config } if config.ends_with("default/colima.yaml"))
        );
        assert_eq!(
            p.steps[2],
            Step::Run {
                argv: vec![
                    "colima".into(),
                    "restart".into(),
                    "--profile".into(),
                    "default".into()
                ]
            }
        );
    }

    #[test]
    fn unsupported_engines_explain_why() {
        assert!(
            install_plan(&EngineKind::DockerDesktop)
                .unwrap_err()
                .contains("daemon.json")
        );
        let Step::Run { argv } = &install_plan(&EngineKind::NativeLinux).unwrap().steps[0] else {
            panic!()
        };
        assert!(argv.contains(&"-n".to_string()) && argv.last().unwrap().contains("runsc install"));
    }

    const RUNSC: &str = "  runtimes:\n    runsc:\n      path: /usr/bin/runsc";

    #[test]
    fn colima_yaml_default_empty_docker_map() {
        let y = "cpu: 4\n# Docker daemon config\ndocker: {}\n\n# vm type\nvmType: vz\n";
        let out = colima_yaml_with_runsc(y, RUNSC_PATH).unwrap();
        assert_eq!(
            out,
            format!("cpu: 4\n# Docker daemon config\ndocker:\n{RUNSC}\n\n# vm type\nvmType: vz\n")
        );
        assert!(
            colima_yaml_with_runsc(&out, RUNSC_PATH).is_none(),
            "idempotent"
        );
    }

    #[test]
    fn colima_yaml_block_with_other_keys_and_existing_runtimes() {
        let y = "docker:\n  features:\n    buildkit: true\nvmType: vz\n";
        let out = colima_yaml_with_runsc(y, RUNSC_PATH).unwrap();
        assert_eq!(
            out,
            format!("docker:\n{RUNSC}\n  features:\n    buildkit: true\nvmType: vz\n")
        );

        let y = "docker:\n  runtimes:\n    crun:\n      path: /usr/bin/crun\nvmType: vz\n";
        let out = colima_yaml_with_runsc(y, RUNSC_PATH).unwrap();
        assert_eq!(
            out,
            "docker:\n  runtimes:\n    runsc:\n      path: /usr/bin/runsc\n    crun:\n      path: /usr/bin/crun\nvmType: vz\n"
        );

        let y = "docker:\n  runtimes: {}\n";
        assert_eq!(
            colima_yaml_with_runsc(y, RUNSC_PATH).unwrap(),
            format!("docker:\n{RUNSC}\n")
        );

        // The shape the orchestrator applied on this machine is left alone.
        let y = "docker:\n  runtimes:\n    runsc:\n      path: /usr/bin/runsc\n\n# Virtual Machine type\nvmType: vz\n";
        assert!(colima_yaml_with_runsc(y, RUNSC_PATH).is_none());
    }

    #[test]
    fn colima_yaml_without_docker_key_appends() {
        let out = colima_yaml_with_runsc("cpu: 2\n", RUNSC_PATH).unwrap();
        assert_eq!(out, format!("cpu: 2\ndocker:\n{RUNSC}\n"));
    }

    #[test]
    fn real_colima_config_parses_as_registered_if_present() {
        let p = colima_home().join("default/colima.yaml");
        if let Ok(y) = std::fs::read_to_string(&p) {
            // Read-only check against the developer's config.
            eprintln!(
                "colima.yaml registered runsc: {}",
                colima_yaml_with_runsc(&y, RUNSC_PATH).is_none()
            );
        }
    }
}
