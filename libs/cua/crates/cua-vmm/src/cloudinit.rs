//! cloud-init NoCloud seed generation.
//!
//! The seed is an ISO 9660 image labelled `CIDATA` containing `meta-data` and
//! `user-data` (and optionally `network-config`). We write the ISO ourselves —
//! it is a few hundred bytes of fixed structures — so seeding a VM needs no
//! `genisoimage`/`xorriso`/`hdiutil` on the host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::error::{Result, VmmError};
use crate::types::{GuestOs, SshAccess, StartSpec};

/// Guest environment variable carrying the spacesd token.
pub const ENV_TOKEN_VAR: &str = "CUA_ENV_TOKEN";
/// Persistent copy of the token (survives reboots; `/run` is tmpfs).
pub const GUEST_TOKEN_PERSISTENT: &str = "/etc/cua/env-token";
/// Where cua-spacesd reads its token by default.
pub const GUEST_TOKEN_RUNTIME: &str = "/run/cua/env-token";
/// `EnvironmentFile` of the cua-spacesd systemd unit.
pub const GUEST_ENV_FILE: &str = "/etc/cua/spacesd.env";

/// NoCloud seed contents.
#[derive(Clone, Debug, Default)]
pub struct Seed {
    pub meta_data: String,
    pub user_data: String,
    pub network_config: Option<String>,
}

impl Seed {
    /// A seed that creates `user` with passwordless sudo and the given
    /// authorized public key, and sets the hostname.
    pub fn with_ssh_user(instance_id: &str, user: &str, public_key: &str) -> Self {
        let key = public_key.trim();
        Self {
            meta_data: format!("instance-id: {instance_id}\nlocal-hostname: {instance_id}\n"),
            user_data: format!(
                "#cloud-config\n\
                 users:\n\
                 \x20 - default\n\
                 \x20 - name: {user}\n\
                 \x20   sudo: ALL=(ALL) NOPASSWD:ALL\n\
                 \x20   shell: /bin/bash\n\
                 \x20   lock_passwd: true\n\
                 \x20   ssh_authorized_keys:\n\
                 \x20     - {key}\n\
                 ssh_pwauth: false\n\
                 disable_root: true\n"
            ),
            network_config: None,
        }
    }

    /// A seed with caller-provided user-data.
    pub fn raw(instance_id: &str, user_data: &str) -> Self {
        Self {
            meta_data: format!("instance-id: {instance_id}\nlocal-hostname: {instance_id}\n"),
            user_data: user_data.to_string(),
            network_config: None,
        }
    }

    /// Adds the guest environment (`StartSpec::env`, typically
    /// `CUA_ENV_TOKEN`) to the seed. See [`guest_env_cloud_config`].
    ///
    /// Our own generated cloud-config gets the keys appended; caller-provided
    /// user-data is wrapped in a MIME multipart archive next to ours, with a
    /// `merge_how` that appends lists, so neither side's `runcmd` / `write_files`
    /// replaces the other's.
    pub fn with_guest_env(self, env: &BTreeMap<String, String>, generated: bool) -> Result<Self> {
        self.with_guest_config(env, None, generated)
    }

    /// [`Self::with_guest_env`] plus the sandbox command
    /// ([`guest_cloud_config`]).
    pub fn with_guest_config(
        mut self,
        env: &BTreeMap<String, String>,
        command: Option<&[String]>,
        generated: bool,
    ) -> Result<Self> {
        let Some(part) = guest_cloud_config(env, command)? else {
            return Ok(self);
        };
        if self.user_data.trim().is_empty() {
            self.user_data = part;
        } else if generated {
            let body = part.strip_prefix("#cloud-config\n").unwrap_or(&part);
            if !self.user_data.ends_with('\n') {
                self.user_data.push('\n');
            }
            self.user_data.push_str(body);
        } else {
            self.user_data = multipart(&[&self.user_data, &part]);
        }
        Ok(self)
    }

    /// Render the seed as an ISO 9660 image.
    pub fn to_iso(&self) -> Vec<u8> {
        let mut files: Vec<(&str, &[u8])> = vec![
            ("META-DATA", self.meta_data.as_bytes()),
            ("USER-DATA", self.user_data.as_bytes()),
        ];
        if let Some(n) = &self.network_config {
            files.push(("NETWORK-CONFIG", n.as_bytes()));
        }
        write_iso("CIDATA", &mut files)
    }

    /// Write the ISO to `path`, readable by the owner only (the seed can
    /// carry the spacesd token).
    pub fn write_iso(&self, path: &Path) -> Result<()> {
        crate::host::write_private(path, &self.to_iso())?;
        Ok(())
    }
}

/// What a VM guest without cloud-init (Windows under QEMU) cannot take:
/// a command or a workload environment. Checked before anything is
/// created, so a refused start leaves nothing behind.
pub fn check_guest_support(spec: &StartSpec) -> Result<()> {
    let linux = spec.os == GuestOs::Linux;
    if !linux && spec.command.as_ref().is_some_and(|c| !c.is_empty()) {
        // Only Linux guests run cloud-init; never drop the command silently.
        return Err(VmmError::Unsupported {
            backend: "vm",
            op: "a sandbox command on a non-Linux VM guest (bake it into the image)",
        });
    }
    if !linux && !workload_env(&spec.env).is_empty() {
        // Only Linux guests run cloud-init, and a Windows guest has no other
        // channel the host could write to (no cloud-init, no SSH, no guest
        // agent in the image). Never drop the environment silently. The
        // spacesd token is not workload env: a driver without it starts
        // in bootstrap mode and the SDK installs the token with `Init`.
        return Err(VmmError::Unsupported {
            backend: "vm",
            op: "environment variables on a non-Linux VM guest (the image has no cloud-init \
                 or other provisioning channel; bake them into the image)",
        });
    }
    Ok(())
}

/// The seed for a Linux VM start: caller user-data, else the SSH user, plus
/// the guest environment. A Linux guest always gets a seed, even an empty
/// `#cloud-config`: without a NoCloud datasource cloud-init disables itself,
/// and images that leave first-boot work to it (SSH host keys on Ubuntu
/// cloud images and libs/images) never start sshd. `None` only for guests
/// that do not run cloud-init (macOS/Windows) and have no user-data.
///
/// The instance id carries a hash of the environment, so a restart with a new
/// token re-runs cloud-init's per-instance modules (`write_files`).
pub fn seed_for_spec(name: &str, spec: &StartSpec) -> Result<Option<Seed>> {
    let linux = spec.os == GuestOs::Linux;
    check_guest_support(spec)?;
    let env: BTreeMap<String, String> = if linux {
        spec.env.clone()
    } else {
        BTreeMap::new()
    };
    let command = if linux { spec.command.as_deref() } else { None };
    let instance_id = instance_id(name, &env, command);
    let (seed, generated) = if let Some(ud) = &spec.cloud_init_user_data {
        (Seed::raw(&instance_id, ud), false)
    } else if let Some(ssh) = &spec.ssh {
        let pubkey_path = PathBuf::from(format!("{}.pub", ssh.private_key.display()));
        let pubkey = std::fs::read_to_string(&pubkey_path).map_err(|e| {
            VmmError::invalid(format!(
                "cannot read public key {}: {e}",
                pubkey_path.display()
            ))
        })?;
        (Seed::with_ssh_user(&instance_id, &ssh.user, &pubkey), true)
    } else if linux {
        (Seed::raw(&instance_id, "#cloud-config\n"), true)
    } else {
        return Ok(None);
    };
    let mut seed = seed.with_guest_config(&env, command, generated)?;
    // Keep the guest hostname the instance name.
    seed.meta_data = format!("instance-id: {instance_id}\nlocal-hostname: {name}\n");
    Ok(Some(seed))
}

/// The SSH access a Linux VM gets when the caller brought none: user `cua`
/// with a per-instance ed25519 key in `dir` (generated once with
/// `ssh-keygen`), so agentless exec works on any cloud-init image. `None`
/// (with a warning) when `ssh-keygen` is missing: the guest is still seeded.
pub async fn managed_ssh(dir: &Path) -> Option<SshAccess> {
    let key = dir.join("id_ed25519");
    if !key.exists() || !dir.join("id_ed25519.pub").exists() {
        let Some(bin) = crate::host::which("ssh-keygen") else {
            tracing::warn!("ssh-keygen not found; the VM gets no managed SSH key");
            return None;
        };
        let _ = std::fs::remove_file(&key);
        let path = key.display().to_string();
        let args = [
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "cua-vmm",
            "-f",
            path.as_str(),
        ];
        if let Err(e) = crate::host::run(&bin, &args).await {
            tracing::warn!(error = %e, "ssh-keygen failed; the VM gets no managed SSH key");
            return None;
        }
    }
    Some(SshAccess {
        user: "cua".into(),
        private_key: key,
        password: None,
    })
}

fn instance_id(name: &str, env: &BTreeMap<String, String>, command: Option<&[String]>) -> String {
    let command = command.filter(|c| !c.is_empty());
    if env.is_empty() && command.is_none() {
        return name.to_string();
    }
    let mut h = Sha256::new();
    for (k, v) in env {
        h.update(k.as_bytes());
        h.update([0]);
        h.update(v.as_bytes());
        h.update([0]);
    }
    if let Some(argv) = command {
        h.update([1]);
        for a in argv {
            h.update(a.as_bytes());
            h.update([0]);
        }
    }
    let digest = h.finalize();
    let hex: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    format!("{name}-{hex}")
}

/// The workload's environment: `env` without the spacesd token
/// ([`ENV_TOKEN_VAR`]), which is delivered on its own channel.
pub fn workload_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .filter(|(k, _)| k.as_str() != ENV_TOKEN_VAR)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Checks a guest environment: names are shell identifiers and values have
/// no control characters (they would break the files that carry them).
pub fn validate_guest_env(env: &BTreeMap<String, String>) -> Result<()> {
    for (k, v) in env {
        let ident = !k.is_empty()
            && !k.starts_with(|c: char| c.is_ascii_digit())
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ident {
            return Err(VmmError::invalid(format!(
                "guest env name {k:?} is not an identifier"
            )));
        }
        if v.chars().any(char::is_control) {
            return Err(VmmError::invalid(format!(
                "guest env {k} contains control characters"
            )));
        }
    }
    Ok(())
}

/// A `#cloud-config` that delivers `env` to the guest (see
/// [`guest_cloud_config`] without a command).
pub fn guest_env_cloud_config(env: &BTreeMap<String, String>) -> Result<Option<String>> {
    guest_cloud_config(env, None)
}

/// A `#cloud-config` that delivers `env` and runs `command` in the guest.
///
/// The spacesd token (`CUA_ENV_TOKEN`) follows the cloud contract (the
/// Fleet claim Secret): a root-owned `0600` file at `/run/cua/env-token`,
/// which cua-spacesd reads in await-token-file mode.
///
/// - `write_files`: `/etc/cua/env-token` (persistent copy) and
///   `/run/cua/env-token`, both `root:root 0600`. They run in
///   `cloud-init.service`, before cua-spacesd starts on first boot.
/// - `bootcmd` (every boot, before `write_files`): on images that ship
///   `cua-env-token-sync.service` (libs/images), `/run/cua` becomes a private
///   tmpfs (`0700`) like the Secret mount, cua-spacesd and the root token-sync
///   helper get `CUA_ENV_AWAIT_TOKEN_FILE=1` (unit drop-ins), and the helper
///   mirrors the file to `/run/cua-env/env-token` (`0600`, the
///   driver's user). Other images get a plain `/run/cua`. On later boots
///   the persistent copy is installed at `/run/cua/env-token` again.
/// - `runcmd` (re)starts the token sync and cua-spacesd, so a driver that
///   raced cloud-init picks the token up.
/// - every other variable → `/etc/cua/spacesd.env` (`0600`), the driver
///   unit's `EnvironmentFile`. The token never goes into an environment
///   file or the command's environment.
/// - `command` (the sandbox's entrypoint, like a container's argv) →
///   `/etc/cua/command.sh` (`0700`, exports `env` and `exec`s the argv) run
///   by the `cua-command.service` systemd unit after the network is up, or
///   with `nohup` (log `/var/log/cua-command.log`) on guests without systemd.
///
/// Variable names must be shell identifiers; values and arguments may not
/// contain control characters (they would break the YAML block and the env
/// file).
pub fn guest_cloud_config(
    env: &BTreeMap<String, String>,
    command: Option<&[String]>,
) -> Result<Option<String>> {
    let command = command.filter(|c| !c.is_empty());
    if env.is_empty() && command.is_none() {
        return Ok(None);
    }
    validate_guest_env(env)?;
    if let Some(argv) = command
        && argv.iter().any(|a| a.chars().any(char::is_control))
    {
        return Err(VmmError::invalid(
            "command arguments may not contain control characters",
        ));
    }
    let token = env.get(ENV_TOKEN_VAR).filter(|t| !t.is_empty());
    // The workload's environment: everything but the driver's token.
    let env: BTreeMap<String, String> = env
        .iter()
        .filter(|(k, _)| k.as_str() != ENV_TOKEN_VAR)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = String::from("#cloud-config\n");
    if token.is_some() {
        out.push_str(&format!("bootcmd:\n  - [sh, -c, '{}']\n", token_bootcmd()));
    }
    let mut files = String::new();
    let block = |out: &mut String, path: &str, mode: &str, content: &str| {
        out.push_str(&format!(
            "  - path: {path}\n    owner: 'root:root'\n    permissions: '{mode}'\n    content: |\n"
        ));
        for line in content.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
    };
    if let Some(token) = token {
        block(&mut files, GUEST_TOKEN_PERSISTENT, "0600", token);
        block(&mut files, GUEST_TOKEN_RUNTIME, "0600", token);
    }
    if !env.is_empty() {
        let env_file: String = env
            .iter()
            .map(|(k, v)| {
                let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{k}=\"{escaped}\"\n")
            })
            .collect();
        block(&mut files, GUEST_ENV_FILE, "0600", &env_file);
    }
    if let Some(argv) = command {
        block(
            &mut files,
            GUEST_COMMAND_SCRIPT,
            "0700",
            &command_script(&env, argv),
        );
        block(&mut files, GUEST_COMMAND_UNIT, "0644", COMMAND_UNIT);
    }
    if !files.is_empty() {
        out.push_str("write_files:\n");
        out.push_str(&files);
    }
    let mut runcmd = String::new();
    if token.is_some() {
        runcmd.push_str(
            "  - [sh, -c, 'systemctl restart cua-env-token-sync.service >/dev/null 2>&1 || true']\n",
        );
    }
    if token.is_some() || !env.is_empty() {
        runcmd.push_str(
            "  - [sh, -c, 'systemctl try-restart cua-spacesd.service >/dev/null 2>&1 || true']\n",
        );
    }
    if command.is_some() {
        runcmd.push_str(&format!(
            "  - [sh, -c, 'if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then systemctl daemon-reload && systemctl enable --now cua-command.service; else nohup {GUEST_COMMAND_SCRIPT} >{GUEST_COMMAND_LOG} 2>&1 & fi']\n"
        ));
    }
    if !runcmd.is_empty() {
        out.push_str("runcmd:\n");
        out.push_str(&runcmd);
    }
    Ok(Some(out))
}

/// Where the token lives in the guest, like the Fleet claim Secret mount.
pub const GUEST_TOKEN_DIR: &str = "/run/cua";
/// Drop-in that puts cua-spacesd in await-token-file mode.
pub const GUEST_DRIVER_DROPIN: &str =
    "/etc/systemd/system/cua-spacesd.service.d/50-cua-token-file.conf";
/// The same drop-in for the root token-sync helper. Without it the helper
/// sees the persistent copy (`/etc/cua/env-token`), takes it for a local
/// token and idles, while the driver waits for the synced copy forever.
pub const GUEST_TOKEN_SYNC_DROPIN: &str =
    "/etc/systemd/system/cua-env-token-sync.service.d/50-cua-token-file.conf";

/// The every-boot `bootcmd` of a token delivery (see [`guest_cloud_config`]).
/// No single quotes: it sits in a single-quoted YAML scalar. The token
/// itself is not in it.
fn token_bootcmd() -> String {
    format!(
        "if systemctl cat cua-env-token-sync.service >/dev/null 2>&1; then \
         mountpoint -q {dir} || {{ mkdir -p {dir} && mount -t tmpfs -o mode=0700,size=1m,nosuid,nodev,noexec cua-env-token {dir}; }}; \
         for d in {dropin} {sync_dropin}; do mkdir -p \"$(dirname $d)\" && printf \"[Service]\\nEnvironment=CUA_ENV_AWAIT_TOKEN_FILE=1\\n\" >$d; done && systemctl daemon-reload; \
         else install -d -m 0755 {dir}; fi; \
         if [ -s {persistent} ]; then install -m 0600 -o root -g root {persistent} {runtime}; fi; \
         systemctl start --no-block cua-env-token-sync.service >/dev/null 2>&1 || true",
        dir = GUEST_TOKEN_DIR,
        dropin = GUEST_DRIVER_DROPIN,
        sync_dropin = GUEST_TOKEN_SYNC_DROPIN,
        persistent = GUEST_TOKEN_PERSISTENT,
        runtime = GUEST_TOKEN_RUNTIME,
    )
}

/// The sandbox command, as the guest runs it (`/etc/cua/command.sh`).
pub const GUEST_COMMAND_SCRIPT: &str = "/etc/cua/command.sh";
/// The systemd unit running [`GUEST_COMMAND_SCRIPT`].
pub const GUEST_COMMAND_UNIT: &str = "/etc/systemd/system/cua-command.service";
/// Log of the command on guests without systemd (journald otherwise:
/// `journalctl -u cua-command`).
pub const GUEST_COMMAND_LOG: &str = "/var/log/cua-command.log";

const COMMAND_UNIT: &str = "[Unit]
Description=cua sandbox command
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
ExecStart=/etc/cua/command.sh
Restart=no

[Install]
WantedBy=multi-user.target
";

/// `sh` script exporting `env` and exec-ing `argv` (every word single-quoted).
fn command_script(env: &BTreeMap<String, String>, argv: &[String]) -> String {
    let mut s = String::from("#!/bin/sh\n");
    for (k, v) in env {
        s.push_str(&format!("export {k}={}\n", sh_quote(v)));
    }
    let words: Vec<String> = argv.iter().map(|a| sh_quote(a)).collect();
    s.push_str(&format!("exec {}\n", words.join(" ")));
    s
}

/// POSIX single-quoting (`'` → `'\''`).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A MIME multipart user-data archive of `#cloud-config` / script parts.
fn multipart(parts: &[&str]) -> String {
    const BOUNDARY: &str = "==cua-vmm-seed==";
    let mut out =
        format!("Content-Type: multipart/mixed; boundary=\"{BOUNDARY}\"\nMIME-Version: 1.0\n\n");
    for part in parts {
        let ctype = if part.starts_with("#cloud-config") {
            "text/cloud-config"
        } else if part.starts_with("#!") {
            "text/x-shellscript"
        } else if part.starts_with("#cloud-boothook") {
            "text/cloud-boothook"
        } else {
            "text/plain"
        };
        let merge = if ctype == "text/cloud-config" {
            "Merge-Type: list(append)+dict(no_replace,recurse_list)+str()\n"
        } else {
            ""
        };
        out.push_str(&format!(
            "--{BOUNDARY}\nContent-Type: {ctype}; charset=\"us-ascii\"\nMIME-Version: 1.0\n{merge}\n{part}"
        ));
        if !part.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&format!("--{BOUNDARY}--\n"));
    out
}

/// Generate (once) an ed25519 keypair at `path` / `path.pub` using `ssh-keygen`.
pub async fn ensure_ssh_key(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let keygen = crate::host::which("ssh-keygen").ok_or_else(|| {
            VmmError::missing(
                "ssh-keygen",
                "install OpenSSH (it ships with macOS and most Linux distros)",
            )
        })?;
        let p = path.display().to_string();
        crate::host::run(
            &keygen,
            &["-t", "ed25519", "-N", "", "-q", "-C", "cua-vmm", "-f", &p],
        )
        .await?;
    }
    Ok(path.to_path_buf())
}

const SECTOR: usize = 2048;

/// Minimal ISO 9660 writer: one root directory, flat files, level-1 style
/// identifiers (`NAME;1`). Linux maps these to lower case without the `;1`
/// version suffix, which is exactly the file name cloud-init looks for.
fn write_iso(volume_id: &str, files: &mut [(&str, &[u8])]) -> Vec<u8> {
    files.sort_by(|a, b| a.0.cmp(b.0));
    let root_lba = 20u32;
    let mut next = root_lba + 1;
    let mut placed = Vec::new();
    for (name, data) in files.iter() {
        let lba = next;
        next += (data.len().div_ceil(SECTOR)).max(1) as u32;
        placed.push((format!("{name};1"), lba, *data));
    }
    let total_sectors = next;
    let mut img = vec![0u8; total_sectors as usize * SECTOR];

    // Root directory extent.
    let mut dir = Vec::new();
    dir.extend(dir_record(&[0], root_lba, SECTOR as u32, true));
    dir.extend(dir_record(&[1], root_lba, SECTOR as u32, true));
    for (ident, lba, data) in &placed {
        dir.extend(dir_record(ident.as_bytes(), *lba, data.len() as u32, false));
    }
    assert!(dir.len() <= SECTOR, "seed directory must fit in one sector");
    put(&mut img, root_lba as usize * SECTOR, &dir);
    for (_, lba, data) in &placed {
        put(&mut img, *lba as usize * SECTOR, data);
    }

    // Path tables (L at 18, M at 19): just the root.
    let mut l = vec![1u8, 0];
    l.extend(root_lba.to_le_bytes());
    l.extend(1u16.to_le_bytes());
    l.extend([0, 0]);
    let mut m = vec![1u8, 0];
    m.extend(root_lba.to_be_bytes());
    m.extend(1u16.to_be_bytes());
    m.extend([0, 0]);
    put(&mut img, 18 * SECTOR, &l);
    put(&mut img, 19 * SECTOR, &m);

    // Primary volume descriptor at sector 16.
    let mut pvd = vec![0u8; SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pad(&mut pvd[8..40], b"");
    pad(&mut pvd[40..72], volume_id.as_bytes());
    both32(&mut pvd[80..88], total_sectors);
    both16(&mut pvd[120..124], 1);
    both16(&mut pvd[124..128], 1);
    both16(&mut pvd[128..132], SECTOR as u16);
    both32(&mut pvd[132..140], l.len() as u32);
    pvd[140..144].copy_from_slice(&18u32.to_le_bytes());
    pvd[148..152].copy_from_slice(&19u32.to_be_bytes());
    pvd[156..190].copy_from_slice(&dir_record(&[0], root_lba, SECTOR as u32, true));
    for range in [
        190..318,
        318..446,
        446..574,
        574..702,
        702..739,
        739..776,
        776..813,
    ] {
        pad(&mut pvd[range], b"");
    }
    for off in [813, 830, 847, 864] {
        pvd[off..off + 16].copy_from_slice(b"0000000000000000");
        pvd[off + 16] = 0;
    }
    pvd[881] = 1;
    put(&mut img, 16 * SECTOR, &pvd);

    // Volume descriptor set terminator at sector 17.
    let mut term = vec![0u8; 7];
    term[0] = 255;
    term[1..6].copy_from_slice(b"CD001");
    term[6] = 1;
    put(&mut img, 17 * SECTOR, &term);
    img
}

fn dir_record(ident: &[u8], lba: u32, len: u32, is_dir: bool) -> Vec<u8> {
    let base = 33 + ident.len();
    let rec_len = base + (base % 2);
    let mut r = vec![0u8; rec_len];
    r[0] = rec_len as u8;
    both32(&mut r[2..10], lba);
    both32(&mut r[10..18], len);
    // 2024-01-01 00:00:00 UTC — fixed so the seed is reproducible.
    r[18..25].copy_from_slice(&[124, 1, 1, 0, 0, 0, 0]);
    r[25] = if is_dir { 2 } else { 0 };
    both16(&mut r[28..32], 1);
    r[32] = ident.len() as u8;
    r[33..33 + ident.len()].copy_from_slice(ident);
    r
}

fn both32(dst: &mut [u8], v: u32) {
    dst[..4].copy_from_slice(&v.to_le_bytes());
    dst[4..8].copy_from_slice(&v.to_be_bytes());
}

fn both16(dst: &mut [u8], v: u16) {
    dst[..2].copy_from_slice(&v.to_le_bytes());
    dst[2..4].copy_from_slice(&v.to_be_bytes());
}

fn pad(dst: &mut [u8], s: &[u8]) {
    dst.fill(b' ');
    dst[..s.len()].copy_from_slice(s);
}

fn put(img: &mut [u8], off: usize, data: &[u8]) {
    img[off..off + data.len()].copy_from_slice(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(b: &[u8]) -> u32 {
        u32::from_le_bytes(b[..4].try_into().unwrap())
    }

    /// Tiny ISO reader used to check what we wrote.
    fn read_iso(img: &[u8]) -> (String, Vec<(String, Vec<u8>)>) {
        let pvd = &img[16 * SECTOR..17 * SECTOR];
        assert_eq!(&pvd[1..6], b"CD001");
        let label = String::from_utf8_lossy(&pvd[40..72]).trim().to_string();
        let root = &pvd[156..190];
        let root_lba = le32(&root[2..]) as usize;
        let dir = &img[root_lba * SECTOR..(root_lba + 1) * SECTOR];
        let mut off = 0;
        let mut files = Vec::new();
        while off < dir.len() && dir[off] != 0 {
            let len = dir[off] as usize;
            let rec = &dir[off..off + len];
            let nlen = rec[32] as usize;
            let name = &rec[33..33 + nlen];
            if rec[25] & 2 == 0 {
                let lba = le32(&rec[2..]) as usize;
                let size = le32(&rec[10..]) as usize;
                files.push((
                    String::from_utf8_lossy(name).into_owned(),
                    img[lba * SECTOR..lba * SECTOR + size].to_vec(),
                ));
            }
            off += len;
        }
        (label, files)
    }

    #[test]
    fn iso_has_cidata_label_and_sorted_files() {
        let seed = Seed::with_ssh_user("vm1", "cua", "ssh-ed25519 AAAA test");
        let iso = seed.to_iso();
        assert_eq!(iso.len() % SECTOR, 0);
        let (label, files) = read_iso(&iso);
        assert_eq!(label, "CIDATA");
        let names: Vec<_> = files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, ["META-DATA;1", "USER-DATA;1"]);
        assert_eq!(files[0].1, seed.meta_data.as_bytes());
        let ud = String::from_utf8(files[1].1.clone()).unwrap();
        assert!(ud.starts_with("#cloud-config\n"));
        assert!(ud.contains("- ssh-ed25519 AAAA test"));
        assert!(ud.contains("name: cua"));
    }

    #[test]
    fn iso_handles_multi_sector_files_and_network_config() {
        let mut seed = Seed::raw("vm2", &"x".repeat(5000));
        seed.network_config = Some("version: 2\n".into());
        let (_, files) = read_iso(&seed.to_iso());
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].0, "NETWORK-CONFIG;1");
        assert_eq!(files[2].1.len(), 5000);
    }

    fn token_env(token: &str) -> BTreeMap<String, String> {
        [(ENV_TOKEN_VAR.to_string(), token.to_string())].into()
    }

    #[test]
    fn env_seed_writes_token_files_and_restarts_driver() {
        let ud = guest_env_cloud_config(&token_env("tok123"))
            .unwrap()
            .unwrap();
        assert!(ud.starts_with("#cloud-config\n"));
        for path in [GUEST_TOKEN_PERSISTENT, GUEST_TOKEN_RUNTIME] {
            assert!(
                ud.contains(&format!(
                    "  - path: {path}\n    owner: 'root:root'\n    permissions: '0600'\n"
                )),
                "{path}\n{ud}"
            );
        }
        assert!(ud.contains("    content: |\n      tok123\n"));
        // The token is a file, never an environment variable.
        assert!(!ud.contains("CUA_ENV_TOKEN"), "{ud}");
        assert!(!ud.contains(GUEST_ENV_FILE), "no other env, no env file");
        assert_eq!(ud.matches("tok123").count(), 2, "{ud}");
        // bootcmd: the Fleet-style private /run/cua and await-token-file
        // mode, and the per-boot copy; no token in it.
        let boot = ud.lines().skip_while(|l| *l != "bootcmd:").nth(1).unwrap();
        assert!(!boot.contains("tok123"));
        assert!(boot.contains("systemctl cat cua-env-token-sync.service"));
        assert!(boot.contains(
            "mount -t tmpfs -o mode=0700,size=1m,nosuid,nodev,noexec cua-env-token /run/cua"
        ));
        assert!(boot.contains("Environment=CUA_ENV_AWAIT_TOKEN_FILE=1"));
        assert!(boot.contains(GUEST_DRIVER_DROPIN));
        // The token-sync helper gets the same await mode, or it idles on the
        // persistent copy and the driver never gets its synced token.
        assert!(boot.contains(GUEST_TOKEN_SYNC_DROPIN));
        assert!(
            boot.contains("install -m 0600 -o root -g root /etc/cua/env-token /run/cua/env-token")
        );
        assert!(
            !token_bootcmd().contains('\''),
            "sits in a single-quoted scalar"
        );
        // bootcmd runs before write_files (cloud-init module order), so the
        // runtime file lands on the tmpfs.
        assert!(ud.find("bootcmd:").unwrap() < ud.find("write_files:").unwrap());
        assert!(ud.contains("systemctl restart cua-env-token-sync.service"));
        assert!(ud.contains("systemctl try-restart cua-spacesd.service"));
        assert_eq!(guest_env_cloud_config(&BTreeMap::new()).unwrap(), None);
        // Other variables go to the env file, the token does not.
        let mut env = token_env("tok123");
        env.insert("FOO".into(), "bar".into());
        let ud = guest_env_cloud_config(&env).unwrap().unwrap();
        assert!(ud.contains("      FOO=\"bar\"\n"));
        assert!(!ud.contains("CUA_ENV_TOKEN"));
    }

    #[test]
    fn token_bootcmd_is_valid_sh() {
        let Some(sh) = crate::host::which("sh") else {
            return;
        };
        let out = std::process::Command::new(sh)
            .args(["-n", "-c", &token_bootcmd()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn env_seed_rejects_bad_names_and_values_and_escapes_quotes() {
        let bad_name: BTreeMap<_, _> = [("A-B".to_string(), "x".to_string())].into();
        assert!(guest_env_cloud_config(&bad_name).is_err());
        let bad_value = token_env("a\nb");
        assert!(guest_env_cloud_config(&bad_value).is_err());
        let quoted: BTreeMap<_, _> = [("X".to_string(), r#"a"b\c"#.to_string())].into();
        let ud = guest_env_cloud_config(&quoted).unwrap().unwrap();
        assert!(ud.contains(r#"X="a\"b\\c""#), "{ud}");
        assert!(!ud.contains("bootcmd"), "no token, no token copy");
    }

    fn spec_with(env: BTreeMap<String, String>) -> StartSpec {
        let mut s = StartSpec::new("vm1", crate::types::ImageSource::Existing);
        s.env = env;
        s
    }

    #[tokio::test]
    async fn managed_ssh_key_is_generated_once() {
        if crate::host::which("ssh-keygen").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let a = managed_ssh(dir.path()).await.expect("key");
        assert_eq!(a.user, "cua");
        let pubkey = std::fs::read_to_string(dir.path().join("id_ed25519.pub")).unwrap();
        assert!(pubkey.starts_with("ssh-ed25519 "));
        let b = managed_ssh(dir.path()).await.expect("key");
        assert_eq!(a.private_key, b.private_key);
        let again = std::fs::read_to_string(dir.path().join("id_ed25519.pub")).unwrap();
        assert_eq!(pubkey, again, "reused, not regenerated");
        // The seed for a spec carrying it authorizes the key for `cua`.
        let mut spec = spec_with(token_env("t"));
        spec.ssh = Some(a);
        let seed = seed_for_spec("vm1", &spec).unwrap().unwrap();
        assert!(seed.user_data.contains(pubkey.trim()), "{}", seed.user_data);
        assert!(seed.user_data.contains("name: cua"));
        assert!(seed.user_data.contains("/run/cua/env-token"));
    }

    #[test]
    fn spec_seed_carries_token_and_instance_id_tracks_env() {
        // A Linux guest with nothing to deliver still gets a seed, so
        // cloud-init runs its first-boot work (SSH host keys).
        let empty = seed_for_spec("vm1", &spec_with(BTreeMap::new()))
            .unwrap()
            .expect("a Linux guest is always seeded");
        assert!(empty.user_data.starts_with("#cloud-config\n"));
        assert_eq!(empty.meta_data, "instance-id: vm1\nlocal-hostname: vm1\n");
        let a = seed_for_spec("vm1", &spec_with(token_env("one")))
            .unwrap()
            .unwrap();
        let b = seed_for_spec("vm1", &spec_with(token_env("two")))
            .unwrap()
            .unwrap();
        assert!(a.user_data.contains("      one\n"));
        assert!(a.meta_data.starts_with("instance-id: vm1-"));
        assert!(a.meta_data.ends_with("local-hostname: vm1\n"));
        assert_ne!(a.meta_data, b.meta_data, "a new token re-runs cloud-init");
        let (_, files) = read_iso(&a.to_iso());
        assert!(String::from_utf8_lossy(&files[1].1).contains("/run/cua/env-token"));
        // macOS / Windows guests have no cloud-init: nothing to seed.
        let mut mac = spec_with(token_env("one"));
        mac.os = GuestOs::Macos;
        assert!(seed_for_spec("vm1", &mac).unwrap().is_none());
    }

    #[test]
    fn non_linux_guests_refuse_workload_env_but_not_the_token() {
        let mut win = spec_with(token_env("tok"));
        win.os = GuestOs::Windows;
        // The token alone is fine (the SDK installs it with Init).
        assert!(seed_for_spec("w1", &win).unwrap().is_none());
        win.env.insert("FOO".into(), "bar".into());
        let err = seed_for_spec("w1", &win).unwrap_err();
        assert!(
            matches!(err, VmmError::Unsupported { backend: "vm", op } if op.contains("environment variables")),
            "{err}"
        );
        // An empty env keeps today's behavior.
        win.env.clear();
        assert!(seed_for_spec("w1", &win).unwrap().is_none());
    }

    #[test]
    fn spec_seed_appends_to_ssh_user_and_wraps_raw_user_data() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id");
        std::fs::write(dir.path().join("id.pub"), "ssh-ed25519 AAAA k\n").unwrap();
        let mut s = spec_with(token_env("tok"));
        s.ssh = Some(crate::types::SshAccess {
            user: "cua".into(),
            private_key: key,
            password: None,
        });
        let seed = seed_for_spec("vm1", &s).unwrap().unwrap();
        assert_eq!(seed.user_data.matches("#cloud-config").count(), 1);
        assert!(seed.user_data.contains("- ssh-ed25519 AAAA k"));
        assert!(
            seed.user_data
                .contains("\nwrite_files:\n  - path: /etc/cua/env-token")
        );

        let mut raw = spec_with(token_env("tok"));
        raw.cloud_init_user_data = Some("#cloud-config\nruncmd:\n  - [echo, hi]\n".into());
        let seed = seed_for_spec("vm1", &raw).unwrap().unwrap();
        assert!(seed.user_data.starts_with("Content-Type: multipart/mixed;"));
        assert_eq!(
            seed.user_data
                .matches("Content-Type: text/cloud-config")
                .count(),
            2
        );
        assert!(seed.user_data.contains("Merge-Type: list(append)"));
        assert!(seed.user_data.contains("[echo, hi]"));
        assert!(seed.user_data.contains("/run/cua/env-token"));
        assert!(seed.user_data.trim_end().ends_with("--==cua-vmm-seed==--"));
        // Without env the caller's user-data is passed through untouched.
        raw.env.clear();
        let seed = seed_for_spec("vm1", &raw).unwrap().unwrap();
        assert_eq!(seed.user_data, "#cloud-config\nruncmd:\n  - [echo, hi]\n");
    }

    #[test]
    fn command_seed_runs_the_argv_with_env_through_systemd_or_nohup() {
        let env: BTreeMap<_, _> = [("GREETING".to_string(), "it's $HOME".to_string())].into();
        let argv = vec![
            "python3".to_string(),
            "-m".into(),
            "srv".into(),
            "a b".into(),
        ];
        let ud = guest_cloud_config(&env, Some(&argv)).unwrap().unwrap();
        assert!(ud.contains(&format!("  - path: {GUEST_COMMAND_SCRIPT}\n    owner: 'root:root'\n    permissions: '0700'\n")), "{ud}");
        assert!(
            ud.contains("      export GREETING='it'\\''s $HOME'\n"),
            "{ud}"
        );
        assert!(
            ud.contains("      exec 'python3' '-m' 'srv' 'a b'\n"),
            "{ud}"
        );
        assert!(ud.contains(&format!("  - path: {GUEST_COMMAND_UNIT}\n")));
        assert!(ud.contains("      ExecStart=/etc/cua/command.sh\n"));
        assert!(ud.contains("systemctl enable --now cua-command.service"));
        assert!(ud.contains("nohup /etc/cua/command.sh >/var/log/cua-command.log 2>&1 &"));
        assert!(ud.contains("systemctl try-restart cua-spacesd.service"));
        assert!(!ud.contains("bootcmd"), "no token, no token setup");
        // The token never reaches the command's environment.
        let mut with_token = env.clone();
        with_token.insert(ENV_TOKEN_VAR.into(), "secret-tok".into());
        let ud = guest_cloud_config(&with_token, Some(&argv))
            .unwrap()
            .unwrap();
        let script = ud.split(GUEST_COMMAND_SCRIPT).nth(1).unwrap();
        assert!(!script.split("path:").next().unwrap().contains("secret-tok"));
        // A command without env: no env file, no driver restart.
        let ud = guest_cloud_config(&BTreeMap::new(), Some(&argv))
            .unwrap()
            .unwrap();
        assert!(!ud.contains(GUEST_ENV_FILE));
        assert!(!ud.contains("try-restart"));
        assert!(ud.contains("cua-command.service"));
        // Control characters would break the YAML block.
        let bad = vec!["sh".to_string(), "a\nb".into()];
        assert!(guest_cloud_config(&BTreeMap::new(), Some(&bad)).is_err());
        // An empty argv is no command.
        assert_eq!(
            guest_cloud_config(&BTreeMap::new(), Some(&[])).unwrap(),
            None
        );
    }

    #[test]
    fn spec_seed_carries_the_command_and_instance_id_tracks_it() {
        let mut a = spec_with(BTreeMap::new());
        a.command = Some(vec!["srv".into(), "--port".into(), "8765".into()]);
        let seed = seed_for_spec("vm1", &a).unwrap().unwrap();
        assert!(seed.user_data.contains("exec 'srv' '--port' '8765'"));
        assert!(seed.meta_data.starts_with("instance-id: vm1-"));
        let mut b = a.clone();
        b.command = Some(vec!["srv".into(), "--port".into(), "9000".into()]);
        let other = seed_for_spec("vm1", &b).unwrap().unwrap();
        assert_ne!(
            seed.meta_data, other.meta_data,
            "a new command re-runs cloud-init"
        );
        // macOS guests have no cloud-init: the command is not seeded here.
        let mut mac = a.clone();
        mac.os = GuestOs::Macos;
        assert!(matches!(
            seed_for_spec("vm1", &mac),
            Err(VmmError::Unsupported { .. })
        ));
    }

    #[test]
    fn iso_is_deterministic() {
        let s = Seed::raw("a", "#cloud-config\n");
        assert_eq!(s.to_iso(), s.to_iso());
    }
}
