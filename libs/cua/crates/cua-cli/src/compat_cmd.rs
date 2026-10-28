//! `cua compat`: run another provider's SDK, unchanged, on Cua Spaces
//! (cua-compat). `serve` runs the gateway; `env` prints what a client
//! exports to use it.

use clap::Subcommand;
use cua_compat::backend::SpaceDefaults;
use cua_compat::{Gateway, GatewayOptions, Provider};
use cua_sdk::CuaError;
use std::io::Write;
use std::net::SocketAddr;

#[derive(Subcommand, Debug, Clone)]
pub enum CompatCmd {
    /// Serve a provider's wire protocol from Cua Spaces until stopped.
    #[command(after_help = "Examples:
  cua compat serve --provider e2b
  cua compat serve --provider daytona --on host:mac-studio --kind vm
  cua compat serve --provider modal
  cua compat serve --provider e2b --overlay cua-spacesd=./target/debug/cua-spacesd")]
    Serve {
        #[command(flatten)]
        opts: CompatOpts,
    },
    /// Print the shell exports for a client of the gateway (`eval` them).
    #[command(after_help = "Examples:
  cua compat env --provider e2b
  cua compat env --provider daytona")]
    Env {
        /// Provider whose SDK the client uses (`e2b`, `daytona`, `modal`).
        #[arg(long, default_value = "e2b")]
        provider: String,
        /// API listen address of the running gateway.
        #[arg(long)]
        listen: Option<SocketAddr>,
        /// Host-router port of the running gateway.
        #[arg(long)]
        host_port: Option<u16>,
    },
}

#[derive(clap::Args, Debug, Clone)]
pub struct CompatOpts {
    /// Provider whose SDK clients use (`e2b`, `daytona`, `modal`).
    #[arg(long, default_value = "e2b")]
    pub provider: String,
    /// API (and envd) listen address. Loopback by default.
    #[arg(long)]
    pub listen: Option<SocketAddr>,
    /// Port of the `<port>-<id>.<domain>` host router (TLS and plain HTTP).
    #[arg(long)]
    pub host_port: Option<u16>,
    /// Where new Spaces run (default: the core default, this machine; `auto`:
    /// least-loaded machine of your pool; `host:<machine>`).
    #[arg(long)]
    pub on: Option<String>,
    /// Space kind (`vm`: Linux on QEMU/HVF or KVM; `container`; `microvm`).
    /// Default: `vm` where this machine can run one, else `container`.
    #[arg(long)]
    pub kind: Option<String>,
    /// Image for the provider's base template (default: the canonical
    /// Linux image).
    #[arg(long)]
    pub image: Option<String>,
    /// vCPUs per Space.
    #[arg(long)]
    pub cpus: Option<u32>,
    /// Memory per Space (MiB).
    #[arg(long)]
    pub memory_mb: Option<u64>,
    /// Inject a freshly built binary into every new sandbox,
    /// `NAME=PATH[:GUEST_PATH]` (as `cua sandbox create --overlay`), for
    /// trying a guest agent that no image carries yet. Repeatable.
    #[arg(long = "overlay", value_name = "NAME=PATH[:GUEST_PATH]", value_parser = parse_overlay)]
    pub overlays: Vec<cua_sdk::Overlay>,
    /// The guest user for the provider's default user (default: the
    /// provider's own: `user` for E2B, `daytona` for Daytona, `root` for
    /// Modal).
    #[arg(long)]
    pub default_user: Option<String>,
    /// Do not make a first sandbox before serving. By default `serve` pulls
    /// the image and boots one default sandbox (then deletes it) and only
    /// then prints the `Clients export` line, so the first create of a
    /// separate client program cannot outlast the SDK's request timeout.
    #[arg(long)]
    pub no_warm: bool,
    /// Do not create the default user in images that lack it.
    #[arg(long)]
    pub no_create_user: bool,
}

fn parse_overlay(s: &str) -> Result<cua_sdk::Overlay, String> {
    cua_sdk::Overlay::parse(s).map_err(|e| e.to_string())
}

fn provider(name: &str) -> Result<Provider, CuaError> {
    Provider::parse(name).ok_or_else(|| {
        CuaError::InvalidArgument(format!(
            "unknown provider {name:?} (supported: e2b, daytona, modal)"
        ))
    })
}

fn options(o: &CompatOpts) -> Result<GatewayOptions, CuaError> {
    let mut g = GatewayOptions::new(provider(&o.provider)?);
    if let Some(l) = o.listen {
        g.listen = l;
    }
    if let Some(p) = o.host_port {
        g.host_port = p;
    }
    let d = g.defaults.clone();
    g.defaults = SpaceDefaults {
        on: o.on.clone().unwrap_or(d.on),
        kind: o.kind.clone().unwrap_or(d.kind),
        image: o.image.clone().unwrap_or_default(),
        cpus: o.cpus.or(d.cpus),
        memory_mb: o.memory_mb.or(d.memory_mb),
        overlays: o.overlays.clone(),
    };
    if let Some(u) = &o.default_user {
        g.default_user = u.clone();
    }
    g.create_default_user = !o.no_create_user;
    Ok(g)
}

fn exports(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(k, v)| format!("export {k}={}", shell_quote(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn shell_quote(v: &str) -> String {
    if v.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c))
    {
        v.to_string()
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

pub async fn run(cmd: CompatCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    match cmd {
        CompatCmd::Serve { opts } => {
            let g = options(&opts)?;
            let gw = if opts.no_warm {
                Gateway::start(g, None).await
            } else {
                eprintln!(
                    "{}",
                    crate::util::hint(
                        "cua compat: warming (image pull and first boot; the first create is made now)..."
                    )
                );
                Gateway::start_warmed(g, None).await
            }
            .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
            let env = gw.env();
            if json {
                let m: serde_json::Map<String, serde_json::Value> = env
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                crate::util::line(out, serde_json::Value::Object(m).to_string());
            } else {
                crate::util::hint_line(
                    out,
                    format!(
                        "cua compat: serving the {} API on http://{} and sandbox hosts on {} (Spaces: --on {} --kind {}).\nClients export:\n{}",
                        gw.provider.name(),
                        gw.api_addr,
                        gw.domain,
                        opts.on.as_deref().unwrap_or("the default"),
                        opts.kind
                            .as_deref()
                            .unwrap_or(cua_compat::backend::default_kind()),
                        exports(&env)
                    ),
                );
            }
            let _ = out.flush();
            tokio::select! {
                _ = gw.wait() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            Ok(0)
        }
        CompatCmd::Env {
            provider: p,
            listen,
            host_port,
        } => {
            let mut g = GatewayOptions::new(provider(&p)?);
            if let Some(l) = listen {
                g.listen = l;
            }
            if let Some(h) = host_port {
                g.host_port = h;
            }
            let dir = g.dir();
            let keys = cua_compat::auth::Keys::load_or_create(
                &dir.join("keys.json"),
                g.provider.key_prefix(),
            )
            .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
            let env = cua_compat::client_env(
                g.provider,
                g.listen,
                &keys.keys[0].key,
                &g.client_domain(g.host_port),
                &cua_compat::tls::TlsFiles::new(dir.join("tls")).bundle(),
            );
            if json {
                let m: serde_json::Map<String, serde_json::Value> = env
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                crate::util::line(out, serde_json::Value::Object(m).to_string());
            } else {
                crate::util::line(out, exports(&env));
            }
            Ok(0)
        }
    }
}

/// Job services for `cua run --compat <provider>`: one cua-compat gateway
/// per provider in this daemon, started on first use and shared by every
/// member that asks for it. A member reaches it on its own loopback (see
/// `cua_spaces::job_services`).
pub struct CompatJobServices {
    /// By provider and the Space kind the job asked for (`--kind`).
    gateways: tokio::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Gateway>>>,
}

/// The Space kind a job asked for (`vm`, `container`, `microvm`), or empty
/// (`auto`: the host's default).
pub fn job_kind(job: &cua_cluster::JobSpec) -> String {
    match job.kind.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => String::new(),
        k @ ("vm" | "container" | "microvm") => k.to_string(),
        _ => String::new(),
    }
}

/// Registers [`CompatJobServices`] for this process (the daemon).
pub fn register_job_services() {
    cua_spaces::job_services::register(std::sync::Arc::new(CompatJobServices {
        gateways: Default::default(),
    }));
}

impl CompatJobServices {
    /// The gateway for `service` and the job's kind: started (and warmed)
    /// by the first caller, shared by the rest, which wait for it.
    async fn gateway(
        &self,
        service: &str,
        job: &cua_cluster::JobSpec,
    ) -> Result<(Provider, std::sync::Arc<Gateway>), String> {
        let provider = service
            .strip_prefix("compat:")
            .and_then(Provider::parse)
            .ok_or_else(|| {
                format!(
                    "unknown job service {service:?} (compat:e2b, compat:daytona, compat:modal)"
                )
            })?;
        let kind = job_kind(job);
        let key = format!("{}/{kind}", provider.name());
        let mut gws = self.gateways.lock().await;
        if let Some(g) = gws.get(&key) {
            return Ok((provider, g.clone()));
        }
        let mut o = GatewayOptions::new(provider);
        o.listen = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        o.host_port = 0;
        // The sandboxes follow the job's `--kind` (a container job gets
        // container sandboxes, not VMs the host's memory limit then
        // refuses); a gateway per kind keeps its own state.
        o.defaults.kind = kind.clone();
        o.dir = Some(cua_compat::auth::provider_dir(&if kind.is_empty() {
            format!("{}-jobs", provider.name())
        } else {
            format!("{}-jobs-{kind}", provider.name())
        }));
        let g = std::sync::Arc::new(
            Gateway::start(o, None)
                .await
                .map_err(|e| format!("starting the {} gateway: {e}", provider.name()))?,
        );
        // The first create of a fresh home pulls the image and boots (over
        // a minute on a VM host), longer than an SDK waits: do it before
        // the program starts.
        if let Err(e) = g.warm().await {
            eprintln!("compat: warming the {} gateway: {e}", provider.name());
        }
        gws.insert(key, g.clone());
        Ok((provider, g))
    }
}

#[async_trait::async_trait]
impl cua_spaces::job_services::JobServices for CompatJobServices {
    async fn prepare(&self, service: &str, job: &cua_cluster::JobSpec) {
        // Errors surface in `start`, which asks again.
        let _ = self.gateway(service, job).await;
    }

    async fn start(
        &self,
        service: &str,
        job: &cua_cluster::JobSpec,
    ) -> Result<cua_spaces::job_services::ServiceGrant, String> {
        let (provider, gw) = self.gateway(service, job).await?;
        let cert = format!(".cua-services/compat-{}.pem", provider.name());
        let bundle =
            std::fs::read(&gw.bundle).map_err(|e| format!("{}: {e}", gw.bundle.display()))?;
        let mut env: std::collections::BTreeMap<String, String> = gw.env().into_iter().collect();
        // Every common HTTP stack trusts the gateway's CA (and the public
        // roots in the same bundle): Python ssl/httpx/aiohttp, requests,
        // curl, Node, gRPC.
        for k in [
            "SSL_CERT_FILE",
            "REQUESTS_CA_BUNDLE",
            "CURL_CA_BUNDLE",
            "NODE_EXTRA_CA_CERTS",
            "GRPC_DEFAULT_SSL_ROOTS_FILE_PATH",
        ] {
            env.insert(k.into(), format!("{{work}}/{cert}"));
        }
        let mut ports = vec![gw.api_addr.port()];
        ports.extend(gw.host_addrs.iter().map(|a| a.port()));
        ports.sort_unstable();
        ports.dedup();
        Ok(cua_spaces::job_services::ServiceGrant {
            ports,
            env,
            files: vec![(cert, bundle)],
            guard: Box::new(gw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(
            shell_quote("http://127.0.0.1:49982"),
            "http://127.0.0.1:49982"
        );
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn a_job_kind_reaches_the_gateways_sandboxes() {
        let mut j: cua_cluster::JobSpec = serde_json::from_value(serde_json::json!({
            "job_id": "j", "attempt": 0, "image": "", "kind": "container", "runtime": "",
            "millicpus": 0, "memory_mb": 0, "argv": [], "timeout_secs": 1, "max_output_bytes": 1
        }))
        .unwrap();
        assert_eq!(job_kind(&j), "container");
        j.kind = "AUTO".into();
        assert_eq!(job_kind(&j), "");
        j.kind = "machine".into();
        assert_eq!(job_kind(&j), "");
        j.kind = "vm".into();
        assert_eq!(job_kind(&j), "vm");
    }

    #[test]
    fn the_gateway_kind_follows_the_flag_then_the_target() {
        use cua_compat::backend::kind_on;
        // An explicit kind is honoured wherever it runs (a refusal by a host
        // that cannot run it surfaces from the create as an error).
        assert_eq!(kind_on("vm", "hetzner"), "vm");
        assert_eq!(kind_on("container", "local"), "container");
        // Without one: a pooled or cloud target gets containers (a VM there
        // means a host with KVM, bought as metal), this machine its own
        // default (a VM with KVM or HVF, else a container).
        for on in ["hetzner", "aws", "auto", "host:spare", "cloud"] {
            assert_eq!(kind_on("", on), "container", "{on}");
        }
        assert_eq!(kind_on("", "local"), cua_compat::backend::default_kind());
        assert_eq!(kind_on("", ""), cua_compat::backend::default_kind());
    }

    #[test]
    fn options_map_flags() {
        let o = CompatOpts {
            provider: "E2B".into(),
            listen: None,
            host_port: Some(1),
            on: Some("local".into()),
            kind: Some("container".into()),
            image: Some("img".into()),
            cpus: Some(1),
            memory_mb: None,
            default_user: Some("root".into()),
            no_create_user: true,
            overlays: vec![],
            no_warm: false,
        };
        let g = options(&o).unwrap();
        assert_eq!(g.provider, Provider::E2b);
        assert_eq!(g.host_port, 1);
        assert_eq!(g.defaults.kind, "container");
        assert!(!g.create_default_user);
        // No flags: this machine, the kind it can run, the provider's size.
        let plain = options(&CompatOpts {
            on: None,
            kind: None,
            cpus: None,
            memory_mb: None,
            ..o.clone()
        })
        .unwrap();
        assert_eq!(plain.defaults.on, "");
        assert_eq!(plain.defaults.kind, "");
        assert_eq!(plain.defaults.cpus, Some(2));
        assert_eq!(plain.defaults.memory_mb, Some(2048));
        assert!(
            options(&CompatOpts {
                provider: "x".into(),
                ..o
            })
            .is_err()
        );
    }
}
