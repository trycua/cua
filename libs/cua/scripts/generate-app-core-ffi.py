#!/usr/bin/env python3
"""Writes the record half of libs/cua/crates/cua-spaces-ffi/src/app_core_types.rs:
`#[uniffi::remote(Record|Enum)]` mirrors of the app core's view-model types
(libs/cua/crates/cua-spaces-app-core).

The compiler checks every mirror field against the core (a missing or extra
field does not compile), so this script only saves typing; `--check` fails
when the checked-in file is stale.

    python3 libs/cua/scripts/generate-app-core-ffi.py [--check]
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CORE = ROOT / "crates/cua-spaces-app-core/src"
OUT = ROOT / "crates/cua-spaces-ffi/src/app_core_types.rs"

# (source file, module path, [types]) in FFI order.
TYPES = [
    ("model.rs", "model", ["SpaceOs", "SpaceStatus", "ThumbnailScene", "SpaceProvider", "SpaceSdkRef", "SpaceProgress", "PowerControl", "SpacePower", "Space", "SpaceRow", "Location", "SpaceKind", "Runtime"]),
    ("spaces/mod.rs", "spaces", ["AmbientDots"]),
    ("spaces/roster.rs", "spaces::roster", ["NoticeKind", "Notice", "RosterState", "RosterAction"]),
    ("spaces/creating.rs", "spaces::creating", ["PendingCreate", "PendingDelete", "PendingPower", "CreatesState", "CreateAction"]),
    ("spaces/sidebar.rs", "spaces::sidebar", ["PowerButton", "SidebarRow", "SidebarSection", "SidebarView", "FactCopy", "FactWarning", "Fact", "DetailActionId", "DetailAction", "DeleteConfirm", "DetailCopy", "SpaceUsage", "SpaceDetail"]),
    ("spaces/cover.rs", "spaces::cover", ["StreamPhase", "DesktopCoverInput", "DesktopCoverKind", "DesktopCover", "ThumbnailPolicy"]),
    ("spaces/stream.rs", "spaces::stream", ["StreamDisplay", "StreamSectionInput", "StreamRowKind", "StreamRowIcon", "StreamRowActionId", "StreamRowAction", "StreamRow", "StreamSection", "PipEvent", "PipCommand"]),
    ("notch/geometry.rs", "notch::geometry", ["WindowMode", "LogicalRect"]),
    ("teleport/drag.rs", "teleport::drag", ["DragOverlayPhase", "DragOverlayState", "DragOverlayEvent", "DragOverlayEffect", "DragOverlayTransition"]),
    ("notch/drag_trigger.rs", "notch::drag_trigger", ["DragKind", "TriggerPhase", "DragDisplay", "StillAnchor", "DragTriggerState", "DragTriggerEvent", "DragTriggerTransition"]),
    ("notch/mod.rs", "notch", ["NotchMotion", "NotchRadii", "ScreenFacts", "NotchLayout", "NotchTile", "NotchPhase", "NotchState", "NotchEvent", "NotchEffect", "NotchTransition", "NotchPermission", "NotchAccess", "NotchTransfer", "NotchTab", "NotchButtonId", "NotchButton", "NotchHeader", "NotchActivityKind", "NotchActivity", "NotchView"]),
    ("teleport/transfer.rs", "teleport::transfer", ["TransferPhase", "TransferOverlayState", "TransferStatus", "TransferSignal", "SentFileInfo"]),
    ("teleport/windows.rs", "teleport::windows", ["OpenWindow", "OpenApp", "RemoteWindow", "RemoteWindowGroup", "PickerTab", "PickerAction", "PickerPrimary"]),
    ("teleport/grid.rs", "teleport::grid", ["PickerTileIcon", "PickerTileThumbnail", "PickerTile", "PickerTileSection", "PickerGrid", "PickerGridTab", "PickerGridTabItem", "PickerGridPrimary"]),
    ("teleport/review.rs", "teleport::review", ["SendSource", "RememberedChoice", "ReviewDomain", "ReviewToggle", "VaultSource", "ReviewChoice"]),
    ("teleport/flow.rs", "teleport::flow", ["Capability", "Move", "SensitiveGroup", "SensitiveOption", "CatalogEntry", "ConsentKind", "ConsentItem", "PlanStepView", "Plan", "RunPhase", "RunEvent", "RunReport", "Consent", "InstallCuaPrompt", "Step", "PickerState", "PickerEvent", "EntrySection", "ReviewView"]),
    ("wizard.rs", "wizard", ["LocalEngine", "CloudEngine", "ImageTier", "ImageDistro", "PlatformSize", "ImageSizes", "SandboxImage", "ImageGroup", "KindOption", "StorageVolume", "LocalStorage", "CloudPricing", "CloudOffer", "ConnectedCloud", "HostLimit", "SpaceHost", "PlacementOption", "GpuChoice", "WizardEnv", "WizardMode", "AddressForm", "WizardState", "WizardAction", "StepState", "StepView", "Tile", "MenuOption", "CreatePlan", "CreateSpaceArgs", "AddressView", "AddressSubmit", "ImageSuggestion", "SuggestionGroup", "ImageFieldView", "WizardField", "WizardLabels", "WizardFact", "GpuRow", "WizardView"]),
    ("keyvault/wire.rs", "keyvault::wire", ["KvItemPolicy", "KvItem", "KvFavicon", "KvDomainCount", "KvInventory", "KvSigning", "KvCaller", "KvSelector", "KvAccessRequest", "KvPending", "KvGrant", "KvRuleCaller", "KvRule", "KvDelivery", "KvAuditEntry", "KvStatus", "KvVerification", "KeyvaultOverview", "KvCommand"]),
    ("keyvault/view.rs", "keyvault::view", ["Tone", "SigningBadge", "Tri", "DecisionTone", "Decision", "PendingRow", "AccessKind", "AccessRow", "KvLabels", "KeyvaultPage"]),
    ("keyvault/vault.rs", "keyvault::vault", ["KvKind", "KvLock", "VaultState", "VaultAction", "VaultRow", "VaultSite", "VaultFiles", "VaultApp", "VaultSelection", "VaultView", "UnlockPrompt", "KvDeleteConfirm", "KvVaultSource"]),
    ("keyvault/browse.rs", "keyvault::browse", ["KvCategory", "CategoryRow", "AppRow", "KvSidebar", "KvSelection", "RecentRow", "KvListView"]),
    ("keyvault/approval.rs", "keyvault::approval", ["ApprovalState", "ApprovalAction", "ApprovalRow", "ApprovalView"]),
    ("keyvault/credential.rs", "keyvault::credential", ["KvFormMode", "KvMethod", "KvCredentialForm", "KvStrength", "KvPassphraseCheck"]),
    ("keyvault/client.rs", "keyvault::client", ["KvOutcome"]),
    ("billing.rs", "billing", ["BillingCard", "BillingCredit", "BillingStatus", "CreditNotice"]),
    ("drive_settings.rs", "drive_settings", ["DriveMountInput", "DriveS3Input", "DriveStorageInput", "DriveCheckInput", "DriveCacheInput", "StorageInput", "DriveStorageUpdate", "StorageForm", "StorageField", "StorageRequest", "StorageState", "StorageAction"]),
    ("onboarding.rs", "onboarding", ["OnboardingStep", "DriveStepRequest", "StorageChoice", "OnboardingMode", "OnboardingState", "OnboardingAction", "StepDot", "UsageToggle", "ModeChoice", "OnboardingView", "DriveCard", "PresentationCard", "OnboardingCopy", "OnboardingCheckbox"]),
    ("onboarding_preview.rs", "onboarding_preview", ["PreviewPoint", "NotchPreview", "MenuPreviewRow", "MenuPreview", "PresentationPreview", "PreviewFrame"]),
    ("driver_preview.rs", "driver_preview", ["PreviewSegment", "PreviewWindow", "DriverPreview", "DriverFrame"]),
    ("drive_mount_preview.rs", "drive_mount_preview", ["DriveMountPreview", "DriveMountFrame"]),
    ("about.rs", "about", ["UpdateChannel", "AboutInput", "AboutLinkId", "AboutLink", "AboutUpdates", "AboutView", "LaunchInput", "LaunchPlan", "DaemonCheck", "RefreshReport"]),
    ("login_item.rs", "login_item", ["LoginItemStatus", "LoginItemInput", "LoginItemPlan"]),
    ("experiments.rs", "experiments", ["Experiments"]),
    ("settings.rs", "settings", ["SwitcherTheme", "AppSettings", "KeyCombo", "SignInPhase", "TelemetryInput", "SettingsInput", "SettingsRowKind", "SettingsOption", "SettingsRow", "SettingsSection", "SettingsPage"]),
    ("window.rs", "window", ["ChromeInput", "MainChrome", "MenuItemId", "MenuItem", "MenuInput"]),
    ("host.rs", "host", ["HostSummaryInput", "HostSetupRequest", "HostClient", "HostAccess", "HostPermissionInput", "HostProvidedSpace", "HostSpacesAudit", "HostState", "HostActionId", "HostSettingChange", "HostToggle", "HostConfirm", "HostAction", "HostAccessRow", "PermissionRow", "HostSetupChoice", "HostPanelView", "HostFormState", "HostFormAction", "HostFormChoice", "HostFormField", "HostFormView"]),
    ("installer.rs", "installer", ["InstallMethod", "CliInstallPlan", "CliInstallRequest"]),
    ("agents.rs", "agents", ["AgentStatus", "SpaceAgentRun", "AgentSetupStatus", "AgentSettingsRow", "AgentSetupOutcomeInput", "AgentSetupSummary"]),
    ("devices.rs", "devices", ["DeviceInput", "MachineInput", "AuditInput", "DevicesInput", "BannerTone", "DeviceAction", "DeviceBanner", "DeviceConfirm", "UnconfirmedMachine", "DeviceRow", "ApprovalPrompt", "ActivityRow", "EnrollmentKind", "ThisDevice", "DevicesLabels", "DevicesView", "EnrollMethod", "EnrollPhase", "EnrollState", "EnrollAction", "EnrollOption", "EnrollView", "ApproveSheetState", "ApproveSheetAction", "ApproveRequest", "ApproveSheetView"]),
    ("cloud_connect.rs", "cloud_connect", ["CloudProviderInput", "CloudConnectInput", "CloudCheckInput", "CloudConnectPhase", "CloudConnectState", "CloudConnectAction", "CloudTargetArgs", "CloudConnectRequest", "CloudProviderRow", "CloudField", "CloudCheckRow", "CloudConnectView"]),
    ("share.rs", "share", ["ShareEntryInput", "ShareInput", "ShareRequest", "ShareSheetState", "ShareSheetAction", "RoleOption", "ShareRowView", "ShareSheetView"]),
    ("persistent.rs", "persistent", ["PersistentAgentInput", "DriveEntryInput", "FileVersionInput", "OpenFileInput", "RoutineInput", "ComputerGrantInput", "AccessAuditInput", "AgentsInput", "AgentTab", "RoutineScheduleKind", "RoutineForm", "AgentsRequest", "AgentsState", "AgentsAction", "AgentRowView", "LineView", "TabView", "FileView", "AgentDetailView", "AgentsView"]),
    ("drive_page.rs", "drive_page", ["DriveRequestInput", "DriveGrantInput", "DriveDeviceInput", "DriveConflictInput", "DriveSyncInput", "DriveInput", "DriveRequest", "DriveState", "DriveAction", "ConflictView", "DriveView"]),
    ("notifications.rs", "notifications", ["NotificationInput", "SystemNote", "NotificationsPlan", "NotificationsView"]),
    ("telemetry.rs", "telemetry", ["TelemetrySignal"]),
]

# FFI names that read better than the plain prefix.
RENAME = {
    "Step": "AppPickerStep",
    "Move": "AppTeleportMove",
    "Capability": "AppTeleportCapability",
    "Plan": "AppTeleportPlan",
    "Consent": "AppTeleportConsent",
    "ConsentKind": "AppConsentKind",
    "RunEvent": "AppTeleportRunEvent",
    "RunReport": "AppTeleportRunReport",
    "RunPhase": "AppTeleportRunPhase",
    "AppRow": "KvAppRow",
    "VaultState": "KvVaultState",
    "VaultAction": "KvVaultAction",
    "VaultRow": "KvVaultRow",
    "VaultSite": "KvVaultSite",
    "VaultFiles": "KvVaultFiles",
    "VaultApp": "KvVaultApp",
    "VaultSelection": "KvVaultSelection",
    "VaultView": "KvVaultView",
    "UnlockPrompt": "KvUnlockPrompt",
    "CategoryRow": "KvCategoryRow",
    "RecentRow": "KvRecentRow",
    "PendingRow": "KvPendingRow",
    "AccessRow": "KvAccessRow",
    "AccessKind": "KvAccessKind",
    "Decision": "KvDecision",
    "DecisionTone": "KvDecisionTone",
    "SigningBadge": "KvSigningBadge",
    "Tone": "KvTone",
    "Tri": "KvTri",
    "KeyvaultPage": "KvPage",
    "ApprovalState": "KvApprovalState",
    "ApprovalAction": "KvApprovalAction",
    "ApprovalRow": "KvApprovalRow",
    "ApprovalView": "KvApprovalView",
    "AppSettings": "AppSettings",
}


def ffi_name(n):
    if n in RENAME:
        return RENAME[n]
    if n.startswith("Kv") or n.startswith("Keyvault"):
        return n
    return "App" + n


ALL = {t: ffi_name(t) for _, _, ts in TYPES for t in ts}


def block(src, name):
    m = re.search(r"((?:^[ \t]*///[^\n]*\n)*)(?:^[ \t]*#\[[^\n]*\n)*^pub (struct|enum) " + name + r" \{", src, re.M)
    if not m:
        raise SystemExit(f"type {name} not found")
    docs, kind = m.group(1), m.group(2)
    i = m.end()
    depth = 1
    while depth:
        c = src[i]
        depth += c == "{"
        depth -= c == "}"
        i += 1
    body = src[m.end():i - 1]
    lines = [l for l in body.split("\n") if not l.strip().startswith("#[")]
    names = "|".join(sorted(ALL, key=len, reverse=True))

    def types(text):
        # Paths into the core (`crate::spaces::sidebar::Fact`) become the mirror.
        text = re.sub(r"(?:crate|super)::(?:[a-z_]+::)*(" + names + r")\b", r"\1", text)
        return re.sub(r"\b(" + names + r")\b(?!::)", lambda mm: ALL[mm.group(1)], text)

    out_lines = []
    for l in lines:
        m2 = re.match(r"^(\s*)([A-Z][A-Za-z0-9]*)(\s*[,{(].*|\s*)$", l)
        if m2 and not l.strip().startswith("///"):
            # An enum variant: keep its name, map the types after it.
            out_lines.append(m2.group(1) + m2.group(2) + types(m2.group(3)))
        elif l.strip().startswith("///"):
            out_lines.append(l)
        else:
            out_lines.append(types(l))
    return docs, kind, "\n".join(out_lines)


out = [
    # cua-spaces-ffi is FSL-1.1-MIT (see LICENSING.md and scripts/spdx-headers.py).
    "// SPDX-License-Identifier: FSL-1.1-MIT",
    "// Copyright (c) 2026 Cua AI, Inc.",
    "",
    "// Generated by libs/cua/scripts/generate-app-core-ffi.py; do not edit.",
    "//! The app core's records, mirrored for UniFFI (`#[uniffi::remote]`).",
    "//! The compiler checks each mirror against `cua-spaces-app-core`.",
    "",
    "#![allow(missing_docs)]",
    "",
    "use cua_spaces_app_core as core;",
    "",
]
for file, module, types in TYPES:
    src = (CORE / file).read_text()
    for t in types:
        docs, kind, body = block(src, t)
        n = ALL[t]
        out.append(docs.rstrip("\n") if docs else f"/// `{module}::{t}`.")
        out.append(f"pub type {n} = core::{module}::{t};")
        out.append(f"#[uniffi::remote({'Record' if kind == 'struct' else 'Enum'})]")
        out.append(f"pub {kind} {n} {{{body}}}")
        out.append("")
text = "\n".join(out)
if "--check" in sys.argv:
    if not OUT.exists() or OUT.read_text() != text:
        raise SystemExit(f"{OUT} is stale; run {sys.argv[0]}")
    print("app core FFI mirrors are current")
else:
    OUT.write_text(text)
    print(f"wrote {OUT}")
