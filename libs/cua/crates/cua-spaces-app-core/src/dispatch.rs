// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One JSON entry point over the whole core: `call(method, args)` with
//! `args` a JSON object of named arguments (camelCase), returning the
//! result as JSON. The webview's wasm shim (`apps/cua-spaces/core-wasm`)
//! and the Tauri `app_core_call` command use it; typed hosts (Rust, Swift)
//! call the functions directly.
//!
//! [`METHODS`] lists every method; `dispatch_covers_every_method` checks it.

use crate::keyvault::{approval, browse, vault, view as kv};
use crate::model::*;
use crate::spaces::{self, creating, roster, sidebar, stream};
use crate::teleport::{drag, flow, grid, transfer, windows};
use crate::{
    CoreError, about, agent_keys, agents, cloud_connect, devices, drive_mount_preview, drive_page,
    drive_settings, driver_preview, experiments, host, login_item, machines, notch, notifications,
    onboarding, onboarding_preview, paths, persistent, presence, settings, share, telemetry,
    window, wizard,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Every method [`call`] answers.
pub const METHODS: &[&str] = &[
    "spaces.rowToSpace",
    "spaces.rowsToSpaces",
    "spaces.displayName",
    "spaces.nameOf",
    "spaces.providerOfId",
    "spaces.normalizeProvider",
    "spaces.cloudNamespaceOf",
    "spaces.sceneForOs",
    "spaces.hasFeature",
    "spaces.sharesDesktop",
    "spaces.isSpacesPool",
    "spaces.sortByMru",
    "spaces.touchSpace",
    "spaces.countActive",
    "spaces.ambientDots",
    "spaces.statusLine",
    "roster.initial",
    "roster.reduce",
    "creates.reduce",
    "creates.compose",
    "creates.isPending",
    "creates.settle",
    "creates.isDeleting",
    "creates.isPowering",
    "sidebar.build",
    "sidebar.detail",
    "sidebar.detailCopy",
    "sidebar.deleteFailedText",
    "sidebar.powerButton",
    "sidebar.statusText",
    "sidebar.streamSection",
    "spaces.desktopCover",
    "stream.pipReduce",
    "stream.pipClick",
    "window.chrome",
    "window.menuBar",
    "window.menu",
    "spaces.openableCount",
    "wizard.initial",
    "wizard.reduce",
    "wizard.view",
    "wizard.createArgs",
    "wizard.pickerImages",
    "wizard.pickerGroups",
    "wizard.findImage",
    "wizard.canPlace",
    "wizard.runtimeOptions",
    "wizard.kindOptions",
    "wizard.localRuntimeReady",
    "wizard.looksLikeAddress",
    "wizard.friendlyAddError",
    "wizard.macosLimit",
    "wizard.imageSuggestions",
    "wizard.validateImageRef",
    "wizard.creatingText",
    "wizard.createFailedText",
    "windows.filterWindows",
    "windows.appsFromWindows",
    "windows.teleportButtonEnabled",
    "windows.pickerPrimary",
    "windows.filterRemoteWindows",
    "windows.groupWindowsByApp",
    "windows.isScreenTarget",
    "windows.screenLabel",
    "grid.tabs",
    "grid.apps",
    "grid.windows",
    "grid.remote",
    "grid.step",
    "grid.primary",
    "flow.initial",
    "flow.reduce",
    "flow.sections",
    "flow.visibleEntries",
    "flow.searchEntries",
    "flow.canPlan",
    "flow.canConfirm",
    "flow.consent",
    "flow.progress",
    "flow.status",
    "flow.formatBytes",
    "flow.defaultMove",
    "flow.review",
    "flow.sensitiveOptions",
    "flow.planSensitive",
    "flow.requiresCuaApp",
    "drag.initial",
    "drag.apply",
    "transfer.reduce",
    "transfer.title",
    "transfer.progress",
    "transfer.sizeLabel",
    "transfer.formatMegabytes",
    "transfer.dropSendingText",
    "transfer.dropSentText",
    "keyvault.liveGrant",
    "keyvault.liveRule",
    "keyvault.liveDelivery",
    "keyvault.duration",
    "keyvault.ago",
    "keyvault.shortCaller",
    "keyvault.signingBadge",
    "keyvault.recentDecisions",
    "keyvault.pendingSummary",
    "keyvault.pendingRows",
    "keyvault.accessRows",
    "keyvault.page",
    "keyvault.sidebar",
    "keyvault.list",
    "keyvault.vaultReduce",
    "keyvault.vaultView",
    "keyvault.vaultSource",
    "keyvault.signedInSpaces",
    "keyvault.spaceAccessKey",
    "keyvault.liveCopySpaces",
    "keyvault.unlockPrompt",
    "keyvault.deleteConfirm",
    "keyvault.labels",
    "keyvault.recoveryKeyText",
    "keyvault.credentialForm",
    "keyvault.passphraseCheck",
    "approval.open",
    "approval.reduce",
    "approval.view",
    "approval.approveCommand",
    "approval.denyCommand",
    "onboarding.initial",
    "onboarding.reduce",
    "onboarding.view",
    "onboarding.copy",
    "onboarding.signedInText",
    "onboarding.signInCodeText",
    "onboarding.replacesText",
    "onboarding.installedAtText",
    "onboarding.preview",
    "onboarding.previewFrame",
    "onboarding.previewStill",
    "onboarding.driverPreview",
    "onboarding.driverPreviewFrame",
    "onboarding.driverPreviewStill",
    "onboarding.drivePreview",
    "onboarding.drivePreviewFrame",
    "onboarding.drivePreviewStill",
    "settings.defaults",
    "settings.formatHotkey",
    "settings.page",
    "loginItem.launchPlan",
    "agentKeys.view",
    "agentKeys.form",
    "agentKeys.removeConfirm",
    "agentKeys.nameProblem",
    "about.view",
    "about.afterLaunch",
    "about.restartDaemon",
    "about.refreshNotice",
    "about.allowedChannels",
    "notch.layout",
    "notch.tiles",
    "notch.initial",
    "notch.reduce",
    "notch.view",
    "notch.motion",
    "notch.tab",
    "notch.filter",
    "notch.activity",
    "notch.progress",
    "notch.osIcon",
    "notch.osIconSvg",
    "notch.tileAt",
    "dragTrigger.initial",
    "dragTrigger.apply",
    "dragTrigger.classify",
    "dragTrigger.displays",
    "dragTrigger.portalDisplays",
    "host.summary",
    "host.thisMachineSpace",
    "host.looksLikeListen",
    "host.parseAllowList",
    "host.clientLabel",
    "presence.name",
    "presence.principalId",
    "host.withThisMachine",
    "host.summaryInput",
    "host.panel",
    "host.permissionRows",
    "host.formInitial",
    "host.formReduce",
    "host.formView",
    "host.settingChange",
    "agents.name",
    "agents.initial",
    "agents.subtitle",
    "agents.statusLabel",
    "agents.filter",
    "agents.order",
    "agents.settingsRows",
    "agents.setupSummary",
    "paths.displayPath",
    "paths.displayPaths",
    "devices.view",
    "machines.merge",
    "devices.activityText",
    "devices.labels",
    "devices.platformName",
    "devices.cleanName",
    "devices.normalizeCode",
    "devices.enrollInitial",
    "devices.enrollReduce",
    "devices.enrollView",
    "devices.approveOpen",
    "devices.approveReduce",
    "devices.approveView",
    "cloudConnect.initial",
    "cloudConnect.reduce",
    "cloudConnect.view",
    "cloudConnect.inputFromStatus",
    "cloudConnect.cloudsFromStatus",
    "cloudConnect.isCloudWord",
    "share.initial",
    "share.reduce",
    "share.view",
    "agents.pageInitial",
    "agents.pageReduce",
    "agents.pageView",
    "drive.initial",
    "drive.reduce",
    "drive.view",
    "storage.initial",
    "storage.reduce",
    "storage.section",
    "storage.press",
    "storage.choose",
    "storage.edit",
    "storage.requestText",
    "notifications.plan",
    "notifications.view",
    "telemetry.launched",
    "telemetry.feature",
    "telemetry.onboarding",
    "telemetry.onboardingFinished",
    "telemetry.onboardingSkipped",
    "telemetry.signInFailed",
    "telemetry.creates",
    "telemetry.storage",
    "telemetry.share",
    "telemetry.enroll",
    "telemetry.experimentsChanged",
    "telemetry.experimentsOn",
    "settings.withStorage",
    "experiments.page",
    "experiments.choose",
    "wizard.placementOptions",
];

struct Args(Value);

impl Args {
    fn get<T: DeserializeOwned>(&self, name: &str) -> Result<T, CoreError> {
        let v = self.0.get(name).cloned().unwrap_or(Value::Null);
        serde_json::from_value(v).map_err(|e| CoreError::Invalid(format!("argument {name}: {e}")))
    }
}

fn out<T: Serialize>(v: T) -> Result<Value, CoreError> {
    serde_json::to_value(v).map_err(|e| CoreError::Invalid(e.to_string()))
}

/// Calls `method` with `args` (a JSON object) and returns JSON.
pub fn call(method: &str, args: &str) -> Result<String, CoreError> {
    let v: Value = if args.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(args).map_err(|e| CoreError::Invalid(format!("args: {e}")))?
    };
    let r = call_value(method, v)?;
    serde_json::to_string(&r).map_err(|e| CoreError::Invalid(e.to_string()))
}

/// [`call`] over `serde_json::Value`s.
pub fn call_value(method: &str, args: Value) -> Result<Value, CoreError> {
    let a = Args(args);
    match method {
        "spaces.rowToSpace" => out(spaces::row_to_space(&a.get("row")?, a.get("now")?)),
        "spaces.rowsToSpaces" => out(spaces::rows_to_spaces(
            &a.get::<Vec<SpaceRow>>("rows")?,
            a.get("now")?,
        )),
        "spaces.displayName" => out(spaces::display_name(&a.get::<String>("name")?)),
        "spaces.nameOf" => out(spaces::name_of(
            &a.get::<String>("id")?,
            &a.get::<Option<String>>("name")?.unwrap_or_default(),
        )),
        "spaces.providerOfId" => out(spaces::provider_of_id(&a.get::<String>("id")?)),
        "spaces.normalizeProvider" => out(spaces::normalize_provider(&a.get::<String>("word")?)),
        "spaces.cloudNamespaceOf" => out(spaces::cloud_namespace_of(&a.get::<String>("id")?)),
        "spaces.sceneForOs" => out(spaces::scene_for_os(a.get("os")?)),
        "spaces.hasFeature" => out(spaces::has_feature(
            &a.get("space")?,
            &a.get::<String>("feature")?,
        )),
        // Whether one of your machines shares its desktop (null: not known
        // or not one of your machines): when it does not, nothing asks its
        // desktop or its processes.
        "spaces.sharesDesktop" => out(sidebar::shares_desktop(&a.get("space")?)),
        "spaces.isSpacesPool" => out(spaces::is_spaces_pool(&a.get("space")?)),
        "spaces.sortByMru" => out(spaces::sort_by_mru(&a.get::<Vec<Space>>("spaces")?)),
        "spaces.touchSpace" => out(spaces::touch_space(
            &a.get::<Vec<Space>>("spaces")?,
            &a.get::<String>("id")?,
            a.get("now")?,
        )),
        "spaces.countActive" => out(spaces::count_active(&a.get::<Vec<Space>>("spaces")?)),
        "spaces.ambientDots" => out(spaces::ambient_dots(&a.get::<Vec<Space>>("spaces")?)),
        "spaces.statusLine" => out(spaces::status_line(a.get("count")?)),
        "roster.initial" => out(roster::initial(&a.get::<Vec<Space>>("spaces")?)),
        "roster.reduce" => out(roster::reduce(&a.get("state")?, &a.get("action")?)),
        "creates.reduce" => out(creating::reduce(
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
            &a.get("action")?,
        )),
        "creates.compose" => out(creating::compose(
            &a.get::<Vec<Space>>("spaces")?,
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
        )),
        "creates.isPending" => out(creating::is_pending(&a.get::<String>("id")?)),
        "creates.settle" => out(creating::settle(
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
            &a.get::<Vec<Space>>("spaces")?,
        )),
        "creates.isDeleting" => out(creating::is_deleting(
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
            &a.get::<String>("id")?,
        )),
        "creates.isPowering" => out(creating::is_powering(
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
            &a.get::<String>("id")?,
        )),
        "sidebar.build" => out(sidebar::sidebar(
            &a.get::<Vec<Space>>("spaces")?,
            &a.get::<Option<String>>("query")?.unwrap_or_default(),
            &a.get::<Option<String>>("selectedId")?.unwrap_or_default(),
        )),
        // With `experiments`, what Settings, Experiments hides is left out;
        // with `access` (this device is signed in but not enrolled), the
        // detail as this device sees it (`detail_for`, as the SwiftUI app's
        // `AppModel.detail`).
        "sidebar.detail" => {
            let space = a.get("space")?;
            let usage = a.get::<Option<sidebar::SpaceUsage>>("usage")?;
            let host_arch = a.get::<Option<String>>("hostArch")?;
            let access = a.get::<Option<devices::MachineAccessNotice>>("access")?;
            out(
                match (
                    a.get::<Option<experiments::Experiments>>("experiments")?,
                    access,
                ) {
                    (None, None) => {
                        sidebar::detail_live(&space, usage.as_ref(), host_arch.as_deref())
                    }
                    (x, access) => sidebar::detail_for(
                        &space,
                        usage.as_ref(),
                        host_arch.as_deref(),
                        &x.unwrap_or_default(),
                        access.as_ref(),
                    ),
                },
            )
        }
        "sidebar.detailCopy" => out(sidebar::detail_copy()),
        "sidebar.deleteFailedText" => out(sidebar::delete_failed_text(
            &a.get::<String>("name")?,
            &a.get::<String>("error")?,
        )),
        "sidebar.powerButton" => out(sidebar::power_button(&a.get::<Space>("space")?)),
        // The status word a Space's row and detail show ("Suspended",
        // "Stopped", "Turning on…").
        "sidebar.statusText" => out(sidebar::status_text(&a.get::<Space>("space")?)),
        "sidebar.streamSection" => out(stream::stream_section(&a.get("input")?)),
        // What the preview shows over (or instead of) the live desktop.
        "spaces.desktopCover" => out(spaces::cover::desktop_cover(&a.get("input")?)),
        "stream.pipReduce" => out(stream::pip_reduce(
            &a.get::<Option<Vec<String>>>("open")?.unwrap_or_default(),
            &a.get("event")?,
        )),
        "stream.pipClick" => out(stream::pip_click(
            &a.get::<Option<Vec<String>>>("open")?.unwrap_or_default(),
            &a.get::<String>("row")?,
        )),
        "window.chrome" => out(window::chrome(
            &a.get::<Option<window::ChromeInput>>("input")?
                .unwrap_or_default(),
        )),
        "window.menu" => out(window::menu(&a.get::<window::MenuInput>("input")?)),
        "spaces.openableCount" => out(spaces::openable_count(&a.get::<Vec<Space>>("spaces")?)),
        "window.menuBar" => out(window::menu_bar_with_keyvault(
            a.get("spaces")?,
            a.get::<Option<String>>("keyvault")?.as_deref(),
        )),
        "wizard.initial" => out(wizard::initial(&a.get("env")?)),
        "wizard.reduce" => out(wizard::reduce(
            &a.get("state")?,
            &a.get("action")?,
            &a.get("env")?,
        )),
        // `hostOs`: the shell's system, for its words (macOS when absent).
        "wizard.view" => out(wizard::view_on(
            &a.get("state")?,
            &a.get("env")?,
            a.get::<Option<SpaceOs>>("hostOs")?
                .unwrap_or(SpaceOs::Macos),
        )),
        "wizard.createArgs" => out(wizard::create_args(&a.get("plan")?)),
        "wizard.placementOptions" => out(wizard::placement_options_on(
            &a.get("state")?,
            &a.get("env")?,
            a.get::<Option<SpaceOs>>("hostOs")?
                .unwrap_or(SpaceOs::Macos),
        )),
        "wizard.pickerImages" => out(wizard::picker_images()),
        "wizard.pickerGroups" => out(wizard::picker_groups()),
        "wizard.findImage" => out(wizard::find_image(&a.get::<String>("ref")?)),
        "wizard.canPlace" => out(wizard::can_place(&a.get("image")?, a.get("on")?)),
        "wizard.runtimeOptions" => out(wizard::runtime_options(&a.get("image")?, a.get("on")?)),
        "wizard.kindOptions" => out(wizard::kind_options(&a.get("image")?, a.get("on")?)),
        "wizard.localRuntimeReady" => out(wizard::local_runtime_ready(
            &a.get("image")?,
            &a.get::<Vec<String>>("backends")?,
        )),
        "wizard.looksLikeAddress" => out(wizard::looks_like_address(&a.get::<String>("value")?)),
        "wizard.friendlyAddError" => out(wizard::friendly_add_error(&a.get::<String>("raw")?)),
        "wizard.macosLimit" => out(wizard::macos_limit_text(
            a.get::<Option<u32>>("spacesBusy")?.unwrap_or(0),
            a.get("vmsRunning")?,
        )),
        "wizard.imageSuggestions" => out(wizard::image_suggestions(&a.get::<String>("query")?)),
        "wizard.validateImageRef" => out(wizard::validate_image_ref(&a.get::<String>("ref")?)),
        "wizard.creatingText" => out(wizard::creating_text(&a.get("plan")?)),
        "wizard.createFailedText" => out(wizard::create_failed_text_on(
            &a.get::<String>("error")?,
            a.get("provider")?,
            a.get::<Option<String>>("hostName")?.as_deref(),
        )),
        "windows.filterWindows" => out(windows::filter_windows(
            &a.get::<Vec<_>>("windows")?,
            &a.get::<String>("query")?,
        )),
        "windows.appsFromWindows" => out(windows::apps_from_windows(&a.get::<Vec<_>>("windows")?)),
        "windows.teleportButtonEnabled" => out(windows::teleport_button_enabled(
            a.get::<Option<windows::OpenWindow>>("selected")?.as_ref(),
        )),
        "windows.pickerPrimary" => out(windows::picker_primary(
            a.get("tab")?,
            a.get("selectedSupported")?,
        )),
        "windows.filterRemoteWindows" => out(windows::filter_remote_windows(
            &a.get::<Vec<_>>("windows")?,
            &a.get::<String>("query")?,
        )),
        "windows.groupWindowsByApp" => {
            out(windows::group_windows_by_app(&a.get::<Vec<_>>("windows")?))
        }
        "windows.isScreenTarget" => out(windows::is_screen_target(&a.get("window")?)),
        "windows.screenLabel" => out(windows::screen_label(
            a.get::<Option<windows::RemoteWindow>>("screen")?.as_ref(),
        )),
        "grid.tabs" => out(grid::grid_tabs(&a.get::<String>("spaceName")?)),
        "grid.apps" => out(grid::app_grid(
            &a.get("state")?,
            &a.get::<Option<Vec<_>>>("windows")?.unwrap_or_default(),
        )),
        "grid.windows" => out(grid::window_grid(
            &a.get::<Vec<_>>("windows")?,
            &a.get::<Option<String>>("query")?.unwrap_or_default(),
            a.get::<Option<String>>("selected")?.as_deref(),
        )),
        "grid.remote" => out(grid::remote_grid(
            &a.get::<Vec<_>>("windows")?,
            &a.get::<Option<String>>("query")?.unwrap_or_default(),
            a.get::<Option<String>>("selected")?.as_deref(),
        )),
        "grid.primary" => out(grid::grid_primary(
            a.get("tab")?,
            &a.get::<String>("spaceName")?,
            &a.get("grid")?,
        )),
        "grid.step" => out(grid::grid_step(
            &a.get("grid")?,
            a.get::<Option<String>>("selected")?.as_deref(),
            a.get::<i32>("delta")?,
        )),
        "flow.initial" => out(flow::initial(&a.get::<String>("spaceName")?)),
        "flow.reduce" => out(flow::reduce(&a.get("state")?, &a.get("event")?)),
        "flow.sections" => out(flow::sections(&a.get("state")?)),
        "flow.visibleEntries" => out(flow::visible_entries(&a.get("state")?)),
        "flow.searchEntries" => out(flow::search_entries(
            &a.get::<Vec<_>>("entries")?,
            &a.get::<String>("query")?,
        )),
        "flow.canPlan" => out(flow::can_plan(&a.get("state")?)),
        "flow.canConfirm" => out(flow::can_confirm(&a.get("state")?)),
        "flow.consent" => out(flow::consent(&a.get("state")?)),
        "flow.progress" => out(flow::progress(&a.get("state")?)),
        "flow.status" => out(flow::status(&a.get("state")?)),
        "flow.formatBytes" => out(flow::format_bytes(a.get("n")?)),
        "flow.defaultMove" => out(flow::default_move(
            &a.get("entry")?,
            &a.get::<Option<Vec<String>>>("files")?.unwrap_or_default(),
        )),
        "flow.review" => out(flow::review(&a.get("state")?)),
        "flow.sensitiveOptions" => out(flow::sensitive_options(&a.get("state")?)),
        "flow.planSensitive" => out(flow::plan_sensitive(&a.get("state")?)),
        "flow.requiresCuaApp" => out(flow::requires_cua_app(
            &a.get::<Vec<String>>("texts")?,
            a.get::<Option<bool>>("installed")?.unwrap_or(false),
        )),
        "drag.initial" => out(drag::initial()),
        "drag.apply" => out(drag::apply(&a.get("state")?, &a.get("event")?)),
        "transfer.reduce" => out(transfer::reduce(
            a.get::<Option<transfer::TransferOverlayState>>("state")?
                .as_ref(),
            &a.get("signal")?,
        )),
        "transfer.title" => out(transfer::title(&a.get("state")?)),
        "transfer.progress" => out(transfer::progress(&a.get("state")?)),
        "transfer.sizeLabel" => out(transfer::size_label(&a.get("state")?)),
        "transfer.formatMegabytes" => out(transfer::format_megabytes(a.get("bytes")?)),
        "transfer.dropSendingText" => {
            out(transfer::drop_sending_text(&a.get::<Vec<String>>("paths")?))
        }
        "transfer.dropSentText" => out(transfer::drop_sent_text(&a.get::<Vec<_>>("files")?)),
        "keyvault.liveGrant" => out(kv::live_grant(&a.get("grant")?, a.get("now")?)),
        "keyvault.liveRule" => out(kv::live_rule(&a.get("rule")?, a.get("now")?)),
        "keyvault.liveDelivery" => out(kv::live_delivery(&a.get("delivery")?, a.get("now")?)),
        "keyvault.duration" => out(kv::duration(a.get("ms")?)),
        "keyvault.ago" => out(kv::ago(a.get("ms")?)),
        "keyvault.shortCaller" => out(kv::short_caller(&a.get::<String>("display")?)),
        "keyvault.signingBadge" => out(kv::signing_badge(&a.get("caller")?)),
        "keyvault.recentDecisions" => out(kv::recent_decisions(
            &a.get("overview")?,
            a.get::<Option<usize>>("limit")?.unwrap_or(12),
        )),
        "keyvault.pendingSummary" => out(kv::pending_summary(&a.get("pending")?)),
        "keyvault.pendingRows" => out(kv::pending_rows(&a.get("overview")?)),
        "keyvault.accessRows" => out(kv::access_rows(&a.get("overview")?, a.get("now")?)),
        "keyvault.page" => out(kv::page(&a.get("overview")?, a.get("now")?)),
        "keyvault.sidebar" => out(browse::sidebar(&a.get("overview")?, a.get("now")?)),
        "keyvault.list" => out(browse::list(
            &a.get("overview")?,
            &a.get("selection")?,
            a.get("now")?,
        )),
        "keyvault.vaultReduce" => out(vault::reduce(
            &a.get("overview")?,
            &a.get("state")?,
            &a.get("action")?,
        )),
        "keyvault.vaultView" => out(vault::view(
            &a.get("overview")?,
            &a.get("state")?,
            a.get("now")?,
        )),
        "keyvault.vaultSource" => out(vault::vault_source(
            &a.get("overview")?,
            &a.get::<String>("providerId")?,
        )),
        "keyvault.signedInSpaces" => out(kv::signed_in_spaces(
            &a.get("overview")?,
            a.get("now")?,
            &a.get::<Vec<String>>("dismissed")?,
            &a.get::<Vec<crate::model::Space>>("spaces")?,
        )),
        "keyvault.spaceAccessKey" => out(kv::space_access_key(
            &a.get("overview")?,
            a.get("now")?,
            &a.get::<crate::model::Space>("space")?,
        )),
        "keyvault.liveCopySpaces" => out(vault::live_copy_spaces(
            &a.get("overview")?,
            &a.get::<Vec<String>>("ids")?,
            a.get("now")?,
        )),
        "keyvault.unlockPrompt" => out(vault::unlock_prompt(
            &a.get("overview")?,
            a.get("count")?,
            a.get::<Option<String>>("name")?.as_deref(),
        )),
        "keyvault.deleteConfirm" => {
            out(vault::delete_confirm(a.get("count")?, a.get("liveCopies")?))
        }
        "keyvault.labels" => out(kv::labels()),
        "keyvault.recoveryKeyText" => out(kv::recovery_key_text(&a.get::<String>("key")?)),
        "keyvault.credentialForm" => out(crate::keyvault::credential::credential_form(
            &a.get("overview")?,
        )),
        "keyvault.passphraseCheck" => out(crate::keyvault::credential::passphrase_check(
            a.get("mode")?,
            &a.get::<String>("passphrase")?,
            &a.get::<String>("confirm")?,
        )),
        "approval.open" => out(approval::open(&a.get::<String>("requestId")?)),
        "approval.reduce" => out(approval::reduce(
            &a.get("overview")?,
            &a.get("state")?,
            &a.get("action")?,
        )),
        "approval.view" => out(approval::view(&a.get("overview")?, &a.get("state")?)),
        "approval.approveCommand" => out(approval::approve_command(
            &a.get("overview")?,
            &a.get("state")?,
        )),
        "approval.denyCommand" => out(approval::deny_command(&a.get("state")?)),
        "onboarding.initial" => out(onboarding::initial(
            a.get("installerMode")?,
            a.get("identity")?,
        )),
        "onboarding.reduce" => out(onboarding::reduce(&a.get("state")?, &a.get("action")?)),
        // `hostOs`: the shell's system, for its words (macOS when absent).
        "onboarding.view" => out(onboarding::view_on(
            &a.get("state")?,
            a.get::<Option<SpaceOs>>("hostOs")?
                .unwrap_or(SpaceOs::Macos),
        )),
        "onboarding.copy" => out(onboarding::copy()),
        "onboarding.signedInText" => out(onboarding::signed_in_text(
            a.get::<Option<String>>("identity")?.as_deref(),
        )),
        "onboarding.signInCodeText" => out(onboarding::sign_in_code_text(
            a.get::<Option<String>>("userCode")?.as_deref(),
        )),
        "onboarding.replacesText" => out(onboarding::replaces_text(
            a.get::<Option<String>>("installedVersion")?.as_deref(),
        )),
        "onboarding.installedAtText" => {
            out(onboarding::installed_at_text(&a.get::<String>("target")?))
        }
        "onboarding.preview" => out(onboarding_preview::preview(a.get("menuBar")?)),
        "onboarding.previewFrame" => {
            out(onboarding_preview::frame(a.get("menuBar")?, a.get("tMs")?))
        }
        "onboarding.previewStill" => out(onboarding_preview::still(a.get("menuBar")?)),
        "onboarding.driverPreview" => out(driver_preview::preview()),
        "onboarding.driverPreviewFrame" => out(driver_preview::frame(a.get("tMs")?)),
        "onboarding.driverPreviewStill" => out(driver_preview::still()),
        "onboarding.drivePreview" => out(drive_mount_preview::preview()),
        "onboarding.drivePreviewFrame" => out(drive_mount_preview::frame(a.get("tMs")?)),
        "onboarding.drivePreviewStill" => out(drive_mount_preview::still()),
        "settings.defaults" => out(settings::AppSettings::default()),
        // `hostOs`: the shell's system, for where the keys stay (macOS when absent).
        "agentKeys.view" => out(agent_keys::view_on(
            &a.get("input")?,
            a.get::<Option<SpaceOs>>("hostOs")?
                .unwrap_or(SpaceOs::Macos),
        )),
        "agentKeys.form" => out(agent_keys::form_on(
            &a.get("input")?,
            &a.get("form")?,
            a.get::<Option<SpaceOs>>("hostOs")?
                .unwrap_or(SpaceOs::Macos),
        )),
        "agentKeys.removeConfirm" => out(agent_keys::remove_confirm(
            &a.get("input")?,
            &a.get::<String>("env")?,
        )),
        "agentKeys.nameProblem" => out(agent_keys::name_problem(&a.get::<String>("name")?)),
        "about.view" => out(about::view(&a.get::<about::AboutInput>("input")?)),
        "about.afterLaunch" => out(about::after_launch(&a.get::<about::LaunchInput>("input")?)),
        "about.restartDaemon" => out(about::restart_daemon(
            &a.get::<about::DaemonCheck>("check")?,
        )),
        "about.refreshNotice" => out(about::refresh_notice(
            &a.get::<about::RefreshReport>("report")?,
        )),
        "about.allowedChannels" => out(about::allowed_channels(
            a.get::<about::UpdateChannel>("channel")?,
        )),
        "settings.formatHotkey" => out(settings::format_hotkey(&a.get("combo")?)),
        "loginItem.launchPlan" => out(login_item::launch_plan(
            a.get("choice")?,
            a.get::<Option<bool>>("onboarded")?.unwrap_or(false),
            a.get::<Option<bool>>("serves")?.unwrap_or(false),
            a.get::<Option<login_item::LoginItemStatus>>("status")?
                .unwrap_or_default(),
        )),
        "settings.page" => out(settings::page(
            &a.get::<Option<settings::SettingsInput>>("input")?
                .unwrap_or_default(),
        )),
        "settings.withStorage" => out(settings::with_storage(
            &a.get("page")?,
            &a.get("storage")?,
            &a.get::<Option<experiments::Experiments>>("experiments")?
                .unwrap_or_default(),
        )),
        "experiments.page" => out(experiments::page(
            &a.get::<Option<experiments::Experiments>>("experiments")?
                .unwrap_or_default(),
        )),
        "experiments.choose" => out(experiments::choose(
            &a.get::<Option<experiments::Experiments>>("experiments")?
                .unwrap_or_default(),
            &a.get::<String>("row")?,
            &a.get::<String>("option")?,
        )),
        "notch.layout" => out(notch::layout(
            &a.get("screen")?,
            a.get::<Option<bool>>("prompt")?.unwrap_or(false),
        )),
        "notch.tiles" => out(notch::tiles(
            &a.get::<Vec<Space>>("spaces")?,
            a.get::<Option<String>>("targeted")?.as_deref(),
        )),
        "notch.initial" => out(notch::NotchState::default()),
        "notch.reduce" => out(notch::reduce(&a.get("state")?, &a.get("event")?)),
        "notch.view" => out(notch::view(
            &a.get("state")?,
            &a.get::<Vec<Space>>("spaces")?,
        )),
        "notch.motion" => out(notch::MOTION),
        "notch.tab" => out(notch::tab(&a.get::<Vec<Space>>("spaces")?)),
        "notch.filter" => out(notch::filter(
            &a.get::<Vec<Space>>("spaces")?,
            &a.get::<Option<String>>("query")?.unwrap_or_default(),
        )),
        "notch.activity" => out(notch::activity(
            &a.get::<Vec<Space>>("spaces")?,
            a.get::<Option<bool>>("hotspot")?.unwrap_or(false),
            a.get::<Option<notch::NotchTransfer>>("transfer")?.as_ref(),
            a.get::<Option<String>>("keyvault")?.as_deref(),
        )),
        "notch.progress" => out(notch::estimated_progress(
            a.get("elapsedMs")?,
            a.get::<Option<u32>>("estimateMs")?
                .unwrap_or(notch::ACTIVITY_ESTIMATE_MS),
        )),
        "notch.osIcon" => out(notch::os_icon(
            a.get("os")?,
            a.get::<Option<String>>("osName")?.as_deref(),
        )),
        "notch.osIconSvg" => out(notch::os_icon_svg(&a.get::<String>("id")?)),
        "notch.tileAt" => out(notch::tile_at(
            &a.get("layout")?,
            &a.get::<Vec<notch::NotchTile>>("tiles")?,
            a.get::<Option<bool>>("row")?.unwrap_or(false),
            a.get("x")?,
            a.get("y")?,
        )),
        "dragTrigger.initial" => out(notch::drag_trigger::initial()),
        "dragTrigger.apply" => out(notch::drag_trigger::apply(
            &a.get("state")?,
            &a.get("event")?,
            &a.get::<Vec<notch::drag_trigger::DragDisplay>>("displays")?,
        )),
        "dragTrigger.classify" => out(notch::drag_trigger::classify(
            &a.get("start")?,
            &a.get("now")?,
        )),
        "dragTrigger.displays" => out(notch::drag_trigger::displays(
            &a.get::<Vec<notch::ScreenFacts>>("screens")?,
        )),
        "dragTrigger.portalDisplays" => out(notch::drag_trigger::portal_displays(&a.get::<Vec<
            notch::ScreenFacts,
        >>(
            "screens"
        )?)),
        "host.summary" => out(host::host_summary(
            a.get::<Option<host::HostSummaryInput>>("status")?.as_ref(),
        )),
        "host.thisMachineSpace" => out(host::this_machine_space(
            a.get::<Option<host::HostSummaryInput>>("status")?.as_ref(),
            a.get("now")?,
            a.get::<Option<SpaceOs>>("os")?.unwrap_or(SpaceOs::Macos),
        )),
        "host.looksLikeListen" => out(host::looks_like_listen(&a.get::<String>("value")?)),
        "host.parseAllowList" => out(host::parse_allow_list(&a.get::<String>("value")?)),
        "machines.merge" => out(machines::merge(
            &a.get::<Option<machines::MachinesInput>>("input")?
                .unwrap_or_default(),
        )),
        "devices.view" => out(devices::devices_view(
            &a.get::<Option<devices::DevicesInput>>("input")?
                .unwrap_or_default(),
            a.get::<Option<u64>>("now")?.unwrap_or(0),
        )),
        "devices.activityText" => out(devices::activity_text(
            &a.get::<Option<devices::AuditInput>>("event")?
                .unwrap_or_default(),
            &a.get::<Option<Vec<devices::DeviceInput>>>("devices")?
                .unwrap_or_default(),
            &a.get::<Option<std::collections::HashMap<String, String>>>("machineNames")?
                .unwrap_or_default(),
        )),
        "devices.labels" => out(devices::labels()),
        "devices.platformName" => out(devices::platform_name(
            a.get::<Option<String>>("platform")?.as_deref(),
        )),
        "devices.cleanName" => out(devices::clean_name(&a.get::<String>("name")?)),
        "devices.normalizeCode" => out(devices::normalize_code(&a.get::<String>("code")?)),
        "devices.enrollInitial" => out(devices::enroll_initial()),
        "devices.enrollReduce" => out(devices::enroll_reduce(
            &a.get::<devices::EnrollState>("state")?,
            &a.get::<devices::EnrollAction>("action")?,
        )),
        "devices.enrollView" => out(devices::enroll_view(
            &a.get::<devices::EnrollState>("state")?,
        )),
        "devices.approveOpen" => out(devices::approve_open(
            &a.get::<devices::ApprovalPrompt>("prompt")?,
        )),
        "devices.approveReduce" => out(devices::approve_reduce(
            &a.get::<devices::ApproveSheetState>("state")?,
            &a.get::<devices::ApproveSheetAction>("action")?,
            &a.get::<Option<Vec<devices::DeviceInput>>>("devices")?
                .unwrap_or_default(),
        )),
        "devices.approveView" => out(devices::approve_view(
            &a.get::<devices::ApproveSheetState>("state")?,
            &a.get::<Option<Vec<devices::DeviceInput>>>("devices")?
                .unwrap_or_default(),
        )),
        "agents.pageInitial" => out(persistent::agents_initial()),
        "agents.pageReduce" => out(persistent::agents_reduce(
            &a.get::<persistent::AgentsInput>("input")?,
            &a.get::<persistent::AgentsState>("state")?,
            &a.get::<persistent::AgentsAction>("action")?,
        )),
        "agents.pageView" => out(persistent::agents_view(
            &a.get::<persistent::AgentsInput>("input")?,
            &a.get::<persistent::AgentsState>("state")?,
            a.get::<u64>("nowMs")?,
        )),
        "drive.initial" => out(drive_page::drive_initial()),
        "drive.reduce" => out(drive_page::drive_reduce(
            &a.get::<drive_page::DriveState>("state")?,
            &a.get::<drive_page::DriveAction>("action")?,
        )),
        "drive.view" => out(drive_page::drive_view(
            &a.get::<drive_page::DriveInput>("input")?,
            &a.get::<drive_page::DriveState>("state")?,
        )),
        "storage.initial" => out(drive_settings::storage_initial()),
        "storage.reduce" => out(drive_settings::storage_reduce(
            &a.get::<drive_settings::StorageState>("state")?,
            &a.get::<drive_settings::StorageAction>("action")?,
        )),
        "storage.section" => out(drive_settings::storage_section(
            &a.get::<drive_settings::StorageInput>("input")?,
            &a.get::<drive_settings::StorageState>("state")?,
        )),
        "storage.press" => out(drive_settings::storage_press(
            &a.get::<drive_settings::StorageInput>("input")?,
            &a.get::<String>("id")?,
        )),
        "storage.choose" => out(drive_settings::storage_choose(
            &a.get::<String>("id")?,
            &a.get::<String>("option")?,
        )),
        "storage.edit" => out(drive_settings::storage_edit(
            &a.get::<String>("id")?,
            &a.get::<String>("value")?,
        )),
        "storage.requestText" => out(drive_settings::storage_request_text(
            &a.get::<drive_settings::StorageRequest>("request")?,
        )),
        "notifications.plan" => out(notifications::notifications_plan(
            &a.get::<Vec<notifications::NotificationInput>>("feed")?,
            a.get::<u64>("seenMs")?,
        )),
        "notifications.view" => out(notifications::notifications_view(
            &a.get::<Vec<notifications::NotificationInput>>("feed")?,
            a.get::<u64>("nowMs")?,
        )),
        "cloudConnect.initial" => out(cloud_connect::cloud_connect_initial()),
        "cloudConnect.reduce" => out(cloud_connect::cloud_connect_reduce(
            &a.get("input")?,
            &a.get("state")?,
            &a.get("action")?,
        )),
        "cloudConnect.view" => out(cloud_connect::cloud_connect_view(
            &a.get("input")?,
            &a.get("state")?,
        )),
        "cloudConnect.inputFromStatus" => out(cloud_connect::connect_input_from_status(
            &a.get::<Value>("status")?,
        )),
        "cloudConnect.cloudsFromStatus" => out(cloud_connect::connected_clouds_from_status(
            &a.get::<Value>("status")?,
        )),
        "cloudConnect.isCloudWord" => out(cloud_connect::is_cloud_word(&a.get::<String>("on")?)),
        "share.initial" => out(share::share_initial()),
        "share.reduce" => out(share::share_reduce(
            &a.get::<share::ShareInput>("input")?,
            &a.get::<share::ShareSheetState>("state")?,
            &a.get::<share::ShareSheetAction>("action")?,
        )),
        "share.view" => out(share::share_view(
            &a.get::<share::ShareInput>("input")?,
            &a.get::<share::ShareSheetState>("state")?,
        )),
        "host.clientLabel" => out(host::client_label(
            &a.get::<String>("id")?,
            a.get::<Option<String>>("email")?.as_deref(),
            a.get::<Option<String>>("name")?.as_deref(),
        )),
        "presence.name" => out(presence::presence_name(
            a.get::<Option<String>>("name")?.as_deref(),
            a.get::<Option<String>>("email")?.as_deref(),
            a.get::<Option<String>>("username")?.as_deref(),
            a.get::<Option<String>>("osFullName")?.as_deref(),
            a.get::<Option<String>>("osUser")?.as_deref(),
        )),
        "presence.principalId" => out(presence::presence_principal_id(
            a.get::<Option<String>>("email")?.as_deref(),
            a.get::<Option<String>>("subject")?.as_deref(),
            a.get::<Option<String>>("username")?.as_deref(),
            a.get::<Option<String>>("osUser")?.as_deref(),
        )),
        "host.withThisMachine" => out(host::with_this_machine(
            &a.get::<Vec<Space>>("spaces")?,
            a.get::<Option<host::HostSummaryInput>>("status")?.as_ref(),
            a.get("now")?,
            a.get::<Option<SpaceOs>>("os")?.unwrap_or(SpaceOs::Macos),
        )),
        "host.summaryInput" => out(a.get::<host::HostState>("state")?.summary_input()),
        "host.panel" => out(host::panel(
            a.get::<Option<host::HostState>>("state")?.as_ref(),
        )),
        "host.permissionRows" => out(host::permission_rows(&a.get::<Vec<_>>("permissions")?)),
        "host.formInitial" => out(host::form_initial()),
        "host.settingChange" => out(host::setting_change(a.get("id")?)),
        "host.formReduce" => out(host::form_reduce(&a.get("state")?, &a.get("action")?)),
        "host.formView" => out(host::form_view(
            &a.get("state")?,
            a.get::<Option<String>>("identity")?.as_deref(),
        )),
        "agents.name" => out(agents::agent_name(&a.get::<String>("agent")?)),
        "agents.initial" => out(agents::agent_initial(&a.get::<String>("agent")?)),
        "agents.subtitle" => out(agents::agent_subtitle(&a.get("run")?)),
        // A run's status word ("Running", "Idle", ...), as the SwiftUI
        // detail's Agents rows say it.
        "agents.statusLabel" => out(a.get::<agents::AgentStatus>("status")?.label()),
        "agents.filter" => out(agents::filter_agent_runs(
            &a.get::<Vec<_>>("runs")?,
            &a.get::<String>("query")?,
        )),
        "agents.order" => out(agents::order_agent_runs(&a.get::<Vec<_>>("runs")?)),
        "agents.settingsRows" => out(agents::agent_settings_rows(
            &a.get::<Vec<_>>("statuses")?,
            a.get("total")?,
        )),
        "agents.setupSummary" => out(agents::agent_setup_summary(
            &a.get::<Vec<_>>("outcomes")?,
            &a.get::<String>("agent")?,
            &a.get::<String>("name")?,
        )),
        "paths.displayPath" => out(paths::display_path(
            &a.get::<String>("path")?,
            a.get::<Option<String>>("home")?.as_deref(),
        )),
        "paths.displayPaths" => out(paths::display_paths(
            &a.get::<String>("text")?,
            a.get::<Option<String>>("home")?.as_deref(),
        )),
        "telemetry.launched" => out(telemetry::launched(a.get("onboardingEligible")?)),
        "telemetry.feature" => out(telemetry::feature_used(&a.get::<String>("feature")?)),
        "telemetry.onboarding" => out(telemetry::onboarding(&a.get("state")?, &a.get("action")?)),
        "telemetry.onboardingFinished" => out(telemetry::onboarding_finished(&a.get("state")?)),
        "telemetry.onboardingSkipped" => out(telemetry::onboarding_skipped(&a.get("state")?)),
        "telemetry.signInFailed" => out(telemetry::sign_in_failed(
            a.get::<Option<String>>("message")?.as_deref(),
        )),
        "telemetry.creates" => out(telemetry::creates(
            &a.get::<Option<creating::CreatesState>>("state")?
                .unwrap_or_default(),
            &a.get("action")?,
            a.get("now")?,
        )),
        "telemetry.storage" => out(telemetry::storage(
            &a.get("input")?,
            &a.get("state")?,
            &a.get("action")?,
        )),
        "telemetry.share" => out(telemetry::share(
            &a.get("input")?,
            &a.get("state")?,
            &a.get("action")?,
        )),
        "telemetry.enroll" => out(telemetry::enroll(&a.get("state")?, &a.get("action")?)),
        "telemetry.experimentsChanged" => out(telemetry::experiments_changed(
            &a.get::<Option<experiments::Experiments>>("before")?
                .unwrap_or_default(),
            &a.get::<Option<experiments::Experiments>>("after")?
                .unwrap_or_default(),
        )),
        "telemetry.experimentsOn" => out(telemetry::experiments_on(
            &a.get::<Option<experiments::Experiments>>("experiments")?
                .unwrap_or_default(),
        )),
        other => Err(CoreError::Invalid(format!("unknown method {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dispatch_covers_every_method() {
        for m in METHODS {
            let r = call_value(m, Value::Object(Default::default()));
            if let Err(CoreError::Invalid(e)) = &r {
                assert!(
                    !e.starts_with("unknown method"),
                    "{m} is listed but not dispatched"
                );
            }
        }
        assert!(matches!(
            call("nope.nope", "{}"),
            Err(CoreError::Invalid(e)) if e.starts_with("unknown method")
        ));
    }

    /// The web asks the core whether a machine shares its desktop before it
    /// reads the machine's agent runs, usage or windows (calls that a
    /// machine that does not share its desktop refuses).
    #[test]
    fn a_machine_that_does_not_share_its_desktop_says_so() {
        let space = |id: &str, features: &[&str]| {
            let row = |features: &[&str]| {
                json!({
                    "id": id, "name": "Studio", "provider": "relay", "os": "macos",
                    "spacesdVersion": "0.4.1", "features": features, "reachable": true,
                })
            };
            let space = call_value(
                "spaces.rowToSpace",
                json!({ "row": row(features), "now": 0 }),
            )
            .unwrap();
            call_value("spaces.sharesDesktop", json!({ "space": space })).unwrap()
        };
        // A spare Mac: it answered, with the desktop's features unsupported.
        assert_eq!(space("relay:m1", &["host_spaces", "files"]), json!(false));
        assert_eq!(
            space("relay:m1", &["desktop_stream", "host_spaces"]),
            json!(true)
        );
        // Not known: it reported no features. Not a machine: a local Space.
        assert_eq!(space("relay:m1", &[]), Value::Null);
        assert_eq!(space("local:dev", &["files"]), Value::Null);
    }

    /// `onboarding.view` names the menu bar only for a Mac (`hostOs`).
    #[test]
    fn onboarding_words_follow_the_hosts_system() {
        let state = call_value(
            "onboarding.initial",
            json!({ "installerMode": null, "identity": null }),
        )
        .unwrap();
        let mut done = state.clone();
        done["step"] = json!("done");
        let lede = |os: Value| {
            call_value("onboarding.view", json!({ "state": done, "hostOs": os })).unwrap()["lede"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(lede(Value::Null), "Cua Spaces is in your menu bar.");
        assert_eq!(lede(json!("linux")), "Cua Spaces is in your system tray.");
        assert_eq!(lede(json!("windows")), "Cua Spaces is in your system tray.");
    }

    /// `agentKeys.view` and `agentKeys.form` say where keys stay in the shell's
    /// system's words (`hostOs`), and in a Mac's when it names none.
    #[test]
    fn agent_keys_words_follow_the_hosts_system() {
        let intro = |args: Value| {
            call_value("agentKeys.view", args).unwrap()["intro"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let input = json!({ "keys": [] });
        let mac = intro(json!({ "input": input }));
        assert!(mac.contains("on this Mac in the Keychain,"));
        assert_eq!(intro(json!({ "input": input, "hostOs": "macos" })), mac);
        assert_eq!(intro(json!({ "input": input, "hostOs": null })), mac);
        assert!(
            intro(json!({ "input": input, "hostOs": "windows" }))
                .contains("on this PC in Windows Credential Manager,")
        );
        let linux = intro(json!({ "input": input, "hostOs": "linux" }));
        assert!(linux.contains("on this computer,") && !linux.contains("Keychain"));
        let help = |os: Value| {
            call_value(
                "agentKeys.form",
                json!({ "input": input, "form": { "provider": "openai" }, "hostOs": os }),
            )
            .unwrap()["valueHelp"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            help(Value::Null),
            "It's saved in the Keychain on this Mac and won't be shown again."
        );
        assert!(help(json!("windows")).contains("in Windows Credential Manager on this PC"));
        assert_eq!(
            help(json!("linux")),
            "It's saved on this computer and won't be shown again."
        );
    }

    /// The run's status line (which step it is on, with bytes) is the
    /// core's, not the page's: only a running picker has one.
    #[test]
    fn flow_status_says_which_step_a_run_is_on() {
        let event = |detail: &str, done: u64, total: u64| {
            json!({ "step": 0, "steps": 2, "kind": "state", "phase": "progress",
                    "detail": detail, "doneBytes": done, "totalBytes": total })
        };
        let mut state = call_value("flow.initial", json!({ "spaceName": "dev" })).unwrap();
        assert_eq!(
            call_value("flow.status", json!({ "state": state })).unwrap(),
            Value::Null
        );
        state["step"] = json!("running");
        state["events"] = json!([event("Uploading", 12 << 20, 80 << 20)]);
        assert_eq!(
            call_value("flow.status", json!({ "state": state })).unwrap(),
            json!("Uploading 12 / 80 MB")
        );
        state["step"] = json!("done");
        assert_eq!(
            call_value("flow.status", json!({ "state": state })).unwrap(),
            Value::Null
        );
    }

    fn overview_with(items: Value, deliveries: Value) -> Value {
        json!({
            "availability": "ready", "serverVerified": true, "namesVisible": true,
            "itemsTotal": 3, "items": items, "deliveries": deliveries,
        })
    }

    fn kv_item(id: &str, kind: &str, provider: &str, updated: u64) -> Value {
        json!({
            "id": id, "kind": kind, "provider_id": provider, "app_display": provider,
            "domain": "github.com", "key": id, "source": "test", "session": false,
            "bytes": 10, "identity_provider": false,
            "policy": { "allowed_targets": [], "ttl_secs": 0, "unattended": false },
            "created_ms": 1, "updated_ms": updated, "rev": 1, "record_digest": "d",
        })
    }

    /// What the Keyvault holds for an app a teleport can send: passwords
    /// never are, and are listed apart.
    #[test]
    fn keyvault_vault_source_counts_what_a_teleport_can_send() {
        let items = json!([
            kv_item("c1", "cookie", "chrome", 100),
            kv_item("c2", "cookie", "chrome", 300),
            kv_item("p1", "password", "chrome", 200),
            kv_item("o1", "cookie", "safari", 900),
        ]);
        let source = call_value(
            "keyvault.vaultSource",
            json!({ "overview": overview_with(items, json!([])), "providerId": "chrome" }),
        )
        .unwrap();
        assert_eq!(source["count"], json!(2));
        assert_eq!(source["newestMs"], json!(300));
        assert_eq!(source["ids"], json!(["c1", "c2"]));
        assert_eq!(source["passwordIds"], json!(["p1"]));
    }

    /// A Space with a live Keyvault copy is "Signed in" (its Access row is
    /// where the badge goes); a dismissed copy still marks the Spaces list
    /// but not the notch, and a wiped one marks nothing.
    #[test]
    fn keyvault_marks_spaces_with_live_access() {
        let space = |id: &str| {
            call_value(
                "spaces.rowToSpace",
                json!({ "row": { "id": id, "name": id.split(':').nth(1).unwrap(), "provider": "relay",
                                  "os": "linux", "reachable": true }, "now": 0 }),
            )
            .unwrap()
        };
        let (dev, other) = (space("local:dev"), space("local:other"));
        let delivery = |import: &str, wiped: bool| {
            json!({ "import_id": import, "target": "dev", "provider_id": "chrome", "items": ["c1"],
                    "caller_fp": "fp", "delivered_ms": 1, "expires_ms": 0, "wiped": wiped })
        };
        let now = 1_800_000_000_000_i64;
        let ids = |overview: &Value, dismissed: Value| {
            call_value(
                "keyvault.signedInSpaces",
                json!({ "overview": overview, "now": now, "dismissed": dismissed,
                        "spaces": [dev.clone(), other.clone()] }),
            )
            .unwrap()
        };
        let live = overview_with(json!([]), json!([delivery("i1", false)]));
        assert_eq!(ids(&live, json!([])), json!(["local:dev"]));
        assert_eq!(ids(&live, json!(["i1"])), json!([]));
        let wiped = overview_with(json!([]), json!([delivery("i1", true)]));
        assert_eq!(ids(&wiped, json!([])), json!([]));
        let key = |space: &Value| {
            call_value(
                "keyvault.spaceAccessKey",
                json!({ "overview": live, "now": now, "space": space }),
            )
            .unwrap()
        };
        assert_eq!(key(&dev), json!("d:dev"));
        assert_eq!(key(&other), Value::Null);
        // Deleting an item with a live copy says how many Spaces are wiped.
        let copies = |overview: &Value, ids: Value| {
            call_value(
                "keyvault.liveCopySpaces",
                json!({ "overview": overview, "ids": ids, "now": now }),
            )
            .unwrap()
        };
        assert_eq!(copies(&live, json!(["c1"])), json!(1));
        assert_eq!(copies(&live, json!(["other"])), json!(0));
        assert_eq!(copies(&wiped, json!(["c1"])), json!(0));
    }

    /// The web reads a Space's detail as this device sees it: with
    /// `access`, a machine on the relay has its connection actions greyed
    /// out under the notice; one that keeps its desktop private says so,
    /// and Share follows Settings, Experiments.
    #[test]
    fn the_detail_follows_access_and_experiments() {
        let space = |id: &str, features: &[&str]| {
            call_value(
                "spaces.rowToSpace",
                json!({ "row": {
                    "id": id, "name": "Studio", "provider": "relay", "os": "macos",
                    "spacesdVersion": "0.4.1", "features": features, "reachable": true,
                }, "now": 0 }),
            )
            .unwrap()
        };
        let shared = space(
            "relay:m1",
            &["desktop_stream", "host_spaces", "relay_attach"],
        );
        let ids = |d: &Value| -> Vec<(String, bool)> {
            d["actions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        a["id"].as_str().unwrap().to_string(),
                        a["enabled"].as_bool().unwrap(),
                    )
                })
                .collect()
        };
        let has = |d: &Value, id: &str| ids(d).iter().any(|(a, _)| a == id);
        // No experiments, no access: every action, as before.
        let live = call_value("sidebar.detail", json!({ "space": shared })).unwrap();
        assert!(has(&live, "share"));
        // Sharing off hides Share; on shows it.
        let off = call_value(
            "sidebar.detail",
            json!({ "space": shared, "experiments": { "sharing": false } }),
        )
        .unwrap();
        assert!(!has(&off, "share"));
        let on = call_value(
            "sidebar.detail",
            json!({ "space": shared, "experiments": { "sharing": true } }),
        )
        .unwrap();
        assert!(has(&on, "share") && on.get("access").is_none());
        // Not enrolled: the notice, Status says why, connection actions off.
        let notice = serde_json::to_value(
            devices::machine_access_notice(devices::EnrollmentKind::NeedsEnrollment, None).unwrap(),
        )
        .unwrap();
        let d = call_value(
            "sidebar.detail",
            json!({ "space": shared, "experiments": { "sharing": true }, "access": notice }),
        )
        .unwrap();
        assert_eq!(d["access"], notice);
        assert_eq!(d["canStream"], json!(false));
        assert_eq!(d["previewText"], notice["text"]);
        for (id, enabled) in ids(&d) {
            if ["teleport", "pip", "share", "open"].contains(&id.as_str()) {
                assert!(!enabled, "{id} stays off while not enrolled");
            }
        }
        // Without experiments, `access` still applies.
        let d = call_value(
            "sidebar.detail",
            json!({ "space": shared, "access": notice }),
        )
        .unwrap();
        assert_eq!(d["access"], notice);
        // A machine that keeps its desktop private: its note, no New Space
        // button word in the note itself, its desktop actions gone.
        let private = space("relay:m1", &["host_spaces", "files"]);
        let d = call_value(
            "sidebar.detail",
            json!({ "space": private, "experiments": {} }),
        )
        .unwrap();
        assert_eq!(
            d["desktopNote"],
            json!("Studio isn\u{2019}t sharing its desktop. You can still create Spaces on it.")
        );
        assert!(!has(&d, "teleport") && !has(&d, "open"));
    }

    /// The preview's cover: Connect while auto-connect is off, Connecting
    /// while it opens, Try again after a failure, and Connect greyed out
    /// under the notice while this device is not enrolled.
    #[test]
    fn the_desktop_cover_is_the_cores() {
        let cover =
            |input: Value| call_value("spaces.desktopCover", json!({ "input": input })).unwrap();
        let input = |auto: bool, stream: &str| {
            json!({ "canStream": true, "previewText": "", "autoConnect": auto,
                    "connectRequested": false, "stream": stream })
        };
        let c = cover(input(true, "nosession"));
        assert_eq!(c["kind"], json!("connecting"));
        assert_eq!(c["openStream"], json!(true));
        let c = cover(input(false, "nosession"));
        assert_eq!(c["kind"], json!("connect"));
        assert_eq!(c["button"], json!("Connect"));
        let c = cover(input(true, "failed"));
        assert_eq!(c["text"], json!("Could not connect to the desktop"));
        assert_eq!(c["button"], json!("Try again"));
        let notice = serde_json::to_value(
            devices::machine_access_notice(devices::EnrollmentKind::Due, None).unwrap(),
        )
        .unwrap();
        let mut locked = input(true, "nosession");
        locked["access"] = notice.clone();
        let c = cover(locked);
        assert_eq!(c["buttonDisabled"], json!(true));
        assert_eq!(c["action"], notice["actionLabel"]);
    }

    /// The words the web's Agents rows and status labels read: the core's,
    /// not the page's.
    #[test]
    fn status_words_are_the_cores() {
        assert_eq!(
            call_value("agents.statusLabel", json!({ "status": "idle" })).unwrap(),
            json!("Idle")
        );
        let space = |reachable: bool| {
            call_value(
                "spaces.rowToSpace",
                json!({ "row": {
                    "id": "local:dev", "name": "dev", "provider": "local", "os": "linux",
                    "spacesdVersion": "0.6.0", "features": [], "reachable": reachable,
                }, "now": 0 }),
            )
            .unwrap()
        };
        let word = |s: Value| call_value("sidebar.statusText", json!({ "space": s })).unwrap();
        // Not reachable, with no power record: the registry's word.
        assert_eq!(word(space(false)), json!("Suspended"));
        assert_eq!(word(space(true)), json!("Running"));
    }

    #[test]
    fn calls_round_trip_json() {
        assert_eq!(
            call("spaces.displayName", r#"{"name":"brave-otter"}"#).unwrap(),
            "\"brave-otter\""
        );
        assert_eq!(
            call("keyvault.duration", r#"{"ms":90000}"#).unwrap(),
            "\"2 min\""
        );
    }
}
