//! Daytona SSH access (`POST|DELETE /sandbox/{id}/ssh-access`,
//! `GET /sandbox/ssh-access/validate`).
//!
//! Daytona issues a token that its SSH gateway takes as the user name. Here
//! each access is a time-limited loopback listener on the gateway host that
//! pipes to the guest's sshd (port 22, through the Space's tunnel forward, so
//! the Space may be on any machine), plus a one-time ed25519 key authorized
//! for the sandbox user and removed again when the access ends. The
//! `sshCommand` names the port and the key file, and works on the machine
//! that runs the gateway (the default: loopback). Images without sshd refuse
//! with the reason.
//!
//! Shapes: daytona_api_client 0.225.0 `SshAccessDto`, `SshAccessValidationDto`.

use super::api::{authed, owned, rfc3339};
use super::{Daytona, backend_error, error, ok_json};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use http::{HeaderMap, Response, StatusCode};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
pub struct Registry {
    items: Mutex<HashMap<String, Access>>,
}

struct Access {
    sandbox_id: String,
    key_id: String,
    expires_ms: u64,
    key_path: PathBuf,
    pub_line: String,
    listener: tokio::task::JoinHandle<()>,
}

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/sandbox/{id}/ssh-access", post(create).delete(revoke))
        .route("/api/sandbox/ssh-access/validate", get(validate))
        .with_state(d)
}

impl Registry {
    pub async fn drop_sandbox(&self, id: &str) {
        let gone: Vec<Access> = {
            let mut m = self.items.lock().await;
            let toks: Vec<String> = m
                .iter()
                .filter(|(_, a)| a.sandbox_id == id)
                .map(|(t, _)| t.clone())
                .collect();
            toks.iter().filter_map(|t| m.remove(t)).collect()
        };
        for a in gone {
            a.listener.abort();
            let _ = std::fs::remove_file(&a.key_path);
        }
    }
}

/// Minutes an access lasts when none are asked for (Daytona: 60).
pub const DEFAULT_MINUTES: u64 = 60;

#[derive(serde::Deserialize, Default)]
struct CreateQuery {
    #[serde(rename = "expiresInMinutes")]
    expires_in_minutes: Option<i64>,
}

/// The `sshCommand` of an access.
pub fn ssh_command(user: &str, port: u16, key: &std::path::Path) -> String {
    format!(
        "ssh -i {} -p {port} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes {user}@127.0.0.1",
        key.display()
    )
}

async fn create(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<CreateQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let minutes = match q.expires_in_minutes {
        None => DEFAULT_MINUTES,
        Some(m) if m >= 1 => m as u64,
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "expiresInMinutes must not be less than 1",
            );
        }
    };
    if super::api::state_of(&r) != "started" {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "Sandbox must be started for SSH access (state {})",
                super::api::state_of(&r)
            ),
        );
    }
    let c = match d.spacesd(&r).await {
        Ok(c) => c,
        Err(e) => return backend_error(e),
    };
    let driver = d.drivers.of(&r.id, &c).await;
    let user = d.user_of(&r);
    // sshd present (started if it is installed but down)?
    let have = crate::guest::run_as(
        &c, &driver, "root",
        "command -v sshd >/dev/null 2>&1 || [ -x /usr/sbin/sshd ] || exit 3;          (pgrep -x sshd >/dev/null 2>&1) || (systemctl start ssh >/dev/null 2>&1 || systemctl start sshd >/dev/null 2>&1 || (mkdir -p /run/sshd; /usr/sbin/sshd) >/dev/null 2>&1);          sleep 1; pgrep -x sshd >/dev/null 2>&1",
        &[],
    )
    .await;
    if !matches!(&have, Ok(o) if o.success()) {
        return error(
            StatusCode::BAD_REQUEST,
            "This sandbox's image has no running sshd, so SSH access cannot be offered (use a PTY: process.create_pty_session)",
        );
    }
    // A one-time key, authorized for the sandbox user.
    let aid = super::api_uuid();
    let dir = d
        .cfg
        .dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join("ssh");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let key_path = dir.join(format!("{aid}.key"));
    let keygen = tokio::process::Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            &format!("cua-daytona-{aid}"),
            "-f",
        ])
        .arg(&key_path)
        .output()
        .await;
    if !matches!(&keygen, Ok(o) if o.status.success()) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ssh-keygen is not available on the gateway host",
        );
    }
    let pub_line = std::fs::read_to_string(key_path.with_extension("key.pub"))
        .or_else(|_| std::fs::read_to_string(format!("{}.pub", key_path.display())))
        .unwrap_or_default()
        .trim()
        .to_string();
    let install = crate::guest::run_as(
        &c, &driver, &user,
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && printf '%s\n' \"$1\" >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys",
        &[&pub_line],
    )
    .await;
    if !matches!(&install, Ok(o) if o.success()) {
        let _ = std::fs::remove_file(&key_path);
        return error(
            StatusCode::BAD_REQUEST,
            "Authorizing the access key in the sandbox failed",
        );
    }
    // The endpoint: loopback listener piped to the guest's sshd.
    let upstream = match d.backend.forward(&r.identity(), &r.cua_name, 22).await {
        Ok(a) => a,
        Err(e) => return backend_error(e),
    };
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let up = upstream.clone();
            tokio::spawn(async move {
                if let Ok(mut u) = tokio::net::TcpStream::connect(&up).await {
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut u).await;
                }
            });
        }
    });
    let token = format!("ssh_{}", &crate::auth::new_token()[..32]);
    let now = crate::state::now_ms();
    let acc = Access {
        sandbox_id: r.id.clone(),
        key_id: key.id(),
        expires_ms: now + minutes * 60_000,
        key_path: key_path.clone(),
        pub_line,
        listener: handle,
    };
    let out = json!({
        "id": aid, "sandboxId": r.id, "token": token,
        "expiresAt": rfc3339(acc.expires_ms), "createdAt": rfc3339(now), "updatedAt": rfc3339(now),
        "sshCommand": ssh_command(&user, port, &key_path),
    });
    d.ssh.items.lock().await.insert(token.clone(), acc);
    // Expire it.
    let d2 = d.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(minutes * 60)).await;
        end(&d2, &token).await;
    });
    ok_json(out)
}

/// Ends an access: listener closed, key file removed, key unauthorized.
async fn end(d: &Arc<Daytona>, token: &str) -> bool {
    let Some(a) = d.ssh.items.lock().await.remove(token) else {
        return false;
    };
    a.listener.abort();
    let _ = std::fs::remove_file(&a.key_path);
    if let Some(r) = d.store.get(&a.sandbox_id).await
        && let Ok(c) = d.spacesd(&r).await
    {
        let driver = d.drivers.of(&r.id, &c).await;
        let user = d.user_of(&r);
        let _ = crate::guest::run_as(
            &c, &driver, &user,
            "[ -f ~/.ssh/authorized_keys ] && grep -vF -- \"$1\" ~/.ssh/authorized_keys > ~/.ssh/authorized_keys.cua && mv ~/.ssh/authorized_keys.cua ~/.ssh/authorized_keys; true",
            &[&a.pub_line],
        )
        .await;
    }
    true
}

#[derive(serde::Deserialize, Default)]
struct TokenQuery {
    token: Option<String>,
}

async fn revoke(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Without a token every SSH access of the sandbox ends (the API's own
    // rule; the Python SDK's `revoke_ssh_access(token)` reaches here this
    // way, its token landing in the organization header).
    let Some(t) = q.token.filter(|t| !t.is_empty()) else {
        let tokens: Vec<String> = d
            .ssh
            .items
            .lock()
            .await
            .iter()
            .filter(|(_, a)| a.sandbox_id == r.id)
            .map(|(t, _)| t.clone())
            .collect();
        for t in tokens {
            end(&d, &t).await;
        }
        return ok_json(super::api::sandbox_dto(&d, &r));
    };
    let mine = d
        .ssh
        .items
        .lock()
        .await
        .get(&t)
        .is_some_and(|a| a.sandbox_id == r.id);
    if !mine || !end(&d, &t).await {
        return error(StatusCode::NOT_FOUND, "SSH access token not found");
    }
    ok_json(super::api::sandbox_dto(&d, &r))
}

async fn validate(
    State(d): State<Arc<Daytona>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let t = q.token.unwrap_or_default();
    let m = d.ssh.items.lock().await;
    match m
        .get(&t)
        .filter(|a| a.key_id == key.id() && a.expires_ms > crate::state::now_ms())
    {
        Some(a) => ok_json(json!({"valid": true, "sandboxId": a.sandbox_id})),
        None => ok_json(json!({"valid": false, "sandboxId": ""})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_the_port_the_user_and_the_key() {
        let c = ssh_command("daytona", 2222, std::path::Path::new("/x/k.key"));
        assert!(
            c.starts_with("ssh -i /x/k.key -p 2222 ") && c.ends_with("daytona@127.0.0.1"),
            "{c}"
        );
        assert!(c.contains("IdentitiesOnly=yes"));
    }
}
