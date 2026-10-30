//! The `code-interpreter-v1` template.
//!
//! E2B's code-interpreter template is its base image plus a Jupyter server on
//! port 49999 that the `e2b-code-interpreter` SDKs talk to. A Cua sandbox
//! made from that template gets a small server (`assets/code_interpreter.py`,
//! standard-library Python) installed and started in the guest; the API
//! listener pipes requests that name port 49999 to it. A memory checkpoint
//! (snapshot, pause, fork, template build) keeps the running server; a disk-only
//! resume starts it again.

use base64::Engine;
use cua_spacesd_client::SpacesdClient;

use super::E2b;

/// The template name the SDKs default to.
pub const TEMPLATE: &str = "code-interpreter-v1";
/// The port the server listens on.
pub const PORT: u16 = 49999;

const SERVER: &str = include_str!("assets/code_interpreter.py");
const PATH: &str = "/opt/cua/code-interpreter.py";

/// Whether `template` (a sandbox's template id, or a base image of a
/// build) is the code-interpreter one.
pub fn wants(template: &str) -> bool {
    let t = template.strip_prefix("e2bdev/").unwrap_or(template);
    t == TEMPLATE || t.starts_with("code-interpreter-v1:")
}

/// Installs the server and starts it (idempotent), as the default user.
pub async fn provision(e: &E2b, c: &SpacesdClient, driver: &str) {
    let user = &e.cfg.default_user;
    let b64 = base64::engine::general_purpose::STANDARD.encode(SERVER);
    let run_as = if user == "root" {
        String::new()
    } else {
        format!("/usr/bin/sudo -n -u {user} env HOME=$(getent passwd {user} | cut -d: -f6) ")
    };
    let script = format!(
        "command -v python3 >/dev/null 2>&1 || {{ echo 'python3 is not installed in this image' >&2; exit 40; }}; \
         mkdir -p /opt/cua && echo '{b64}' | base64 -d > {PATH} && chmod 755 {PATH} || exit 42; \
         probe() {{ python3 -c \"import socket;socket.create_connection(('127.0.0.1',{PORT}),1)\" 2>/dev/null; }}; \
         probe && exit 0; \
         {run_as}setsid nohup python3 {PATH} >/tmp/cua-code-interpreter.log 2>&1 </dev/null & \
         for i in $(seq 1 100); do probe && exit 0; sleep 0.1; done; \
         cat /tmp/cua-code-interpreter.log >&2; exit 41"
    );
    match crate::guest::run_as_idempotent(c, driver, "root", &script, &[]).await {
        Ok(out) if out.success() => {}
        Ok(out) => tracing::warn!(
            "compat: code interpreter: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(err) => tracing::warn!("compat: code interpreter: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_template() {
        assert!(wants("code-interpreter-v1"));
        assert!(wants("e2bdev/code-interpreter-v1"));
        assert!(!wants("base"));
        assert!(!wants("code-interpreter-v2"));
    }
}
