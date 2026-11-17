//! The command line of `cua compat`: run another provider's SDK, unchanged, on
//! Cua Spaces (cua-compat). `serve` runs the gateway; `env` prints what a
//! client exports to use it.
//!
//! Only the arguments are here (clap and the SDK's `Overlay`), so `cua-cli`
//! can parse the command without building the gateway; the implementation is
//! `cua_compat::cli`.

use clap::Subcommand;
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
