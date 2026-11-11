//! Gives a VM restored from a shared memory snapshot its own identity
//! through its cua-spacesd ([`cua_vmm::GuestIdentity`] for the QEMU
//! backend's warm starts and memory forks).
//!
//! Authenticated with the snapshot's token: an `Init` steps the guest clock
//! and, with `new_machine` and fresh entropy, makes the guest a new machine
//! in drivers that implement it (machine-seal key, random pool, machine-id;
//! older drivers ignore both fields). Then the guest's root renewal script
//! ([`cua_vmm::cloudinit::renew_identity_script`], through cua-spacesd and
//! `sudo`) rotates the token in the files the driver follows (a seeded VM's
//! driver runs in await-token-file mode, where `Init` cannot rotate it),
//! steps the clock again, re-seeds the kernel's CRNG, and regenerates
//! machine-id, hostname and SSH host keys. Done when the driver accepts the
//! new token.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use cua_spacesd_client::{SpacesdClient, pb};
use cua_vmm::{ClaimIdentity, Endpoints, GuestIdentity, VmmError};

/// [`GuestIdentity`] over cua-spacesd's `SystemService.Init`.
pub struct SpacesdIdentity;

/// Bytes of host entropy sent to the guest.
const ENTROPY_BYTES: usize = 64;

fn init_request(identity: &ClaimIdentity, now: SystemTime) -> pb::InitRequest {
    let d = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let mut entropy = vec![0u8; ENTROPY_BYTES];
    rand::fill(&mut entropy[..]);
    let mut labels = std::collections::HashMap::new();
    if let Some(h) = &identity.hostname {
        labels.insert("sandbox".to_string(), h.clone());
    }
    pb::InitRequest {
        token: identity.token.clone().unwrap_or_default(),
        now: Some(pbjson_types::Timestamp {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        }),
        labels,
        new_machine: true,
        entropy,
        ..Default::default()
    }
}

/// The host clock as `<secs>.<nanos>` (for `date -s @...`).
fn clock_arg(now: SystemTime) -> String {
    let d = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}", d.as_secs(), d.subsec_nanos())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why one renewal attempt failed.
#[derive(Debug)]
enum RenewFailure {
    /// The guest ran the renewal script and it failed (`sudo` wants a
    /// password, a read-only file): the same guest fails the same way
    /// again.
    Refused(String),
    /// The guest did not answer, or not yet (still restoring, the driver
    /// dropped the session as it followed the new token): worth retrying.
    Transport(String),
}

impl std::fmt::Display for RenewFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "the guest's identity renewal failed: {why}"),
            Self::Transport(why) => f.write_str(why),
        }
    }
}

impl From<cua_spacesd_client::Error> for RenewFailure {
    fn from(e: cua_spacesd_client::Error) -> Self {
        Self::Transport(e.to_string())
    }
}

/// Renewal attempts the guest refused (it ran the script, which failed)
/// before [`SpacesdIdentity::renew`] gives up. More than one: the driver
/// can drop the call's session part way through as it follows the new
/// token. A refusal never heals by waiting out the budget.
const MAX_REFUSALS: u32 = 3;

/// Run as the guest's agent: succeeds when the renewal script can run as
/// root (the agent is root, or has passwordless sudo).
const RENEWABLE_COMMAND: &str = "[ \"$(id -u)\" = 0 ] || sudo -n true";

/// Whether a renewal stops retrying: the guest refused it
/// [`MAX_REFUSALS`] times, or the budget is spent.
fn gives_up(refusals: u32, past_deadline: bool) -> bool {
    refusals >= MAX_REFUSALS || past_deadline
}

/// One renewal over a connected client: `Init` (clock, `new_machine`,
/// entropy; the token is left to the files cua-spacesd follows), then the
/// guest's root renewal script for the token and the machine identity.
async fn renew_once(
    url: &str,
    current: &str,
    identity: &ClaimIdentity,
    new_machine: bool,
) -> Result<(), RenewFailure> {
    let started = std::time::Instant::now();
    let client = connect(url, current).await?;
    let connected = started.elapsed();
    // Seeded guests run cua-spacesd in await-token-file mode, where Init
    // may not rotate the token: everything else goes through Init.
    if new_machine {
        let mut init = init_request(identity, SystemTime::now());
        init.token.clear();
        client.init(init).await?;
    }
    let inited = started.elapsed();
    let [token_var, entropy_var, now_var, host_var, new_machine_var] =
        cua_vmm::cloudinit::RENEW_ENV;
    let mut entropy = vec![0u8; ENTROPY_BYTES];
    rand::fill(&mut entropy[..]);
    let cmd = cua_spacesd_client::Command::new("sudo")
        .args([
            "-n".to_string(),
            format!("--preserve-env={}", cua_vmm::cloudinit::RENEW_ENV.join(",")),
            "sh".to_string(),
            "-c".to_string(),
            cua_vmm::cloudinit::renew_identity_script(),
        ])
        .env(token_var, identity.token.clone().unwrap_or_default())
        .env(entropy_var, hex(&entropy))
        .env(host_var, identity.hostname.clone().unwrap_or_default())
        .env(new_machine_var, if new_machine { "1" } else { "0" })
        // Read last, right before the call.
        .env(now_var, clock_arg(SystemTime::now()))
        .timeout(Duration::from_secs(20));
    let out = client.run(cmd).await?;
    tracing::debug!(
        connect_ms = connected.as_millis() as u64,
        init_ms = (inited - connected).as_millis() as u64,
        script_ms = (started.elapsed() - inited).as_millis() as u64,
        "guest identity renewed"
    );
    if !out.success() {
        return Err(RenewFailure::Refused(out.stderr_str().trim().to_string()));
    }
    Ok(())
}

/// A client of the guest's cua-spacesd: native gRPC (a seeded VM's driver
/// speaks it), without the capability probe a first call makes anyway.
async fn connect(url: &str, token: &str) -> cua_spacesd_client::Result<SpacesdClient> {
    let mut opts = cua_spacesd_client::ConnectOptions::parse(url)?;
    opts.token = Some(token.to_string());
    opts.transport = cua_spacesd_client::TransportPreference::Native;
    opts.probe = false;
    SpacesdClient::connect(opts).await
}

/// Whether the driver at `url` accepts `token` for an authenticated call
/// (`Health` answers without one).
async fn accepts(url: &str, token: &str) -> cua_spacesd_client::Result<()> {
    let probe = async {
        let out = connect(url, token)
            .await?
            .run(cua_spacesd_client::Command::new("true").timeout(Duration::from_secs(5)))
            .await?;
        if out.success() {
            Ok(())
        } else {
            Err(cua_spacesd_client::Error::Protocol("`true` failed".into()))
        }
    };
    tokio::time::timeout(Duration::from_secs(10), probe)
        .await
        .unwrap_or_else(|_| Err(cua_spacesd_client::Error::Protocol("timed out".into())))
}

/// Waits until the guest's cua-spacesd at `endpoints` accepts `token` for an
/// authenticated call and the guest's cloud-init (where it has one) is done,
/// for at most `timeout`. A Space is ready then, not when its port first
/// answers: a VM's cloud-init puts the token in place and restarts
/// cua-spacesd, and under load (64 QEMU VMs booting at once) that restart
/// came after the first authenticated call, failing the caller's next one.
pub(crate) async fn wait_accepts(
    endpoints: &Endpoints,
    token: &str,
    timeout: Duration,
) -> cua_vmm::Result<()> {
    let url = spacesd_url(endpoints)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut backoff = cua_vmm::probe::Backoff::default();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let last = match settled(&url, token, left).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(VmmError::Timeout {
                name: url,
                secs: timeout.as_secs(),
                detail: format!("cua-spacesd never accepted the Space's token: {last}"),
            });
        }
        tokio::time::sleep(backoff.step()).await;
    }
}

/// The readiness command: done at once when this boot's cloud-init has
/// finished (`/run/cloud-init/result.json`, on tmpfs, so a VM restored from
/// a snapshot taken after cloud-init has it) or there is no cloud-init;
/// else it waits for cloud-init. `cloud-init status --wait` is a Python
/// program that takes about a second to start in a 1-vCPU guest, which
/// every warm start (a golden-snapshot restore) paid before.
pub(crate) const SETTLED_COMMAND: &str = "test -e /run/cloud-init/result.json || \
     { command -v cloud-init >/dev/null 2>&1 && cloud-init status --wait >/dev/null 2>&1; }; true";

/// One readiness attempt: an authenticated call that returns once the
/// guest's cloud-init has finished (at once without cloud-init).
async fn settled(url: &str, token: &str, left: Duration) -> cua_spacesd_client::Result<()> {
    let budget = left.max(Duration::from_secs(1));
    let probe = async {
        let out = connect(url, token)
            .await?
            .run(cua_spacesd_client::Command::shell(SETTLED_COMMAND).timeout(budget))
            .await?;
        if out.success() {
            Ok(())
        } else {
            Err(cua_spacesd_client::Error::Protocol(
                "the readiness command failed".into(),
            ))
        }
    };
    tokio::time::timeout(budget + Duration::from_secs(5), probe)
        .await
        .unwrap_or_else(|_| Err(cua_spacesd_client::Error::Protocol("timed out".into())))
}

fn spacesd_url(endpoints: &Endpoints) -> cua_vmm::Result<String> {
    let port = endpoints
        .ports
        .get(&cua_proto::SPACESD_DEFAULT_PORT)
        .copied()
        .ok_or_else(|| {
            VmmError::invalid(format!(
                "the VM does not publish cua-spacesd's port {}",
                cua_proto::SPACESD_DEFAULT_PORT
            ))
        })?;
    let host = if endpoints.host.is_empty() {
        "127.0.0.1"
    } else {
        endpoints.host.as_str()
    };
    Ok(format!("http://{host}:{port}"))
}

#[async_trait]
impl GuestIdentity for SpacesdIdentity {
    async fn resync_clock(&self, endpoints: &Endpoints, token: &str) -> cua_vmm::Result<()> {
        let url = spacesd_url(endpoints)?;
        let step = async {
            let client = connect(&url, token).await?;
            let cmd = cua_spacesd_client::Command::new("sudo")
                .args(["-n", "date", "-u", "-s"])
                .arg(format!("@{}", clock_arg(SystemTime::now())))
                .timeout(Duration::from_secs(10));
            client.run(cmd).await
        };
        match tokio::time::timeout(Duration::from_secs(20), step).await {
            Ok(Ok(out)) if out.success() => Ok(()),
            Ok(Ok(out)) => Err(VmmError::other(format!(
                "date -s in the guest failed: {}",
                out.stderr_str().trim()
            ))),
            Ok(Err(e)) => Err(VmmError::other(e.to_string())),
            Err(_) => Err(VmmError::other("timed out stepping the guest clock")),
        }
    }

    async fn check_renewable(
        &self,
        endpoints: &Endpoints,
        token: &str,
    ) -> cua_vmm::Result<Option<String>> {
        let url = spacesd_url(endpoints)?;
        let check = async {
            let client = connect(&url, token).await?;
            let cmd = cua_spacesd_client::Command::new("sh")
                .args(["-c", RENEWABLE_COMMAND])
                .timeout(Duration::from_secs(10));
            client.run(cmd).await
        };
        match tokio::time::timeout(Duration::from_secs(20), check).await {
            Ok(Ok(out)) if out.success() => Ok(None),
            Ok(Ok(out)) => Ok(Some(format!(
                "its cua-spacesd cannot run the identity renewal as root (it runs as an \
                 unprivileged user without passwordless sudo): {}",
                out.stderr_str().trim()
            ))),
            Ok(Err(e)) => Err(VmmError::other(format!(
                "could not ask its cua-spacesd whether it can renew its identity: {e}"
            ))),
            Err(_) => Err(VmmError::other(
                "timed out asking its cua-spacesd whether it can renew its identity",
            )),
        }
    }

    async fn renew(
        &self,
        endpoints: &Endpoints,
        current: &str,
        identity: &ClaimIdentity,
        new_machine: bool,
        timeout: Duration,
    ) -> cua_vmm::Result<()> {
        let url = spacesd_url(endpoints)?;
        let deadline = tokio::time::Instant::now() + timeout;
        let fail = |what: &str, last: &str| {
            VmmError::other(format!(
                "could not give the restored VM its identity through cua-spacesd at {url}: \
                 {what}: {last}"
            ))
        };
        let mut refusals = 0;
        loop {
            let failure = match tokio::time::timeout(
                Duration::from_secs(30),
                renew_once(&url, current, identity, new_machine),
            )
            .await
            {
                Ok(Ok(())) => break,
                Ok(Err(e)) => e,
                Err(_) => RenewFailure::Transport("timed out".to_string()),
            };
            let last = failure.to_string();
            tracing::debug!(error = %last, "guest identity renewal attempt ended");
            // The driver drops the renewal's own session once it follows
            // the new token, which can end the call before it returns.
            if let Some(new) = identity.token.as_deref().filter(|t| *t != current)
                && accepts(&url, new).await.is_ok()
            {
                break;
            }
            if matches!(failure, RenewFailure::Refused(_)) {
                refusals += 1;
            }
            if gives_up(refusals, tokio::time::Instant::now() >= deadline) {
                return Err(fail("renewal", &last));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // The driver follows its token file: wait until the new token is
        // the one it accepts.
        let Some(new) = identity.token.clone().filter(|t| t != current) else {
            return Ok(());
        };
        loop {
            let last = match accepts(&url, &new).await {
                Ok(()) => {
                    tracing::debug!("the guest accepts its new token");
                    return Ok(());
                }
                Err(e) => e.to_string(),
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(fail("the new token was not accepted", &last));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Readiness never pays cloud-init's CLI start-up once this boot's
    /// cloud-init is done (a restored VM), and still succeeds without it.
    #[cfg(unix)]
    #[test]
    fn the_settled_command_checks_the_result_file_first_and_exits_zero() {
        assert!(SETTLED_COMMAND.starts_with("test -e /run/cloud-init/result.json ||"));
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", SETTLED_COMMAND])
            .env("PATH", "/nonexistent")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    #[test]
    fn init_carries_token_clock_and_fresh_entropy() {
        let id = ClaimIdentity {
            token: Some("t-123".into()),
            hostname: Some("box".into()),
        };
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let a = init_request(&id, now);
        let b = init_request(&id, now);
        assert_eq!(a.token, "t-123");
        assert!(a.new_machine);
        assert_eq!(a.now.as_ref().unwrap().seconds, 1_800_000_000);
        assert_eq!(a.entropy.len(), ENTROPY_BYTES);
        assert_ne!(a.entropy, b.entropy, "every Init gets its own entropy");
        assert_eq!(a.labels.get("sandbox").map(String::as_str), Some("box"));
        // No token: leave the guest's unchanged.
        let none = init_request(&ClaimIdentity::default(), now);
        assert!(none.token.is_empty());
    }

    /// A guest whose agent cannot sudo (the OSWorld disk's runs as `user`)
    /// refuses every renewal: it is given up after a few refusals, not
    /// retried until the readiness budget is gone (25 min of a 30 min
    /// release verify). Transport errors keep retrying until the deadline.
    #[test]
    fn a_refused_renewal_is_given_up_without_waiting_out_the_budget() {
        assert!(!gives_up(0, false));
        assert!(!gives_up(MAX_REFUSALS - 1, false));
        assert!(gives_up(MAX_REFUSALS, false));
        assert!(gives_up(0, true));
        let refused = RenewFailure::Refused("sudo: a password is required".into());
        assert_eq!(
            refused.to_string(),
            "the guest's identity renewal failed: sudo: a password is required"
        );
    }

    /// The renewability check passes as root or with passwordless sudo and
    /// fails for an unprivileged agent whose sudo wants a password.
    #[cfg(unix)]
    #[test]
    fn the_renewable_command_needs_root_or_passwordless_sudo() {
        let bin = tempfile::tempdir().unwrap();
        let run = |sudo_exit: i32| {
            let sudo = bin.path().join("sudo");
            std::fs::write(&sudo, format!("#!/bin/sh\nexit {sudo_exit}\n")).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
            let path = format!("{}:/usr/bin:/bin", bin.path().display());
            std::process::Command::new("/bin/sh")
                .args(["-c", RENEWABLE_COMMAND])
                .env("PATH", path)
                .status()
                .unwrap()
                .success()
        };
        let root = std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|o| o.stdout.starts_with(b"0\n"));
        assert!(run(0));
        assert_eq!(run(1), root, "an unprivileged agent without sudo must fail");
    }
}
