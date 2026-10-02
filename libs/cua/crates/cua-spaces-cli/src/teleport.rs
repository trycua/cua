// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua teleport` in the Cua Spaces build: the providers, the manifest and
//! the push, on the `cua-teleport` providers. The command line is
//! `cua_cli::teleport`.

use std::io::Write;
use std::sync::Arc;

use cua_cli::teleport::{AppArgs, PushArgs, TeleportCmd};
use cua_cli::util::line;
use cua_sdk::{Cua, CuaError};
use cua_teleport::{
    AppRef, ApprovalRequest, ExportRegistry, HostEffects, Platform, Selection, SendOptions,
    Teleporter, TransferScope,
};

/// Runs `cmd` against the real host (`cua` is open for `push`).
pub async fn run(
    cua: Option<&Cua>,
    cmd: TeleportCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let host = cua_teleport::default_host();
    match cmd {
        TeleportCmd::Providers => providers(&host, out),
        TeleportCmd::Manifest(args) => manifest(&host, args, out).await,
        TeleportCmd::Push(args) => {
            let cua =
                cua.ok_or_else(|| CuaError::Internal("teleport push needs the SDK".into()))?;
            push(cua, &host, args, json, out).await
        }
    }
}

fn scope(args: &AppArgs) -> Result<TransferScope, CuaError> {
    TransferScope::from_cli(&args.scope).ok_or_else(|| {
        CuaError::InvalidArgument(format!("invalid scope {:?}; use tabs or full", args.scope))
    })
}

/// A teleport error as an SDK error (as the SDK's own teleport maps it).
pub fn teleport_err(e: cua_teleport::Error) -> CuaError {
    use cua_teleport::{Error as E, TeleportError as T};
    let m = e.to_string();
    match e {
        E::Env(e) => e.into(),
        E::InvalidSelection(_) => CuaError::InvalidArgument(m),
        E::NotApproved => CuaError::PermissionDenied(m),
        E::Unsupported { .. } => CuaError::Unsupported(m),
        E::Teleport(T::NoProviderForApp { .. } | T::UnknownProvider { .. }) => {
            CuaError::Unsupported(m)
        }
        E::Teleport(T::UnsupportedScope { .. }) => CuaError::Unsupported(m),
        E::Teleport(T::Provider(p)) if p.contains("not authorized") => {
            CuaError::PermissionDenied(m)
        }
        // S1: refused because the destination is a relay: Space and
        // nothing has sealed this delivery yet; `--relay-plaintext-ack`
        // (or the equivalent option) opts in for one send.
        E::RelayUnsealed => CuaError::PermissionDenied(m),
        E::Teleport(_) | E::Task(_) => CuaError::Internal(m),
    }
}

fn app_ref(args: &AppArgs) -> AppRef {
    AppRef {
        app_id: args.app.clone(),
        display_name: args
            .display_name
            .clone()
            .unwrap_or_else(|| args.app.clone()),
        platform: Platform::current(),
    }
}

fn teleporter(host: &Arc<dyn HostEffects>, args: &AppArgs) -> Teleporter {
    Teleporter::with_registry(ExportRegistry::with_builtin_host_and_chrome_profile(
        host.clone(),
        args.profile.clone(),
    ))
}

fn pretty(v: &impl serde::Serialize) -> Result<String, CuaError> {
    serde_json::to_string_pretty(v).map_err(|e| CuaError::Internal(e.to_string()))
}

/// `cua teleport providers`.
fn providers(host: &Arc<dyn HostEffects>, out: &mut dyn Write) -> Result<i32, CuaError> {
    let infos: Vec<serde_json::Value> = ExportRegistry::with_builtin_host(host.clone())
        .infos()
        .into_iter()
        .map(|info| {
            let installed = info.install_probe.as_ref().map(cua_teleport::is_installed);
            let mut v = serde_json::to_value(&info).unwrap_or_default();
            v["installed"] = serde_json::json!(installed);
            v
        })
        .collect();
    line(out, pretty(&infos)?);
    Ok(0)
}

/// `cua teleport manifest`.
async fn manifest(
    host: &Arc<dyn HostEffects>,
    args: AppArgs,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let t = teleporter(host, &args);
    let app = app_ref(&args);
    let scope = scope(&args)?;
    let m = tokio::task::spawn_blocking(move || t.manifest(&app, scope))
        .await
        .map_err(|e| CuaError::Internal(e.to_string()))?
        .map_err(teleport_err)?;
    line(out, pretty(&m)?);
    Ok(0)
}

/// `cua teleport push`. A named `--sandbox` is a real Space the local Cua
/// Keyvault can target by name: route through it (`import_and_teleport`,
/// one Touch ID prompt, never saved to the vault afterward), so a push is
/// audited, respects the kill switch and auto-wipes like every other
/// teleport path. `--url`/`--token` aims at an arbitrary spacesd endpoint
/// that is not necessarily a Space the daemon's registry knows by name at
/// all -- the Keyvault cannot target what it does not manage, so that mode
/// keeps the direct path it always had (a lower-level escape hatch for
/// reaching a spacesd directly, not "Teleport an app").
async fn push(
    cua: &Cua,
    host: &Arc<dyn HostEffects>,
    args: PushArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match (&args.sandbox, &args.url) {
        (Some(name), _) => push_through_keyvault(host, name, &args, json, out).await,
        (None, Some(_)) => push_direct(cua, host, args, json, out).await,
        (None, None) => Err(CuaError::InvalidArgument(
            "pass --sandbox NAME or --url URL".into(),
        )),
    }
}

/// `cua teleport push --sandbox NAME`: resolves the selection locally (the
/// exact same [`cua_teleport::resolve_selection`] `Teleporter::send` uses,
/// so `--include`/`--all`/the default set mean exactly what `teleport
/// manifest` showed), then hands capture and delivery to the Cua Keyvault
/// in one call, as the signed `cua` this process is.
async fn push_through_keyvault(
    host: &Arc<dyn HostEffects>,
    sandbox: &str,
    args: &PushArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if args.app.profile.is_some() {
        // The daemon's own Keyvault capture has no per-call profile
        // selection yet (it always reads the machine's default Chrome
        // profile) -- refuse rather than silently ignore an explicit
        // --profile, which `--url` mode still honors in full.
        return Err(CuaError::Unsupported(
            "--profile is not supported for a --sandbox push yet (the Keyvault always reads \
             the default profile); use --url for a specific profile"
                .into(),
        ));
    }
    let t = teleporter(host, &args.app);
    let app_ref = app_ref(&args.app);
    let scope_v = scope(&args.app)?;
    let selection = if args.all {
        cua_teleport::Selection::All
    } else if args.include.is_empty() {
        cua_teleport::Selection::Default
    } else {
        cua_teleport::Selection::Items(args.include.clone())
    };
    let manifest = tokio::task::spawn_blocking({
        let t = t;
        let app_ref = app_ref.clone();
        move || t.manifest(&app_ref, scope_v)
    })
    .await
    .map_err(|e| CuaError::Internal(e.to_string()))?
    .map_err(teleport_err)?;
    let (_, selected, withheld) =
        cua_teleport::resolve_selection(&manifest, &selection).map_err(teleport_err)?;
    let paths: Vec<String> = selected.iter().map(|i| i.rel_path.clone()).collect();
    eprintln!(
        "teleporting {} to {sandbox}: {}",
        manifest.app_display_name,
        paths.join(", ")
    );
    if !withheld.is_empty() {
        eprintln!(
            "note: not sending {} (not selected); pass --include <item> to add it",
            withheld
                .iter()
                .map(|i| i.rel_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let spec = cua_keyvault::broker::ImportSpec {
        app: manifest.provider_id.clone(),
        profile: None,
        sites: vec![],
        whole_app: true,
        cookies: cua_keyvault::broker::CookieFilter::default(),
        confirm_passwords: true,
        paths: Some(paths.clone()),
        domains: None,
        passwords: false,
    };
    let mut client = cua_keyvault::client::KeyvaultClient::connect_default()
        .await
        .map_err(|e| {
            CuaError::Unsupported(format!(
                "teleport push goes through the Cua Keyvault, and it is not reachable ({e}); \
                 install or open Cua, or run this as the signed `cua` app/CLI"
            ))
        })?;
    let outcome = client
        .import_and_teleport_launching(spec, sandbox.to_string(), false, !args.no_launch, |_| {})
        .await
        .map_err(|e| CuaError::PermissionDenied(format!("{}: {e}", manifest.provider_id)))?;
    let delivery = outcome.deliveries.first().cloned().unwrap_or_default();
    if json {
        line(
            out,
            serde_json::json!({
                "provider_id": manifest.provider_id,
                "import_id": delivery.import_id,
                "sent": paths,
                "withheld": withheld.iter().map(|i| i.rel_path.clone()).collect::<Vec<_>>(),
                "imported": delivery.imported,
                "skipped": delivery.skipped,
                "launched": delivery.launched,
            })
            .to_string(),
        );
    } else {
        line(
            out,
            format!(
                "teleported {} ({} items){}",
                manifest.provider_id,
                paths.len(),
                if delivery.launched { ", launched" } else { "" }
            ),
        );
        for s in &delivery.skipped {
            line(out, format!("skipped: {s}"));
        }
    }
    Ok(0)
}

/// `cua teleport push --url URL`: the direct path, unchanged -- an arbitrary
/// spacesd endpoint the Keyvault does not manage as a Space.
async fn push_direct(
    cua: &Cua,
    host: &Arc<dyn HostEffects>,
    args: PushArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let env = match (&args.sandbox, &args.url) {
        // A direct sandbox needs its registered token.
        (Some(name), _) => cua_cli::sandbox::env_of(cua, name).await?,
        (None, Some(url)) => cua.spacesd(url.clone(), args.token.clone()).await?,
        (None, None) => {
            return Err(CuaError::InvalidArgument(
                "pass --sandbox NAME or --url URL".into(),
            ));
        }
    };
    let selection = if args.all {
        Selection::All
    } else if args.include.is_empty() {
        Selection::Default
    } else {
        Selection::Items(args.include.clone())
    };
    let progress: Option<cua_teleport::Progress> = args.progress.then(|| {
        Arc::new(|sent: u64, total: u64| {
            // Machine-readable, flushed per line so a piped consumer can drive
            // a progress bar.
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "progress {sent} {total}");
            let _ = stdout.flush();
        }) as cua_teleport::Progress
    });
    let t = teleporter(host, &args.app).options(SendOptions {
        launch_after: !args.no_launch,
        progress,
        relay_plaintext_ack: args.relay_plaintext_ack,
        ..SendOptions::default()
    });
    // Running `cua teleport push` is the consent; say what is going and what
    // the default selection left behind.
    let approval = Arc::new(|r: &ApprovalRequest<'_>| {
        let names: Vec<&str> = r.selected.iter().map(|i| i.rel_path.as_str()).collect();
        eprintln!(
            "teleporting {} to {}: {}",
            r.manifest.app_display_name,
            r.destination,
            names.join(", ")
        );
        let left: Vec<&str> = r
            .manifest
            .items
            .iter()
            .filter(|i| !names.contains(&i.rel_path.as_str()))
            .map(|i| i.rel_path.as_str())
            .collect();
        if !left.is_empty() {
            eprintln!(
                "note: not sending {} (not selected); pass --include <item> to add it",
                left.join(", ")
            );
        }
        true
    });
    let outcome = t
        .send(
            env.inner(),
            &app_ref(&args.app),
            scope(&args.app)?,
            selection,
            approval,
        )
        .await
        .map_err(teleport_err)?;
    if json {
        line(
            out,
            serde_json::json!({
                "provider_id": outcome.provider_id,
                "import_id": outcome.import_id,
                "bundle_bytes": outcome.bundle_bytes,
                "sha256": outcome.sha256,
                "sent": outcome.sent,
                "withheld": outcome.withheld,
                "imported": outcome.imported,
                "skipped": outcome.skipped,
                "launched": outcome.launched,
            })
            .to_string(),
        );
    } else {
        line(
            out,
            format!(
                "teleported {} ({} items, {} bytes){}",
                outcome.provider_id,
                outcome.sent.len(),
                outcome.bundle_bytes,
                if outcome.launched { ", launched" } else { "" }
            ),
        );
        for s in &outcome.skipped {
            line(out, format!("skipped: {s}"));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    //! Host-safe: every command runs on a `FakeHost` whose home is a temp dir
    //! with a fake Slack profile; pushes go to the in-process mock
    //! spacesd, which only reassembles the upload.
    use super::*;
    use clap::Parser;
    use cua_teleport::FakeHost;
    use cua_teleport::layout::electron::app_support_root;

    fn fake_home() -> (tempfile::TempDir, Arc<dyn HostEffects>) {
        let home = tempfile::tempdir().unwrap();
        let prof = home
            .path()
            .join(app_support_root(Platform::current()))
            .join("Slack");
        std::fs::create_dir_all(&prof).unwrap();
        std::fs::write(prof.join("Cookies"), b"slack-cookies").unwrap();
        std::fs::write(prof.join("Preferences"), b"{}").unwrap();
        let host: Arc<dyn HostEffects> = Arc::new(FakeHost::new().with_home(home.path()));
        (home, host)
    }

    #[derive(clap::Parser)]
    struct Parsed {
        #[command(subcommand)]
        cmd: TeleportCmd,
    }

    /// `cua teleport <args>` as the CLI parses it.
    fn parse(args: &[&str]) -> TeleportCmd {
        Parsed::try_parse_from(std::iter::once("teleport").chain(args.iter().copied().skip(1)))
            .unwrap()
            .cmd
    }

    #[test]
    fn providers_lists_every_builtin_app_as_json() {
        let (_home, host) = fake_home();
        let mut out = Vec::new();
        assert_eq!(providers(&host, &mut out).unwrap(), 0);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let ids: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, cua_teleport::layout::PROVIDER_IDS);
        assert!(
            v[2]["app_ids"]
                .to_string()
                .contains("com.tinyspeck.slackmacgap")
        );
        assert!(v[2].get("installed").is_some());
    }

    #[tokio::test]
    async fn manifest_reads_the_fake_home() {
        let (_home, host) = fake_home();
        let TeleportCmd::Manifest(args) = parse(&["teleport", "manifest", "--app", "Slack"]) else {
            panic!()
        };
        let mut out = Vec::new();
        assert_eq!(manifest(&host, args, &mut out).await.unwrap(), 0);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["provider_id"], "slack");
        assert!(v["items"].to_string().contains("electron/Cookies"));
        // An unknown app is a clean error, not a panic.
        let TeleportCmd::Manifest(args) = parse(&["teleport", "manifest", "--app", "nope"]) else {
            panic!()
        };
        assert!(matches!(
            manifest(&host, args, &mut Vec::new()).await,
            Err(CuaError::Unsupported(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn push_uploads_to_an_spacesd_url() {
        let env = cua_daemon::fixtures::start_env(Some("t"), None).await;
        let (_home, host) = fake_home();
        let dirs = tempfile::tempdir().unwrap();
        let cua = Cua::from_runtime(
            cua_daemon::Runtime::new(cua_daemon::RuntimeConfig {
                state_dir: Some(dirs.path().to_path_buf()),
                spaces_home: Some(dirs.path().join("cua")),
                teleport_home: Some(dirs.path().join("cua")),
                ..Default::default()
            })
            .unwrap(),
        );
        let TeleportCmd::Push(args) = parse(&[
            "teleport",
            "push",
            "--app",
            "Slack",
            "--url",
            &env.url,
            "--token",
            "t",
            "--include",
            "electron/Preferences",
        ]) else {
            panic!()
        };
        let mut out = Vec::new();
        assert_eq!(push(&cua, &host, args, true, &mut out).await.unwrap(), 0);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["provider_id"], "slack");
        assert_eq!(v["sent"], serde_json::json!(["electron/Preferences"]));
        let imports = env.mock.state.teleport_imports();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].app, "slack");
        assert!(imports[0].options.launch_after);
    }
}
