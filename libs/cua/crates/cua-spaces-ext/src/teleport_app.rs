// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Teleport an app…" into a Space: plan and run.
//!
//! The UX core (host app catalog, classification, plans, consent items,
//! drag payloads, window drags) is `cua_teleport::ux`, shared by every
//! client. This module is the part that needs a Space:
//!
//! - [`Space::app_teleport_facts`]: the Space's OS, CPU, home and importers;
//! - [`Space::plan_app_teleport`]: a [`TeleportPlan`] for one catalog entry;
//! - [`Space::run_app_teleport`]: an [`ApprovedPlan`], step by step, with
//!   [`RunEvent`]s: install (pinned, verified, through `cua-agents`), send
//!   files (`send_file`, SHA-256 per file), import state (the provider's
//!   consented items, through [`Space::teleport`]), launch.

use std::sync::Arc;
use std::time::Duration;

use cua_keyvault::broker::TeleportStage;
use cua_spacesd_client::{Command, pb};
pub use cua_teleport::ux::{
    ApprovedPlan, CatalogEntry, Consent, PlanOptions, PlanStep, RunEvent, RunPhase, RunReport,
    SpaceFacts, TeleportPlan, UxError,
};
use cua_teleport::ux::{InstallSource, MoveKind};

use crate::teleport::{
    AppSessions, ImportOptions, SpaceTeleport as _, TeleportScope, TeleportSelection,
};
use cua_spaces::Space;
use cua_spaces::error::{Error, Result};

/// A teleport UX error as a Spaces error.
pub fn ux_err(e: UxError) -> Error {
    {
        match e {
            UxError::Invalid(m) => Error::InvalidArgument(m),
            UxError::NotApproved(m) | UxError::PermissionDenied(m) => Error::TeleportRefused(m),
            UxError::Unsupported(m) => Error::HostCapabilityMissing {
                what: "teleport".into(),
                why: m,
            },
            UxError::HostEffectsRefused(m) => Error::HostCapabilityMissing {
                what: "host effects".into(),
                why: m,
            },
            UxError::Io(m) => Error::Io(std::io::Error::other(m)),
        }
    }
}

/// How long a launch is watched for an immediate failure.
const LAUNCH_GRACE: Duration = Duration::from_secs(4);

/// `uname -m` spellings to the install manifest's CPU families.
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim() {
        "arm64" | "aarch64" => "aarch64".into(),
        "x86_64" | "amd64" | "x64" => "x86_64".into(),
        other => other.to_string(),
    }
}

/// App teleport on a [`Space`]: planning and running the move of a host app
/// (and its files or state) into the Space.
#[allow(async_fn_in_trait)]
pub trait SpaceAppTeleport {
    /// What the Space is, for [`cua_teleport::ux::plan::build`] and the
    /// catalog's [`cua_teleport::ux::TargetHint`].
    async fn app_teleport_facts(&self) -> Result<SpaceFacts>;

    /// The plan for teleporting `entry` into this Space. Reads the chosen
    /// files' sizes and, for state, the provider's manifest (never the
    /// items themselves).
    async fn plan_app_teleport(
        &self,
        sessions: Arc<AppSessions>,
        entry: &CatalogEntry,
        options: &PlanOptions,
    ) -> Result<TeleportPlan>;

    /// Runs an approved plan. `progress` sees every step start, progress,
    /// finish or failure, then `done`. The first failing step stops the run.
    async fn run_app_teleport(
        &self,
        sessions: Arc<AppSessions>,
        approved: &ApprovedPlan,
        progress: impl FnMut(RunEvent) + Send,
    ) -> Result<RunReport>;

    async fn run_step(
        &self,
        sessions: &Arc<AppSessions>,
        plan: &TeleportPlan,
        step: &PlanStep,
        home: &str,
        report: &mut RunReport,
        progress: &mut (dyn FnMut(RunPhase, String, u64, u64) + Send),
    ) -> Result<()>;
}

/// Whether a teleport's secrets would cross `cua-relay` in the clear (S1):
/// a relay-routed Space that reports no sealed-delivery key, the same check
/// the real send makes right before sending, so a plan's "send anyway?"
/// warning matches what actually happens.
async fn relay_plaintext_risk(space: &Space) -> bool {
    let Ok(client) = space.spacesd() else {
        return false;
    };
    if !matches!(
        client.endpoint().kind(),
        cua_spacesd_client::EndpointKind::Relay { .. }
    ) {
        return false;
    }
    !matches!(client.machine_seal_public_key().await, Ok(Some(_)))
}

impl SpaceAppTeleport for Space {
    /// What the Space is, for [`cua_teleport::ux::plan::build`] and the
    /// catalog's [`cua_teleport::ux::TargetHint`].
    async fn app_teleport_facts(&self) -> Result<SpaceFacts> {
        let cua_teleport::ux::TargetHint { os, arch } = self.app_teleport_hint().await?;
        let (os, arch) = (
            os.unwrap_or(cua_teleport::Platform::Linux),
            arch.unwrap_or_default(),
        );
        let importers = self
            .capabilities()
            .features
            .iter()
            .filter(|f| f.supported)
            .filter_map(|f| f.name.strip_prefix("teleport.").map(str::to_string))
            // `teleport.wipe` advertises WipeImport, not an importer.
            .filter(|name| name != "wipe")
            .collect();
        Ok(SpaceFacts {
            space_id: self.id().to_string(),
            os,
            arch,
            home: self.home().await?,
            importers,
        })
    }

    /// The plan for teleporting `entry` into this Space. Reads the chosen
    /// files' sizes and, for state, the provider's manifest (never the
    /// items themselves).
    async fn plan_app_teleport(
        &self,
        sessions: Arc<AppSessions>,
        entry: &CatalogEntry,
        options: &PlanOptions,
    ) -> Result<TeleportPlan> {
        let facts = self.app_teleport_facts().await?;
        let files = if options.files.is_empty() {
            vec![]
        } else {
            let paths = options.files.clone();
            tokio::task::spawn_blocking(move || cua_teleport::ux::plan::stat_paths(&paths))
                .await
                .map_err(|e| Error::Transfer(e.to_string()))?
                .map_err(ux_err)?
        };
        let manifest = match (options.moves, &entry.provider_id) {
            (MoveKind::AppWithState, Some(provider)) => {
                let provider = provider.clone();
                let scope = options.scope;
                Some(
                    tokio::task::spawn_blocking(move || {
                        sessions.transfer_manifest(&provider, scope)
                    })
                    .await
                    .map_err(|e| Error::Transfer(e.to_string()))??,
                )
            }
            _ => None,
        };
        let mut plan =
            cua_teleport::ux::plan::build(entry, &facts, options, manifest.as_ref(), &files)
                .map_err(ux_err)?;
        plan.relay_unsealed = relay_plaintext_risk(self).await;
        Ok(plan)
    }

    /// Runs an approved plan. `progress` sees every step start, progress,
    /// finish or failure, then `done`. The first failing step stops the run.
    async fn run_app_teleport(
        &self,
        sessions: Arc<AppSessions>,
        approved: &ApprovedPlan,
        mut progress: impl FnMut(RunEvent) + Send,
    ) -> Result<RunReport> {
        let plan = approved.plan();
        if plan.space_id != self.id().to_string() {
            return Err(Error::TeleportRefused(format!(
                "this plan is for {}, not {}",
                plan.space_id,
                self.id()
            )));
        }
        let steps = plan.steps.len() as u32;
        let mut report = RunReport {
            app_id: plan.app.id.clone(),
            ..Default::default()
        };
        let home = self.home().await?;
        for (i, step) in plan.steps.iter().enumerate() {
            let i = i as u32;
            let kind = step.name().to_string();
            let ev = |phase, detail: String, done: u64, total: u64| RunEvent {
                step: i,
                steps,
                kind: kind.clone(),
                phase,
                detail,
                done_bytes: done,
                total_bytes: total,
            };
            progress(ev(RunPhase::Started, describe(step), 0, 0));
            let result = self
                .run_step(
                    &sessions,
                    plan,
                    step,
                    &home,
                    &mut report,
                    &mut |p, d, a, b| progress(ev(p, d, a, b)),
                )
                .await;
            match result {
                Ok(()) => progress(ev(RunPhase::Finished, String::new(), 0, 0)),
                Err(e) => {
                    progress(ev(RunPhase::Failed, e.to_string(), 0, 0));
                    return Err(e);
                }
            }
        }
        progress(RunEvent {
            step: steps,
            steps,
            kind: "done".into(),
            phase: RunPhase::Done,
            detail: plan.app.name.clone(),
            done_bytes: 0,
            total_bytes: 0,
        });
        Ok(report)
    }

    async fn run_step(
        &self,
        sessions: &Arc<AppSessions>,
        plan: &TeleportPlan,
        step: &PlanStep,
        home: &str,
        report: &mut RunReport,
        progress: &mut (dyn FnMut(RunPhase, String, u64, u64) + Send),
    ) -> Result<()> {
        match step {
            PlanStep::Install { ids } => {
                self.install_for_teleport(ids, home, progress).await?;
                report.installed.extend(ids.iter().cloned());
                Ok(())
            }
            PlanStep::SendFiles { paths, subdir } => {
                let total: u64 = plan
                    .consent
                    .iter()
                    .filter(|c| paths.contains(&c.key))
                    .map(|c| c.bytes)
                    .sum();
                let mut done = 0u64;
                for p in paths {
                    progress(RunPhase::Progress, p.clone(), done, total);
                    let r = self
                        .send_file(
                            std::path::Path::new(p),
                            cua_spaces::files::SendFileOptions {
                                subdir: subdir.clone(),
                                ..Default::default()
                            },
                        )
                        .await?;
                    done += plan
                        .consent
                        .iter()
                        .find(|c| &c.key == p)
                        .map(|c| c.bytes)
                        .unwrap_or(0);
                    progress(RunPhase::Progress, r.dest.clone(), done, total);
                    report.sent.push(r.dest);
                }
                Ok(())
            }
            PlanStep::ImportState {
                provider_id,
                scope,
                items,
            } => {
                let scope = match scope {
                    cua_teleport::TransferScope::TabsOnly => TeleportScope::Tabs,
                    cua_teleport::TransferScope::FullProfile => TeleportScope::Full,
                };
                let s2 = sessions.clone();
                let pid = provider_id.clone();
                let manifest = tokio::task::spawn_blocking(move || s2.manifest(&pid, scope))
                    .await
                    .map_err(|e| Error::Transfer(e.to_string()))??;
                // The plan's consent already listed and acknowledged these
                // exact items.
                let approval = manifest.approving(&self.id().to_string(), items, plan.sensitive)?;
                // Saved Keyvault items are sent without reading the live app,
                // so the host's Keychain is never asked.
                let cookies = plan.from_vault.is_none()
                    && items.iter().any(|i| i.to_lowercase().contains("cookies"));
                let selection = TeleportSelection {
                    cookie_domains: plan.cookie_domains.clone(),
                    from_vault: plan.from_vault.clone(),
                    include_passwords: plan.include_passwords,
                };
                let app = plan.app.name.clone();
                let receipt = self
                    .teleport_selected(
                        sessions.clone(),
                        &approval,
                        ImportOptions {
                            replace_existing: false,
                            close_running_app: true,
                            launch_after: true,
                            save_to_keyvault: plan.save_to_keyvault,
                            // This plan only runs as an `ApprovedPlan`, so a
                            // `relay_unsealed` plan already carries the
                            // user's acknowledgement (S1).
                            relay_plaintext_ack: plan.relay_unsealed,
                        },
                        &selection,
                        &mut |st| {
                            let (done, total) = match &st {
                                TeleportStage::Uploading { done, total } => (*done, *total),
                                _ => (0, 0),
                            };
                            progress(
                                RunPhase::Progress,
                                stage_text(&st, &app, cookies, cfg!(target_os = "macos")),
                                done,
                                total,
                            );
                        },
                    )
                    .await?;
                report.imported = receipt.imported;
                report.skipped = receipt.skipped;
                report.launched |= receipt.launched;
                Ok(())
            }
            PlanStep::Launch {
                bin,
                args,
                files: _,
                terminal,
            } => {
                let program = match &plan.app.install {
                    Some(InstallSource::Manifest { .. }) => format!(
                        "{}/{}/{bin}",
                        home.trim_end_matches('/'),
                        cua_agents::installables::BIN_DIR
                    ),
                    _ => bin.clone(),
                };
                let args: Vec<String> = args.iter().map(|a| a.replace("$HOME", home)).collect();
                let tag = format!("cua-teleport-{}", plan.app.id);
                let line = std::iter::once(program.clone())
                    .chain(args.iter().cloned())
                    .map(|a| cua_agents::quote(&a))
                    .collect::<Vec<_>>()
                    .join(" ");
                // The app's output goes to a log in the Space, so a failed
                // start can say why.
                let log = format!("$HOME/.cua/logs/{tag}.log");
                let exec = if *terminal {
                    format!(
                        "for t in x-terminal-emulator xfce4-terminal gnome-terminal xterm; do \
                         command -v \"$t\" >/dev/null 2>&1 && exec \"$t\" -e {}; done; exit 127",
                        cua_agents::quote(&line)
                    )
                } else {
                    format!("exec {line}")
                };
                let cmd = Command::shell(format!(
                    "mkdir -p \"$HOME/.cua/logs\"; exec >>\"{log}\" 2>&1; {exec}"
                ))
                .cwd(home.to_string())
                .tag(tag.clone());
                let mut handle = self.spacesd()?.spawn(cmd).await?;
                // A start that fails at once (a missing library, no display)
                // is an error; a running or cleanly handed-off app is not.
                let deadline = tokio::time::Instant::now() + LAUNCH_GRACE;
                let mut exited = None;
                for _ in 0..10_000 {
                    match tokio::time::timeout_at(deadline, handle.next_event()).await {
                        Err(_) => break,
                        Ok(Ok(Some(cua_spacesd_client::ProcessEvent::Exit(s)))) => {
                            exited = Some(s);
                            break;
                        }
                        Ok(Ok(Some(_))) => {}
                        Ok(Ok(None)) | Ok(Err(_)) => break,
                    }
                }
                match exited {
                    Some(s) if !s.success() => {
                        let tail = self
                            .bash(&format!("tail -n 20 \"{log}\""), Duration::from_secs(20))
                            .await
                            .map(|o| o.stdout)
                            .unwrap_or_default();
                        return Err(Error::Agent(format!(
                            "{program} did not start (exit {:?}): {}",
                            s.code,
                            tail.trim()
                        )));
                    }
                    Some(_) => {}
                    None => handle.detach(),
                }
                progress(RunPhase::Progress, format!("started {program}"), 0, 0);
                report.launched = true;
                Ok(())
            }
        }
    }
}

/// App teleport on a [`Space`]: the install step and what the Space is
/// (its OS and architecture, for the catalog and plans).
#[allow(async_fn_in_trait)]
pub trait SpaceAppTeleportFacts {
    /// The install step: the app's OS packages resolve in the Space while
    /// its archive comes from the host cache (local Spaces, see
    /// [`cua_spaces::install_cache`]); then the pinned install runs and finds
    /// both in place. Progress carries bytes while the archive moves.
    async fn install_for_teleport(
        &self,
        ids: &[String],
        home: &str,
        progress: &mut (dyn FnMut(RunPhase, String, u64, u64) + Send),
    ) -> Result<()>;

    /// What the catalog classifies against: the Space's OS and CPU only,
    /// from its capabilities (no guest round trip, unless an older spacesd
    /// does not report its CPU). [`Self::app_teleport_facts`] also reads the
    /// home folder, which only a plan needs.
    async fn app_teleport_hint(&self) -> Result<cua_teleport::ux::TargetHint>;

    /// The Space's CPU family (`aarch64`, `x86_64`).
    async fn app_teleport_facts_arch(&self) -> Result<String>;
}

impl SpaceAppTeleportFacts for Space {
    /// The install step: the app's OS packages resolve in the Space while
    /// its archive comes from the host cache (local Spaces, see
    /// [`cua_spaces::install_cache`]); then the pinned install runs and finds
    /// both in place. Progress carries bytes while the archive moves.
    async fn install_for_teleport(
        &self,
        ids: &[String],
        home: &str,
        progress: &mut (dyn FnMut(RunPhase, String, u64, u64) + Send),
    ) -> Result<()> {
        use cua_spaces::install_cache::StageEvent;
        let agents = self.agents().await?;
        let ids_ref: Vec<&str> = ids.iter().map(String::as_str).collect();
        let stage = cua_spaces::install_cache::wanted(self.spacesd()?.endpoint().base_url());
        let arch = if stage {
            Some(self.app_teleport_facts_arch().await?)
        } else {
            None
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StageEvent>();
        let line = |tx: &tokio::sync::mpsc::UnboundedSender<StageEvent>| {
            let tx = tx.clone();
            move |p: &cua_agents::installables::Progress| {
                let _ = tx.send(StageEvent {
                    detail: format!("{} {} {}", p.id, p.phase, p.detail)
                        .trim()
                        .to_string(),
                    done: 0,
                    total: 0,
                });
            }
        };
        let work = {
            let tx = tx.clone();
            let agents = &agents;
            let ids_ref = &ids_ref;
            async move {
                if let Some(arch) = arch {
                    let staging = self.stage_install_archives(ids_ref, &arch, home, tx.clone());
                    let deps = agents.ensure_os_packages(ids_ref, line(&tx));
                    let (staged, deps) = tokio::join!(staging, deps);
                    if let Err(e) = staged {
                        let _ = tx.send(StageEvent {
                            detail: format!("host cache skipped ({e}); the Space downloads"),
                            done: 0,
                            total: 0,
                        });
                    }
                    deps?;
                }
                agents.ensure(ids_ref, line(&tx)).await
            }
        };
        drop(tx);
        tokio::pin!(work);
        // Bounded: the install future ends.
        let result = loop {
            tokio::select! {
                r = &mut work => break r,
                Some(e) = rx.recv() => progress(RunPhase::Progress, e.detail, e.done, e.total),
            }
        };
        while let Ok(e) = rx.try_recv() {
            progress(RunPhase::Progress, e.detail, e.done, e.total);
        }
        result?;
        Ok(())
    }

    /// What the catalog classifies against: the Space's OS and CPU only,
    /// from its capabilities (no guest round trip, unless an older spacesd
    /// does not report its CPU). [`Self::app_teleport_facts`] also reads the
    /// home folder, which only a plan needs.
    async fn app_teleport_hint(&self) -> Result<cua_teleport::ux::TargetHint> {
        let os = match self.os_family() {
            pb::OsFamily::Macos => cua_teleport::Platform::MacOS,
            pb::OsFamily::Windows => cua_teleport::Platform::Windows,
            _ => cua_teleport::Platform::Linux,
        };
        let arch = match self.capabilities().arch() {
            pb::Architecture::Unspecified if os == cua_teleport::Platform::Windows => {
                "x86_64".to_string()
            }
            _ => self.app_teleport_facts_arch().await?,
        };
        Ok(cua_teleport::ux::TargetHint {
            os: Some(os),
            arch: Some(arch),
        })
    }

    /// The Space's CPU family (`aarch64`, `x86_64`).
    async fn app_teleport_facts_arch(&self) -> Result<String> {
        match self.capabilities().arch() {
            pb::Architecture::Arm64 => Ok("aarch64".into()),
            pb::Architecture::X8664 => Ok("x86_64".into()),
            _ => {
                let out = self.bash("uname -m", Duration::from_secs(20)).await?;
                if !out.success() {
                    return Err(Error::Agent(format!("uname -m failed: {}", out.render())));
                }
                Ok(normalize_arch(&out.stdout))
            }
        }
    }
}

/// A step as it starts, in words the app shows under its progress bar.
fn describe(step: &PlanStep) -> String {
    match step {
        PlanStep::Install { ids } => format!("Installing {}", ids.join(", ")),
        PlanStep::SendFiles { paths, .. } => match paths.len() {
            1 => "Sending 1 item".into(),
            n => format!("Sending {n} items"),
        },
        PlanStep::ImportState { .. } => "Preparing the sign-in".into(),
        PlanStep::Launch { bin, .. } => format!(
            "Opening {}",
            std::path::Path::new(bin)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(bin)
        ),
    }
}

/// The Keyvault's stage of a signed-in state move, in words. Reading says
/// why macOS is about to ask for the Keychain (the app's cookies are
/// encrypted with a key kept there). Uploading leaves the bytes to the
/// event (the app appends "12 / 80 MB").
fn stage_text(stage: &TeleportStage, app: &str, cookies: bool, macos: bool) -> String {
    match stage {
        TeleportStage::Reading => {
            let what = if cookies { "cookies" } else { "sign-in" };
            if macos && cookies {
                format!("Reading {app} {what} (macOS will ask for Keychain access)\u{2026}")
            } else {
                format!("Reading {app} {what}\u{2026}")
            }
        }
        TeleportStage::Saving => "Saving to Keyvault".into(),
        TeleportStage::Packing => "Packing profile".into(),
        TeleportStage::Uploading { .. } => "Uploading".into(),
        TeleportStage::Importing => "Importing into the Space".into(),
    }
}

impl AppSessions {
    /// The provider's raw manifest (for [`cua_teleport::ux::plan::build`]).
    pub fn transfer_manifest(
        &self,
        app: &str,
        scope: cua_teleport::TransferScope,
    ) -> Result<cua_teleport::TransferManifest> {
        let s = match scope {
            cua_teleport::TransferScope::TabsOnly => TeleportScope::Tabs,
            cua_teleport::TransferScope::FullProfile => TeleportScope::Full,
        };
        let m = self.manifest(app, s)?;
        Ok(cua_teleport::TransferManifest {
            provider_id: m.app.clone(),
            app_display_name: m.display_name.clone(),
            scope,
            total_est_bytes: m.total_estimated_bytes,
            notes: m.notes.clone(),
            items: m
                .items
                .into_iter()
                .map(|i| cua_teleport::ManifestItem {
                    label: i.label,
                    rel_path: i.relative_path,
                    est_bytes: i.estimated_bytes,
                    count: i.count,
                    count_noun: i.count_noun,
                    sensitive: i.is_sensitive,
                    default_checked: i.is_checked_by_default,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stages_read_as_steps_and_name_the_keychain_prompt() {
        use cua_keyvault::broker::TeleportStage as S;
        assert_eq!(
            super::stage_text(&S::Reading, "Chrome", true, true),
            "Reading Chrome cookies (macOS will ask for Keychain access)\u{2026}"
        );
        assert_eq!(
            super::stage_text(&S::Reading, "Chrome", true, false),
            "Reading Chrome cookies\u{2026}"
        );
        assert_eq!(
            super::stage_text(&S::Reading, "Slack", false, true),
            "Reading Slack sign-in\u{2026}"
        );
        assert_eq!(
            super::stage_text(&S::Packing, "Chrome", true, true),
            "Packing profile"
        );
        assert_eq!(
            super::stage_text(&S::Uploading { done: 1, total: 2 }, "Chrome", true, true),
            "Uploading"
        );
        assert_eq!(
            super::stage_text(&S::Importing, "Chrome", true, true),
            "Importing into the Space"
        );
        assert_eq!(
            super::stage_text(&S::Saving, "Chrome", true, true),
            "Saving to Keyvault"
        );
        for st in [S::Reading, S::Saving, S::Packing, S::Importing] {
            assert!(!super::stage_text(&st, "Chrome", true, true).contains('\u{2014}'));
        }
    }

    use super::*;

    #[test]
    fn arches_normalize() {
        assert_eq!(normalize_arch("arm64\n"), "aarch64");
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn ux_errors_map_to_space_errors() {
        assert_eq!(
            ux_err(UxError::Invalid("x".into())).tag(),
            "invalid_argument"
        );
        assert_eq!(
            ux_err(UxError::NotApproved("x".into())).tag(),
            "teleport_refused"
        );
        assert_eq!(
            ux_err(UxError::Unsupported("x".into())).tag(),
            "host_capability_missing"
        );
    }
}
