// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Getting machines into your pool, the parts `cua host setup`, `cua host
//! pair` and `cua cloud add aws` share:
//!
//! - [`HostState`]: whether this machine already is the host `cua host
//!   setup` asks for (re-running it changes nothing), or is that host with
//!   its service stopped (it starts it again).
//! - [`jobs_line`]: where jobs run here and the accelerators found.
//! - [`InstallSource`] and [`add_line`]: the one line that installs cua on
//!   another machine and joins it with a pairing code (`CUA_INSTALL_URL`
//!   points it at a staging build).
//! - [`machines_table`]: the pool's hosts as the coordinator sees them
//!   (`cua machines`).

use clap::Args;
use cua_sdk::CuaError;
use serde_json::Value;

/// `cua cloud add aws`.
#[derive(Args, Debug, Clone)]
pub struct AwsArgs {
    /// How many hosts.
    #[arg(long, default_value_t = 1)]
    pub count: u32,
    /// The EC2 instance type. GPU types (g*, p*) only when named here.
    #[arg(long = "type", value_name = "TYPE", default_value = "c7i.large")]
    pub instance_type: String,
    /// Region (default: the connected one).
    #[arg(long)]
    pub region: Option<String>,
    /// Hours until each host powers off and is terminated (default: the
    /// connection's, 8).
    #[arg(long)]
    pub ttl_hours: Option<u32>,
    /// An extra `key=value` tag on every resource. Repeatable.
    #[arg(long = "tag", value_name = "KEY=VALUE")]
    pub tags: Vec<String>,
    /// Root disk in GiB (default 60 for CPU types, 100 for GPU types).
    #[arg(long)]
    pub disk_gb: Option<u32>,
    /// Return once the instances exist, without waiting for them to join.
    #[arg(long)]
    pub no_wait: bool,
    /// The hosts `cua cloud add aws` made, their state and whether they joined.
    #[arg(long, conflicts_with = "down")]
    pub list: bool,
    /// Terminate the hosts `cua cloud add aws` made (all, or `--name`) and remove
    /// them from your pool.
    #[arg(long)]
    pub down: bool,
    /// With --down: only this host (`aws-3`).
    #[arg(long, requires = "down")]
    pub name: Option<String>,
}

/// Where `cua host pair` and `cua cloud add aws` install cua from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallSource {
    /// The release installer (`https://cua.ai/install.sh`).
    Release,
    /// A tarball of the Linux `cua` and `cua-spacesd` (`CUA_INSTALL_URL`),
    /// checked against `sha256`.
    Tarball {
        /// Where to download it (no fragment).
        url: String,
        /// Its sha256 (lowercase hex).
        sha256: String,
    },
}

/// The release installer.
pub const INSTALL_SH: &str = "https://cua.ai/install.sh";

impl InstallSource {
    /// From `CUA_INSTALL_URL` (`<url>#sha256=<hex>`, or the hash in
    /// `CUA_INSTALL_SHA256`); unset: the release installer.
    pub fn from_env() -> Result<Self, CuaError> {
        Self::parse(
            std::env::var("CUA_INSTALL_URL").ok().as_deref(),
            std::env::var("CUA_INSTALL_SHA256").ok().as_deref(),
        )
    }

    /// [`InstallSource::from_env`] over given values.
    pub fn parse(url: Option<&str>, sha: Option<&str>) -> Result<Self, CuaError> {
        let Some(raw) = url.map(str::trim).filter(|u| !u.is_empty()) else {
            return Ok(Self::Release);
        };
        if raw == INSTALL_SH {
            return Ok(Self::Release);
        }
        let (url, frag) = match raw.split_once('#') {
            Some((u, f)) => (u, Some(f)),
            None => (raw, None),
        };
        if !url.starts_with("https://") {
            return Err(CuaError::InvalidArgument(
                "CUA_INSTALL_URL must be an https:// URL".into(),
            ));
        }
        if url.contains('\'') || url.chars().any(char::is_whitespace) {
            return Err(CuaError::InvalidArgument(
                "CUA_INSTALL_URL has a quote or a space".into(),
            ));
        }
        let sha = frag
            .and_then(|f| f.strip_prefix("sha256="))
            .or(sha)
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CuaError::InvalidArgument(
                "CUA_INSTALL_URL needs its sha256: append #sha256=<hex> or set CUA_INSTALL_SHA256"
                    .into(),
            ));
        }
        Ok(Self::Tarball {
            url: url.to_string(),
            sha256: sha,
        })
    }
}

/// `CUA_ENV=<name> ` when this `cua` is not on production (the other
/// machine must join the same deployment).
pub fn env_prefix() -> String {
    let env = cua_home::env::CuaEnv::current();
    if env.is_prod() {
        String::new()
    } else {
        format!("CUA_ENV={} ", env.name())
    }
}

/// The `cua host setup` arguments that join a machine to your pool with a
/// pairing code: it provides Spaces, so jobs can run there, and does not
/// share its own desktop (`--no-desktop`; the default `desktop` profile
/// would).
pub fn pair_setup_args(code: &str, name: Option<&str>, relay: Option<&str>) -> String {
    let mut v = format!("host setup --pair {code} --no-desktop --provide-spaces");
    // The code only labels the machine in `cua host pair --list`; the name
    // it joins under is the one `cua host setup` registers.
    if let Some(n) = name {
        v.push_str(&format!(" --name {}", shell_word(n)));
    }
    if let Some(r) = relay {
        v.push_str(&format!(" --relay {r}"));
    }
    v
}

/// The one line that installs cua on another machine and runs `cua host
/// setup --pair <code> --no-desktop --provide-spaces` there.
pub fn add_line(
    code: &str,
    src: &InstallSource,
    env_prefix: &str,
    relay: Option<&str>,
    name: Option<&str>,
) -> String {
    let setup = pair_setup_args(code, name, relay);
    match src {
        // Install the CLI, then run it (root installs into /usr/local/bin,
        // others ~/.local/bin).
        InstallSource::Release => format!(
            "curl -fsSL {INSTALL_SH} | sh -s -- --cli-only --no-onboarding && \
             PATH=\"$HOME/.local/bin:$PATH\" {env_prefix}cua {setup}"
        ),
        InstallSource::Tarball { url, sha256 } => format!(
            "curl -fsSL '{url}' -o /tmp/cua-linux.tgz && echo '{sha256}  /tmp/cua-linux.tgz' | \
             sha256sum -c --quiet - && mkdir -p ~/.local/bin && tar -xzf /tmp/cua-linux.tgz \
             -C ~/.local/bin cua cua-spacesd && {env_prefix}CUA_SPACESD_BIN=$HOME/.local/bin/cua-spacesd \
             ~/.local/bin/cua {setup}"
        ),
    }
}

/// `s` as one POSIX shell word (quoted only when it needs to be).
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The relay the other machine must join: `--relay`, else `$CUA_RELAY_URL`
/// (a custom relay the default of `CUA_ENV` would not reach).
pub fn relay_for_line(flag: Option<&str>, env: Option<&str>) -> Option<String> {
    flag.or(env)
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string)
}

/// What a `cua host setup` on the relay asks this machine to be.
#[derive(Clone, Debug, Default)]
pub struct Want {
    /// The relay URL.
    pub relay: String,
    /// Share this machine's own desktop (`None`: not asked, either way).
    pub share_desktop: Option<bool>,
    /// Provide Spaces (`None`: not asked, either way).
    pub provide_spaces: Option<bool>,
    /// Dedicated (`--dedicated`; not asked: either way).
    pub dedicated: bool,
    /// `--name`, when given.
    pub name: Option<String>,
    /// `--runner`, unless `auto`.
    pub runner: Option<String>,
}

/// What `cua host setup` finds on this machine before it sets it up.
#[derive(Clone, Debug, Default)]
pub struct HostState {
    /// `cua host status` as JSON (camelCase keys).
    pub status: Value,
    /// Set up as dedicated (`HostPolicy.dedicated`).
    pub dedicated: bool,
}

impl HostState {
    /// This machine's host status and policy (no network beyond the
    /// relay's online check).
    pub async fn read(host: &cua_host::Host) -> Self {
        let status = host
            .status()
            .await
            .ok()
            .and_then(|s| serde_json::to_value(s).ok())
            .unwrap_or(Value::Null);
        let dedicated = host.policy().ok().flatten().is_some_and(|p| p.dedicated);
        HostState { status, dedicated }
    }

    /// Whether this machine already is the host `w` asks for.
    pub fn is_up(&self, w: &Want) -> bool {
        let s = &self.status;
        s["configured"].as_bool() == Some(true)
            && s["mode"].as_str() == Some("relay")
            && s["service"]["running"].as_bool() == Some(true)
            && w.provide_spaces
                .is_none_or(|p| s["provideSpaces"].as_bool() == Some(p))
            && w.share_desktop
                .is_none_or(|d| s["shareDesktop"].as_bool() == Some(d))
            && (!w.dedicated || self.dedicated)
            && w.name
                .as_deref()
                .is_none_or(|n| s["name"].as_str() == Some(n))
            && w.runner
                .as_deref()
                .is_none_or(|r| s["service"]["kind"].as_str() == Some(r))
            && s["relayUrl"]
                .as_str()
                .is_some_and(|r| r.trim_end_matches('/') == w.relay.trim_end_matches('/'))
    }

    /// Whether this machine is the host `w` asks for except that its
    /// service is not running (it was stopped, crashed or the machine
    /// restarted): `cua host setup` starts it again with the machine token
    /// it has, with no sign-in and no code.
    pub fn is_down(&self, w: &Want) -> bool {
        let mut running = self.clone();
        running.status["service"]["running"] = Value::Bool(true);
        self.status["service"]["running"].as_bool() != Some(true)
            && self.status["machineId"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
            && running.is_up(w)
    }

    /// The status line of a machine that already is the host asked for.
    pub fn summary(&self) -> String {
        let s = &self.status;
        let online = match s["online"].as_bool() {
            Some(true) => "online",
            Some(false) => "offline",
            None => "relay not reached",
        };
        let mut what = vec![];
        if s["provideSpaces"].as_bool() == Some(true) {
            what.push("provides Spaces");
        }
        if s["shareDesktop"].as_bool() == Some(true) {
            what.push("shares its desktop");
        }
        if self.dedicated {
            what.push("dedicated");
        }
        format!(
            "Already set up: {} is on {} (computer {}, {online}; {}). Nothing to change.",
            s["name"].as_str().unwrap_or("this computer"),
            s["relayUrl"].as_str().unwrap_or("the relay"),
            s["machineId"].as_str().unwrap_or("?"),
            what.join(", ")
        )
    }
}

/// What `cua host setup` says about where jobs run here and the
/// accelerators it found.
pub fn jobs_line(dedicated: bool, accelerators: &[String]) -> String {
    let gpus = if accelerators.is_empty() {
        "no GPU found, so CPU jobs only".to_string()
    } else {
        format!("GPU: {}", accelerators.join(", "))
    };
    let kind = if dedicated {
        "this computer's own Space (dedicated)"
    } else if cfg!(target_os = "macos") {
        "macOS VMs"
    } else {
        "containers"
    };
    crate::util::hint(&format!(
        "Jobs: run in {kind} here ({gpus}). `cua run` sends Python functions here."
    ))
}

fn pct(free: &Value, total: &Value) -> String {
    let (f, t) = (free.as_u64().unwrap_or(0), total.as_u64().unwrap_or(0));
    (100 * t.saturating_sub(f))
        .checked_div(t)
        .map_or("-".to_string(), |p| format!("{p}%"))
}

/// One host's accelerators as text (`-`: none reported).
fn accelerators(m: &Value) -> String {
    let a: Vec<&str> = m["accelerators"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if a.is_empty() {
        "-".into()
    } else {
        a.join(", ")
    }
}

/// One host's $/h as text (`-`: not reported).
fn price(m: &Value) -> String {
    match m["usd_per_hour"].as_f64() {
        Some(p) if p > 0.0 => format!("${p:.3}"),
        Some(_) => "$0".into(),
        None => "-".into(),
    }
}

/// The table `cua machines` prints for a pool `status` answer.
pub fn machines_table(v: &Value) -> Vec<String> {
    let mut rows = vec![format!(
        "{:<20}  {:<9}  {:<14}  {:>5}  {:>6}  {:>4}  {:<24}  {:>7}  NOTE",
        "MACHINE", "STATE", "OS/ARCH", "CPU", "MEMORY", "JOBS", "ACCELERATORS", "$/H"
    )];
    let machines = v["machines"].as_array().cloned().unwrap_or_default();
    for m in &machines {
        rows.push(format!(
            "{:<20}  {:<9}  {:<14}  {:>5}  {:>6}  {:>4}  {:<24}  {:>7}  {}",
            m["name"].as_str().unwrap_or(""),
            m["health"].as_str().unwrap_or(""),
            format!(
                "{}/{}",
                m["os"].as_str().unwrap_or("?"),
                m["arch"].as_str().unwrap_or("?")
            ),
            pct(&m["millicpus_free"], &m["millicpus_total"]),
            pct(&m["memory_free_mb"], &m["memory_total_mb"]),
            m["jobs_running"].as_u64().unwrap_or(0),
            accelerators(m),
            price(m),
            m["problem"].as_str().unwrap_or("")
        ));
    }
    if machines.is_empty() {
        rows.push(crate::util::hint(
            "No hosts yet: `cua host setup --share spaces` here, `cua host pair` for another computer.",
        ));
    }
    rows
}

/// The table `cua machines` prints in a terminal `width` columns
/// wide: the full table when it fits, else a narrow one (machine, state,
/// jobs, GPU, $/h; 58 columns) that does not wrap, with a host's problem on
/// a line of its own.
pub fn machines_table_fit(v: &Value, width: usize) -> Vec<String> {
    let full = machines_table(v);
    if full.iter().all(|r| r.trim_end().chars().count() <= width) {
        return full;
    }
    let machines = v["machines"].as_array().cloned().unwrap_or_default();
    if machines.is_empty() {
        return full;
    }
    let mut rows = vec![format!(
        "{:<12} {:<8} {:>4}  {:<24} {:>5}",
        "MACHINE", "STATE", "JOBS", "GPU", "$/H"
    )];
    for m in &machines {
        rows.push(format!(
            "{:<12} {:<8} {:>4}  {:<24} {:>5}",
            m["name"].as_str().unwrap_or(""),
            m["health"].as_str().unwrap_or(""),
            m["jobs_running"].as_u64().unwrap_or(0),
            short_accelerators(m),
            price(m),
        ));
        if let Some(p) = m["problem"].as_str().filter(|p| !p.is_empty()) {
            rows.push(format!("  {p}"));
        }
    }
    rows
}

/// A host's accelerators without the vendor and memory
/// (`apple:M5 Max:32-core GPU:36 GB` -> `M5 Max 32-core GPU`).
fn short_accelerators(m: &Value) -> String {
    let a: Vec<String> = m["accelerators"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| {
                    let parts: Vec<&str> = s.split(':').collect();
                    match parts.len() {
                        0 | 1 => s.to_string(),
                        2 => parts[1].to_string(),
                        _ => format!("{} {}", parts[1], parts[2]),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if a.is_empty() {
        "-".into()
    } else {
        a.join(", ")
    }
}

/// The terminal's width in columns when stdout is one.
pub fn stdout_columns() -> Option<usize> {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ fills a plain struct.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return Some(ws.ws_col as usize);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn install_url_needs_https_and_a_hash() {
        assert_eq!(
            InstallSource::parse(None, None).unwrap(),
            InstallSource::Release
        );
        assert_eq!(
            InstallSource::parse(Some(INSTALL_SH), None).unwrap(),
            InstallSource::Release
        );
        let t = InstallSource::parse(
            Some(&format!(
                "https://b.s3.amazonaws.com/x.tgz?X-Amz-Signature=ab#sha256={SHA}"
            )),
            None,
        )
        .unwrap();
        assert_eq!(
            t,
            InstallSource::Tarball {
                url: "https://b.s3.amazonaws.com/x.tgz?X-Amz-Signature=ab".into(),
                sha256: SHA.into()
            }
        );
        // The hash may come from CUA_INSTALL_SHA256 instead.
        assert!(InstallSource::parse(Some("https://h/x.tgz"), Some(SHA)).is_ok());
        assert!(InstallSource::parse(Some("https://h/x.tgz"), None).is_err());
        assert!(InstallSource::parse(Some("http://h/x.tgz"), Some(SHA)).is_err());
        assert!(InstallSource::parse(Some("https://h/x'.tgz"), Some(SHA)).is_err());
    }

    #[test]
    fn the_add_line_installs_then_runs_host_setup_with_the_code() {
        let l = add_line("K7QM-4XPB-2RHD", &InstallSource::Release, "", None, None);
        assert!(l.starts_with("curl -fsSL https://cua.ai/install.sh | sh -s -- "));
        assert!(l.ends_with("cua host setup --pair K7QM-4XPB-2RHD --no-desktop --provide-spaces"));
        assert!(!l.contains('\n'));
        let t = InstallSource::Tarball {
            url: "https://b/x.tgz?a=b&c=d".into(),
            sha256: SHA.into(),
        };
        let l = add_line(
            "K7QM-4XPB-2RHD",
            &t,
            "CUA_ENV=staging ",
            Some("https://r.example"),
            None,
        );
        assert!(l.contains("'https://b/x.tgz?a=b&c=d'"));
        assert!(l.contains(&format!("echo '{SHA}  /tmp/cua-linux.tgz' | sha256sum -c")));
        assert!(l.contains("CUA_ENV=staging CUA_SPACESD_BIN="));
        assert!(l.ends_with(
            "~/.local/bin/cua host setup --pair K7QM-4XPB-2RHD --no-desktop --provide-spaces --relay https://r.example"
        ));
        // `cua host pair --name` names the machine through the line it prints.
        let l = add_line(
            "K7QM-4XPB-2RHD",
            &InstallSource::Release,
            "",
            Some("https://r.example"),
            Some("studio"),
        );
        assert!(l.ends_with(
            "cua host setup --pair K7QM-4XPB-2RHD --no-desktop --provide-spaces --name studio --relay https://r.example"
        ));
        let l = add_line("C", &InstallSource::Release, "", None, Some("Dee's Mac"));
        assert!(
            l.ends_with("--pair C --no-desktop --provide-spaces --name 'Dee'\\''s Mac'"),
            "{l}"
        );
        // A custom relay from the environment goes into the line too.
        assert_eq!(
            relay_for_line(None, Some("https://mine.example")).as_deref(),
            Some("https://mine.example")
        );
        assert_eq!(
            relay_for_line(Some("https://flag.example"), Some("https://mine.example")).as_deref(),
            Some("https://flag.example")
        );
        assert_eq!(relay_for_line(None, Some(" ")), None);
    }

    /// The keys `is_up` reads are the ones `cua host status` writes.
    #[test]
    fn setup_reads_the_host_status_as_it_serializes() {
        let v = serde_json::to_value(cua_host::HostStatus::default()).unwrap();
        for k in [
            "configured",
            "mode",
            "relayUrl",
            "machineId",
            "name",
            "online",
            "provideSpaces",
            "shareDesktop",
        ] {
            assert!(v.get(k).is_some(), "{k} missing from {v}");
        }
        assert!(v["service"].get("running").is_some(), "{v}");
        assert!(v["service"].get("kind").is_some(), "{v}");
    }

    fn want() -> Want {
        Want {
            relay: "https://relay.example".into(),
            share_desktop: Some(false),
            provide_spaces: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn setup_is_a_no_op_only_when_everything_matches() {
        let status = json!({
            "configured": true, "mode": "relay", "relayUrl": "https://relay.example/",
            "machineId": "abc", "name": "studio", "online": true,
            "service": {"running": true, "kind": "systemd"}, "provideSpaces": true,
            "shareDesktop": false,
        });
        let s = HostState {
            status: status.clone(),
            dedicated: false,
        };
        let w = want();
        assert!(s.is_up(&w));
        assert!(!s.is_up(&Want {
            relay: "https://other.example".into(),
            ..want()
        }));
        assert!(!s.is_up(&Want {
            name: Some("other".into()),
            ..want()
        }));
        assert!(s.is_up(&Want {
            runner: Some("systemd".into()),
            ..want()
        }));
        assert!(!s.is_up(&Want {
            runner: Some("process".into()),
            ..want()
        }));
        // Sharing the desktop too, or not providing Spaces, is set up again.
        assert!(!s.is_up(&Want {
            share_desktop: Some(true),
            ..want()
        }));
        assert!(!s.is_up(&Want {
            provide_spaces: Some(false),
            ..want()
        }));
        // What the command line leaves out stays as it is.
        assert!(s.is_up(&Want {
            share_desktop: None,
            provide_spaces: None,
            ..want()
        }));
        // Asked to be dedicated, a host that is not is set up again; a
        // dedicated one stays dedicated when not asked.
        let dedicated = Want {
            dedicated: true,
            ..want()
        };
        assert!(!s.is_up(&dedicated));
        let d = HostState {
            status: status.clone(),
            dedicated: true,
        };
        assert!(d.is_up(&dedicated));
        assert!(d.is_up(&w));
        assert!(d.summary().contains("dedicated"));
        let mut stopped = status;
        stopped["service"]["running"] = json!(false);
        assert!(
            !HostState {
                status: stopped,
                dedicated: false
            }
            .is_up(&w)
        );
        assert!(s.summary().starts_with("Already set up: studio is on"));
        assert!(s.summary().contains("provides Spaces"));
        assert!(!s.is_down(&w));
    }

    #[test]
    fn setup_starts_a_configured_host_whose_service_stopped() {
        let mut status = json!({
            "configured": true, "mode": "relay", "relayUrl": "https://relay.example",
            "machineId": "abc", "name": "studio", "online": false,
            "service": {"running": false, "kind": "process"}, "provideSpaces": true,
            "shareDesktop": false,
        });
        let s = HostState {
            status: status.clone(),
            dedicated: false,
        };
        let w = want();
        assert!(!s.is_up(&w));
        assert!(s.is_down(&w));
        // Another relay, or a machine that never registered, is set up anew.
        assert!(!s.is_down(&Want {
            relay: "https://other.example".into(),
            ..want()
        }));
        status["machineId"] = json!("");
        assert!(
            !HostState {
                status,
                dedicated: false
            }
            .is_down(&w)
        );
    }

    #[test]
    fn setup_names_the_gpus_it_found() {
        assert_eq!(
            jobs_line(true, &["nvidia:L4:24 GB".into()]),
            "Jobs: run in this computer's own Space (dedicated) here (GPU: nvidia:L4:24 GB). \
             `cua run` sends Python functions here."
        );
        assert!(jobs_line(false, &[]).contains("no GPU found, so CPU jobs only"));
        assert!(!jobs_line(false, &[]).contains("dedicated"));
    }

    #[test]
    fn machines_show_accelerators_and_price_when_reported() {
        let v = json!({"machines": [
            {"name": "aws-1", "health": "ready", "os": "linux", "arch": "x86_64",
             "millicpus_total": 2000, "millicpus_free": 1000, "memory_total_mb": 4096,
             "memory_free_mb": 1024, "jobs_running": 1, "accelerators": ["nvidia:L4:24 GB"],
             "usd_per_hour": 0.8048, "problem": ""},
            {"name": "studio", "health": "ready", "os": "macos", "arch": "arm64",
             "millicpus_total": 0, "millicpus_free": 0, "jobs_running": 0},
        ]});
        let t = machines_table(&v);
        assert_eq!(t.len(), 3);
        assert!(t[1].contains("aws-1") && t[1].contains("50%") && t[1].contains("75%"));
        assert!(t[1].contains("nvidia:L4:24 GB") && t[1].contains("$0.805"));
        assert!(t[2].contains("studio") && t[2].contains(" - ") && t[2].contains("macos/arm64"));
        assert!(machines_table(&json!({"machines": []}))[1].starts_with("No hosts yet"));
    }

    #[test]
    fn machines_fit_a_narrow_terminal() {
        let v = json!({"machines": [
            {"name": "macbook", "health": "ready", "os": "macos", "arch": "aarch64",
             "jobs_running": 1, "accelerators": ["apple:M5 Max:32-core GPU:36 GB"], "usd_per_hour": 0.0},
            {"name": "studio", "health": "lost", "problem": "not seen for 40 s",
             "accelerators": ["apple:M2 Max:38-core GPU:96 GB"], "usd_per_hour": 0.0}
        ]});
        assert_eq!(machines_table_fit(&v, 200), machines_table(&v));
        let rows = machines_table_fit(&v, 58);
        assert!(rows.iter().all(|r| r.chars().count() <= 58), "{rows:#?}");
        assert!(rows[1].starts_with("macbook") && rows[1].contains("M5 Max 32-core GPU"));
        assert!(rows[2].starts_with("studio") && rows[2].contains("lost"));
        assert_eq!(rows[3], "  not seen for 40 s");
    }
}
