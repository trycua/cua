// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first boot of a cloud VM that runs one sandbox: cloud-init user
//! data that installs Docker and runs the sandbox's image with its
//! cua-spacesd joined to the relay as the machine the client registered.
//! No inbound port and no account credential on the VM; nothing but Docker
//! is installed (the image carries its own driver).
//!
//! Three guardrails make a forgotten VM end on its own:
//!
//! - `shutdown -h +<ttl>` from the first boot, with the cloud's
//!   instance-initiated shutdown set to terminate (or delete);
//! - a `cua-expire` systemd timer that shuts the VM down once the time in
//!   `/etc/cua/expires` has passed, again after every reboot or start;
//! - the `cua-expires` tag the sweeper reads.

use std::collections::BTreeMap;

use base64::Engine as _;

use crate::relay::Join;

/// The container the VM runs the sandbox in.
pub const CONTAINER: &str = "cua-sandbox";

/// The user the Linux images run cua-spacesd as (`cua`, uid 1000): the
/// relay files are theirs, readable by nobody else.
pub const GUEST_UID: u32 = 1000;

/// The Docker drop-in that keeps containers from the metadata service
/// (169.254.169.254) but lets DNS through (Compute Engine's resolver is that
/// address). `-I` inserts at the top, so the DNS exceptions go in last.
pub const METADATA_RULES: &str = "[Service]\n\
     ExecStartPost=-/sbin/iptables -I DOCKER-USER -d 169.254.169.254 -j DROP\n\
     ExecStartPost=-/sbin/iptables -I DOCKER-USER -d 169.254.169.254 -p udp --dport 53 -j RETURN\n\
     ExecStartPost=-/sbin/iptables -I DOCKER-USER -d 169.254.169.254 -p tcp --dport 53 -j RETURN\n";

/// The environment variables a Linux image's start script reads extra
/// driver arguments from, newest spelling first.
pub const JOIN_ARG_VARS: &[&str] = &["CUA_SPACESD_ARGS", "CUA_GUESTD_ARGS", "CUA_ENV_DRIVER_ARGS"];

/// What one VM's first boot needs.
#[derive(Clone, Debug)]
pub struct VmBoot<'a> {
    /// The relay machine the sandbox joins as.
    pub join: &'a Join,
    /// The image (pinned by digest when resolved).
    pub image: &'a str,
    /// The guest environment (the spacesd token among it: secret).
    pub env: &'a BTreeMap<String, String>,
    /// Unix seconds after which the VM shuts itself down (0: never).
    pub expires: u64,
    /// Seconds from boot for the `shutdown -h` backstop (0: none).
    pub ttl_secs: u64,
    /// Shared memory for the desktop's browsers, in MiB.
    pub shm_mb: u32,
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

/// Single-quotes `s` for `sh`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The script the VM runs once (as root) from cloud-init.
pub fn first_boot_script(boot: &VmBoot<'_>) -> String {
    let mut s = String::from(
        "#!/bin/sh\n\
         # Cua: this VM runs one sandbox; its cua-spacesd dials out to the relay.\n\
         set -eu\n\
         export DEBIAN_FRONTEND=noninteractive HOME=/root\n\
         log() { echo \"[cua-first-boot] $(date -u +%FT%TZ) $*\"; }\n\
         retry() { n=0; until \"$@\"; do n=$((n+1)); [ $n -ge 5 ] && return 1; sleep $((n*5)); done; }\n",
    );
    if boot.ttl_secs > 0 {
        let minutes = boot.ttl_secs.div_ceil(60).max(1);
        s.push_str(&format!(
            "log 'backstop: shutdown in {minutes} min'\n\
             shutdown -h +{minutes} 'cua: this sandbox reached its time limit' || true\n"
        ));
    }
    // Docker's bridge defaults to MTU 1500; on a smaller network (Compute
    // Engine's 1460) TLS from the sandbox stalls while the host's own pulls
    // work. Use the host interface's MTU.
    s.push_str(
        "dev=$(ip route show default 2>/dev/null | awk '{for (i=1;i<NF;i++) if ($i==\"dev\") {print $(i+1); exit}}')\n\
         mtu=$(cat /sys/class/net/${dev:-eth0}/mtu 2>/dev/null || echo 1500)\n\
         mkdir -p /etc/docker\n\
         [ -s /etc/docker/daemon.json ] || printf '{\"mtu\": %s}\\n' \"$mtu\" > /etc/docker/daemon.json\n\
         log \"docker mtu $mtu\"\n",
    );
    s.push_str(
        "log 'installing Docker'\n\
         if ! command -v docker >/dev/null; then\n\
         \x20 if command -v apt-get >/dev/null; then\n\
         \x20   retry apt-get update -q\n\
         \x20   retry apt-get install -y -q docker.io\n\
         \x20 elif command -v dnf >/dev/null; then\n\
         \x20   retry dnf install -y docker\n\
         \x20 fi\n\
         fi\n\
         systemctl enable --now docker\n",
    );
    s.push_str(&format!(
        "chown -R {uid}:{uid} /etc/cua/relay && chmod 0700 /etc/cua/relay && chmod 0600 /etc/cua/relay/*\n\
         log 'pulling the image'\n\
         retry docker pull {image}\n\
         docker rm -f {CONTAINER} >/dev/null 2>&1 || true\n\
         log 'starting the sandbox'\n\
         docker run -d --name {CONTAINER} --restart unless-stopped --shm-size {shm}m \\\n\
         \x20 --env-file /etc/cua/sandbox.env \\\n\
         \x20 -v /etc/cua/relay:/run/cua-relay:ro \\\n\
         \x20 {image}\n\
         shred -u /etc/cua/sandbox.env 2>/dev/null || rm -f /etc/cua/sandbox.env\n\
         log 'started; the driver log follows (the boot log shows why a sandbox never comes online)'\n\
         sleep 60\n\
         docker exec {CONTAINER} sh -c 'tail -n 40 /var/log/supervisor/cua-spacesd.log /var/log/supervisor/cua-guestd.log /var/log/supervisor/cua-env-driver.log 2>/dev/null' \\\n\
         \x20 | grep -v -i -e token -e 'cmt_' || true\n\
         log 'done'\n",
        uid = GUEST_UID,
        image = sh_quote(boot.image),
        shm = boot.shm_mb.max(64),
    ));
    s
}

/// The environment file of the container: the guest environment plus the
/// relay join (files mounted at `/run/cua-relay`, which every cua-spacesd
/// with `join` reads). Secret.
pub fn container_env(boot: &VmBoot<'_>) -> String {
    let mut env = boot.env.clone();
    // Every spelling of the driver's extra arguments: images from before
    // the cua-spacesd rename read CUA_GUESTD_ARGS (or CUA_ENV_DRIVER_ARGS).
    for k in JOIN_ARG_VARS {
        env.insert((*k).into(), "join".into());
    }
    env.insert("CUA_ENV_RELAY_URL".into(), boot.join.relay_url.clone());
    env.insert(
        "CUA_RELAY_TOKEN_FILE".into(),
        "/run/cua-relay/machine-token".into(),
    );
    env.insert(
        "CUA_ENV_MACHINE_ID_FILE".into(),
        "/run/cua-relay/machine-id".into(),
    );
    env.insert("CUA_RELAY_JWKS".into(), "/run/cua-relay/jwks.json".into());
    env.insert(
        "CUA_HOST_POLICY".into(),
        "/run/cua-relay/policy.json".into(),
    );
    let mut out = String::new();
    for (k, v) in env {
        // `--env-file` takes the rest of the line verbatim.
        let v = v.replace('\n', " ");
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

/// The `#cloud-config` user data for `boot`. Secret: the machine and
/// spacesd tokens are in it (base64, never in clear), and reach only this
/// VM; the providers limit the metadata service to the VM itself.
pub fn cloud_init(boot: &VmBoot<'_>) -> String {
    let expire = "#!/bin/sh\n\
         # Cua: shut this VM down once its sandbox's time limit has passed.\n\
         exp=$(cat /etc/cua/expires 2>/dev/null || echo 0)\n\
         [ \"${exp:-0}\" -gt 0 ] 2>/dev/null || exit 0\n\
         if [ \"$(date +%s)\" -ge \"$exp\" ]; then\n\
         \x20 logger -t cua-expire 'time limit reached; shutting down'\n\
         \x20 exec /sbin/shutdown -h now 'cua: this sandbox reached its time limit'\n\
         fi\n";
    let service = "[Unit]\nDescription=Cua: end this VM after its sandbox's time limit\n\n\
         [Service]\nType=oneshot\nExecStart=/usr/local/bin/cua-expire\n";
    let timer = "[Unit]\nDescription=Cua: check this sandbox's time limit\n\n\
         [Timer]\nOnBootSec=1min\nOnUnitActiveSec=2min\n\n\
         [Install]\nWantedBy=timers.target\n";
    let file = |path: &str, perm: &str, content: &str| {
        format!(
            "  - path: {path}\n    permissions: '{perm}'\n    owner: root:root\n    encoding: b64\n    content: {}\n",
            b64(content)
        )
    };
    let mut y = String::from("#cloud-config\n");
    y.push_str("write_files:\n");
    y.push_str(&file(
        "/etc/cua/relay/machine-token",
        "0600",
        &boot.join.machine_token,
    ));
    y.push_str(&file(
        "/etc/cua/relay/machine-id",
        "0600",
        &format!("{}\n", boot.join.machine_id),
    ));
    y.push_str(&file(
        "/etc/cua/relay/jwks.json",
        "0600",
        &boot.join.jwks_json,
    ));
    y.push_str(&file(
        "/etc/cua/relay/policy.json",
        "0600",
        &boot.join.policy_json(),
    ));
    y.push_str(&file("/etc/cua/sandbox.env", "0600", &container_env(boot)));
    y.push_str(&file(
        "/etc/cua/expires",
        "0644",
        &format!("{}\n", boot.expires),
    ));
    y.push_str(&file("/usr/local/bin/cua-expire", "0755", expire));
    y.push_str(&file(
        "/etc/systemd/system/cua-expire.service",
        "0644",
        service,
    ));
    y.push_str(&file("/etc/systemd/system/cua-expire.timer", "0644", timer));
    // After every Docker start (reboots, stop and start): the sandbox cannot
    // read the cloud's metadata service (the user data holds its tokens),
    // except DNS, which Compute Engine serves at the same address.
    y.push_str(&file(
        "/etc/systemd/system/docker.service.d/cua-metadata.conf",
        "0644",
        METADATA_RULES,
    ));
    y.push_str(&file(
        "/usr/local/sbin/cua-first-boot",
        "0700",
        &first_boot_script(boot),
    ));
    y.push_str(
        "runcmd:\n\
         \x20 - [systemctl, daemon-reload]\n\
         \x20 - [systemctl, enable, --now, cua-expire.timer]\n\
         \x20 - [sh, -c, '/usr/local/sbin/cua-first-boot 2>&1 | tee /var/log/cua-first-boot.log']\n",
    );
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join() -> Join {
        Join {
            relay_url: "https://relay.example".into(),
            machine_id: "cloud-0123456789abcdef".into(),
            machine_token: "cmt_secret".into(),
            jwks_json: r#"{"keys":[]}"#.into(),
            owner: "acct".into(),
            owner_email: "a@example.com".into(),
        }
    }

    pub(crate) fn decode(y: &str, path: &str) -> String {
        let at = y.find(&format!("path: {path}\n")).expect(path);
        let rest = &y[at..];
        let c = rest.find("content: ").unwrap() + "content: ".len();
        let end = rest[c..].find('\n').unwrap();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(&rest[c..c + end])
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn the_user_data_runs_the_image_joined_to_the_relay_and_ends_itself() {
        let j = join();
        let env = BTreeMap::from([("CUA_ENV_TOKEN".to_string(), "envtok".to_string())]);
        let y = cloud_init(&VmBoot {
            join: &j,
            image: "ghcr.io/trycua/linux@sha256:abc",
            env: &env,
            expires: 1_900_000_000,
            ttl_secs: 8 * 3600,
            shm_mb: 2048,
        });
        assert!(y.starts_with("#cloud-config\n"));
        // Secrets never appear in clear text.
        assert!(!y.contains("cmt_secret") && !y.contains("envtok"));
        assert_eq!(decode(&y, "/etc/cua/relay/machine-token"), "cmt_secret");
        assert_eq!(
            decode(&y, "/etc/cua/relay/machine-id").trim(),
            "cloud-0123456789abcdef"
        );
        let policy: serde_json::Value =
            serde_json::from_str(&decode(&y, "/etc/cua/relay/policy.json")).unwrap();
        assert_eq!(policy["owner"], "acct");
        let envf = decode(&y, "/etc/cua/sandbox.env");
        assert!(envf.contains("CUA_SPACESD_ARGS=join\n"));
        assert!(envf.contains("CUA_GUESTD_ARGS=join\n"));
        assert!(envf.contains("CUA_ENV_TOKEN=envtok\n"));
        assert!(envf.contains("CUA_RELAY_TOKEN_FILE=/run/cua-relay/machine-token\n"));
        assert!(envf.contains("CUA_ENV_RELAY_URL=https://relay.example\n"));
        assert_eq!(decode(&y, "/etc/cua/expires").trim(), "1900000000");
        let script = decode(&y, "/usr/local/sbin/cua-first-boot");
        assert!(script.contains("shutdown -h +480"), "{script}");
        assert!(script.contains("docker run -d --name cua-sandbox --restart unless-stopped"));
        assert!(script.contains("'ghcr.io/trycua/linux@sha256:abc'"));
        assert!(script.contains("-v /etc/cua/relay:/run/cua-relay:ro"));
        assert!(script.contains("/etc/docker/daemon.json"), "{script}");
        // The user data (with the machine token) is not readable from the
        // sandbox through the cloud's metadata service.
        let rules = decode(&y, "/etc/systemd/system/docker.service.d/cua-metadata.conf");
        let drop = rules.find("-j DROP").unwrap();
        let dns = rules.find("--dport 53 -j RETURN").unwrap();
        // DNS is inserted after the drop, so it sits above it in the chain.
        assert!(dns > drop, "{rules}");
        assert!(script.contains("shred -u /etc/cua/sandbox.env"));
        assert!(y.contains("enable, --now, cua-expire.timer"));
        assert!(y.len() < 16 * 1024, "{}", y.len());
    }

    #[test]
    fn no_ttl_means_no_backstop() {
        let j = join();
        let env = BTreeMap::new();
        let y = cloud_init(&VmBoot {
            join: &j,
            image: "x'y",
            env: &env,
            expires: 0,
            ttl_secs: 0,
            shm_mb: 0,
        });
        let script = decode(&y, "/usr/local/sbin/cua-first-boot");
        assert!(!script.contains("shutdown -h +"));
        assert!(script.contains("'x'\\''y'"), "{script}");
        assert!(script.contains("--shm-size 64m"));
    }
}
