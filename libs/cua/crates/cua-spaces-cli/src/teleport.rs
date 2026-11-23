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
        TeleportCmd::Doctor(args) => doctor(args, json, out).await,
        TeleportCmd::Push(args) => {
            let cua =
                cua.ok_or_else(|| CuaError::Internal("teleport push needs the SDK".into()))?;
            push(cua, &host, args, json, out).await
        }
    }
}

/// Says, before a teleport reads or asks anything, what stops it: the
/// Keyvault not set up, macOS refusing to let the daemon read the browser, or
/// a profile with no sign-in data. Each is a plain sentence with the next
/// step; none of them is routed around. An app without profiles passes.
pub async fn preflight(
    client: &mut cua_keyvault::client::KeyvaultClient,
    app: &str,
    profile: Option<&str>,
) -> Result<(), CuaError> {
    use cua_keyvault::broker::Access;
    let status = client.status().await.map_err(kv_err)?;
    if !status.initialized {
        return Err(CuaError::PermissionDenied(
            "The Keyvault is not set up yet. Open Cua and create it; teleport keeps what it moves in it.".into(),
        ));
    }
    let ready = client.readiness(app, profile).await.map_err(kv_err)?;
    if ready.files.state == Access::Denied {
        return Err(CuaError::PermissionDenied(ready.files.say));
    }
    if ready.profile_empty {
        return Err(CuaError::InvalidArgument(ready.files.say));
    }
    Ok(())
}

/// `cua teleport doctor`: what a teleport needs, checked now, in plain words.
/// Nothing is read from the browser and no presence is asked; the checks run
/// in the daemon, the process whose permissions matter.
async fn doctor(
    args: cua_cli::teleport::DoctorArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use cua_keyvault::broker::Access;
    let mut client = cua_keyvault::client::KeyvaultClient::connect_default()
        .await
        .map_err(|e| {
            CuaError::Unsupported(format!(
                "The Cua daemon is not reachable ({e}). Open Cua, or start it with `cua daemon start`, then try again."
            ))
        })?;
    let status = client.status().await.map_err(kv_err)?;
    let ready = client
        .readiness(&args.app, args.profile.as_deref())
        .await
        .map_err(kv_err)?;
    let keyvault = if !status.initialized {
        "The Keyvault is not set up yet. Open Cua and create it; teleport keeps what it moves in it."
    } else {
        ""
    };
    let ok = keyvault.is_empty() && ready.files.state != Access::Denied && !ready.profile_empty;
    if json {
        util_json(
            out,
            serde_json::json!({
                "ready": ok,
                "keyvault": { "initialized": status.initialized, "unlocked": status.unlocked, "say": keyvault },
                "readiness": ready,
            }),
        );
        return Ok(i32::from(!ok));
    }
    let word = |ok: bool| if ok { "ready" } else { "needs attention" };
    line(out, format!("Keyvault        {}", word(status.initialized)));
    if !keyvault.is_empty() {
        line(out, format!("                {keyvault}"));
    }
    line(
        out,
        format!(
            "{:<15} {}",
            format!("{} files", ready.app_display),
            word(ready.files.state != Access::Denied && !ready.profile_empty)
        ),
    );
    if !ready.files.say.is_empty() {
        line(out, format!("                {}", ready.files.say));
    }
    let tabs = match ready.tabs.state {
        Access::Granted => "ready",
        Access::Denied => "needs attention",
        Access::Unknown => "asked the first time",
    };
    line(
        out,
        format!("{:<15} {tabs}", format!("{} tabs", ready.app_display)),
    );
    if !ready.tabs.say.is_empty() {
        line(out, format!("                {}", ready.tabs.say));
    }
    if let Some(p) = &ready.profile {
        let shown = ready
            .profiles
            .iter()
            .find(|x| &x.dir == p)
            .map(|x| match &x.account {
                Some(a) => format!("{} ({a})", x.name),
                None => x.name.clone(),
            })
            .unwrap_or_else(|| p.clone());
        line(out, format!("Profile         {shown}"));
    }
    if ready.profiles.len() > 1 {
        let all: Vec<String> = ready
            .profiles
            .iter()
            .map(|x| format!("{}{}", x.dir, if x.last_used { " (used last)" } else { "" }))
            .collect();
        line(out, format!("Profiles        {}", all.join(", ")));
        line(
            out,
            "Pick one with --profile; without it a push reads Default.",
        );
    }
    Ok(i32::from(!ok))
}

fn util_json(out: &mut dyn Write, v: serde_json::Value) {
    line(out, v.to_string());
}

fn kv_err(e: cua_keyvault::Error) -> CuaError {
    CuaError::PermissionDenied(e.to_string())
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
        E::Teleport(T::Provider(p))
            if p.contains("not authorized")
                || p.contains("Full Disk Access")
                || p.contains("Automation") =>
        {
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
        (None, Some(_)) if args.read_as_caller => {
            eprintln!("{READ_AS_CALLER_WARNING}");
            push_direct(cua, host, args, json, out).await
        }
        (None, Some(url)) => {
            let spaces = cua.spaces();
            let url = url.clone();
            let token = args.token.clone();
            with_destination(spaces.as_ref(), &url, token, |id| {
                let host = host.clone();
                let args = &args;
                async move { push_through_keyvault(&host, &id, args, json, out).await }
            })
            .await
        }
        (None, None) => Err(CuaError::InvalidArgument(
            "pass --sandbox NAME or --url URL".into(),
        )),
    }
}

/// What `--read-as-caller` costs, said once before it reads.
const READ_AS_CALLER_WARNING: &str = "warning: --read-as-caller reads the app in this process, \
not in the Cua daemon: Keyvault consent, the audit record and the wipe are skipped, and macOS \
uses this process's permissions. This flag is going away; use --url without it.";

/// Where the local daemon keeps Spaces: the part of `with_destination` that
/// registers and forgets one, so the order of events can be tested.
#[async_trait::async_trait]
pub trait DestinationRegistry: Send + Sync {
    /// The ids of the Spaces registered now.
    async fn ids(&self) -> Result<Vec<String>, CuaError>;
    /// Registers the destination at `url`; returns its Space id.
    async fn add(&self, url: &str, token: Option<String>, name: &str) -> Result<String, CuaError>;
    /// Forgets a Space (its machine is not touched).
    async fn remove(&self, id: &str) -> Result<(), CuaError>;
}

#[async_trait::async_trait]
impl DestinationRegistry for cua_sdk::Spaces {
    async fn ids(&self) -> Result<Vec<String>, CuaError> {
        Ok(self.list().await?.into_iter().map(|s| s.id).collect())
    }
    async fn add(&self, url: &str, token: Option<String>, name: &str) -> Result<String, CuaError> {
        Ok(self
            .add(url.to_string(), token, Some(name.to_string()))
            .await?
            .id)
    }
    async fn remove(&self, id: &str) -> Result<(), CuaError> {
        self.remove(id.to_string()).await
    }
}

/// The Space id a `--url` destination has once registered: the address
/// (`host:port`) without scheme or path, as `direct:<authority>`; an id given
/// as is stays as it is.
pub fn destination_id(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("space://")
        || ["local:", "cloud:", "direct:", "relay:"]
            .iter()
            .any(|p| url.starts_with(p))
    {
        return url.to_string();
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or(rest);
    format!("direct:{authority}")
}

/// Runs `push` against the destination `url` as a Space of the local daemon,
/// so the read, the consent, the audit record and the wipe are the daemon's,
/// as for `--sandbox`. A destination already registered is used as it is (its
/// own token, and it stays). Otherwise it is registered for this push and
/// forgotten after it, whether it worked or not; a destination that does not
/// answer fails here, before anything is read or asked.
pub async fn with_destination<R, F, Fut>(
    registry: &R,
    url: &str,
    token: Option<String>,
    push: F,
) -> Result<i32, CuaError>
where
    R: DestinationRegistry + ?Sized,
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<i32, CuaError>>,
{
    let wanted = destination_id(url);
    if registry.ids().await?.contains(&wanted) {
        return push(wanted).await;
    }
    let name = format!("teleport-destination-{}", std::process::id());
    let id = registry.add(url, token, &name).await.map_err(|e| match e {
        CuaError::PermissionDenied(_) | CuaError::InvalidArgument(_) => e,
        other => CuaError::Unsupported(format!(
            "the destination {url} did not answer as a Cua Space, so nothing was read: {other}"
        )),
    })?;
    let result = push(id.clone()).await;
    let _ = registry.remove(&id).await;
    result
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
    let mut client = cua_keyvault::client::KeyvaultClient::connect_default()
        .await
        .map_err(|e| {
            CuaError::Unsupported(format!(
                "teleport push goes through the Cua Keyvault, and it is not reachable ({e}); \
                 install or open Cua, or run this as the signed `cua` app/CLI"
            ))
        })?;
    // Before anything is read or any prompt shown: can this work at all?
    preflight(&mut client, &args.app.app, args.app.profile.as_deref()).await?;
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
        profile: args.app.profile.clone(),
        sites: vec![],
        whole_app: true,
        cookies: cua_keyvault::broker::CookieFilter::default(),
        confirm_passwords: true,
        paths: Some(paths.clone()),
        domains: None,
        passwords: false,
        password_domains: None,
    };
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

/// `cua teleport push --url URL --read-as-caller`: the old direct path, kept
/// only behind that flag: this process reads the app and uploads to an
/// arbitrary spacesd endpoint, skipping the Keyvault.
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
            "--read-as-caller",
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

    // --- `--url` is a destination; the daemon reads --------------------

    #[derive(Default)]
    struct FakeRegistry {
        existing: Vec<String>,
        fail_add: bool,
        events: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl DestinationRegistry for FakeRegistry {
        async fn ids(&self) -> Result<Vec<String>, CuaError> {
            Ok(self.existing.clone())
        }
        async fn add(
            &self,
            url: &str,
            token: Option<String>,
            _name: &str,
        ) -> Result<String, CuaError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("add {url} token={}", token.is_some()));
            if self.fail_add {
                return Err(CuaError::Internal("connection refused".into()));
            }
            Ok(destination_id(url))
        }
        async fn remove(&self, id: &str) -> Result<(), CuaError> {
            self.events.lock().unwrap().push(format!("remove {id}"));
            Ok(())
        }
    }

    #[test]
    fn a_url_destination_has_the_id_the_registry_gives_it() {
        assert_eq!(
            destination_id("http://10.0.0.5:3211"),
            "direct:10.0.0.5:3211"
        );
        assert_eq!(destination_id("10.0.0.5:3211/x"), "direct:10.0.0.5:3211");
        assert_eq!(
            destination_id("https://box.example:443"),
            "direct:box.example:443"
        );
        assert_eq!(destination_id("relay:abc"), "relay:abc");
        assert_eq!(destination_id("direct:h:1"), "direct:h:1");
    }

    #[tokio::test]
    async fn a_new_destination_is_registered_for_the_push_and_forgotten_after_it() {
        let reg = FakeRegistry::default();
        let seen = std::sync::Mutex::new(None);
        let code = with_destination(&reg, "http://10.0.0.5:3211", Some("t".into()), |id| {
            *seen.lock().unwrap() = Some(id);
            async { Ok(0) }
        })
        .await
        .unwrap();
        assert_eq!(code, 0);
        // The daemon's push targets the registered Space id, not the URL.
        assert_eq!(
            seen.lock().unwrap().as_deref(),
            Some("direct:10.0.0.5:3211")
        );
        assert_eq!(
            *reg.events.lock().unwrap(),
            [
                "add http://10.0.0.5:3211 token=true",
                "remove direct:10.0.0.5:3211"
            ]
        );
    }

    #[tokio::test]
    async fn a_refused_push_still_forgets_the_destination_and_keeps_its_exact_refusal() {
        let reg = FakeRegistry::default();
        let err = with_destination(&reg, "10.0.0.5:3211", None, |_| async {
            Err::<i32, _>(CuaError::PermissionDenied("chrome: declined".into()))
        })
        .await
        .unwrap_err();
        assert!(
            matches!(&err, CuaError::PermissionDenied(m) if m == "chrome: declined"),
            "{err:?}"
        );
        assert_eq!(
            reg.events.lock().unwrap().last().unwrap(),
            "remove direct:10.0.0.5:3211"
        );
    }

    #[tokio::test]
    async fn a_destination_that_is_already_registered_is_used_and_kept() {
        let reg = FakeRegistry {
            existing: vec!["direct:10.0.0.5:3211".into()],
            ..Default::default()
        };
        let seen = std::sync::Mutex::new(None);
        with_destination(&reg, "http://10.0.0.5:3211", Some("other".into()), |id| {
            *seen.lock().unwrap() = Some(id);
            async { Ok(0) }
        })
        .await
        .unwrap();
        assert_eq!(
            seen.lock().unwrap().as_deref(),
            Some("direct:10.0.0.5:3211")
        );
        assert!(reg.events.lock().unwrap().is_empty(), "no add, no remove");
    }

    #[tokio::test]
    async fn a_destination_that_does_not_answer_fails_before_anything_is_read() {
        let reg = FakeRegistry {
            fail_add: true,
            ..Default::default()
        };
        let read = std::sync::atomic::AtomicBool::new(false);
        let err = with_destination(&reg, "10.0.0.5:3211", None, |_| {
            read.store(true, std::sync::atomic::Ordering::SeqCst);
            async { Ok(0) }
        })
        .await
        .unwrap_err();
        assert!(
            !read.load(std::sync::atomic::Ordering::SeqCst),
            "nothing was read or asked"
        );
        assert!(err.to_string().contains("nothing was read"), "{err}");
        // Nothing was registered, so nothing is removed.
        assert!(
            reg.events
                .lock()
                .unwrap()
                .iter()
                .all(|e| !e.starts_with("remove"))
        );
    }

    #[test]
    fn read_as_caller_is_explicit_needs_a_url_and_is_not_the_default() {
        let TeleportCmd::Push(a) = parse(&["teleport", "push", "--app", "chrome", "--url", "x:1"])
        else {
            panic!()
        };
        assert!(!a.read_as_caller);
        let TeleportCmd::Push(a) = parse(&[
            "teleport",
            "push",
            "--app",
            "chrome",
            "--url",
            "x:1",
            "--read-as-caller",
        ]) else {
            panic!()
        };
        assert!(a.read_as_caller);
        // The warning names what is skipped.
        for word in ["consent", "audit", "wipe", "going away"] {
            assert!(READ_AS_CALLER_WARNING.contains(word), "{word}");
        }
        assert!(!READ_AS_CALLER_WARNING.contains('\u{b7}'));
    }

    #[test]
    fn a_profile_is_accepted_with_either_destination() {
        for dest in [["--sandbox", "dev"], ["--url", "10.0.0.5:3211"]] {
            let TeleportCmd::Push(a) = parse(&[
                "teleport",
                "push",
                "--app",
                "chrome",
                dest[0],
                dest[1],
                "--profile",
                "Profile 1",
            ]) else {
                panic!()
            };
            assert_eq!(a.app.profile.as_deref(), Some("Profile 1"));
        }
    }
}
