//! Unreachable Tailscale and other direct targets.
//!
//! `cua do switch url` and the daemon remember a direct spacesd in
//! `~/.cua/sandboxes/<name>.json` (`runtime_type: "direct"`) and the selected
//! target in `~/.cua/do_target.json`. `cua spaces add <addr> --host` records
//! the same kind of address in `direct-hosts.json` and `spaces.json`. Those
//! rows stay after the peer moves or goes offline. This checks them and,
//! with `--clean`, drops the ones that no longer accept a connection.
//! Relay Spaces are never removed.

use std::collections::BTreeMap;
use std::time::Duration;

use cua_sdk::{Cua, CuaError};
use cua_spaces::SpaceId;
use cua_spaces::registry::Registry;
use cua_spacesd_client::{Endpoint, EndpointKind};

use crate::util;

/// Same budget as the direct-host probe in `Spaces::hosts`: a gone Tailscale
/// peer blackholes the handshake, a refused port returns at once.
const PROBE: Duration = Duration::from_secs(4);

struct Entry {
    label: String,
    shown: String,
    host: String,
    port: u16,
    sandbox_name: Option<String>,
    space_id: Option<String>,
}

/// Warn about dead direct targets. With `clean`, drop them.
pub(crate) async fn check(
    cua: &Cua,
    clean: bool,
    out: &mut dyn std::io::Write,
) -> Result<i32, CuaError> {
    let (entries, relay) = collect(cua).await?;
    let up = probe(&entries).await;
    let mut dead: Vec<&Entry> = entries
        .iter()
        .filter(|e| up.get(&(e.host.clone(), e.port)) != Some(&true))
        .collect();
    dead.sort_by(|a, b| a.shown.cmp(&b.shown).then(a.label.cmp(&b.label)));
    let current = crate::do_cmd::current_target_name();
    let current_dead = dead.iter().find(|e| names_target(e, &current));

    if dead.is_empty() {
        let msg = if entries.is_empty() {
            "No direct targets are registered."
        } else {
            "Direct targets are reachable."
        };
        util::line(out, msg);
        return Ok(0);
    }

    if let Some(e) = current_dead {
        util::line(
            out,
            format!("Current target {} ({}) is unreachable.", e.label, e.shown),
        );
    }
    if !clean {
        util::line(out, "Unreachable direct targets:");
        for e in &dead {
            util::line(out, format!("  {}  {}", e.label, e.shown));
        }
        if current_dead.is_some()
            && let Some(id) = &relay
        {
            util::line(out, format!("Next: cua do switch {id}"));
        }
        util::line(
            out,
            "Run `cua do check --clean` to drop them. Relay Spaces stay.",
        );
        return Ok(1);
    }

    let mut failed = false;
    for e in &dead {
        if let Err(err) = drop_entry(cua, e).await {
            eprintln!("{}: {err}", e.label);
            failed = true;
            continue;
        }
        util::line(out, format!("Removed {} ({})", e.label, e.shown));
    }
    if current_dead.is_some() {
        crate::do_cmd::clear_current_target()?;
        util::line(out, "Cleared the current cua do target.");
        if let Some(id) = &relay {
            util::line(out, format!("Next: cua do switch {id}"));
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn names_target(e: &Entry, current: &str) -> bool {
    !current.is_empty()
        && (e.sandbox_name.as_deref() == Some(current) || e.label == current || e.shown == current)
}

async fn collect(cua: &Cua) -> Result<(Vec<Entry>, Option<String>), CuaError> {
    let mut entries = Vec::new();
    for sandbox in cua.sandboxes().list_known(Some("direct")).await? {
        let Some((shown, host, port)) = parse_direct(&sandbox.id) else {
            continue;
        };
        entries.push(Entry {
            label: sandbox.name.clone(),
            shown,
            host,
            port,
            sandbox_name: Some(sandbox.name),
            space_id: None,
        });
    }

    let reg = Registry::new(util::cua_home());
    let spaces = reg
        .list()
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    let mut relay = None;
    for space in spaces {
        match SpaceId::parse(&space.id) {
            Ok(SpaceId::Relay { .. }) if relay.is_none() => relay = Some(space.id),
            Ok(SpaceId::Direct { .. }) => {
                let Some((shown, host, port)) = parse_direct(&space.id) else {
                    continue;
                };
                if let Some(existing) = entries.iter_mut().find(|e| e.shown == shown) {
                    existing.space_id = Some(shown);
                } else {
                    let label = if space.name.is_empty() {
                        host.clone()
                    } else {
                        space.name
                    };
                    entries.push(Entry {
                        label,
                        shown: shown.clone(),
                        host,
                        port,
                        sandbox_name: None,
                        space_id: Some(shown),
                    });
                }
            }
            _ => {}
        }
    }
    for host in reg
        .direct_hosts()
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?
    {
        let Some((shown, addr, port)) = parse_direct(&host.space) else {
            continue;
        };
        if let Some(existing) = entries.iter_mut().find(|e| e.shown == shown) {
            existing.space_id = Some(shown);
            continue;
        }
        let label = if host.name.is_empty() {
            addr.clone()
        } else {
            host.name
        };
        entries.push(Entry {
            label,
            shown: shown.clone(),
            host: addr,
            port,
            sandbox_name: None,
            space_id: Some(shown),
        });
    }
    Ok((entries, relay))
}

fn parse_direct(id: &str) -> Option<(String, String, u16)> {
    let id = SpaceId::parse(id).ok()?;
    let SpaceId::Direct { authority } = &id else {
        return None;
    };
    let ep = Endpoint::parse(authority).ok()?;
    if !matches!(ep.kind(), EndpointKind::Direct) {
        return None;
    }
    let host = ep.host();
    if host.is_empty() {
        return None;
    }
    Some((id.to_string(), host.to_string(), ep.port()))
}

async fn probe(entries: &[Entry]) -> BTreeMap<(String, u16), bool> {
    let mut keys: Vec<(String, u16)> = entries.iter().map(|e| (e.host.clone(), e.port)).collect();
    keys.sort();
    keys.dedup();
    let probed = futures_util::future::join_all(keys.into_iter().map(|(host, port)| async move {
        let up = matches!(
            tokio::time::timeout(PROBE, tokio::net::TcpStream::connect((host.as_str(), port)))
                .await,
            Ok(Ok(_))
        );
        ((host, port), up)
    }))
    .await;
    probed.into_iter().collect()
}

async fn drop_entry(cua: &Cua, entry: &Entry) -> Result<(), CuaError> {
    if let Some(name) = &entry.sandbox_name {
        drop_sandbox(cua, name, &entry.shown).await?;
    }
    if let Some(id) = &entry.space_id {
        drop_space(cua, id).await?;
    }
    Ok(())
}

async fn drop_sandbox(cua: &Cua, name: &str, id: &str) -> Result<(), CuaError> {
    let mut err = None;
    for key in [name, id] {
        match cua.sandboxes().delete(key.to_string()).await {
            Ok(()) => {
                err = None;
                break;
            }
            Err(CuaError::NotFound(_)) => {}
            Err(e) => err = Some(e),
        }
    }
    let store = cua_sandbox_core::StateStore::new(util::cua_home().join("sandboxes"));
    if store.load(name).is_some() {
        store
            .delete(name)
            .map_err(|e| CuaError::Internal(format!("removing {name}: {e}")))?;
        err = None;
    }
    for state in store.list_all() {
        if state.runtime_type() == "direct" && state.sandbox_ref().to_string() == id {
            store
                .delete(state.name())
                .map_err(|e| CuaError::Internal(format!("removing {}: {e}", state.name())))?;
            err = None;
        }
    }
    err.map_or(Ok(()), Err)
}

async fn drop_space(cua: &Cua, id: &str) -> Result<(), CuaError> {
    match cua.spaces().remove(id.to_string()).await {
        Ok(()) | Err(CuaError::NotFound(_)) => Registry::new(util::cua_home())
            .forget_direct(id)
            .map(|_| ())
            .map_err(|e| CuaError::InvalidArgument(e.to_string())),
        Err(e) => Err(e),
    }
}
