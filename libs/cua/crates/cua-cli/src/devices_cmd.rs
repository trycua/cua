//! `cua devices …`: this device as a client of your cua.ai account on the
//! relay. A device that lists or reaches your machines through the relay is
//! enrolled once with a second factor (a fresh sign-in, or an approval from
//! an enrolled device) and re-verified after the relay's TTL. A new key of
//! the same machine replaces its old record. Hosting never enrolls a
//! device.

use crate::auth;
use crate::util::{self, line};
use clap::Subcommand;
use cua_host::{DeviceAuth, DeviceState, DeviceView};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

#[derive(Subcommand, Debug, Clone)]
pub enum DevicesCmd {
    /// This device: enrolled, waiting for approval, or not enrolled.
    #[command(after_help = "Examples:
  cua devices status")]
    Status {
        /// Relay URL (default $CUA_RELAY_URL, else https://relay.cua.ai).
        #[arg(long)]
        relay: Option<String>,
    },
    /// Enroll this device. Right after a fresh `cua auth login` it enrolls
    /// at once (and replaces this machine's older device key, if any);
    /// otherwise it shows a one-time code to approve from an enrolled
    /// device (`cua devices approve <code>` or the Cua Spaces app).
    #[command(after_help = "Examples:
  cua devices enroll
  cua devices enroll --name \"work laptop\" --wait")]
    Enroll {
        /// Display name (default: the host name).
        #[arg(long)]
        name: Option<String>,
        /// Wait (up to 10 minutes) until an enrolled device approves.
        #[arg(long)]
        wait: bool,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Approve a new device (or re-verify an expired one) from this
    /// enrolled device, by the code it shows or by its id.
    #[command(after_help = "Examples:
  cua devices approve K7QX-M2RP
  cua devices approve dev_0123456789abcdef01234567")]
    Approve {
        /// The one-time code the new device shows, or its device id
        /// (`dev_…`, from `cua devices ls`).
        code: Option<String>,
        /// Or the device id (`cua devices ls`).
        #[arg(long = "device", conflicts_with = "code")]
        device: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Your devices and their state.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua devices ls
  cua devices ls --json"
    )]
    Ls {
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Rename a device.
    #[command(after_help = "Examples:
  cua devices rename dev_0123456789abcdef01234567 \"work phone\"")]
    Rename {
        /// Device id.
        id: String,
        /// New name.
        name: String,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Revoke a device: it can no longer list or reach your machines.
    #[command(after_help = "Examples:
  cua devices revoke dev_0123456789abcdef01234567")]
    Revoke {
        /// Device id.
        id: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Who accessed what, and when: enrollments, device sessions, machine
    /// access (yours and by accounts you share with), sharing changes.
    #[command(after_help = "Examples:
  cua devices audit
  cua devices audit --limit 20 --json")]
    Audit {
        /// Newest events to show.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
}

impl DevicesCmd {
    fn relay(&self) -> Option<String> {
        match self {
            DevicesCmd::Status { relay }
            | DevicesCmd::Enroll { relay, .. }
            | DevicesCmd::Approve { relay, .. }
            | DevicesCmd::Ls { relay }
            | DevicesCmd::Rename { relay, .. }
            | DevicesCmd::Revoke { relay, .. }
            | DevicesCmd::Audit { relay, .. } => relay.clone(),
        }
    }
}

/// This device on `relay_url`, as the signed-in account.
pub fn device_auth(relay_url: &str) -> Result<Arc<DeviceAuth>, CuaError> {
    named_device_auth(relay_url, None)
}

/// [`device_auth`] shown as `name` (default: the host name).
fn named_device_auth(relay_url: &str, name: Option<&str>) -> Result<Arc<DeviceAuth>, CuaError> {
    DeviceAuth::new(
        relay_url,
        Arc::new(crate::host::SessionTokens),
        Arc::new(auth::Store::from_env()),
        name.map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(cua_host::device_name),
    )
    .map(Arc::new)
    .map_err(crate::host::host_err)
}

/// After `cua auth login`: a device that already has a key re-registers,
/// so the fresh sign-in enrolls (or re-verifies) it without an approval.
/// Best effort: a relay that is down, slow or refuses never fails the
/// login, and a device that never enrolled stays unregistered.
pub async fn after_login(home: &Path, out: &mut dyn Write) {
    let Ok(auth) = device_auth(&crate::host::relay_url(None, home)) else {
        return;
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        auth.enroll_after_sign_in(),
    )
    .await;
    // `cua_device_enroll`: enrolled by this sign-in (`rekey` when it
    // replaced the machine's other key), or failed; nothing otherwise.
    let enrolled = match &result {
        Ok(Ok(Some(r))) if r.device.state == DeviceState::Enrolled => Some((
            if r.superseded.is_empty() {
                "sign_in"
            } else {
                "rekey"
            },
            cua_telemetry::Outcome::Ok,
        )),
        Ok(Ok(_)) => None,
        _ => Some(("sign_in", cua_telemetry::Outcome::Error)),
    };
    if let Some(e) = enrolled.and_then(|(m, o)| cua_telemetry::events::device_enroll(m, o)) {
        cua_telemetry::capture(e);
    }
    match result {
        Ok(Ok(Some(r))) if r.device.state == DeviceState::Enrolled => line(
            out,
            format!(
                "This device ({}) is enrolled until {}.",
                r.device.id,
                when(r.device.enrolled_until)
            ),
        ),
        Ok(Ok(Some(r))) => {
            if let Some(code) = r.code {
                line(
                    out,
                    format!(
                        "This device is waiting for approval: approve it from an enrolled device with `cua devices approve {code}`."
                    ),
                );
            }
        }
        // Not enrolled before, or the relay is unreachable: `cua devices
        // enroll` says more.
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {}
    }
}

fn when(ts: Option<u64>) -> String {
    ts.and_then(|t| chrono::DateTime::<chrono::Utc>::from_timestamp(t as i64, 0))
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "-".into())
}

fn state_label(d: &DeviceView) -> &'static str {
    match d.state {
        DeviceState::Pending => "waiting for approval",
        DeviceState::Enrolled => "enrolled",
        DeviceState::Expired => "re-verification due",
        DeviceState::Revoked => "revoked",
    }
}

fn view_json(d: &DeviceView) -> serde_json::Value {
    serde_json::to_value(d).unwrap_or_default()
}

/// Runs `cmd` for the device `auth` (`confirm` answers the consent
/// prompts; tests pass a fixed answer).
pub async fn run_with(
    cmd: DevicesCmd,
    auth: &DeviceAuth,
    confirm: &dyn Fn(&str) -> bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let err = crate::host::host_err;
    match cmd {
        DevicesCmd::Status { .. } => {
            let id = auth.device_id().map_err(err)?;
            let this = match &id {
                Some(id) => auth
                    .devices()
                    .await
                    .ok()
                    .and_then(|l| l.into_iter().find(|d| &d.id == id)),
                None => None,
            };
            if json {
                line(
                    out,
                    serde_json::json!({
                        "relay": auth.relay_url(),
                        "device_id": id,
                        "device": this.as_ref().map(view_json),
                    })
                    .to_string(),
                );
                return Ok(0);
            }
            match (&id, &this) {
                (None, _) => {
                    line(
                        out,
                        "This device is not enrolled (run `cua devices enroll`).",
                    );
                    return Ok(1);
                }
                (Some(id), None) => line(
                    out,
                    format!(
                        "This device ({id}) is not registered on {}.",
                        auth.relay_url()
                    ),
                ),
                (Some(_), Some(d)) => {
                    line(out, format!("{} ({}): {}", d.name, d.id, state_label(d)));
                    if d.state == DeviceState::Enrolled {
                        line(out, format!("  re-verify by {}", when(d.enrolled_until)));
                    }
                }
            }
        }
        DevicesCmd::Enroll { wait, .. } => {
            let r = auth.enroll().await.map_err(err)?;
            if json {
                line(
                    out,
                    serde_json::json!({
                        "device": view_json(&r.device),
                        "code": r.code,
                        "enforce_after": r.enforce_after,
                        "superseded": r.superseded,
                    })
                    .to_string(),
                );
            } else if r.device.state == DeviceState::Enrolled {
                line(
                    out,
                    format!(
                        "This device ({}) is enrolled until {}.",
                        r.device.id,
                        when(r.device.enrolled_until)
                    ),
                );
                if !r.superseded.is_empty() {
                    line(
                        out,
                        format!(
                            "It replaces this machine's older device key ({}).",
                            r.superseded.join(", ")
                        ),
                    );
                }
            } else if let Some(code) = &r.code {
                line(
                    out,
                    format!("Approve this device from an enrolled device with the code {code}:"),
                );
                line(out, format!("  cua devices approve {code}"));
                line(
                    out,
                    "(or approve it in the Cua Spaces app). The code expires in 10 minutes.",
                );
                line(
                    out,
                    "Or sign in again: `cua auth login` enrolls this device (older relays: only your first device).",
                );
            }
            if wait && r.device.state != DeviceState::Enrolled {
                let _ = out.flush();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
                loop {
                    if auth.session().await.is_ok() {
                        if !json {
                            line(out, "Approved: this device is enrolled.");
                        }
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(CuaError::Timeout(
                            "no enrolled device approved this device in 10 minutes".into(),
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }
        DevicesCmd::Approve {
            code, device, yes, ..
        } => {
            let target = code
                .as_deref()
                .map(|c| {
                    if c.trim().starts_with(cua_host::relay::DEVICE_ID_PREFIX) {
                        format!("device {}", c.trim())
                    } else {
                        format!("the device showing code {c}")
                    }
                })
                .or_else(|| device.as_deref().map(|d| format!("device {d}")))
                .ok_or_else(|| {
                    CuaError::InvalidArgument(
                        "give the code the new device shows, or its device id".into(),
                    )
                })?;
            // Enrolling a device widens who reaches your machines: ask.
            if !yes && !confirm(&format!("Let {target} list and reach your machines?")) {
                return Err(CuaError::PermissionDenied(
                    "not approved (pass --yes to approve without a prompt)".into(),
                ));
            }
            let d = auth
                .approve(code.as_deref(), device.as_deref())
                .await
                .map_err(err)?;
            if json {
                line(out, view_json(&d).to_string());
            } else {
                line(
                    out,
                    format!(
                        "Approved {} ({}) until {}.",
                        d.name,
                        d.id,
                        when(d.enrolled_until)
                    ),
                );
            }
        }
        DevicesCmd::Ls { .. } => {
            let list = auth.devices().await.map_err(err)?;
            if json {
                line(
                    out,
                    serde_json::json!({ "devices": list.iter().map(view_json).collect::<Vec<_>>() })
                        .to_string(),
                );
                return Ok(0);
            }
            if list.is_empty() {
                line(out, "No devices.");
            }
            for d in &list {
                line(
                    out,
                    format!(
                        "{}{:<30} {:<24} {:<22} last seen {}",
                        if d.current { "* " } else { "  " },
                        d.id,
                        d.name,
                        state_label(d),
                        when(d.last_seen)
                    ),
                );
            }
        }
        DevicesCmd::Rename { id, name, .. } => {
            let d = auth.rename(&id, &name).await.map_err(err)?;
            if json {
                line(out, view_json(&d).to_string());
            } else {
                line(out, format!("Renamed {} to {}.", d.id, d.name));
            }
        }
        DevicesCmd::Revoke { id, yes, .. } => {
            if !yes && !confirm(&format!("Revoke device {id}?")) {
                return Err(CuaError::PermissionDenied(
                    "not revoked (pass --yes to revoke without a prompt)".into(),
                ));
            }
            let d = auth.revoke(&id).await.map_err(err)?;
            if json {
                line(out, view_json(&d).to_string());
            } else {
                line(out, format!("Revoked {} ({}).", d.name, d.id));
            }
        }
        DevicesCmd::Audit { limit, .. } => {
            let events = auth.audit(limit).await.map_err(err)?;
            if json {
                line(out, serde_json::json!({ "events": events }).to_string());
                return Ok(0);
            }
            if events.is_empty() {
                line(out, "No events.");
            }
            for e in &events {
                let mut parts = vec![when(Some(e.ts)), e.kind.clone()];
                if let Some(d) = &e.device {
                    parts.push(format!("device {d}"));
                }
                if let Some(m) = &e.machine {
                    parts.push(format!("machine {m}"));
                }
                if let Some(s) = &e.subject {
                    parts.push(s.clone());
                }
                if let Some(d) = &e.detail {
                    parts.push(d.clone());
                }
                line(out, parts.join("  "));
            }
        }
    }
    Ok(0)
}

/// `cua devices …` as the signed-in account.
pub async fn run(
    cmd: DevicesCmd,
    home: &Path,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let url = crate::host::relay_url(cmd.relay(), home);
    let name = match &cmd {
        DevicesCmd::Enroll { name, .. } => name.clone(),
        _ => None,
    };
    let auth = named_device_auth(&url, name.as_deref())?;
    run_with(
        cmd,
        &auth,
        &|q| util::interactive() && util::confirm(q, false),
        json,
        out,
    )
    .await
}

#[cfg(test)]
mod tests {

    #[test]
    fn enroll_uses_the_given_name_else_the_host_name() {
        let named = named_device_auth("http://127.0.0.1:9", Some("  work laptop ")).unwrap();
        assert!(format!("{named:?}").contains("\"work laptop\""));
        let blank = named_device_auth("http://127.0.0.1:9", Some("  ")).unwrap();
        assert!(format!("{blank:?}").contains(&format!("{:?}", cua_host::device_name())));
    }

    use super::*;
    use cua_host::testing::FakeRelay;
    use cua_host::{MemoryKeySlot, StaticToken};

    fn device(relay: &FakeRelay, name: &str) -> DeviceAuth {
        DeviceAuth::new(
            &relay.url,
            Arc::new(StaticToken("acct".into())),
            Arc::new(MemoryKeySlot::default()),
            name,
        )
        .unwrap()
        .with_machine_id(Some(format!("machine-of-{name}")))
    }

    async fn run_text(
        cmd: DevicesCmd,
        auth: &DeviceAuth,
        answer: bool,
    ) -> Result<String, CuaError> {
        let mut out = Vec::new();
        run_with(cmd, auth, &|_| answer, false, &mut out).await?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[tokio::test]
    async fn enroll_approve_list_revoke_and_audit() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        relay.require_devices(true);
        relay.fresh_sign_in("user-1");
        let laptop = device(&relay, "laptop");
        let text = run_text(
            DevicesCmd::Enroll {
                name: None,
                wait: false,
                relay: None,
            },
            &laptop,
            false,
        )
        .await
        .unwrap();
        assert!(text.contains("is enrolled until"), "{text}");

        // On a long-lived session the next device shows a code.
        relay.stale_sign_in("user-1");
        let phone = device(&relay, "phone");
        let text = run_text(
            DevicesCmd::Enroll {
                name: None,
                wait: false,
                relay: None,
            },
            &phone,
            false,
        )
        .await
        .unwrap();
        let code = text
            .split_whitespace()
            .find(|w| w.len() == 10 && w.contains('-') && w.ends_with(':'))
            .map(|w| w.trim_end_matches(':').to_string())
            .expect(&text);

        // Approving needs consent.
        let approve = |yes| DevicesCmd::Approve {
            code: Some(code.clone()),
            device: None,
            yes,
            relay: None,
        };
        assert!(matches!(
            run_text(approve(false), &laptop, false).await,
            Err(CuaError::PermissionDenied(_))
        ));
        let text = run_text(approve(false), &laptop, true).await.unwrap();
        assert!(text.starts_with("Approved phone"), "{text}");
        assert!(phone.session().await.is_ok());

        let text = run_text(DevicesCmd::Ls { relay: None }, &laptop, false)
            .await
            .unwrap();
        assert!(text.contains("* dev_") && text.contains("laptop"), "{text}");
        assert!(
            text.contains("phone") && text.contains("enrolled"),
            "{text}"
        );

        let phone_id = phone.device_id().unwrap().unwrap();
        assert!(matches!(
            run_text(
                DevicesCmd::Revoke {
                    id: phone_id.clone(),
                    yes: false,
                    relay: None
                },
                &laptop,
                false
            )
            .await,
            Err(CuaError::PermissionDenied(_))
        ));
        run_text(
            DevicesCmd::Revoke {
                id: phone_id.clone(),
                yes: true,
                relay: None,
            },
            &laptop,
            false,
        )
        .await
        .unwrap();
        assert_eq!(relay.device_state(&phone_id), Some(DeviceState::Revoked));
        let text = run_text(
            DevicesCmd::Audit {
                limit: 50,
                relay: None,
            },
            &laptop,
            false,
        )
        .await
        .unwrap();
        assert!(text.contains("device_revoked"), "{text}");
        assert!(text.contains("device_enrolled"), "{text}");
    }

    #[tokio::test]
    async fn a_fresh_sign_in_enrolls_and_approve_takes_a_device_id() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        relay.require_devices(true);
        relay.fresh_sign_in("user-1");
        let enroll = || DevicesCmd::Enroll {
            name: None,
            wait: false,
            relay: None,
        };
        let laptop = device(&relay, "laptop");
        run_text(enroll(), &laptop, false).await.unwrap();
        // Another device enrolls by the fresh sign-in alone.
        let desktop = device(&relay, "desktop");
        let text = run_text(enroll(), &desktop, false).await.unwrap();
        assert!(text.contains("is enrolled until"), "{text}");
        // A new key of the laptop's machine replaces its record.
        let rekeyed = DeviceAuth::new(
            &relay.url,
            Arc::new(StaticToken("acct".into())),
            Arc::new(MemoryKeySlot::default()),
            "laptop",
        )
        .unwrap()
        .with_machine_id(Some("machine-of-laptop".into()));
        let text = run_text(enroll(), &rekeyed, false).await.unwrap();
        let old = laptop.device_id().unwrap().unwrap();
        assert!(
            text.contains(&format!("replaces this machine's older device key ({old})")),
            "{text}"
        );
        // A pending device, approved by the id `cua devices ls` shows.
        relay.stale_sign_in("user-1");
        let phone = device(&relay, "phone");
        let text = run_text(enroll(), &phone, false).await.unwrap();
        assert!(text.contains("Or sign in again"), "{text}");
        let phone_id = phone.device_id().unwrap().unwrap();
        let text = run_text(
            DevicesCmd::Approve {
                code: Some(phone_id.clone()),
                device: None,
                yes: true,
                relay: None,
            },
            &desktop,
            false,
        )
        .await
        .unwrap();
        assert!(text.starts_with("Approved phone"), "{text}");
        // An unknown code explains how to approve instead.
        let e = run_text(
            DevicesCmd::Approve {
                code: Some("ZZZZ-ZZZZ".into()),
                device: None,
                yes: true,
                relay: None,
            },
            &desktop,
            false,
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("approve by id"), "{e}");
    }
}
