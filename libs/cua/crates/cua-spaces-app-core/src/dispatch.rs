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
    CoreError, about, agents, cloud_connect, devices, drive_mount_preview, drive_page,
    drive_settings, driver_preview, experiments, host, login_item, notch, notifications,
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
    "sidebar.streamSection",
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
    "agents.filter",
    "agents.order",
    "agents.settingsRows",
    "agents.setupSummary",
    "paths.displayPath",
    "paths.displayPaths",
    "devices.view",
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
        // With `experiments`, what Settings, Experiments hides is left out.
        "sidebar.detail" => {
            let space = a.get("space")?;
            let usage = a.get::<Option<sidebar::SpaceUsage>>("usage")?;
            let host_arch = a.get::<Option<String>>("hostArch")?;
            out(
                match a.get::<Option<experiments::Experiments>>("experiments")? {
                    Some(x) => {
                        sidebar::detail_with(&space, usage.as_ref(), host_arch.as_deref(), &x)
                    }
                    None => sidebar::detail_live(&space, usage.as_ref(), host_arch.as_deref()),
                },
            )
        }
        "sidebar.detailCopy" => out(sidebar::detail_copy()),
        "sidebar.deleteFailedText" => out(sidebar::delete_failed_text(
            &a.get::<String>("name")?,
            &a.get::<String>("error")?,
        )),
        "sidebar.powerButton" => out(sidebar::power_button(&a.get::<Space>("space")?)),
        "sidebar.streamSection" => out(stream::stream_section(&a.get("input")?)),
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
        "wizard.view" => out(wizard::view(&a.get("state")?, &a.get("env")?)),
        "wizard.createArgs" => out(wizard::create_args(&a.get("plan")?)),
        "wizard.placementOptions" => {
            out(wizard::placement_options(&a.get("state")?, &a.get("env")?))
        }
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
        "wizard.imageSuggestions" => out(wizard::image_suggestions(&a.get::<String>("query")?)),
        "wizard.validateImageRef" => out(wizard::validate_image_ref(&a.get::<String>("ref")?)),
        "wizard.creatingText" => out(wizard::creating_text(&a.get("plan")?)),
        "wizard.createFailedText" => out(wizard::create_failed_text(&a.get::<String>("error")?)),
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
        "onboarding.view" => out(onboarding::view(&a.get("state")?)),
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
        "telemetry.launched" => out(telemetry::launched()),
        "telemetry.feature" => out(telemetry::feature_used(&a.get::<String>("feature")?)),
        "telemetry.onboarding" => out(telemetry::onboarding(&a.get("state")?, &a.get("action")?)),
        "telemetry.onboardingFinished" => out(telemetry::onboarding_finished(&a.get("state")?)),
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

    #[test]
    fn calls_round_trip_json() {
        assert_eq!(
            call("spaces.displayName", r#"{"name":"brave-otter"}"#).unwrap(),
            "\"Brave Otter\""
        );
        assert_eq!(
            call("keyvault.duration", r#"{"ms":90000}"#).unwrap(),
            "\"2 min\""
        );
    }
}
