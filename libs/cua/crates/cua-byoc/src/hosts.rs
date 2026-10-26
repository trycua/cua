// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Elastic pool hosts: one lifecycle for every cloud that supplies hosts
//! ([`crate::Supplies::Hosts`]). A provider only creates and deletes a
//! machine of the exact type and region it is asked for
//! (`provider-contract.md`); everything else is here, the same for AWS,
//! GCP, Hetzner and any future cloud:
//!
//! 1. **Offer.** Each connected cloud's machine types ([`Hosts::offers`]),
//!    priced, with what they run and how many more may be added.
//! 2. **Add.** A one-time pairing code is minted on the account's relay,
//!    the host's first-boot script ([`user_data`]) is written, the machine
//!    is recorded in this home's cloud state, then created with every tag
//!    (`cua-managed`, `cua-owner`, `cua-role=host`, `cua-expires`) in the
//!    create call. Its first boot installs cua and runs `cua host setup
//!    --pair <code> --no-desktop --provide-spaces --dedicated`: it joins the
//!    pool as a host that provides Spaces, with no account credential on it.
//! 3. **Run.** Many Spaces run on it, sharing one copy of each image
//!    (container layers, VM base disks with per-Space overlays).
//! 4. **End.** [`Hosts::remove`] deletes the machine and its relay entry.
//!    Backstops when nobody removes it: where a stopped machine stops
//!    billing (AWS, which then deletes it) it powers off after
//!    [`IDLE_POWEROFF_SECS`] without Spaces; everywhere it shuts down at its
//!    TTL (Hetzner and GCP keep billing a stopped machine until `cua cloud
//!    sweep` or the scaler deletes it, so they skip the idle power-off).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{AddHost, CloudHost, HostTypeOffer};

use crate::api::{CloudApi, Result};
use crate::audit;
use crate::model::{self, Connection, HostType, ProvisionSpec, ROLE_HOST, Resource, Supplies};
use crate::relay::RelayAccess;
use crate::state::Store;

/// A host's lifetime unless asked otherwise.
pub const DEFAULT_TTL_SECS: u64 = 24 * 3600;
/// The longest lifetime a host may be given.
pub const MAX_TTL_SECS: u64 = 7 * 24 * 3600;
/// A host with no Spaces for this long powers itself off (the scaler
/// normally removes it sooner, at its billing boundary).
pub const IDLE_POWEROFF_SECS: u64 = 20 * 60;
/// How long a host's pairing code is valid (boot and install take
/// minutes).
pub const CODE_TTL_SECS: u64 = 3600;
/// How long offers are cached.
pub const OFFERS_TTL: Duration = Duration::from_secs(600);
/// The release installer.
pub const INSTALL_SH: &str = "https://cua.ai/install.sh";
/// The `extra` key and value that mark a host record (older `cua cloud
/// add aws` hosts say `pool-host`).
pub const EXTRA_ROLE: &str = "role";
const LEGACY_POOL_HOST: &str = "pool-host";

/// Where a host installs cua from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The release installer.
    Release,
    /// A tarball with `cua` and `cua-spacesd` for the host's arch, checked
    /// against its sha256.
    Tarball {
        /// `https://` URL.
        url: String,
        /// sha256 (lowercase hex).
        sha256: String,
    },
}

/// Where hosts of each arch install cua from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// x86_64 hosts.
    pub amd64: Source,
    /// Arm hosts.
    pub arm64: Source,
}

impl Install {
    /// From `CUA_INSTALL_URL` (+ `CUA_INSTALL_SHA256`, or `#sha256=` on the
    /// URL) for x86_64 hosts and `CUA_INSTALL_URL_ARM64` (+
    /// `CUA_INSTALL_SHA256_ARM64`) for Arm hosts; unset: the release
    /// installer.
    pub fn from_env() -> Result<Self> {
        let get = |k: &str| std::env::var(k).ok();
        Ok(Install {
            amd64: Source::parse(
                get("CUA_INSTALL_URL").as_deref(),
                get("CUA_INSTALL_SHA256").as_deref(),
            )?,
            arm64: Source::parse(
                get("CUA_INSTALL_URL_ARM64").as_deref(),
                get("CUA_INSTALL_SHA256_ARM64").as_deref(),
            )?,
        })
    }

    /// Whether a host of `arch` can install the cua this one speaks: the
    /// release serves both, but when some arch installs a build of your own
    /// (`CUA_INSTALL_URL`), the others would get the release instead, a
    /// different cua that may not join, so only the arches with a build do.
    pub fn installs(&self, arch: &str) -> bool {
        let own = |s: &Source| matches!(s, Source::Tarball { .. });
        !(own(&self.amd64) || own(&self.arm64)) || own(self.for_arch(arch))
    }

    /// The source for `arch`.
    pub fn for_arch(&self, arch: &str) -> &Source {
        if arch == "arm64" {
            &self.arm64
        } else {
            &self.amd64
        }
    }
}

impl Source {
    /// A source from a URL (with `#sha256=<hex>` or `sha`); none: release.
    pub fn parse(url: Option<&str>, sha: Option<&str>) -> Result<Self> {
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
            return Err(Error::InvalidArgument(
                "the cua install URL must be an https:// URL".into(),
            ));
        }
        if url.contains('\'') || url.chars().any(char::is_whitespace) {
            return Err(Error::InvalidArgument(
                "the cua install URL has a quote or a space".into(),
            ));
        }
        let sha = frag
            .and_then(|f| f.strip_prefix("sha256="))
            .or(sha)
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidArgument(
                "a cua install URL needs its sha256 (`#sha256=<64 hex>` or the SHA256 variable)"
                    .into(),
            ));
        }
        Ok(Self::Tarball {
            url: url.to_string(),
            sha256: sha,
        })
    }
}

/// What one host's first boot does.
#[derive(Clone, Debug)]
pub struct HostBoot<'a> {
    /// Where cua comes from.
    pub source: &'a Source,
    /// The one-time pairing code (the only secret it carries: used once,
    /// valid an hour).
    pub code: &'a str,
    /// Its pool name.
    pub name: &'a str,
    /// The relay the code was minted on.
    pub relay: &'a str,
    /// The cloud it runs in (`hetzner`): its relay machine says so, so the
    /// pool knows it whichever machine coordinates.
    pub provider: &'a str,
    /// Extra environment (`CUA_ENV=staging`), as `K=V` words.
    pub env: &'a [String],
    /// Its cost per hour (shown in the pool).
    pub usd_per_hour: f64,
    /// Seconds until it powers off in any case (its absolute expiry is
    /// [`HostBoot::expires`]).
    pub ttl_secs: u64,
    /// Seconds without Spaces after which it powers off (0: never).
    pub idle_poweroff_secs: u64,
    /// Unix seconds it expires (the cua-expire timer reads it).
    pub expires: u64,
    /// Images pulled right after it joined.
    pub prepull: &'a [String],
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The host's first-boot script (cloud-init runs it once as root).
pub fn user_data(b: &HostBoot<'_>) -> String {
    let install = match b.source {
        // The same release as the cua that adds the host, so the two speak
        // the same protocol (the latest release may be older or newer).
        Source::Release => format!(
            "curl -fsSL --retry 5 {INSTALL_SH} | CUA_INSTALL_NONINTERACTIVE=1 sh -s -- --cli-only --no-onboarding --version {}\n",
            env!("CARGO_PKG_VERSION")
        ),
        Source::Tarball { url, sha256 } => format!(
            "curl -fsSL --retry 5 -o /tmp/cua-linux.tgz {}\n\
             echo '{sha256}  /tmp/cua-linux.tgz' | sha256sum -c -\n\
             tar -xzf /tmp/cua-linux.tgz -C /usr/local/bin cua cua-spacesd\n\
             rm -f /tmp/cua-linux.tgz\n\
             export CUA_SPACESD_BIN=/usr/local/bin/cua-spacesd\n",
            sh_quote(url)
        ),
    };
    let env: String = b
        .env
        .iter()
        .filter(|kv| kv.contains('=') && !kv.contains('\'') && !kv.contains('\n'))
        .map(|kv| format!("export {kv}\n"))
        .collect();
    let idle = if b.idle_poweroff_secs > 0 {
        format!(
            "# Backstop: power off after {secs} s with no Spaces (the scaler removes it sooner).\n\
             cat > /usr/local/bin/cua-idle-poweroff <<'IDLE'\n\
             #!/bin/bash\n\
             # Spaces are containers (Docker, runc or gVisor) or VMs (QEMU).\n\
             busy() {{ [ \"$(docker ps -q 2>/dev/null | wc -l)\" -gt 0 ] || pgrep -x qemu-system-x86_64 >/dev/null || pgrep -x qemu-system-aarch64 >/dev/null; }}\n\
             idle=0\n\
             while sleep 60; do\n\
               if busy; then idle=0; else idle=$((idle+60)); fi\n\
               if [ \"$idle\" -ge {secs} ]; then logger 'cua: idle, powering off'; shutdown -h now; fi\n\
             done\n\
             IDLE\n\
             chmod 755 /usr/local/bin/cua-idle-poweroff\n\
             cat > /etc/systemd/system/cua-idle-poweroff.service <<'UNIT'\n\
             [Unit]\n\
             Description=Cua: power off an idle elastic host\n\
             [Service]\n\
             ExecStart=/usr/local/bin/cua-idle-poweroff\n\
             Restart=always\n\
             [Install]\n\
             WantedBy=multi-user.target\n\
             UNIT\n\
             systemctl daemon-reload\n\
             systemctl enable --now cua-idle-poweroff.service\n",
            secs = b.idle_poweroff_secs
        )
    } else {
        String::new()
    };
    format!(
        "#!/bin/bash\n\
         # Cua elastic pool host: joins your pool (cua host setup) and ends at its TTL.\n\
         set -euo pipefail\n\
         exec > >(tee -a /var/log/cua-up.log >/dev/console) 2>&1\n\
         mkdir -p /etc/cua && echo {expires} > /etc/cua/expires\n\
         # TTL backstop: a unit that sleeps until the absolute expiry in\n\
         # /etc/cua/expires, so a reboot does not extend the host's life.\n\
         cat > /usr/local/bin/cua-expire <<'EXPIRE'\n\
         #!/bin/bash\n\
         left=$(( $(cat /etc/cua/expires) - $(date +%s) ))\n\
         if [ \"$left\" -gt 0 ]; then sleep \"$left\"; fi\n\
         logger 'cua: this host reached its TTL'\n\
         shutdown -h now 'cua: this host reached its TTL'\n\
         EXPIRE\n\
         chmod 755 /usr/local/bin/cua-expire\n\
         cat > /etc/systemd/system/cua-expire.service <<'UNIT'\n\
         [Unit]\n\
         Description=Cua: power off this elastic host at its TTL\n\
         [Service]\n\
         ExecStart=/usr/local/bin/cua-expire\n\
         [Install]\n\
         WantedBy=multi-user.target\n\
         UNIT\n\
         systemctl daemon-reload\n\
         systemctl enable --now cua-expire.service\n\
         export HOME=/root DEBIAN_FRONTEND=noninteractive PATH=/usr/local/bin:$PATH\n\
         # cua-spacesd links the X11 client libraries, a cua built where libopus\n\
         # is installed links it; Docker runs container Spaces.\n\
         pkgs='libx11-6 libxext6 libxi6 libopus0'\n\
         command -v docker >/dev/null || pkgs=\"$pkgs docker.io\"\n\
         for i in 1 2 3 4 5; do apt-get update -qq && apt-get install -y -qq $pkgs && break; sleep 5; done\n\
         systemctl enable --now docker\n\
         # gVisor for container Spaces. Pool hosts are shared, so a missing gVisor\n\
         # must not become silent runc: the boot fails (the boot log says why) and\n\
         # spacesd refuses runc (CUA_REQUIRE_GVISOR). --overlay2=none keeps the\n\
         # container's root filesystem on the host so it survives stop and start;\n\
         # --host-uds=create lets the Space create Unix sockets there (its\n\
         # supervisord's); --allow-suid lets the Space user sudo.\n\
         ( curl -fsSL https://gvisor.dev/archive.key | gpg --dearmor -o /usr/share/keyrings/gvisor-archive-keyring.gpg \\\n\
           && echo \"deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gvisor-archive-keyring.gpg] https://storage.googleapis.com/gvisor/releases release main\" > /etc/apt/sources.list.d/gvisor.list \\\n\
           && apt-get update -qq && apt-get install -y -qq runsc && runsc install -- {runsc_args} && systemctl reload docker ) \\\n\
           || {{ echo 'cua: gVisor could not be installed; refusing to run Spaces on runc (shared host)'; exit 1; }}\n\
         export CUA_REQUIRE_GVISOR=1\n\
         {install}\
         # Your devices reach the host's Spaces over the mesh (direct when\n\
         # they can, else relayed ciphertext); the relay's HTTPS path stays.\n\
         export CUA_MESH=prefer\n\
         {env}\
         export CUA_HOST_USD_PER_HOUR={usd}\n\
         export CUA_HOST_CLOUD={provider}\n\
         echo 'cua: joining the pool'\n\
         cua host setup --pair {code} --no-desktop --provide-spaces --max-spaces 0 --profile spare --name {name} --runner systemd --dedicated --relay {relay}\n\
         {idle}\
         {prepull}\
         echo 'cua: host ready'\n",
        runsc_args = RUNSC_ARGS,
        expires = b.expires,
        usd = b.usd_per_hour,
        code = sh_quote(b.code),
        name = sh_quote(b.name),
        relay = sh_quote(b.relay),
        provider = sh_quote(b.provider),
        prepull = prepull_lines(b.prepull),
    )
}

/// The flags runsc is registered with on a pool host.
///
/// `--overlay2=none` keeps a Space's root filesystem on the host (it
/// survives stop and start), which makes it gofer-backed: creating a Unix
/// socket there needs `--host-uds=create`, or the image's supervisord
/// cannot bind `/run/supervisor.sock`, loops, and no cua-spacesd ever
/// starts (every Space on the host hangs at boot). `create` only lets the
/// sandbox create sockets inside its own root filesystem and mounts; it
/// never connects to the host's sockets, so Spaces on a shared pool host
/// stay apart.
pub const RUNSC_ARGS: &str = "--allow-suid --overlay2=none --host-uds=create";

/// Pulls `images` in the background (one shared copy per host; Spaces use
/// overlays on it): a reference with a quote or a space is skipped.
fn prepull_lines(images: &[String]) -> String {
    let ok: Vec<String> = images
        .iter()
        .filter(|i| !i.is_empty() && !i.contains('\'') && !i.chars().any(char::is_whitespace))
        .map(|i| sh_quote(i))
        .collect();
    if ok.is_empty() {
        return String::new();
    }
    format!(
        "( for i in {}; do docker pull -q \"$i\" || true; done ) >/var/log/cua-prepull.log 2>&1 &\n",
        ok.join(" ")
    )
}

/// `k=v` tags a person adds to their hosts (`--tag`), refusing the keys
/// that prove ownership (`cua-...`) and those a cloud reserves.
pub fn user_tags(tags: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for t in tags {
        let (k, v) = t
            .split_once('=')
            .ok_or_else(|| Error::InvalidArgument(format!("--tag {t}: use KEY=VALUE")))?;
        let k = k.trim();
        if k.is_empty() || k.len() > 128 || v.len() > 256 {
            return Err(Error::InvalidArgument(format!(
                "--tag {t}: keys are 1 to 128 characters, values at most 256"
            )));
        }
        if k.starts_with("cua-") || k.starts_with("aws:") || k == "Name" {
            return Err(Error::InvalidArgument(format!(
                "--tag {t}: {k} is set by Cua (or the cloud) and cannot be overridden"
            )));
        }
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

/// Options only some hosts need (GPU images, gang ports).
#[derive(Clone, Debug, Default)]
pub struct HostOptions {
    /// Boot disk, GiB (default: the type's, at least 60).
    pub disk_gb: Option<u32>,
    /// A cloud image parameter to boot instead of the default (AWS SSM).
    pub image_parameter: String,
    /// TCP ports hosts of this kind open to each other only.
    pub peer_ports: Option<(u16, u16)>,
    /// The user's own tags.
    pub extra_tags: BTreeMap<String, String>,
    /// Extra `K=V` environment for the host's cua (`CUA_ENV=staging`).
    pub env: Vec<String>,
}

/// The hosts of one cua home, over its clouds.
pub struct Hosts {
    apis: Vec<Arc<dyn CloudApi>>,
    store: Arc<Store>,
    relay: Option<RelayAccess>,
    offers: Mutex<Option<(Instant, String, Vec<HostTypeOffer>)>>,
    /// Held while a name is picked and recorded, so concurrent adds never
    /// pick the same one.
    naming: tokio::sync::Mutex<()>,
    /// When each machine that did not answer was last asked about at its
    /// cloud (by cloud id and state).
    checked: Mutex<BTreeMap<String, Instant>>,
    /// Pairing codes already revoked for hosts that ended unjoined.
    revoked: Mutex<BTreeSet<String>>,
    /// How long an add waits for the relay to have room for a pairing code.
    code_wait: Duration,
    /// How long after its create a host its cloud does not list yet is
    /// still being created, not lost (cloud lists are eventually
    /// consistent: EC2 answers NotFound for an instance it just started).
    not_found_grace: Duration,
}

/// How long after its create a host its cloud does not list yet is still
/// being created (EC2 lists an instance it just started a few seconds
/// late).
pub const NOT_FOUND_GRACE: Duration = Duration::from_secs(180);

/// How often a host that does not answer is asked about at its cloud.
const LOST_CHECK: Duration = Duration::from_secs(30);

/// Whether a record is a host's machine.
pub fn is_host_record(r: &Resource) -> bool {
    r.resource_type == "instance"
        && matches!(
            r.extra.get(EXTRA_ROLE).map(String::as_str),
            Some(ROLE_HOST) | Some(LEGACY_POOL_HOST)
        )
}

impl Hosts {
    /// The hosts of `store`'s home over `apis`, joining `relay`.
    pub fn new(
        apis: Vec<Arc<dyn CloudApi>>,
        store: Arc<Store>,
        relay: Option<RelayAccess>,
    ) -> Self {
        Hosts {
            apis,
            store,
            relay,
            offers: Mutex::new(None),
            naming: tokio::sync::Mutex::new(()),
            checked: Mutex::new(BTreeMap::new()),
            revoked: Mutex::new(BTreeSet::new()),
            code_wait: Duration::from_secs(180),
            not_found_grace: NOT_FOUND_GRACE,
        }
    }

    /// How long a host its cloud does not list yet counts as being created
    /// (tests make it zero).
    pub fn with_not_found_grace(mut self, grace: Duration) -> Self {
        self.not_found_grace = grace;
        self
    }

    /// How long an add waits for room under the relay's cap on unused
    /// pairing codes (tests make it short).
    pub fn with_code_wait(mut self, wait: Duration) -> Self {
        self.code_wait = wait;
        self
    }

    /// Revokes the pairing code of every host that ended before it joined
    /// (listing marks them), freeing room under the relay's cap.
    async fn reap_unused_codes(&self) {
        let _ = self.list().await;
    }

    fn api(&self, provider: &str) -> Result<&Arc<dyn CloudApi>> {
        self.apis
            .iter()
            .find(|a| a.name() == provider)
            .ok_or_else(|| Error::InvalidArgument(format!("unknown cloud {provider:?}")))
    }

    fn io(e: std::io::Error) -> Error {
        Error::Io(e)
    }

    /// One audit line for host record `r` (actor `pool`).
    fn audit(&self, action: &str, r: &Resource, outcome: &str, detail: &str) {
        self.store
            .audit(audit::of_resource(action, r, "pool", outcome, detail));
    }

    fn connection(&self, provider: &str) -> Result<Connection> {
        self.store
            .connection(provider)
            .map_err(Self::io)?
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "{provider} is not connected here: `cua cloud connect {provider}` first"
                ))
            })
    }

    /// The host records of this home.
    pub fn records(&self) -> Result<Vec<Resource>> {
        Ok(self
            .store
            .resources()
            .map_err(Self::io)?
            .into_iter()
            .filter(is_host_record)
            .collect())
    }

    /// Every connected cloud's machine types, one row per type and region.
    pub async fn offers(&self) -> Result<Vec<HostTypeOffer>> {
        // Cached until it is old or a cloud is linked, unlinked or moved.
        let linked: String = self
            .apis
            .iter()
            .filter_map(|a| self.store.connection(a.name()).ok().flatten())
            .map(|c| format!("{}/{}/{}/{:?};", c.provider, c.region, c.zone, c.max_size))
            .collect();
        if let Some((at, sig, v)) = self.offers.lock().unwrap().as_ref()
            && at.elapsed() < OFFERS_TTL
            && *sig == linked
        {
            return Ok(self.with_room(v.clone()));
        }
        // Only machine types whose arch can install this cua: the pool never
        // buys a machine that cannot join.
        let install = Install::from_env().ok();
        let mut out = Vec::new();
        for api in &self.apis {
            let Ok(Some(conn)) = self.store.connection(api.name()) else {
                continue;
            };
            let caps = api.capabilities(&conn);
            if caps.supplies == Supplies::Nothing {
                continue;
            }
            let types = match api.host_types(&conn).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(provider = api.name(), "host types not read: {e}");
                    continue;
                }
            };
            for t in types {
                if install.as_ref().is_some_and(|i| !i.installs(&t.arch)) {
                    continue;
                }
                // Larger than the connection allows: never offered.
                if conn.max_size.is_some_and(|m| !m.fits(t.vcpus, t.memory_mb)) {
                    continue;
                }
                let mut regions: Vec<String> = if t.regions.is_empty() {
                    caps.regions.clone()
                } else {
                    t.regions
                        .iter()
                        .filter(|r| caps.regions.is_empty() || caps.regions.contains(r))
                        .cloned()
                        .collect()
                };
                regions.sort();
                regions.dedup();
                // The region you linked is where the pool puts machines
                // (latency, data residency, quotas): the plan and the
                // create see only it. `cua cloud hosts add --region` still
                // adds one elsewhere by hand.
                for region in regions {
                    let fallback = if conn.region.is_empty() || region == conn.region {
                        0
                    } else if let Some(n) = fallback_rank(api.name(), &conn.region, &region) {
                        n
                    } else {
                        continue;
                    };
                    let mut row = offer_row(api.name(), &t, &region, &caps);
                    row.fallback = fallback;
                    out.push(row);
                }
            }
        }
        *self.offers.lock().unwrap() = Some((Instant::now(), linked, out.clone()));
        Ok(self.with_room(out))
    }

    /// `offers` with each provider's room left (its limit minus the hosts
    /// of this home that run there).
    fn with_room(&self, mut v: Vec<HostTypeOffer>) -> Vec<HostTypeOffer> {
        let live: Vec<Resource> = self.records().unwrap_or_default();
        for o in v.iter_mut() {
            let used = live
                .iter()
                .filter(|r| r.provider == o.provider && r.state != "gone")
                .count() as u32;
            o.max_new = o.max_new.saturating_sub(used);
        }
        v
    }

    /// The hosts this home added, with their state.
    pub async fn list(&self) -> Result<Vec<CloudHost>> {
        let recs = self.records()?;
        if recs.is_empty() {
            return Ok(vec![]);
        }
        // `machines` is `None` when the relay could not be asked (then no
        // host is judged by its relay entry).
        let (paired, machines) = match &self.relay {
            Some(r) => (
                r.paired_machines().await.unwrap_or_default(),
                r.machine_states().await.ok(),
            ),
            None => (BTreeMap::new(), None),
        };
        let mut out = Vec::new();
        for r in recs {
            let mut h = cloud_host(&r);
            if h.machine.is_empty()
                && let Some(code) = r.extra.get("pair_code_id")
                && let Some(m) = paired.get(code)
            {
                h.machine = m.clone();
            }
            let entry = machines.as_ref().and_then(|m| m.get(&h.machine));
            h.state = match (r.pending(), entry) {
                (true, _) => "creating".into(),
                (false, Some((true, true))) => "joined".into(),
                (false, Some((true, false))) => "cordoned".into(),
                _ if r.state == "gone" => "gone".into(),
                // Not answering: lost when its cloud says the machine is
                // terminated or not found, or when the relay no longer
                // knows a machine that had joined (its lease expired).
                (false, entry) => {
                    let forgotten = entry.is_none() && !h.machine.is_empty() && machines.is_some();
                    if forgotten || self.terminated(&r, entry.is_some()).await {
                        let mut gone = r.clone();
                        gone.state = "gone".into();
                        if let Err(e) = self.store.record(gone) {
                            tracing::warn!(host = h.name, "lost host not recorded: {e}");
                        }
                        self.audit(
                            "host_lost",
                            &r,
                            "ok",
                            if forgotten {
                                "its relay lease expired"
                            } else {
                                "its cloud ended it"
                            },
                        );
                        tracing::warn!(
                            host = h.name,
                            machine = h.machine,
                            "pool host lost ({})",
                            if forgotten {
                                "its relay lease expired"
                            } else {
                                "its cloud ended it"
                            }
                        );
                        "gone".into()
                    } else if entry.is_some() {
                        "offline".into()
                    } else {
                        "booting".into()
                    }
                }
            };
            // A host that ended without joining leaves its code unused:
            // revoke it (once), or the relay's cap fills up with dead codes.
            if h.state == "gone"
                && h.machine.is_empty()
                && let Some(code) = r.extra.get("pair_code_id")
                && let Some(relay) = &self.relay
                && self.revoked.lock().unwrap().insert(code.clone())
                && relay.revoke_pairing_code(code).await.is_err()
            {
                self.revoked.lock().unwrap().remove(code);
            }
            out.push(h);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Whether host record `r`'s cloud says its machine is terminated or not
    /// found. Asked at most once per [`LOST_CHECK`] per machine and state
    /// (`offline`: it had joined; else booting), so a host that stops
    /// answering is asked about at once; an error is "not known to be
    /// lost".
    async fn terminated(&self, r: &Resource, offline: bool) -> bool {
        {
            let key = format!("{}/{offline}", r.id);
            let mut checked = self.checked.lock().unwrap();
            if checked.get(&key).is_some_and(|t| t.elapsed() < LOST_CHECK) {
                return false;
            }
            checked.insert(key, Instant::now());
        }
        let Ok(api) = self.api(&r.provider) else {
            return false;
        };
        let Ok(mut conn) = self.connection(&r.provider) else {
            return false;
        };
        if !r.region.is_empty() {
            conn.region = r.region.clone();
        }
        match api.describe(&conn, r).await {
            // Not listed: lost, unless it was only just created (its
            // cloud may not list it yet). A cloud that ended it says
            // "terminated" for a while, which counts at once.
            Ok(None) => {
                let age = model::now().saturating_sub(r.created);
                let lost = Duration::from_secs(age) >= self.not_found_grace;
                if !lost {
                    // Ask again on the next listing, not after LOST_CHECK.
                    self.checked
                        .lock()
                        .unwrap()
                        .remove(&format!("{}/{offline}", r.id));
                }
                lost
            }
            Ok(Some((state, _))) => matches!(state.as_str(), "terminated" | "gone"),
            Err(e) => {
                tracing::debug!(id = r.id, "host not described: {e}");
                false
            }
        }
    }

    /// The next free `<provider>-<n>` name.
    fn next_name(&self, provider: &str) -> Result<String> {
        let used: BTreeSet<u32> = self
            .records()?
            .iter()
            .filter_map(|r| {
                r.extra
                    .get("host")?
                    .strip_prefix(&format!("{provider}-"))?
                    .parse()
                    .ok()
            })
            .collect();
        let n = (1..).find(|n| !used.contains(n)).unwrap_or(1);
        Ok(format!("{provider}-{n}"))
    }

    /// Adds a host (see the module docs).
    pub async fn add(&self, req: &AddHost, opts: &HostOptions) -> Result<CloudHost> {
        let api = self.api(&req.provider)?.clone();
        let conn = self.connection(&req.provider)?;
        let caps = api.capabilities(&conn);
        if caps.supplies != Supplies::Hosts {
            return Err(Error::InvalidArgument(format!(
                "{} {}: add Spaces there directly, it has no hosts the pool controls",
                api.title(),
                if caps.supplies == Supplies::Spaces {
                    "runs Spaces itself"
                } else {
                    "is not an elastic pool cloud"
                }
            )));
        }
        let types = api.host_types(&conn).await?;
        let t = types
            .iter()
            .find(|t| t.name == req.type_name)
            .cloned()
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "{} has no machine type {:?} (offered: {})",
                    api.title(),
                    req.type_name,
                    types
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        let region = if req.region.is_empty() {
            conn.region.clone()
        } else {
            req.region.clone()
        };
        if !t.regions.is_empty() && !region.is_empty() && !t.regions.contains(&region) {
            return Err(Error::InvalidArgument(format!(
                "{} {} is not offered in {region} now (it is in: {})",
                api.title(),
                t.name,
                t.regions.join(", ")
            )));
        }
        crate::alerts::check_all(&self.store, model::now());
        crate::limits::check_new_machine(
            &conn,
            api.title(),
            Some(t.vcpus),
            Some(t.memory_mb),
            t.usd_per_hour,
            self.store
                .spent_for_limits(&conn, model::now())
                .await
                .map_err(Self::io)?,
        )?;
        let live = self
            .records()?
            .iter()
            .filter(|r| r.provider == req.provider && r.state != "gone")
            .count() as u32;
        if live >= caps.max_machines {
            return Err(Error::Cloud(format!(
                "{} allows {} machines here and this home runs {live}",
                api.title(),
                caps.max_machines
            )));
        }
        let relay = self.relay.clone().ok_or_else(|| {
            Error::Cloud(
                "cua.ai relay: hosts join your pool there; sign in with `cua auth login`".into(),
            )
        })?;
        let owner = self.store.owner_id().map_err(Self::io)?;
        let ttl = match req.ttl_secs {
            0 => DEFAULT_TTL_SECS,
            s => s.min(MAX_TTL_SECS),
        };
        let install = Install::from_env()?;
        if !install.installs(&t.arch) {
            return Err(Error::InvalidArgument(format!(
                "{} is an {} machine, and this cua installs hosts from a build of its own (CUA_INSTALL_URL) for the other arch only: \
                 pick a type of that arch, or set {} to a build for {}",
                t.name,
                t.arch,
                if t.arch == "arm64" {
                    "CUA_INSTALL_URL_ARM64"
                } else {
                    "CUA_INSTALL_URL"
                },
                t.arch
            )));
        }
        let source = install.for_arch(&t.arch).clone();
        // The code is minted last, once everything else checked out, and
        // only when the relay has room: it caps the unused codes of an
        // account, so a burst of launches waits (with backoff, reaping
        // the codes of hosts that ended unjoined) instead of failing.
        let mut waited = Duration::ZERO;
        let mut step = Duration::from_millis(500);
        let (naming, name, code_id, code) = loop {
            let naming = self.naming.lock().await;
            let name = if req.name.is_empty() {
                self.next_name(&req.provider)?
            } else {
                req.name.clone()
            };
            match relay.mint_pairing_code(&name, CODE_TTL_SECS).await {
                Ok((id, code)) => break (naming, name, id, code),
                Err(e) if crate::relay::is_code_cap(&e) => {
                    drop(naming);
                    if waited >= self.code_wait {
                        return Err(Error::Cloud(format!(
                            "cua.ai relay: too many hosts are starting at once (the relay allows \
                             a few unused pairing codes per account); waited {} s, try again in a \
                             minute",
                            waited.as_secs()
                        )));
                    }
                    self.reap_unused_codes().await;
                    let pause = step + Duration::from_millis(u64::from(rand::random::<u8>()) * 2);
                    tokio::time::sleep(pause).await;
                    waited += pause;
                    step = (step * 2).min(Duration::from_secs(8));
                }
                Err(e) => return Err(e),
            }
        };
        let created = model::now();
        let expires = created + ttl;
        let mut tags = model::tags(&owner, &format!("host:{name}"), "", expires);
        tags.insert(model::tag::ROLE.into(), ROLE_HOST.into());
        for (k, v) in &opts.extra_tags {
            tags.entry(k.clone()).or_insert_with(|| v.clone());
        }
        let resource_name = model::label_value(&format!(
            "cua-host-{name}-{:06x}",
            rand::random::<u32>() & 0xff_ffff
        ));
        let billing = caps.billing;
        let extra = BTreeMap::from([
            (EXTRA_ROLE.to_string(), ROLE_HOST.to_string()),
            ("host".to_string(), name.clone()),
            ("instance_type".to_string(), t.name.clone()),
            ("arch".to_string(), t.arch.clone()),
            ("usd_per_hour".to_string(), format!("{:.4}", t.usd_per_hour)),
            (
                "billing_step_secs".to_string(),
                billing.granularity_secs.to_string(),
            ),
            (
                "billing_min_secs".to_string(),
                billing.minimum_secs.to_string(),
            ),
            ("pair_code_id".to_string(), code_id.clone()),
            ("pinned".to_string(), req.pinned.to_string()),
            ("vcpus".to_string(), t.vcpus.to_string()),
            ("memory_mb".to_string(), t.memory_mb.to_string()),
            ("kvm".to_string(), t.kvm.to_string()),
            ("quota_pool".to_string(), t.quota_pool.clone()),
        ]);
        let pending = Resource {
            provider: req.provider.clone(),
            resource_type: "instance".into(),
            name: resource_name.clone(),
            region: region.clone(),
            sandbox: format!("host:{name}"),
            created,
            expires,
            tags: tags.clone(),
            state: "pending".into(),
            extra: extra.clone(),
            ..Default::default()
        };
        // Recorded before the cloud call: a create that dies half way is
        // still found (`cua cloud sweep`); the name is taken from here on.
        self.store.record(pending.clone()).map_err(Self::io)?;
        self.audit(
            "host_recorded",
            &pending,
            "ok",
            &format!("{} in {region}, recorded before the cloud call", t.name),
        );
        drop(naming);
        let spec = ProvisionSpec {
            name: resource_name,
            family: "host".into(),
            kind: "vm".into(),
            machine_type: t.name.clone(),
            disk_gb: opts.disk_gb.unwrap_or(t.disk_gb.max(60)),
            user_data: user_data(&HostBoot {
                source: &source,
                code: &code,
                name: &name,
                relay: &relay.url,
                provider: &req.provider,
                env: &opts.env,
                usd_per_hour: t.usd_per_hour,
                ttl_secs: ttl,
                // A cloud that bills a stopped machine gains nothing from
                // powering it off, and the pool loses a warm host: only the
                // TTL ends it there when nobody removes it.
                idle_poweroff_secs: if caps.billing.stopped_bills {
                    0
                } else {
                    IDLE_POWEROFF_SECS
                },
                expires,
                prepull: &req.prepull,
            }),
            arch: t.arch.clone(),
            tags,
            ttl_secs: ttl,
            image_parameter: opts.image_parameter.clone(),
            peer_ports: opts.peer_ports,
            role: ROLE_HOST.into(),
            region: region.clone(),
            ..Default::default()
        };
        let made = match api.provision(&conn, &spec).await {
            Ok(made) => made,
            Err(e) => {
                let _ = self.store.forget(&pending);
                let _ = relay.revoke_pairing_code(&code_id).await;
                self.audit("host_create_failed", &pending, "failed", &e.to_string());
                return Err(e);
            }
        };
        let _ = self.store.forget(&pending);
        let mut host = None;
        for mut r in made {
            if r.resource_type == "instance" && host.is_none() {
                r.sandbox = pending.sandbox.clone();
                let mut e = extra.clone();
                e.extend(r.extra.clone());
                // The price the provider saw wins over the table's.
                if let Some(p) = r.extra.get("usd_per_hour") {
                    e.insert("usd_per_hour".into(), p.clone());
                }
                r.extra = e;
                if r.region.is_empty() {
                    r.region = region.clone();
                }
                host = Some(r.clone());
            }
            // Shared resources (the outbound-only firewall) are recorded as
            // the provider made them.
            if let Err(e) = self.store.record(r) {
                tracing::warn!("cloud state not written: {e}");
            }
        }
        let host = host.ok_or_else(|| {
            Error::Cloud(format!("{}: the create returned no machine", api.title()))
        })?;
        self.audit(
            "host_created",
            &host,
            "ok",
            &format!("{} in {region}, ttl {} h", t.name, ttl / 3600),
        );
        let mut h = cloud_host(&host);
        h.state = "booting".into();
        *self.offers.lock().unwrap() = None;
        Ok(h)
    }

    /// Adds `count` hosts, all or none: when one cannot be created, the ones
    /// already created are removed again and the error says so.
    pub async fn add_many(
        &self,
        req: &AddHost,
        count: u32,
        opts: &HostOptions,
    ) -> Result<Vec<CloudHost>> {
        let mut made: Vec<CloudHost> = Vec::new();
        for i in 0..count.max(1) {
            let mut r = req.clone();
            if count > 1 && !req.name.is_empty() {
                r.name = format!("{}-{}", req.name, i + 1);
            }
            match self.add(&r, opts).await {
                Ok(h) => made.push(h),
                Err(e) => {
                    let mut left = Vec::new();
                    for h in &made {
                        if let Err(re) = self.remove(&h.name).await {
                            left.push(format!("{} ({re})", h.name));
                        }
                    }
                    let mut msg = format!("host {} of {count} could not be created: {e}", i + 1);
                    if !made.is_empty() {
                        msg.push_str(&format!(
                            "; the {} already created {} removed",
                            made.len(),
                            if left.is_empty() {
                                "were".to_string()
                            } else {
                                format!("were not all (still recorded: {})", left.join(", "))
                            }
                        ));
                    }
                    return Err(Error::Cloud(msg));
                }
            }
        }
        Ok(made)
    }

    /// Waits until hosts `names` joined the pool (or `timeout`); calls
    /// `joined` once for each as it does. Returns the hosts, joined first.
    pub async fn wait_joined(
        &self,
        names: &[String],
        timeout: Duration,
        mut joined: impl FnMut(&CloudHost),
    ) -> Result<Vec<CloudHost>> {
        let started = Instant::now();
        let mut done: BTreeMap<String, CloudHost> = BTreeMap::new();
        loop {
            for h in self.list().await? {
                if names.contains(&h.name) && h.state == "joined" && !done.contains_key(&h.name) {
                    joined(&h);
                    done.insert(h.name.clone(), h);
                }
            }
            if done.len() >= names.len() || started.elapsed() >= timeout {
                let mut out: Vec<CloudHost> = done.into_values().collect();
                for h in self.list().await? {
                    if names.contains(&h.name) && !out.iter().any(|x| x.name == h.name) {
                        out.push(h);
                    }
                }
                return Ok(out);
            }
            tokio::time::sleep(
                Duration::from_secs(if timeout < Duration::from_secs(10) {
                    0
                } else {
                    5
                })
                .max(Duration::from_millis(50)),
            )
            .await;
        }
    }

    /// Removes host `name`: its machine (owner-checked), its relay entry and
    /// its records.
    pub async fn remove(&self, name: &str) -> Result<()> {
        let recs: Vec<Resource> = self
            .records()?
            .into_iter()
            .filter(|r| r.extra.get("host").map(String::as_str) == Some(name))
            .collect();
        if recs.is_empty() {
            return Err(Error::NotFound(format!(
                "host {name} (not one this home added: `cua cloud hosts ls`)"
            )));
        }
        let owner = self.store.owner_id().map_err(Self::io)?;
        for r in recs {
            let api = self.api(&r.provider)?.clone();
            let mut conn = self.connection(&r.provider)?;
            if !r.region.is_empty() {
                conn.region = r.region.clone();
            }
            if !r.pending()
                && let Err(e) = api.delete(&conn, &r, &owner).await
            {
                self.audit("host_removed", &r, "failed", &e.to_string());
                return Err(e);
            }
            self.drop_relay(&r, name).await;
            self.store.forget(&r).map_err(Self::io)?;
            self.audit("host_removed", &r, "ok", &format!("host {name}"));
        }
        *self.offers.lock().unwrap() = None;
        Ok(())
    }

    /// Removes host record `r`'s relay machine (found through its pairing
    /// code when the record never saw it join), or revokes its pairing
    /// code when it never joined.
    pub(crate) async fn drop_relay(&self, r: &Resource, name: &str) {
        let Some(relay) = &self.relay else {
            return;
        };
        let machine = if r.machine.is_empty() {
            match r.extra.get("pair_code_id") {
                Some(code) => relay
                    .paired_machines()
                    .await
                    .ok()
                    .and_then(|m| m.get(code).cloned())
                    .unwrap_or_default(),
                None => String::new(),
            }
        } else {
            r.machine.clone()
        };
        if !machine.is_empty() {
            if let Err(e) = relay.forget(&machine).await {
                tracing::warn!(machine, "relay entry of host {name} not removed: {e}");
            }
        } else if let Some(code) = r.extra.get("pair_code_id") {
            let _ = relay.revoke_pairing_code(code).await;
        }
    }

    /// Cordons host `name` (it takes no new Spaces and drains) or opens it
    /// again.
    pub async fn cordon(&self, name: &str, cordoned: bool) -> Result<()> {
        let host = self
            .list()
            .await?
            .into_iter()
            .find(|h| h.name == name)
            .ok_or_else(|| Error::NotFound(format!("host {name}")))?;
        if host.machine.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "{name} has not joined the pool yet"
            )));
        }
        let relay = self
            .relay
            .as_ref()
            .ok_or_else(|| Error::Cloud("cua.ai relay: not signed in".into()))?;
        relay.set_sharing(&host.machine, !cordoned).await
    }
}

/// One offer row.
/// Where the pool may add a machine when the region you linked has no
/// capacity for any type that fits: the provider's nearby regions, nearest
/// first (1, 2...), within the same jurisdiction. Only Hetzner's German
/// locations and Helsinki (EU) for now; other clouds stay in the linked
/// region (their quotas are per region).
pub fn fallback_rank(provider: &str, linked: &str, region: &str) -> Option<u8> {
    let order: &[&str] = match (provider, linked) {
        ("hetzner", "fsn1") => &["nbg1", "hel1"],
        ("hetzner", "nbg1") => &["fsn1", "hel1"],
        ("hetzner", "hel1") => &["fsn1", "nbg1"],
        _ => &[],
    };
    order.iter().position(|r| *r == region).map(|i| i as u8 + 1)
}

pub fn offer_row(
    provider: &str,
    t: &HostType,
    region: &str,
    caps: &crate::model::Capabilities,
) -> HostTypeOffer {
    let mut kinds = vec!["container".to_string()];
    if t.kvm {
        kinds.push("vm".into());
        kinds.push("microvm".into());
    }
    HostTypeOffer {
        provider: provider.into(),
        type_name: t.name.clone(),
        region: region.into(),
        arch: t.arch.clone(),
        vcpus: t.vcpus,
        memory_mb: t.memory_mb,
        disk_gb: t.disk_gb,
        kvm: t.kvm,
        kinds,
        gpu: t.gpu.clone(),
        usd_per_hour: t.usd_per_hour,
        billing_step_secs: caps.billing.granularity_secs,
        billing_min_secs: caps.billing.minimum_secs,
        boot_secs: t.boot_secs,
        supplies: caps.supplies.as_str().into(),
        max_new: caps.max_machines,
        quota: if t.quota_pool.is_empty() {
            String::new()
        } else {
            format!("{provider}/{}", t.quota_pool)
        },
        quota_vcpus: caps.vcpu_quotas.get(&t.quota_pool).copied().unwrap_or(0),
        fallback: 0,
    }
}

/// A host record as the wire row.
pub fn cloud_host(r: &Resource) -> CloudHost {
    let num = |k: &str| {
        r.extra
            .get(k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    CloudHost {
        name: r
            .extra
            .get("host")
            .cloned()
            .unwrap_or_else(|| r.name.clone()),
        provider: r.provider.clone(),
        type_name: r
            .extra
            .get("instance_type")
            .or_else(|| r.extra.get("machine_type"))
            .cloned()
            .unwrap_or_default(),
        region: r.region.clone(),
        id: r.id.clone(),
        machine: r.machine.clone(),
        state: r.state.clone(),
        started: r.created,
        expires: r.expires,
        usd_per_hour: num("usd_per_hour"),
        billing_step_secs: num("billing_step_secs").max(1.0) as u64,
        billing_min_secs: num("billing_min_secs") as u64,
        pinned: r.extra.get("pinned").map(String::as_str) == Some("true"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot<'a>(source: &'a Source, env: &'a [String]) -> HostBoot<'a> {
        HostBoot {
            source,
            code: "K7QM-4XPB-2RHD",
            name: "hetzner-1",
            relay: "https://relay.cua.ai",
            provider: "hetzner",
            env,
            usd_per_hour: 0.0684,
            ttl_secs: 3600,
            idle_poweroff_secs: 1200,
            expires: 1_800_000_000,
            prepull: &[],
        }
    }

    #[test]
    fn the_first_boot_joins_the_pool_and_ends() {
        let env = vec!["CUA_ENV=staging".to_string()];
        let ud = user_data(&boot(&Source::Release, &env));
        assert!(ud.starts_with("#!/bin/bash\n"));
        assert!(ud.contains(
            "cua host setup --pair 'K7QM-4XPB-2RHD' --no-desktop --provide-spaces --max-spaces 0 \
             --profile spare --name 'hetzner-1' --runner systemd --dedicated --relay 'https://relay.cua.ai'"
        ));
        // The TTL is an absolute expiry in a unit, so it survives a reboot.
        assert!(
            !ud.contains("shutdown -h +"),
            "a one-shot timer is lost on reboot"
        );
        assert!(ud.contains("systemctl enable --now cua-expire.service"));
        assert!(ud.contains("$(cat /etc/cua/expires)"));
        assert!(ud.contains("shutdown -h now 'cua: this host reached its TTL'"));
        assert!(ud.contains("echo 1800000000 > /etc/cua/expires"));
        assert!(ud.contains("cua-idle-poweroff"), "idle backstop");
        assert!(ud.contains("[ \"$idle\" -ge 1200 ]"));
        assert!(ud.contains("export CUA_ENV=staging"));
        // The mesh is on before setup, so the service keeps it; a person's
        // own CUA_MESH (in `env`, exported after) still wins.
        let mesh = ud.find("export CUA_MESH=prefer").expect("mesh on");
        assert!(mesh < ud.find("export CUA_ENV=staging").unwrap());
        assert!(mesh < ud.find("cua host setup --pair").unwrap());
        assert!(ud.contains("export CUA_HOST_USD_PER_HOUR=0.0684"));
        // Its relay machine says which cloud it runs in, before it joins.
        let cloud = ud.find("export CUA_HOST_CLOUD='hetzner'").expect("cloud");
        assert!(cloud < ud.find("cua host setup --pair").unwrap());
        assert!(ud.contains("install.sh"));
        // A cua linked against libopus starts on the host.
        assert!(
            ud.contains("pkgs='libx11-6 libxext6 libxi6 libopus0'"),
            "{ud}"
        );
        // The host installs this cua's own release, not the latest.
        assert!(
            ud.contains(&format!("--version {}\n", env!("CARGO_PKG_VERSION"))),
            "{ud}"
        );
        assert!(ud.contains("runsc install"), "gVisor for containers");
        // J-E6: the container root filesystem must outlive stop and start,
        // so runsc is not left on its (sandbox-lifetime) default overlay.
        assert!(
            ud.contains("runsc install -- --allow-suid --overlay2=none --host-uds=create "),
            "{ud}"
        );
        // With the root filesystem gofer-backed (overlay2=none), a Space
        // can create Unix sockets in it only with --host-uds=create: without
        // it the desktop image's supervisord never starts (no spacesd).
        assert!(RUNSC_ARGS.contains("--host-uds=create"), "{RUNSC_ARGS}");
        // Isolation axiom: no silent runc on a shared host.
        assert!(
            ud.contains("exit 1; }"),
            "install failure must stop the boot"
        );
        assert!(!ud.contains("containers use runc"));
        let req = ud
            .find("export CUA_REQUIRE_GVISOR=1")
            .expect("require gVisor");
        assert!(req < ud.find("cua host setup --pair").unwrap());
        // No account credential on the machine: only the one-time code.
        for secret in [
            "CUA_TOKEN",
            "CUA_CLIENT_SECRET",
            "access_token",
            "HCLOUD_TOKEN",
        ] {
            assert!(!ud.contains(secret), "{secret} in user data");
        }
        let tar = Source::Tarball {
            url: "https://example.com/cua-linux-arm64.tgz".into(),
            sha256: "a".repeat(64),
        };
        let ud = user_data(&boot(&tar, &[]));
        assert!(ud.contains("sha256sum -c -"));
        assert!(ud.contains("'https://example.com/cua-linux-arm64.tgz'"));
        assert!(!ud.contains("install.sh |"));
        // Images pulled after it joined.
        let images = vec![
            "ghcr.io/trycua/linux:24.04-slim".to_string(),
            "bad image".to_string(),
        ];
        let mut b = boot(&Source::Release, &[]);
        b.prepull = &images;
        let ud = user_data(&b);
        assert!(ud.contains("docker pull -q \"$i\""), "{ud}");
        assert!(ud.contains("'ghcr.io/trycua/linux:24.04-slim'"));
        assert!(!ud.contains("bad image"));
        // No idle backstop when asked for none.
        let mut b = boot(&Source::Release, &[]);
        b.idle_poweroff_secs = 0;
        assert!(!user_data(&b).contains("cua-idle-poweroff"));
    }

    /// A full linked region falls back to the provider's nearby ones,
    /// nearest first, and never across a jurisdiction or another cloud.
    #[test]
    fn a_full_region_falls_back_nearby() {
        assert_eq!(fallback_rank("hetzner", "fsn1", "nbg1"), Some(1));
        assert_eq!(fallback_rank("hetzner", "fsn1", "hel1"), Some(2));
        assert_eq!(fallback_rank("hetzner", "fsn1", "ash"), None);
        assert_eq!(fallback_rank("hetzner", "fsn1", "fsn1"), None);
        assert_eq!(fallback_rank("aws", "us-west-2", "us-east-1"), None);
    }

    /// The pool buys only machines that can install this cua: with the
    /// release, both arches; with a build of your own for one arch
    /// (`CUA_INSTALL_URL`, amd64), only that arch (an Arm machine would get
    /// the release, a different cua, and never join).
    #[test]
    fn hosts_are_only_of_an_arch_that_installs_this_cua() {
        let own = Source::Tarball {
            url: "https://x/cua.tgz".into(),
            sha256: "a".repeat(64),
        };
        let release = Install {
            amd64: Source::Release,
            arm64: Source::Release,
        };
        assert!(release.installs("amd64") && release.installs("arm64"));
        let amd64_build = Install {
            amd64: own.clone(),
            arm64: Source::Release,
        };
        assert!(amd64_build.installs("amd64"));
        assert!(!amd64_build.installs("arm64"));
        let both = Install {
            amd64: own.clone(),
            arm64: own,
        };
        assert!(both.installs("amd64") && both.installs("arm64"));
    }

    #[test]
    fn install_sources_need_https_and_a_hash() {
        assert_eq!(Source::parse(None, None).unwrap(), Source::Release);
        assert_eq!(
            Source::parse(Some(INSTALL_SH), None).unwrap(),
            Source::Release
        );
        assert!(Source::parse(Some("http://x/y.tgz"), Some(&"a".repeat(64))).is_err());
        assert!(Source::parse(Some("https://x/y.tgz"), None).is_err());
        let s = Source::parse(
            Some(&format!("https://x/y.tgz#sha256={}", "B".repeat(64))),
            None,
        )
        .unwrap();
        assert_eq!(
            s,
            Source::Tarball {
                url: "https://x/y.tgz".into(),
                sha256: "b".repeat(64)
            }
        );
        assert!(Source::parse(Some("https://x/a b.tgz"), Some(&"a".repeat(64))).is_err());
    }

    #[test]
    fn host_records_and_rows() {
        let r = Resource {
            provider: "hetzner".into(),
            id: "123".into(),
            resource_type: "instance".into(),
            region: "fsn1".into(),
            created: 100,
            expires: 3700,
            extra: BTreeMap::from([
                (EXTRA_ROLE.into(), ROLE_HOST.into()),
                ("host".into(), "hetzner-2".into()),
                ("instance_type".into(), "cax41".into()),
                ("usd_per_hour".into(), "0.0684".into()),
                ("billing_step_secs".into(), "3600".into()),
                ("billing_min_secs".into(), "3600".into()),
                ("pinned".into(), "true".into()),
            ]),
            ..Default::default()
        };
        assert!(is_host_record(&r));
        let h = cloud_host(&r);
        assert_eq!(h.name, "hetzner-2");
        assert_eq!(h.type_name, "cax41");
        assert_eq!(h.billing_step_secs, 3600);
        assert!(h.pinned);
        assert!((h.usd_per_hour - 0.0684).abs() < 1e-9);
        let mut legacy = r.clone();
        legacy.extra.insert(EXTRA_ROLE.into(), "pool-host".into());
        assert!(is_host_record(&legacy));
        let mut sandbox = r;
        sandbox.extra.remove(EXTRA_ROLE);
        assert!(!is_host_record(&sandbox));
    }
}
