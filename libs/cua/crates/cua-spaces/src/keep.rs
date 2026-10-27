//! Keep running: commands a Space's cua-spacesd keeps up (services).
//!
//! A service is started at once, started again by its restart policy (with
//! a backoff) when it exits or fails its health check, started when the
//! Space boots, and its output is kept in log files in the Space
//! (`ProcessService.PutService` and the rest). This module is the host's
//! side: plain types for the CLI, the SDKs and the MCP tool.

use std::collections::BTreeMap;
use std::time::Duration;

use cua_spacesd_client::pb;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::space::Space;

/// The guest feature.
pub const SERVICES_FEATURE: &str = "process.services";

/// When a service is started again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    /// Whenever it ends.
    #[default]
    Always,
    /// When it fails (non-zero exit, a signal, a failed health check).
    OnFailure,
    /// Never: it runs once.
    Never,
}

impl Restart {
    /// Parses `always`, `on-failure` or `never`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "" | "always" => Self::Always,
            "on-failure" | "on_failure" => Self::OnFailure,
            "never" | "no" => Self::Never,
            _ => return None,
        })
    }

    fn pb(self) -> pb::RestartPolicy {
        match self {
            Self::Always => pb::RestartPolicy::Always,
            Self::OnFailure => pb::RestartPolicy::OnFailure,
            Self::Never => pb::RestartPolicy::Never,
        }
    }
}

/// A health check: exactly one of `http`, `tcp`, `command`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    /// A URL that answers 2xx or 3xx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<String>,
    /// `host:port` that accepts a TCP connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp: Option<String>,
    /// A shell command that exits 0 when healthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Seconds between checks (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
    /// Failed checks in a row that restart it (default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<u32>,
}

/// A service to keep running.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceDef {
    /// Name.
    pub name: String,
    /// The shell command line it runs.
    pub command: String,
    /// Extra environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// When it is started again.
    #[serde(default)]
    pub restart: Restart,
    /// Health check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    /// Start it when the Space boots (default true).
    #[serde(default = "yes")]
    pub start_on_boot: bool,
}

fn yes() -> bool {
    true
}

/// A service and how it is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Name.
    pub name: String,
    /// The command line.
    pub command: String,
    /// `starting`, `running`, `unhealthy`, `backoff`, `stopped`, `exited`
    /// or `failed`.
    pub state: String,
    /// Its process, while it runs.
    pub pid: u32,
    /// Starts after the first.
    pub restarts: u32,
    /// How the last process ended (`exited 3`, `signal 9`, ...), if one did.
    pub last_end: String,
    /// The last failed health check, if the last one failed.
    pub probe_error: String,
    /// `always`, `on-failure` or `never`.
    pub restart: String,
    /// Starts when the Space boots.
    pub start_on_boot: bool,
    /// Its log file in the Space.
    pub log_path: String,
}

fn secs(n: u64) -> Option<cua_proto::wkt::Duration> {
    Some(cua_proto::wkt::Duration {
        seconds: n as i64,
        nanos: 0,
    })
}

impl ServiceDef {
    fn spec(&self, windows: bool) -> Result<pb::ServiceSpec> {
        if self.command.trim().is_empty() {
            return Err(Error::InvalidArgument("a service needs a command".into()));
        }
        let shell = |line: &str| {
            if windows {
                pb::ProcessConfig {
                    command: "cmd.exe".into(),
                    args: vec!["/C".into(), line.into()],
                    ..Default::default()
                }
            } else {
                pb::ProcessConfig {
                    command: "/bin/sh".into(),
                    args: vec!["-c".into(), line.into()],
                    ..Default::default()
                }
            }
        };
        let mut config = shell(&self.command);
        config.env = self.env.clone().into_iter().collect();
        config.cwd = self.cwd.clone().unwrap_or_default();
        let probe = match &self.probe {
            None => None,
            Some(p) => {
                let set = [p.http.is_some(), p.tcp.is_some(), p.command.is_some()]
                    .iter()
                    .filter(|x| **x)
                    .count();
                if set != 1 {
                    return Err(Error::InvalidArgument(
                        "a health check checks one thing: http, tcp or command".into(),
                    ));
                }
                Some(pb::HealthProbe {
                    exec: p.command.as_deref().map(shell),
                    http: p.http.clone().unwrap_or_default(),
                    tcp: p.tcp.clone().unwrap_or_default(),
                    interval: p.interval_secs.and_then(secs),
                    failure_threshold: p.failures.unwrap_or(0),
                    ..Default::default()
                })
            }
        };
        Ok(pb::ServiceSpec {
            name: self.name.clone(),
            config: Some(config),
            restart: self.restart.pb() as i32,
            probe,
            start_on_boot: Some(self.start_on_boot),
            ..Default::default()
        })
    }
}

fn line_of(spec: &pb::ServiceSpec) -> String {
    spec.config
        .as_ref()
        .map(|c| match c.args.as_slice() {
            [flag, line] if flag == "-c" || flag == "/C" => line.clone(),
            args => std::iter::once(c.command.clone())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
        })
        .unwrap_or_default()
}

fn info(s: pb::ServiceStatus) -> ServiceInfo {
    let spec = s.spec.unwrap_or_default();
    let state = match pb::ServiceState::try_from(s.state).unwrap_or_default() {
        pb::ServiceState::Starting => "starting",
        pb::ServiceState::Running => "running",
        pb::ServiceState::Unhealthy => "unhealthy",
        pb::ServiceState::Backoff => "backoff",
        pb::ServiceState::Stopped => "stopped",
        pb::ServiceState::Exited => "exited",
        pb::ServiceState::Failed => "failed",
        pb::ServiceState::Unspecified => "unknown",
    };
    let last_end = s
        .last_end
        .map(|e| {
            if !e.error.is_empty() {
                e.error
            } else if let Some(code) = e.exit_code {
                format!("exited {code}")
            } else {
                format!("signal {}", e.signal_number)
            }
        })
        .unwrap_or_default();
    ServiceInfo {
        command: line_of(&spec),
        restart: match pb::RestartPolicy::try_from(spec.restart).unwrap_or_default() {
            pb::RestartPolicy::OnFailure => "on-failure",
            pb::RestartPolicy::Never => "never",
            _ => "always",
        }
        .into(),
        start_on_boot: spec.start_on_boot.unwrap_or(true),
        name: spec.name,
        state: state.into(),
        pid: s.pid,
        restarts: s.restarts,
        last_end,
        probe_error: s.probe_error,
        log_path: s.log_path,
    }
}

/// What [`Space::control_service`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceControl {
    /// Start it.
    Start,
    /// Stop it and keep it stopped.
    Stop,
    /// Stop, then start.
    Restart,
}

impl Space {
    /// Keeps `def` running in this Space (replacing a service of that name).
    pub async fn put_service(&self, def: &ServiceDef) -> Result<ServiceInfo> {
        self.require(SERVICES_FEATURE)?;
        let r = self
            .spacesd()?
            .process()
            .put_service(pb::PutServiceRequest {
                spec: Some(def.spec(self.is_windows())?),
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        Ok(info(r.service.unwrap_or_default()))
    }

    /// This Space's services.
    pub async fn services(&self) -> Result<Vec<ServiceInfo>> {
        self.require(SERVICES_FEATURE)?;
        let r = self
            .spacesd()?
            .process()
            .list_services(pb::ListServicesRequest {})
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        Ok(r.services.into_iter().map(info).collect())
    }

    /// Starts, stops or restarts a service.
    pub async fn control_service(&self, name: &str, what: ServiceControl) -> Result<ServiceInfo> {
        self.require(SERVICES_FEATURE)?;
        let action = match what {
            ServiceControl::Start => pb::ServiceAction::Start,
            ServiceControl::Stop => pb::ServiceAction::Stop,
            ServiceControl::Restart => pb::ServiceAction::Restart,
        };
        let r = self
            .spacesd()?
            .process()
            .control_service(pb::ControlServiceRequest {
                name: name.into(),
                action: action as i32,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        Ok(info(r.service.unwrap_or_default()))
    }

    /// Stops a service and forgets it.
    pub async fn delete_service(&self, name: &str) -> Result<()> {
        self.require(SERVICES_FEATURE)?;
        self.spacesd()?
            .process()
            .delete_service(pb::DeleteServiceRequest { name: name.into() })
            .await
            .map_err(cua_spacesd_client::Error::from)?;
        Ok(())
    }

    /// The end of a service's log (`tail_bytes` 0: 64 KiB).
    pub async fn service_logs(&self, name: &str, tail_bytes: u64) -> Result<String> {
        self.require(SERVICES_FEATURE)?;
        let r = tokio::time::timeout(
            Duration::from_secs(60),
            self.spacesd()?
                .process()
                .get_service_logs(pb::GetServiceLogsRequest {
                    name: name.into(),
                    tail_bytes,
                }),
        )
        .await
        .map_err(|_| Error::Timeout("service logs".into()))?
        .map_err(cua_spacesd_client::Error::from)?
        .into_inner();
        Ok(String::from_utf8_lossy(&r.data).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_def_becomes_a_spec_and_back() {
        let def = ServiceDef {
            name: "web".into(),
            command: "python3 -m http.server 8080".into(),
            restart: Restart::OnFailure,
            probe: Some(Probe {
                http: Some("http://127.0.0.1:8080/".into()),
                failures: Some(2),
                ..Default::default()
            }),
            start_on_boot: true,
            ..Default::default()
        };
        let spec = def.spec(false).unwrap();
        assert_eq!(spec.config.as_ref().unwrap().command, "/bin/sh");
        assert_eq!(spec.restart, pb::RestartPolicy::OnFailure as i32);
        let back = info(pb::ServiceStatus {
            spec: Some(spec),
            state: pb::ServiceState::Running as i32,
            ..Default::default()
        });
        assert_eq!(back.command, def.command);
        assert_eq!(
            (back.state.as_str(), back.restart.as_str()),
            ("running", "on-failure")
        );
        let mut two = def.clone();
        two.probe.as_mut().unwrap().tcp = Some("127.0.0.1:1".into());
        assert!(two.spec(false).is_err());
        assert_eq!(Restart::parse("never"), Some(Restart::Never));
        assert_eq!(Restart::parse("sometimes"), None);
    }
}
