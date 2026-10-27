// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The shape of every answer that crosses the bridge, defined once.
 *
 * - `OP_SHAPES`: each operation's result as the page gets it
 *   (`OpResult<op>`, after the adapter). Checked against the demo host
 *   (`__tests__/contract.test.ts`), the Electron router's live host and its
 *   demo path (`apps/cua-spaces-desktop/test/contract.test.ts`), and
 *   inside the Electron shell (`e2e/electron-bridge.spec.ts`).
 * - `WEBKIT_SHAPES`: what the SwiftUI host answers each method with, before
 *   `adapters/webkit.ts` maps it: the shapes that adapter reads. Checked
 *   against `WebUIBridge` on fixture backends
 *   (`apps/cua-spaces-macos/Tests/CuaSpacesMacTests/BridgeContractTests.swift`).
 *   Methods without a shape say why in `WEBKIT_UNSHAPED`.
 *
 * The builders check each object against its contract type, so a field
 * added to a contract fails to compile until its shape says what it is.
 * `bridge-shapes.json` is the exported document (JSON Schema keywords,
 * `$defs` for the named shapes); `pnpm contract:shapes` writes it, and the
 * contract test fails when it is out of date.
 */

import type { AgentDetailInput, AgentEventsPage, AgentOpenFile, AgentSetupRow, ConnectionReport, ConnectionRow, ConnectResult, PersistentAgent, SpaceAgentRun } from "./agents";
import type { DeviceInput, DevicesInput, EnrollResult, MachineAccessNotice, PairingCode, TeamChoice, TeamsList } from "./devices";
import type {
  CancelOutcome,
  DaemonStatus,
  DefaultLocation,
  AccountStatus,
  HostStatus,
  MachineRow,
  OnboardingState,
  SignInStart,
  SpacePowerReport,
  TelemetryView,
  CrashReportingConfig,
  DetectedRuntime,
  RuntimeKindInput,
  RuntimePathCheck,
  RuntimesInput,
} from "./host";
import type { KeyvaultOverview, KvGrant } from "./keyvault";
import type {
  CloudCheckWire,
  CloudDisconnectedWire,
  CloudProviderWire,
  CloudTestWire,
  ConnectedCloud,
  GpuChoice,
  LocalStatus,
  RuntimeFix,
  LocalStorage,
  NewSpaceOptions,
  StorageVolume,
  WizardEnv,
} from "./new-space";
import type { NotificationInput } from "./notifications";
import type { AboutInput, DriveCheckInput, DriveStorageInput, LoginItemReport, StorageInput, UpdateState } from "./settings";
import type { ShareEntry, ShareRemoval } from "./share";
import type { BulkResult } from "./spaces-page";
import type { CreateFix, CreatingElsewhere, SpaceAccess, SpaceProgress, SpaceRow, SpaceSharingFacts } from "./spaces";
import type { CatalogEntry, ConsentItem, KvDomainCount, KvInventory, OpenWindow, PlanStep, RemoteWindow, RunReport, TeleportPlan } from "./teleport";
import type { AgentSetupOutcome, DriveGrantInput, DriveMountInput, DriveRequestInput, Experiments, VolumeOverview } from "./volume";
import type { AgentKeysReport } from "../ops/agent-keys";
import type { TelemetryLinkNotice } from "../ops/telemetry";
import type { SentFileInfo, SpaceThumbnail, SpaceUsage, SpaceWindows } from "../ops/space-detail";
import type { RelayUsage } from "../ops/relay-usage";
import type { RiskyRequest } from "../ops/nudges";
import type { ComputerResources, DiskOnHost, DiskWarning, GuestUsage, ResourcesSnapshot, SharedBase, SpaceResources } from "../ops/resources";
import type { HostChecklistState } from "../ops/checklist";
import type { TipsState } from "./spaces-page";
import type { ShareLink } from "../ops/links";
import type { CliInstallPlan } from "./first-run";
import type { AgentMember, JoinCode, JoinCodeRow, MemberIdentity, ThreadAttachment, ThreadAuthor, ThreadEntry, ThreadPage } from "../ops/thread";
import type { PresenceCursorWire, PresenceJoinInfo, PresenceParticipantWire } from "../ops/presence";
import type { StartupState } from "../ops/startup";
import type { StreamTicket } from "../ops/stream";
import type { CliInfo, GlobalShortcut, HostSettings, HostSignIn, HostSignInFailure, OpName, SessionSnapshot, SettingsSnapshot, SettingsValues } from "../protocol";
import type { WebkitMethod, WkHostState, WkMachines, WkOnboarding, WkSession, WkSettings, WkSettingsRow, WkSpace, WkSpaces } from "../webkit-protocol";
import { Defs, any, bool, dict, list, nul, nullable, num, obj, object, oneOf, opt, optNull, str, union, type Schema } from "./schema";

const defs = new Defs();
const ref = (name: string, s: Schema) => defs.ref(name, s);
const strings = list(str);
const limits = list(object({ resource: str, used: num, limit: num, reason: str }));
const os = oneOf("macos", "windows", "linux", "unknown");

/* ---- Spaces and machines --------------------------------------------------- */

const CliInstallPlanShape = ref(
  "CliInstallPlan",
  obj<CliInstallPlan>()({
    source: optNull(str),
    target: str,
    binDir: str,
    method: optNull(oneOf("symlink", "copy")),
    installed: bool,
    upToDate: bool,
    installedVersion: optNull(str),
    bundledVersion: optNull(str),
    shadowedBy: optNull(str),
    onPath: bool,
    pathProfile: optNull(str),
    pathLine: optNull(str),
    found: optNull(str),
  }),
);

const DiskWarningShape = ref("DiskWarning", obj<DiskWarning>()({ level: oneOf("ok", "low", "critical"), free: num, total: num, threshold: num }));

/** `resources.get`'s computers (CUA-1880). */
const ComputerResourcesShape = ref(
  "ComputerResources",
  obj<ComputerResources>()({
    id: str,
    name: str,
    online: bool,
    sampledAtMs: num,
    cpuCount: num,
    cpuPercent: num,
    loadAverage1m: num,
    memoryTotal: num,
    memoryUsed: num,
    diskTotal: num,
    diskFree: num,
    spacesCount: num,
    spacesRunning: num,
    spacesCpuAllocated: num,
    spacesMemoryAllocated: num,
    spacesMemoryUsed: num,
    spacesDiskBytes: num,
    spacesDiskExact: bool,
    warning: DiskWarningShape,
  }),
);

const CreateFixShape = ref(
  "CreateFix",
  obj<CreateFix>()({ text: str, label: str, action: oneOf("copy-command", "open-url", "details"), value: opt(str) }),
);

const SpaceProgressShape = ref(
  "SpaceProgress",
  obj<SpaceProgress>()({
    phase: str,
    permille: num,
    label: str,
    error: opt(str),
    transfer: opt(str),
    cancellable: opt(bool),
    cancelling: opt(bool),
    retrying: opt(bool),
    fix: opt(CreateFixShape),
  }),
);

const SpaceSharingShape = ref(
  "SpaceSharingFacts",
  obj<SpaceSharingFacts>()({
    ownerId: str,
    ownerEmail: optNull(str),
    ownerName: optNull(str),
    role: str,
    team: optNull(str),
    teamName: optNull(str),
    editors: opt(strings),
    viewers: opt(strings),
    teams: opt(list(object({ key: str, name: str, role: str }))),
  }),
);

const SpaceAccessShape = ref(
  "SpaceAccess",
  obj<SpaceAccess>()({
    kind: oneOf("mine", "withMe", "team"),
    role: oneOf("owner", "editor", "viewer"),
    ownerName: optNull(str),
    ownerEmail: optNull(str),
    teamName: optNull(str),
    place: str,
    people: list(object({ who: str, name: optNull(str), role: oneOf("editor", "viewer"), team: opt(bool) })),
    canPower: bool,
    canTeleport: bool,
    canShare: bool,
    canRename: bool,
    canDelete: bool,
    canLeave: bool,
  }),
);

const BulkResultsShape = ref(
  "BulkResults",
  object({ results: list(obj<BulkResult>()({ id: str, ok: bool, error: opt(str) })) }),
);

const SpaceRowShape = ref(
  "SpaceRow",
  obj<SpaceRow>()({
    id: str,
    hostProgress: opt(SpaceProgressShape),
    name: str,
    provider: oneOf("local", "direct", "relay"),
    spacesdVersion: str,
    features: strings,
    addedAt: opt(str),
    os: opt(os),
    osName: opt(str),
    osPrettyName: opt(str),
    image: opt(str),
    imageDigest: opt(str),
    kind: opt(oneOf("container", "vm")),
    arch: opt(str),
    reachable: bool,
    error: opt(str),
    host: opt(str),
    hostName: opt(str),
    power: opt(str),
    powerState: opt(str),
    stopReason: opt(str),
    cloud: opt(str),
    cloudPlace: opt(str),
    cloudDelete: opt(str),
    sharing: opt(SpaceSharingShape),
  }),
);

const HostStatusShape = ref(
  "HostStatus",
  obj<HostStatus>()({
    configured: bool,
    mode: optNull(oneOf("relay", "direct")),
    relayUrl: optNull(str),
    directUrl: optNull(str),
    machineId: optNull(str),
    name: optNull(str),
    sharing: bool,
    service: object({ installed: bool, running: bool, kind: str, detail: opt(str) }),
    online: optNull(bool),
    clients: list(object({ id: str, email: opt(str), name: opt(str), streams: opt(num), since: opt(num) })),
    permissions: list(object({ id: str, label: str, instructions: opt(str), settingsUrl: opt(str), granted: opt(bool) })),
    error: optNull(str),
    shareDesktop: opt(bool),
    provideSpaces: opt(bool),
    maxSpaces: opt(num),
    maxMacosVms: opt(num),
    recentAccess: opt(list(object({ atMs: num, via: str, who: str, what: str }))),
    accessLogError: optNull(str),
    providedSpaces: opt(list(any)),
    spacesAudit: opt(list(any)),
    spacesAuditError: optNull(str),
    pausedSignedOut: opt(bool),
    owner: optNull(str),
    ownerEmail: optNull(str),
    account: optNull(object({ id: optNull(str), email: optNull(str), display: optNull(str) })),
    progress: optNull(str),
    appName: optNull(str),
  }),
);

const MachineAccessNoticeShape = ref(
  "MachineAccessNotice",
  obj<MachineAccessNotice>()({
    kind: oneOf("enrolled", "grace", "needs-enrollment", "waiting", "due", "revoked"),
    status: str,
    text: str,
    actionLabel: str,
  }),
);

const MachineRowShape = ref(
  "MachineRow",
  obj<MachineRow>()({
    id: str,
    name: str,
    via: str,
    online: bool,
    os: str,
    limits,
    current: opt(bool),
    model: opt(str),
    arch: opt(str),
    detail: opt(str),
    lastSeen: optNull(num),
    host: optNull(HostStatusShape),
    device: opt(bool),
    deviceState: opt(str),
    hostname: opt(str),
    presence: optNull(bool),
    accessNotice: optNull(MachineAccessNoticeShape),
    merged: opt(bool),
    status: opt(str),
    spacesLabel: opt(str),
  }),
);

/* ---- Session and settings --------------------------------------------------- */

const SessionShape = ref(
  "SessionSnapshot",
  obj<SessionSnapshot>()({
    account: obj<AccountStatus>()({
      configured: bool,
      authMode: oneOf("user", "client-credentials", "static-token", "none"),
      clientId: opt(str),
      identity: opt(str),
    }),
    onboarding: obj<OnboardingState>()({
      completed: bool,
      mode: optNull(oneOf("client", "host")),
      installerMode: optNull(oneOf("client", "host")),
    }),
    daemon: nullable(
      obj<DaemonStatus>()({ connected: bool, version: opt(str), socketPath: opt(str), loopbackUrl: opt(str), error: opt(str) }),
    ),
    hostSignIn: optNull(obj<HostSignIn>()({ userCode: nullable(str), url: nullable(str), opened: opt(bool) })),
    hostSignInFailure: optNull(obj<HostSignInFailure>()({ reason: str, message: str, detail: nullable(str) })),
  }),
);

/** `AppSettings.tips` (`tips::TipsState`). */
const TipsStateShape = obj<TipsState>()({ off: bool, done: strings });

const RuntimePathCheckShape = ref(
  "RuntimePathCheck",
  obj<RuntimePathCheck>()({
    path: str,
    exists: bool,
    executable: bool,
    tool: optNull(str),
    name: optNull(str),
    version: optNull(str),
    minVersion: optNull(str),
    expected: strings,
    guests: optNull(str),
  }),
);

const RuntimesInputShape = ref(
  "RuntimesInput",
  obj<RuntimesInput>()({
    os: optNull(os),
    hostArch: optNull(str),
    localBackends: optNull(strings),
    localDetails: optNull(any),
    kinds: optNull(
      list(
        obj<RuntimeKindInput>()({
          kind: str,
          source: optNull(str),
          path: optNull(str),
          detected: optNull(
            list(obj<DetectedRuntime>()({ tool: str, name: str, path: str, version: optNull(str), guests: optNull(str) })),
          ),
          check: optNull(RuntimePathCheckShape),
          expected: opt(strings),
        }),
      ),
    ),
  }),
);

const SettingsShape = ref(
  "SettingsSnapshot",
  obj<SettingsSnapshot>()({
    values: obj<SettingsValues>()({
      theme: oneOf("system", "light", "dark"),
      menuBar: bool,
      hotkey: str,
      telemetry: bool,
      defaultLocation: str,
      launchAtLogin: nullable(bool),
      updateChannel: nullable(oneOf("stable", "beta")),
      autoConnect: opt(bool),
      tips: opt(TipsStateShape),
    }),
    defaultLocation: nullable(obj<DefaultLocation>()({ value: str, source: oneOf("env", "config", "default"), env: opt(str), path: str })),
    telemetry: nullable(
      obj<TelemetryView>()({ enabled: bool, source: str, sourceKind: str, noticeShown: bool, noticeText: str, docsUrl: str }),
    ),
    hostSettings: opt(
      obj<HostSettings>()({
        runtimes: optNull(RuntimesInputShape),
        autoConnect: optNull(bool),
        overflow: optNull(bool),
        keyvaultAutoWipe: optNull(bool),
        keyvaultUnlockPrompt: optNull(bool),
        keyvaultSiteIcons: optNull(bool),
        keyvaultProtection: optNull(list(obj<{ label: string; value: string }>()({ label: str, value: str }))),
      }),
    ),
    globalShortcut: optNull(obj<GlobalShortcut>()({ label: str, registered: bool })),
    accountUrl: optNull(str),
    cli: optNull(obj<CliInfo>()({ name: str, installed: bool })),
  }),
);

const ExperimentsShape = ref(
  "Experiments",
  obj<Experiments>()({ cuaVolume: opt(bool), yourCloud: opt(bool), sharing: opt(bool), webUi: opt(bool) }),
);

const AboutShape = ref(
  "AboutInput",
  obj<AboutInput>()({
    platform: str,
    version: str,
    build: str,
    os: str,
    updater: bool,
    autoCheck: bool,
    autoInstall: bool,
    channel: oneOf("stable", "beta"),
    lastCheck: nullable(str),
    checking: bool,
  }),
);

const UpdateShape = ref(
  "UpdateState",
  obj<UpdateState>()({
    status: oneOf("idle", "checking", "available", "downloading", "downloaded", "error"),
    currentVersion: str,
    version: nullable(str),
    percent: nullable(num),
    notesUrl: nullable(str),
    message: nullable(str),
  }),
);

const LoginItemShape = ref(
  "LoginItemReport",
  obj<LoginItemReport>()({ status: oneOf("enabled", "notRegistered", "requiresApproval", "notFound"), providesSpaces: bool, runsAgents: bool }),
);

const DevicesShape = ref(
  "DevicesInput",
  obj<DevicesInput>()({
    devices: opt(
      list(
        obj<DeviceInput>()({
          id: str,
          name: opt(str),
          state: opt(str),
          enrolledUntil: optNull(num),
          lastSeen: optNull(num),
          current: opt(bool),
          platform: optNull(str),
        }),
      ),
    ),
    audit: opt(list(object({ ts: num, kind: str }))),
    localDeviceId: optNull(str),
    pendingCode: optNull(str),
    enforceAfter: optNull(num),
    machineNames: opt(dict(str)),
    machines: opt(list(object({ id: str, name: opt(str), confirmed: opt(bool) }))),
    readError: optNull(str),
    deviceName: optNull(str),
  }),
);

const PairingCodeShape = ref(
  "PairingCode",
  obj<PairingCode>()({
    id: str,
    code: nullable(str),
    url: nullable(str),
    installSh: nullable(str),
    installPowershell: nullable(str),
    state: oneOf("active", "used", "revoked", "expired"),
    team: nullable(str),
    createdAt: num,
    expiresAt: num,
    usedAt: nullable(num),
    machine: nullable(str),
  }),
);

const TeamsListShape = ref(
  "TeamsList",
  obj<TeamsList>()({
    teams: list(
      obj<TeamChoice>()({ id: str, alias: str, name: str, role: str, plan: str }),
    ),
    currentTeamId: nullable(str),
  }),
);

const DriveMountShape = ref(
  "DriveMountInput",
  obj<DriveMountInput>()({
    enabled: opt(bool),
    state: str,
    method: str,
    path: optNull(str),
    volume_name: opt(str),
    detail: optNull(str),
    settings_url: optNull(str),
    volume_errors: opt(list(object({ space: str, error: str }))),
  }),
);

const DriveStorageShape = ref(
  "DriveStorageInput",
  obj<DriveStorageInput>()({ backend: str, fs_path: str, s3: optNull(any), has_keys: bool }),
);

const DriveCheckShape = ref(
  "DriveCheckInput",
  obj<DriveCheckInput>()({ ok: bool, reachable: bool, authorized: bool, versioning: bool, detail: optNull(str), applied: bool }),
);

const StorageShape = ref(
  "StorageInput",
  obj<StorageInput>()({
    os,
    home: nullable(str),
    storage: nullable(DriveStorageShape),
    mount: nullable(DriveMountShape),
    cache: nullable(object({ size_bytes: num, capacity_bytes: num })),
  }),
);

const VolumeShape = ref(
  "VolumeOverview",
  obj<VolumeOverview>()({
    os,
    home: nullable(str),
    requests: list(obj<DriveRequestInput>()({ id: str, principal: str, prefix: str, mode: str, reason: str })),
    grants: list(obj<DriveGrantInput>()({ id: str, principal: str, prefix: str, mode: str, revoked: opt(bool) })),
    mount: nullable(DriveMountShape),
    sync: nullable(object({ device_id: str, device_name: str, feed: str })),
  }),
);

const NotificationShape = ref(
  "NotificationInput",
  obj<NotificationInput>()({ id: str, atMs: num, agent: optNull(str), kind: str, title: str, body: str, read: opt(bool), space: optNull(str) }),
);

const AgentKeysShape = ref(
  "AgentKeysReport",
  obj<AgentKeysReport>()({
    keys: list(object({ provider: oneOf("anthropic", "openai", "other"), env: str, last4: str, addedMs: num })),
    providers: list(object({ id: oneOf("anthropic", "openai", "other"), label: str, env: str })),
    available: bool,
    unavailable: nullable(str),
  }),
);

const StartupShape = ref(
  "StartupState",
  obj<StartupState>()({
    phase: oneOf("starting", "needsKeychain", "waitingForKeychain", "keychainDenied", "startFailed", "ready"),
    slow: bool,
    title: str,
    body: str,
    actions: list(oneOf("allowAccess", "tryAgain", "signInAgain")),
  }),
);

const LinkNoticeShape = ref(
  "TelemetryLinkNotice",
  obj<TelemetryLinkNotice>()({
    due: bool,
    title: str,
    body: str,
    okLabel: str,
    turnOffLabel: str,
    detailsLabel: str,
    docsUrl: str,
  }),
);

const CrashReportingConfigShape = ref(
  "CrashReportingConfig",
  obj<CrashReportingConfig>()({
    allowed: bool,
    dsn: str,
    release: str,
    environment: str,
    shell: str,
    crashTest: opt(oneOf("ui")),
  }),
);

/* ---- New Space ------------------------------------------------------------- */

const StorageVolumeShape = obj<StorageVolume>()({ availableBytes: num, totalBytes: num, name: str });
const LocalStorageShape = ref(
  "LocalStorage",
  obj<LocalStorage>()({
    reserveBytes: num,
    linuxRuntimeDownloadBytes: opt(num),
    lume: nullable(StorageVolumeShape),
    qemu: nullable(StorageVolumeShape),
    container: nullable(StorageVolumeShape),
    pulled: strings,
  }),
);
const GpuShape = ref(
  "GpuChoice",
  obj<GpuChoice>()({ runtime: str, id: str, label: str, experimental: bool, supported: bool, reason: optNull(str), learnMore: optNull(str) }),
);
const LocalStatusShape = ref(
  "LocalStatus",
  obj<LocalStatus>()({
    available: bool,
    backends: strings,
    containerImage: opt(str),
    macosImage: optNull(str),
    error: nullable(str),
    fix: optNull(
      ref(
        "RuntimeFix",
        obj<RuntimeFix>()({ action: nullable(oneOf("install-docker", "start-docker")), label: str, note: nullable(str), busy: bool, termsUrl: nullable(str) }),
      ),
    ),
    hostArch: opt(str),
    storage: optNull(LocalStorageShape),
  }),
);
const WizardEnvShape = ref(
  "WizardEnv",
  obj<WizardEnv>()({
    defaultLocation: oneOf("local", "yours", "host"),
    localAvailable: bool,
    localReason: optNull(str),
    localBackends: optNull(strings),
    localDetails: optNull(dict(str)),
    maxCpus: num,
    hostArch: optNull(str),
    storage: optNull(LocalStorageShape),
    clouds: opt(
      list(
        obj<ConnectedCloud>()({
          name: str,
          title: str,
          label: str,
          isDefault: bool,
          ttlHours: num,
          offers: list(object({ image: str, kind: str, supported: bool })),
        }),
      ),
    ),
    hosts: opt(list(object({ id: str, name: str, via: str, online: bool, os: str, limits }))),
    experiments: opt(ExperimentsShape),
    gpus: optNull(list(GpuShape)),
    nameSeed: optNull(num),
  }),
);
const createOptions = (strict: boolean) =>
  obj<NewSpaceOptions>()({
    local: strict ? LocalStatusShape : nullable(LocalStatusShape),
    gpus: nullable(list(GpuShape)),
    experiments: ExperimentsShape,
    maxCpus: nullable(num),
    // The SwiftUI host's wizard runs on its env: there it is required.
    env: strict ? (WizardEnvShape as never) : optNull(WizardEnvShape),
    macosVmsRunning: optNull(num),
    pending: opt(bool),
  });
const NewSpaceOptionsShape = ref("NewSpaceOptions", createOptions(false));

const CloudCheckShape = obj<CloudCheckWire>()({ name: str, ok: bool, detail: opt(str) });
const CloudProviderShape = ref(
  "CloudProviderWire",
  obj<CloudProviderWire>()({
    name: str,
    title: str,
    tier: str,
    connected: bool,
    default: opt(bool),
    credentials: opt(object({ found: bool, source: opt(str) })),
    account: opt(str),
    profile: opt(str),
    region: opt(str),
    zone: opt(str),
    project: opt(str),
    environment: opt(str),
    label: opt(str),
    ttl_hours: opt(num),
    kinds: opt(list(object({ image: str, kind: str, supported: bool }))),
    // What `cloud_connect` with limits adds (per-connection budget and size).
    auth: opt(str),
    status: opt(str),
    budget_usd_month: opt(num),
    max_size: opt(str),
    spent_month: opt(num),
    running: opt(num),
    approval: opt(str),
    holders: opt(strings),
    approval_agent: opt(str),
    pending_budget_usd_month: opt(nullable(num)),
    pending_max_size: opt(str),
    can_approve: opt(bool),
    suggest_auth: opt(str),
    issue: opt(str),
  }),
);

/* ---- Teleport, sharing, a Space's detail ------------------------------------ */

const teleportMove = oneOf("app_only", "app_with_files", "app_with_state");

const CatalogEntryShape = ref(
  "CatalogEntry",
  obj<CatalogEntry>()({
    id: str,
    name: str,
    hostPath: nullable(str),
    hostAppId: nullable(str),
    version: nullable(str),
    capability: oneOf("full", "install_only", "unsupported"),
    reason: nullable(str),
    moves: list(teleportMove),
    providerId: nullable(str),
    sensitiveGroups: opt(list(oneOf("sign_ins", "passwords", "history"))),
    installSource: nullable(str),
    installId: nullable(str),
    installVersion: nullable(str),
    launchBin: nullable(str),
    lastUsedMs: nullable(num),
    json: str,
  }),
);
const RemoteWindowShape = ref(
  "RemoteWindow",
  obj<RemoteWindow>()({
    id: str,
    appName: str,
    title: str,
    visible: bool,
    appId: str,
    targetEpoch: num,
    widthPx: optNull(num),
    heightPx: optNull(num),
    pid: optNull(num),
  }),
);
const ShareShape = ref("ShareEntry", obj<ShareEntry>()({ who: str, role: str, connected: opt(bool) }));
const AgentSetupRowShape = ref(
  "AgentSetupRow",
  obj<AgentSetupRow>()({
    agent: str,
    name: str,
    installed: bool,
    configured: bool,
    detail: str,
    skillsInstalled: num,
    skillsTotal: num,
    mcpConfig: nullable(str),
    skillsDir: nullable(str),
    signIn: nullable(str),
    signedIn: bool,
  }),
);

/* ---- Keyvault ----------------------------------------------------------------- */

const KeyvaultShape = ref(
  "KeyvaultOverview",
  obj<KeyvaultOverview>()({
    availability: str,
    message: opt(str),
    status: opt(any),
    serverVerified: bool,
    items: list(object({ id: str, kind: any, provider_id: str, app_display: str, key: str })),
    namesVisible: bool,
    itemsTotal: num,
    pending: list(object({ id: str })),
    grants: list(object({ id: str })),
    rules: list(any),
    deliveries: list(any),
    audit: list(any),
    auditVerification: opt(any),
    partialErrors: strings,
    dismissed: opt(strings),
  }),
);
const KvGrantShape = ref(
  "KvGrant",
  obj<KvGrant>()({
    id: str,
    request_id: str,
    caller_fp: str,
    caller_display: str,
    items: strings,
    targets: strings,
    actions: strings,
    created_ms: num,
    not_after_ms: num,
    uses_left: optNull(num),
    revoked: bool,
    agent: optNull(str),
  }),
);

/* ---- Every operation ---------------------------------------------------------- */

const hostStatusOrNull = HostStatusShape;
const agentSetupRows = list(AgentSetupRowShape);
/** `agents.status` (CONTRACT §4; snake_case, as the core serializes it). */
const ConnectionReportShape = ref(
  "ConnectionReport",
  obj<ConnectionReport>()({
    agents: list(
      ref(
        "ConnectionRow",
        obj<ConnectionRow>()({
          id: str,
          name: str,
          installed: bool,
          version: nullable(str),
          previous_version: nullable(str),
          signed_in: oneOf("yes", "no", "unknown", "expired"),
          sign_in: nullable(str),
          mcp: oneOf("ok", "missing", "broken", "stale", "unsupported"),
          mcp_config: nullable(str),
          signin_imported: bool,
          signin_import: oneOf("ok", "expired", "missing", "unsupported"),
          state: oneOf("connected", "attention", "not_connected", "not_installed"),
          problem: nullable(object({ line: str, action: oneOf("reconnect", "sign_in", "install", "disconnect", "none") })),
          checked_at_ms: num,
        }),
      ),
    ),
    checked_at_ms: num,
  }),
);
const ConnectResultsShape = list(
  ref(
    "ConnectResult",
    obj<ConnectResult>()({
      agent: str,
      tools_ok: bool,
      tools_line: nullable(str),
      sign_in: nullable(
        object({
          imported: nullable(object({ source: str, env: str, label: str, expires_ms: nullable(num), added_ms: num })),
          refused: nullable(str),
          line: nullable(str),
        }),
      ),
    }),
  ),
);
const DisconnectShape = object({ removed_sign_ins: list(str) });
const AgentDetailShape = ref(
  "AgentDetailInput",
  obj<AgentDetailInput>()({
    home: list(object({ path: str, name: str, size: num })),
    routines: list(object({ id: str, title: str, label: str, enabled: bool })),
    grants: list(object({ agent: str, machine: str, revoked: bool })),
    audit: list(object({ tsMs: num, action: str, principal: str, path: str, detail: str })),
    thisMachine: nullable(str),
  }),
);
const AgentOpenFileShape = ref(
  "AgentOpenFile",
  obj<AgentOpenFile>()({
    path: str,
    text: str,
    binary: bool,
    versions: list(object({ version: str, modifiedMs: num, deleted: bool, latest: bool })),
  }),
);
const shares = list(ShareShape);
const ShareRemovalShape = ref("ShareRemoval", obj<ShareRemoval>()({ who: str, machineName: str }));
const shareRemovals = list(ShareRemovalShape);
const PresenceParticipantShape = ref(
  "PresenceParticipant",
  obj<PresenceParticipantWire>()({ participantId: str, principalId: str, displayName: str, color: str, kind: str }),
);
const PresenceCursorShape = ref(
  "PresenceCursor",
  obj<PresenceCursorWire>()({
    displayId: str,
    windowId: opt(str),
    x: num,
    y: num,
    visible: bool,
    pressed: bool,
    shape: str,
    shapeSource: str,
    atMs: num,
    receivedMs: num,
  }),
);

/** The host's copy of the checklist (`ops/checklist.ts`). */
const ChecklistAnswerShape = object({
  state: obj<HostChecklistState>()({ done: strings, dismissed: bool, shown: bool, skipped: strings }),
});

/** A link to something you run (`ops/links.ts`). */
const ShareLinkShape = obj<ShareLink>()({
  id: str,
  url: str,
  name: nullable(str),
  access: oneOf("private", "team", "public"),
  kind: str,
  target: str,
  owner: str,
  createdAtUnix: nullable(num),
  expiresAtUnix: nullable(num),
});

/** An agent member's identity (CUA-2262, `ops/thread.ts`). */
const memberRef = object({ id: str, name: str });
const MemberIdentityShape = ref(
  "MemberIdentity",
  obj<MemberIdentity>()({
    origin: oneOf("space", "external"),
    handle: str,
    displayName: str,
    collides: bool,
    icon: nullable(str),
    harness: object({ id: str, version: nullable(str), reported: bool }),
    owner: memberRef,
    device: nullable(memberRef),
    joined: object({ method: oneOf("started", "prompt", "setup"), atMs: nullable(num), by: nullable(memberRef) }),
    state: oneOf("active", "pending"),
    lastSeenMs: nullable(num),
  }),
);

/** One Space thread entry as the page reads it (`ops/thread.ts`, DESIGN §3.2). */
const taskState = oneOf("queued", "claimed", "working", "done", "failed");
const ThreadEntryShape = ref(
  "ThreadEntry",
  obj<ThreadEntry>()({
    seq: num,
    id: str,
    tsMs: num,
    author: obj<ThreadAuthor>()({ kind: oneOf("person", "agent", "system"), id: str, name: str, identity: opt(nullable(MemberIdentityShape)) }),
    kind: oneOf("message", "task_update", "approval", "answer", "system", "join_request"),
    text: str,
    mentions: strings,
    replyTo: nullable(num),
    task: nullable(object({ to: str, session: nullable(num) })),
    attachments: list(obj<ThreadAttachment>()({ path: str, name: str, size: nullable(num), mime: nullable(str) })),
    approval: nullable(object({ action: str, details: str, state: str })),
    answer: nullable(object({ approval: num, approve: bool, note: nullable(str) })),
    noteFor: nullable(num),
    update: nullable(object({ task: num, state: taskState, session: nullable(num), note: nullable(str) })),
    runsOnKeysOf: nullable(object({ id: str, name: str })),
    event: nullable(object({ kind: oneOf("member_joined", "member_removed", "name_refused", "rate_limited"), member: nullable(MemberIdentityShape), by: nullable(memberRef) })),
    joinRequest: opt(
      nullable(
        object({ member: nullable(MemberIdentityShape), canTask: oneOf("owner", "members"), state: str, codeId: str, youCanAnswer: bool }),
      ),
    ),
    view: opt(nullable(object({ kind: oneOf("desktop", "window"), app: nullable(str), title: nullable(str), windowId: nullable(str) }))),
  }),
);

/** A join code (CUA-2263): its plaintext only on create; the list never has it. */
const JoinCodeShape = obj<JoinCode>()({ id: str, code: str, url: str, line: str, expiresAtMs: num });
const JoinCodeRowShape = obj<JoinCodeRow>()({
  id: str,
  createdAtMs: num,
  expiresAtMs: num,
  state: oneOf("open", "pending_approval", "redeemed", "denied", "expired", "cancelled"),
  canTask: oneOf("owner", "members"),
  askBeforeJoin: bool,
  createdBy: nullable(memberRef),
  memberHandle: nullable(str),
});
const ThreadPageShape = obj<ThreadPage>()({
  entries: list(ThreadEntryShape),
  cursor: num,
  caughtUp: bool,
  unread: num,
  readSeq: nullable(num),
  me: nullable(str),
  hasMoreBefore: bool,
  role: nullable(oneOf("owner", "admin", "member", "guest", "agent")),
});
const rightHolders = union(oneOf("owner", "admins", "members"), strings);
const AgentMemberShape = ref(
  "AgentMember",
  obj<AgentMember>()({
    name: str,
    harness: str,
    status: str,
    activity: nullable(str),
    sessions: num,
    rights: nullable(object({ task: rightHolders, control: rightHolders, approve: rightHolders })),
    youCan: nullable(object({ task: bool, control: bool, approve: bool })),
    heldBy: nullable(str),
    identity: nullable(MemberIdentityShape),
    dm: opt(nullable(object({ unread: num, last: nullable(object({ seq: num, tsMs: num, text: str, author: object({ kind: oneOf("person", "agent", "system"), id: str, name: str, identity: opt(nullable(MemberIdentityShape)) }) })) }))),
    needsYou: opt(list(object({ seq: num, kind: oneOf("approval", "question"), text: str, tsMs: num }))),
  }),
);

/** One Machines row as the core lists it (`machines::MachinePageRow`). */
const MachinePageRowShape = ref(
  "MachinePageRow",
  object({
    id: str,
    name: str,
    online: bool,
    os: str,
    via: str,
    current: bool,
    device: nullable(str),
    deviceOnly: bool,
    spaceId: nullable(str),
    stoppedSharing: bool,
    lastSeen: nullable(num),
    status: str,
    spaces: num,
    spacesLabel: str,
  }),
);

const HostConfirmShape = object({ title: str, message: str, confirmLabel: str, cancelLabel: str });

/** A machine's remove (`machines_view::MachinesViewRemove`). */
const MachinesViewRemoveShape = ref(
  "MachinesViewRemove",
  object({
    label: str,
    enabled: bool,
    help: optNull(str),
    host: bool,
    machine: optNull(str),
    device: optNull(str),
    confirm: HostConfirmShape,
  }),
);

/** The Machines page as the core draws it (`machines_view::MachinesView`). */
const MachinesViewShape = ref(
  "MachinesView",
  object({
    summary: str,
    groups: list(
      object({
        id: str,
        title: str,
        collapsed: bool,
        ids: strings,
        removeAll: optNull(object({ label: str, confirm: HostConfirmShape, removes: list(MachinesViewRemoveShape) })),
      }),
    ),
    machines: list(
      object({
        id: str,
        name: str,
        badge: optNull(str),
        line: str,
        online: bool,
        // The machine's issues (`machines_health`): a host that predates
        // them sends none.
        health: opt(str),
        healthTarget: optNull(str),
        issuesTitle: opt(str),
        issues: opt(
          list(
            object({
              id: str,
              level: str,
              text: str,
              action: optNull(object({ label: str, url: optNull(str), hostAction: optNull(str), appUpdate: opt(bool) })),
            }),
          ),
        ),
        os: str,
        current: bool,
        team: bool,
        status: str,
        sharedBy: optNull(str),
        rename: optNull(object({ target: str, id: str, title: str, confirmLabel: str })),
        notice: optNull(str),
        noticeAction: optNull(object({ id: str, label: str, destructive: bool })),
        toggles: list(
          object({
            id: str,
            label: str,
            help: str,
            on: bool,
            editable: bool,
            enabled: bool,
            actions: strings,
            setup: optNull(str),
          }),
        ),
        togglesNote: optNull(str),
        spacesTitle: str,
        spaces: list(object({ id: str, name: str, os: str, status: str, running: bool })),
        desktop: optNull(object({ id: str, name: str, os: str, status: str, running: bool, label: str })),
        spacesEmpty: optNull(str),
        newSpace: object({ label: str, on: str, enabled: bool, help: optNull(str) }),
        detailsTitle: str,
        details: list(object({ label: str, value: str })),
        remove: optNull(MachinesViewRemoveShape),
      }),
    ),
    order: opt(strings),
    // The Clouds group (`cloud_page`); a host that predates it sends none.
    clouds: opt(
      object({
        group: object({
          title: str,
          rows: list(object({ id: str, name: str, logo: str, line: str, health: str })),
          connectLabel: str,
        }),
        pages: list(
          object({
            id: str,
            name: str,
            logo: str,
            status: str,
            health: str,
            issuesTitle: str,
            issues: list(object({ id: str, level: str, text: str, action: optNull(object({ id: str, label: str, url: optNull(str) })) })),
            spend: optNull(object({ amount: str, ofBudget: str, resets: str, percent: num, tone: str, note: str })),
            running: optNull(object({ title: str, machines: list(object({ id: str, name: str, by: str, cost: str })) })),
            token: optNull(
              object({
                title: str,
                lock: str,
                note: str,
                holdersLabel: str,
                holders: str,
                revokeLabel: str,
                revokeHelp: str,
                replace: object({ id: str, label: str, url: optNull(str) }),
              }),
            ),
            settingsTitle: str,
            settings: list(object({ id: str, label: str, help: str, value: str })),
            limits: optNull(object({ id: str, label: str, url: optNull(str) })),
            switchAuth: optNull(object({ id: str, label: str, url: optNull(str) })),
            approval: optNull(
              object({
                title: str,
                intro: str,
                rows: list(object({ label: str, value: str })),
                note: str,
                approveLabel: str,
                denyLabel: str,
              }),
            ),
            disconnect: optNull(object({ label: str, confirm: HostConfirmShape, delete: bool })),
          }),
        ),
      }),
    ),
  }),
);

export const OP_SHAPES: Record<OpName, Schema> = {
  "spaces.list": list(SpaceRowShape),
  "spaces.create": SpaceRowShape,
  "spaces.cancelCreate": obj<CancelOutcome>()({ id: str, state: oneOf("cancelled", "not_creating", "already_created"), message: str }),
  "spaces.setPower": obj<SpacePowerReport>()({ space: str, state: str, power: str, message: str }),
  "spaces.delete": str,
  "spaces.deleteMany": BulkResultsShape,
  "spaces.leave": BulkResultsShape,
  "spaces.rename": nul,
  "spaces.open": nul,
  "machines.list": list(MachineRowShape),
  "host.status": hostStatusOrNull,
  "settings.get": SettingsShape,
  "settings.set": SettingsShape,
  "settings.choose": SettingsShape,
  "keyvault.overview": KeyvaultShape,
  "keyvault.unlock": nul,
  "keyvault.setup": object({ recoveryKey: optNull(str) }),
  "keyvault.showItems": KeyvaultShape,
  "keyvault.delete": nul,
  "keyvault.run": nul,
  "keyvault.dismiss": nul,
  "keyvault.setUnattended": list(object({ id: str })),
  "keyvault.setDisabled": nul,
  "keyvault.approve": KvGrantShape,
  "keyvault.deny": nul,
  "keyvault.revokeGrant": num,
  "session.get": SessionShape,
  "session.signIn": obj<SignInStart>()({ method: opt(oneOf("browser", "device")), userCode: opt(str), verificationUri: str }),
  "session.signOut": nul,
  "session.completeOnboarding": nul,
  "session.openExternal": nul,
  "agents.list": list(
    obj<PersistentAgent>()({
      name: str,
      harness: str,
      space: str,
      paused: bool,
      spaceState: str,
      runId: optNull(str),
      savedMs: num,
      lastError: optNull(str),
    }),
  ),
  "agents.runs": list(
    obj<SpaceAgentRun>()({
      runId: str,
      agent: str,
      status: oneOf("running", "idle", "failed", "crashed", "unknown"),
      reason: str,
      summary: str,
      createdAt: nullable(num),
      phase: str,
      turn: num,
      needsYou: optNull(obj<{ title: string; detail: string }>()({ title: str, detail: str })),
    }),
  ),
  "agents.events": obj<AgentEventsPage>()({
    run_id: str,
    status: oneOf("running", "idle", "failed", "crashed", "unknown"),
    phase: str,
    events: list(object({ seq: num, ts_ms: num, turn: num, kind: str, category: oneOf("message", "user", "activity", "hidden") })),
    cursor: num,
    caught_up: bool,
  }),
  "agents.start": object({ run_id: str, agent: str }),
  "agents.message": object({ delivered: bool, run_id: str, status: oneOf("running", "idle", "failed", "crashed", "unknown"), phase: str }),
  "agents.pause": nul,
  "agents.resume": nul,
  "agents.setup": agentSetupRows,
  "agents.configure": agentSetupRows,
  "agents.signIn": agentSetupRows,
  "agents.status": ConnectionReportShape,
  "agents.connect": ConnectResultsShape,
  "agents.disconnect": DisconnectShape,
  "agents.detail": AgentDetailShape,
  "agents.readFile": AgentOpenFileShape,
  "agents.restoreFile": nul,
  "agents.addRoutine": nul,
  "agents.setRoutine": nul,
  "agents.removeRoutine": nul,
  "agents.allowComputer": nul,
  "agents.revokeComputer": nul,

  "spaces.createOptions": NewSpaceOptionsShape,
  "spaces.add": SpaceRowShape,
  "clouds.status": object({
    default_on: opt(str),
    can_approve: opt(bool),
    providers: list(CloudProviderShape),
    resources: opt(list(object({ provider: str, id: str, type: str, sandbox: opt(str), state: opt(str), started_by: opt(str), cost_usd: opt(num) }))),
  }),
  "clouds.test": obj<CloudTestWire>()({ provider: str, ok: bool, account: opt(str), checks: list(CloudCheckShape) }),
  "clouds.connect": CloudProviderShape,
  "clouds.disconnect": obj<CloudDisconnectedWire>()({ provider: str, disconnected: bool }),
  "clouds.approve": nul,

  "teleport.catalog": list(CatalogEntryShape),
  "teleport.entryForPath": CatalogEntryShape,
  "teleport.windows": list(
    obj<OpenWindow>()({ windowId: num, appId: str, appName: str, windowTitle: str, supported: bool, bundlePath: optNull(str) }),
  ),
  "teleport.remoteWindows": list(RemoteWindowShape),
  "teleport.icon": nullable(str),
  "teleport.thumbnail": nullable(str),
  "teleport.plan": obj<TeleportPlan>()({
    app: CatalogEntryShape,
    spaceId: str,
    moves: teleportMove,
    steps: list(obj<PlanStep>()({ kind: str, summary: str })),
    consent: list(
      obj<ConsentItem>()({
        kind: oneOf("install", "file", "folder", "state", "secret"),
        key: str,
        label: str,
        detail: str,
        bytes: num,
        sensitive: bool,
      }),
    ),
    sensitive: bool,
    totalBytes: num,
    warnings: strings,
    relayUnsealed: opt(bool),
    json: str,
  }),
  "teleport.run": obj<RunReport>()({ appId: str, installed: strings, sent: strings, imported: strings, skipped: strings, launched: bool }),
  "teleport.sites": obj<KvInventory>()({
    provider_id: str,
    app_display: str,
    domains: list(
      obj<KvDomainCount>()({
        domain: str,
        cookies: opt(num),
        session_cookies: opt(num),
        local_storage: opt(num),
        passwords: opt(num),
        signin: opt(bool),
        identity_provider: opt(bool),
        unavailable: opt(num),
        unavailable_reason: opt(str),
      }),
    ),
    notes: opt(strings),
  }),
  "teleport.remembered": nullable(strings),
  "teleport.streamWindow": nul,
  "sharing.list": shares,
  "sharing.share": shares,
  "sharing.unshare": shares,
  "sharing.removals": shareRemovals,
  "sharing.ackRemovals": nul,

  "agentKeys.list": AgentKeysShape,
  "agentKeys.set": AgentKeysShape,
  "agentKeys.remove": AgentKeysShape,

  "volume.overview": VolumeShape,
  "volume.storage": nullable(DriveStorageShape),
  "volume.storageSet": DriveCheckShape,
  "volume.mount": DriveMountShape,
  "volume.unmount": DriveMountShape,
  "volume.approve": nul,
  "volume.deny": nul,
  "volume.revoke": nul,
  "volume.resolve": nul,
  "volume.reveal": nul,
  "agents.setupDriver": list(obj<AgentSetupOutcome>()({ agents: strings, target: str, item: str, change: str, detail: str })),

  "machines.rename": nul,
  "machines.remove": nul,
  "machines.checkHealth": nul,
  "spaces.openStream": obj<StreamTicket>()({ wsUrl: str, expiresAt: nullable(str) }),
  "nudges.pending": list(obj<RiskyRequest>()({ id: str, agent: str, action: str, atMs: num, expiresMs: num })),
  "nudges.answer": nul,
  "relay.usage": nullable(
    obj<RelayUsage>()({
      metered: bool,
      period: str,
      plan: str,
      team: nullable(object({ id: str, seats: num })),
      liveSessions: num,
      liveSessionsLimit: nullable(num),
      hoursUsed: num,
      hoursLimit: nullable(num),
      myHoursUsed: num,
      softLimitReached: bool,
      reasons: strings,
      upgradeUrl: nullable(str),
    }),
  ),
  "resources.get": nullable(
    obj<ResourcesSnapshot>()({
      sequence: num,
      sampledAtMs: num,
      thisMac: ComputerResourcesShape,
      spaces: list(
        ref(
          "SpaceResources",
          obj<SpaceResources>()({
            id: str,
            powerState: oneOf("running", "stopped", "suspended", ""),
            cpuAllocated: num,
            memoryAllocated: num,
            diskAllocated: num,
            usage: nullable(
              obj<GuestUsage>()({
                cpuCount: num,
                cpuPercent: num,
                loadAverage1m: num,
                memoryUsed: num,
                memoryTotal: num,
                memoryLimited: bool,
                diskUsed: num,
                diskTotal: nullable(num),
                diskLimited: bool,
              }),
            ),
            diskOnHost: nullable(obj<DiskOnHost>()({ bytes: nullable(num), allocated: num, sharesSpace: bool })),
            base: opt(nullable(obj<SharedBase>()({ key: str, image: str, bytes: nullable(num), sharedWith: num }))),
          }),
        ),
      ),
      computers: list(ComputerResourcesShape),
      warning: DiskWarningShape,
    }),
  ),
  "checklist.get": ChecklistAnswerShape,
  "checklist.send": ChecklistAnswerShape,
  "links.list": object({ links: list(ShareLinkShape) }),
  "links.create": ShareLinkShape,
  "links.stop": object({ stopped: strings }),
  "cli.plan": nullable(CliInstallPlanShape),
  "cli.install": CliInstallPlanShape,
  "thread.read": ThreadPageShape,
  "thread.post": nullable(ThreadEntryShape),
  "thread.members": object({ members: list(AgentMemberShape) }),
  "thread.setRights": nullable(AgentMemberShape),
  "thread.agentAdd": nullable(AgentMemberShape),
  "thread.agentRemove": nul,
  "thread.joinCodeCreate": JoinCodeShape,
  "thread.joinCodeList": object({ codes: list(JoinCodeRowShape) }),
  "thread.joinCodeCancel": nul,
  "thread.settings": object({ askBeforeJoin: bool }),
  "presence.join": obj<PresenceJoinInfo>()({
    handle: str,
    me: PresenceParticipantShape,
    members: list(object({ participant: PresenceParticipantShape, cursor: opt(PresenceCursorShape) })),
    delayMs: num,
    datagrams: bool,
  }),
  "presence.publish": nul,
  "presence.leave": nul,
  "spaces.fixRuntime": nul,

  "about.get": AboutShape,
  "about.set": AboutShape,
  "about.checkNow": AboutShape,
  "update.get": UpdateShape,
  "update.install": UpdateShape,
  "experiments.get": ExperimentsShape,
  "experiments.set": ExperimentsShape,
  "loginItem.get": LoginItemShape,
  "loginItem.set": LoginItemShape,
  "loginItem.openSettings": nul,
  "runtimes.check": RuntimePathCheckShape,
  "runtimes.browse": object({ path: nullable(str) }),
  "devices.get": DevicesShape,
  "devices.enroll": obj<EnrollResult>()({ enrolled: bool, code: nullable(str) }),
  "devices.checkEnrolled": bool,
  "devices.approve": nul,
  "devices.rename": nul,
  "devices.revoke": nul,
  "devices.confirmMachine": nul,
  "devices.createPairingCode": PairingCodeShape,
  "devices.pairingCodes": list(PairingCodeShape),
  "devices.revokePairingCode": nul,
  "teams.list": TeamsListShape,
  "storage.get": StorageShape,
  "storage.run": nullable(DriveCheckShape),
  "notifications.list": list(NotificationShape),
  "notifications.markAllRead": nul,
  "notifications.markRead": nul,

  "telemetry.track": nul,
  "telemetry.linkNotice": nullable(LinkNoticeShape),
  "telemetry.acknowledgeLinkNotice": nul,
  "crashReporting.config": CrashReportingConfigShape,
  "spaces.usage": nullable(
    obj<SpaceUsage>()({ memoryUsed: num, memoryTotal: num, memoryLimited: bool, diskUsed: num, diskTotal: num, diskLimited: bool }),
  ),
  "spaces.windows": obj<SpaceWindows>()({ windows: list(RemoteWindowShape), display: nullable(object({ widthPx: num, heightPx: num })), open: opt(strings) }),
  "stream.pip": strings,
  "spaces.thumbnail": nullable(obj<SpaceThumbnail>()({ url: str, capturedAtMs: num })),
  "spaces.chooseFiles": strings,
  "spaces.droppedFiles": strings,
  "spaces.sendFiles": list(obj<SentFileInfo>()({ name: str, dest: str, bytes: num })),
  "host.setUp": HostStatusShape,
  "host.joinWithCode": HostStatusShape,
  "host.action": HostStatusShape,
  "host.openSettings": nul,
  "startup.get": StartupShape,
  "startup.act": StartupShape,
};

/* ---- The SwiftUI host's answers -------------------------------------------------- */

const WkHostShape = ref(
  "WkHostState",
  obj<WkHostState>()({
    configured: bool,
    mode: optNull(str),
    relayUrl: optNull(str),
    directUrl: optNull(str),
    machineId: optNull(str),
    name: optNull(str),
    sharing: bool,
    serviceInstalled: bool,
    serviceRunning: bool,
    serviceKind: str,
    online: optNull(bool),
    clients: list(object({ id: str, email: optNull(str), name: optNull(str), streams: optNull(num) })),
    permissions: list(object({ id: str, title: str, settingsUrl: optNull(str), instructions: optNull(str), granted: bool })),
    error: optNull(str),
    shareDesktop: bool,
    provideSpaces: bool,
    maxSpaces: num,
    maxMacosVms: optNull(num),
    recentAccess: optNull(list(object({ atMs: num, via: str, who: str, what: str }))),
    accessLogError: optNull(str),
    providedSpaces: optNull(list(any)),
    spacesAudit: optNull(list(any)),
    spacesAuditError: optNull(str),
    pausedSignedOut: optNull(bool),
    owner: optNull(str),
    ownerEmail: optNull(str),
    account: optNull(object({ id: optNull(str), email: optNull(str), display: optNull(str) })),
    progress: optNull(str),
    appName: optNull(str),
  }),
);

/** `spaces.list`'s Spaces your other computers are creating (CUA-2349). */
const CreatingElsewhereShape = ref(
  "CreatingElsewhere",
  obj<CreatingElsewhere>()({ machineId: str, machineName: str, name: str, os: str, phase: str, percent: num }),
);

const WkSpaceShape = ref(
  "WkSpace",
  obj<WkSpace>()({
    id: str,
    name: str,
    os,
    status: oneOf("local", "running", "approval", "suspended", "provisioning", "deleting"),
    detail: str,
    lastUsedAt: num,
    startedAt: optNull(num),
    provider: optNull(oneOf("local", "direct", "relay")),
    sdk: optNull(object({ features: strings, spacesdVersion: str, reachable: bool, error: optNull(str) })),
    osName: optNull(str),
    osPrettyName: optNull(str),
    image: optNull(str),
    imageDigest: optNull(str),
    kind: optNull(oneOf("container", "vm")),
    arch: optNull(str),
    host: optNull(str),
    hostName: optNull(str),
    power: optNull(object({ control: str, off: bool, turningOn: optNull(bool), error: optNull(str), stopReason: optNull(str) })),
    cloud: optNull(str),
    cloudPlace: optNull(str),
    cloudDelete: optNull(str),
    access: optNull(SpaceAccessShape),
    progress: optNull(SpaceProgressShape),
  }),
);

const WkSpacesShape = ref(
  "WkSpaces",
  obj<WkSpaces>()({ loaded: bool, selectedId: nullable(str), spaces: list(object({ space: WkSpaceShape, deleting: bool })), rosterError: optNull(str), daemonNotice: optNull(str), incomplete: opt(bool), reaching: opt(num), stale: opt(bool), creatingElsewhere: opt(list(CreatingElsewhereShape)) }),
);

const WkSessionShape = ref(
  "WkSession",
  obj<WkSession>()({
    identity: nullable(str),
    signedIn: bool,
    cloudConfigured: bool,
    signIn: union(
      oneOf("idle", "starting"),
      object({ type: str, userCode: optNull(str), opened: opt(bool), reason: opt(str), message: opt(str), detail: optNull(str) }),
    ),
    signInUrl: optNull(str),
  }),
);

const WkSettingsShape = ref(
  "WkSettings",
  obj<WkSettings>()({
    page: object({
      title: str,
      sections: list(
        object({
          id: str,
          rows: list(
            obj<WkSettingsRow>()({
              id: str,
              kind: str,
              label: str,
              enabled: bool,
              help: optNull(str),
              value: optNull(str),
              options: list(object({ id: str, label: str, active: bool })),
            }),
          ),
        }),
      ),
    }),
    updateChannel: optNull(oneOf("stable", "beta")),
    globalShortcut: optNull(object({ label: str, registered: bool })),
    accountUrl: optNull(str),
    cli: optNull(object({ name: str, installed: bool })),
    tips: opt(TipsStateShape),
    runtimes: optNull(RuntimesInputShape),
  }),
);

const WkKeyvaultShape = ref(
  "WkKeyvault",
  object({
    availability: str,
    overview: optNull(object({ items: list(object({ id: str })), pending: list(any), grants: list(any) })),
    dismissed: opt(strings),
    busy: bool,
    error: nullable(str),
  }),
);

const WkMachinesShape = ref(
  "WkMachines",
  obj<WkMachines>()({
    thisMachine: optNull(object({ status: str, statusText: str, detail: str })),
    devices: nullable(
      object({
        rows: list(object({ id: str, name: str, platform: str, current: bool, detail: opt(str), lastSeen: optNull(num) })),
      }),
    ),
    signedIn: bool,
    host: optNull(WkHostShape),
    hostnames: optNull(dict(str)),
    presence: optNull(dict(bool)),
    deviceStates: optNull(dict(str)),
    accessNotice: optNull(MachineAccessNoticeShape),
    rows: opt(list(MachinePageRowShape)),
    view: optNull(MachinesViewShape),
  }),
);

/** The daemon's `agent_keys.*` answer, which `agentKeysFromWire` reads. */
const AgentKeysWire = object({
  keys: list(object({ provider: str, env: str, last4: str, added_ms: num })),
  providers: list(object({ id: str, env: str, label: opt(str) })),
  available: bool,
  unavailable: optNull(str),
});

const WkAppInfoShape = object({ platform: str, host: str, version: str, experiments: ExperimentsShape, methods: strings });
/** The SwiftUI host's teleport records (`AppCatalogEntry`, `AppTeleportPlan`,
 * the Keyvault's `KvInventory`), camelCased by `BridgeValue.encode`; the
 * adapter maps them to the core's snake_case (`ops/webkit-pages.ts`). */
const WkCatalogEntry = ref(
  "WkCatalogEntry",
  object({
    id: str,
    name: str,
    hostPath: nullable(str),
    hostAppId: nullable(str),
    version: nullable(str),
    capability: oneOf("full", "installOnly", "unsupported"),
    reason: nullable(str),
    moves: list(oneOf("appOnly", "appWithFiles", "appWithState")),
    providerId: nullable(str),
    sensitiveGroups: list(oneOf("signIns", "passwords", "history")),
    installSource: nullable(str),
    installId: nullable(str),
    installVersion: nullable(str),
    launchBin: nullable(str),
    lastUsedMs: nullable(num),
    json: str,
  }),
);
const WkTeleportPlan = object({
  app: WkCatalogEntry,
  spaceId: str,
  moves: oneOf("appOnly", "appWithFiles", "appWithState"),
  steps: list(object({ kind: str, summary: str })),
  consent: list(
    object({ kind: oneOf("install", "file", "folder", "state", "secret"), key: str, label: str, detail: str, bytes: num, sensitive: bool }),
  ),
  sensitive: bool,
  totalBytes: num,
  warnings: strings,
  relayUnsealed: bool,
  json: str,
});
const WkKvInventory = object({
  providerId: str,
  appDisplay: str,
  domains: list(
    object({
      domain: str,
      cookies: num,
      sessionCookies: num,
      localStorage: num,
      passwords: num,
      signin: bool,
      identityProvider: bool,
      unavailable: num,
      unavailableReason: str,
    }),
  ),
  notes: strings,
});

/** What the SwiftUI host answers each method with (shapes `adapters/webkit.ts` reads). */
export const WEBKIT_SHAPES: Partial<Record<WebkitMethod, Schema>> = {
  "app.info": WkAppInfoShape,
  "session.get": WkSessionShape,
  "session.signIn": WkSessionShape,
  "session.signOut": WkSessionShape,
  // The first run is this page's on both hosts (`WkOnboarding`).
  "onboarding.get": ref("WkOnboarding", obj<WkOnboarding>()({ completed: bool, mode: oneOf("client", "host") })),
  "onboarding.complete": ref("WkOnboarding", obj<WkOnboarding>()({ completed: bool, mode: oneOf("client", "host") })),
  "spaces.list": WkSpacesShape,
  "spaces.setPower": WkSpacesShape,
  "spaces.delete": WkSpacesShape,
  "spaces.deleteMany": BulkResultsShape,
  "spaces.leave": BulkResultsShape,
  // `{}`: the new name comes with the next `spaces.list`.
  "spaces.rename": object({}),
  "spaces.open": nul,
  // The page's wizard runs on `env`, and reads `local`.
  "spaces.createOptions": ref("WkCreateOptions", createOptions(true)),
  "spaces.create": WkSpaceShape,
  "spaces.add": WkSpaceShape,
  "spaces.cancelCreate": OP_SHAPES["spaces.cancelCreate"],
  "machines.list": WkMachinesShape,
  "host.status": nullable(WkHostShape),
  "host.setUp": nullable(WkHostShape),
  "host.joinWithCode": nullable(WkHostShape),
  "host.action": nullable(WkHostShape),
  "agents.list": OP_SHAPES["agents.list"],
  "agents.runs": OP_SHAPES["agents.runs"],
  "agents.events": OP_SHAPES["agents.events"],
  "agents.start": OP_SHAPES["agents.start"],
  "agents.message": OP_SHAPES["agents.message"],
  "agents.pause": any,
  "agents.resume": any,
  "agents.setup": agentSetupRows,
  "agents.configure": agentSetupRows,
  "agents.signIn": agentSetupRows,
  "agents.status": ConnectionReportShape,
  "agents.connect": ConnectResultsShape,
  "agents.disconnect": DisconnectShape,
  "agents.setupDriver": OP_SHAPES["agents.setupDriver"],
  "agents.detail": AgentDetailShape,
  "agents.readFile": AgentOpenFileShape,
  "agents.restoreFile": nul,
  "agents.addRoutine": nul,
  "agents.setRoutine": nul,
  "agents.removeRoutine": nul,
  "agents.allowComputer": nul,
  "agents.revokeComputer": nul,
  "agentKeys.list": AgentKeysWire,
  "agentKeys.set": AgentKeysWire,
  "agentKeys.remove": AgentKeysWire,
  "clouds.status": nullable(OP_SHAPES["clouds.status"]),
  "clouds.test": OP_SHAPES["clouds.test"],
  "clouds.connect": OP_SHAPES["clouds.connect"],
  "clouds.disconnect": OP_SHAPES["clouds.disconnect"],
  "clouds.approve": OP_SHAPES["clouds.approve"],
  "teleport.catalog": list(WkCatalogEntry),
  "teleport.entryForPath": WkCatalogEntry,
  "teleport.windows": OP_SHAPES["teleport.windows"],
  "teleport.remoteWindows": OP_SHAPES["teleport.remoteWindows"],
  "teleport.icon": nullable(str),
  "teleport.thumbnail": nullable(str),
  "teleport.plan": WkTeleportPlan,
  "teleport.run": OP_SHAPES["teleport.run"],
  "teleport.sites": WkKvInventory,
  "teleport.remembered": nullable(strings),
  "teleport.streamWindow": nul,
  "sharing.list": shares,
  "sharing.share": shares,
  "sharing.unshare": shares,
  "sharing.removals": shareRemovals,
  "sharing.ackRemovals": nul,
  "volume.overview": VolumeShape,
  "volume.storage": nullable(DriveStorageShape),
  "volume.storageSet": DriveCheckShape,
  "volume.mount": DriveMountShape,
  "volume.unmount": DriveMountShape,
  "volume.approve": nul,
  "volume.deny": nul,
  "volume.revoke": nul,
  "volume.resolve": nul,
  "volume.reveal": nul,
  "about.get": AboutShape,
  "about.set": AboutShape,
  "about.checkNow": AboutShape,
  "update.get": UpdateShape,
  "update.install": UpdateShape,
  "loginItem.get": LoginItemShape,
  "loginItem.set": LoginItemShape,
  "loginItem.openSettings": nul,
  "runtimes.check": RuntimePathCheckShape,
  "runtimes.browse": object({ path: nullable(str) }),
  "devices.get": DevicesShape,
  "devices.enroll": OP_SHAPES["devices.enroll"],
  "devices.checkEnrolled": bool,
  "devices.approve": nul,
  "devices.rename": nul,
  "devices.revoke": nul,
  "devices.confirmMachine": nul,
  "devices.createPairingCode": PairingCodeShape,
  "devices.pairingCodes": list(PairingCodeShape),
  "devices.revokePairingCode": nul,
  "teams.list": TeamsListShape,
  "storage.get": StorageShape,
  "storage.run": nullable(DriveCheckShape),
  "notifications.list": list(NotificationShape),
  "notifications.markAllRead": nul,
  "notifications.markRead": nul,
  "telemetry.track": nul,
  "telemetry.linkNotice": nullable(LinkNoticeShape),
  "telemetry.acknowledgeLinkNotice": nul,
  "crashReporting.config": CrashReportingConfigShape,
  "spaces.usage": OP_SHAPES["spaces.usage"],
  "spaces.windows": OP_SHAPES["spaces.windows"],
  "stream.pip": strings,
  "spaces.thumbnail": OP_SHAPES["spaces.thumbnail"],
  "spaces.chooseFiles": strings,
  "spaces.droppedFiles": strings,
  "spaces.sendFiles": OP_SHAPES["spaces.sendFiles"],
  "host.openSettings": nul,
  "settings.get": WkSettingsShape,
  "settings.choose": WkSettingsShape,
  "settings.update": WkSettingsShape,
  "keyvault.get": WkKeyvaultShape,
  "keyvault.lock": WkKeyvaultShape,
  "keyvault.unlock": WkKeyvaultShape,
  "keyvault.unlockVault": any,
  "keyvault.setup": object({ recoveryKey: optNull(str) }),
  "keyvault.showItems": WkKeyvaultShape,
  "keyvault.delete": WkKeyvaultShape,
  "keyvault.run": WkKeyvaultShape,
  "keyvault.dismiss": WkKeyvaultShape,
  "keyvault.setDisabled": any,
  "keyvault.approve": object({ id: str, requestId: str, items: strings }),
  "keyvault.deny": any,
  "keyvault.revokeGrant": num,
  "window.setBackgroundColor": nul,
  "window.setDragRegions": nul,
  "window.setDropTargets": nul,
  "startup.get": StartupShape,
  "startup.act": StartupShape,
  "relay.usage": OP_SHAPES["relay.usage"],
  "nudges.pending": OP_SHAPES["nudges.pending"],
  "nudges.answer": nul,
  "resources.get": OP_SHAPES["resources.get"],
  "machines.rename": nul,
  "machines.remove": nul,
  "machines.checkHealth": nul,
  "checklist.get": OP_SHAPES["checklist.get"],
  "checklist.send": OP_SHAPES["checklist.send"],
  "links.list": OP_SHAPES["links.list"],
  "links.create": OP_SHAPES["links.create"],
  "links.stop": OP_SHAPES["links.stop"],
  "cli.plan": OP_SHAPES["cli.plan"],
  "cli.install": OP_SHAPES["cli.install"],
};

/** Methods whose answer has no shape here, and why. */
const THREAD_TOOL_JSON =
  "the daemon tool's JSON as-is (thread_read, thread_post, agent_members, agent_set_rights, agent_add, agent_remove, thread_join_code_create, thread_join_code_list, thread_join_code_cancel, thread_settings); ops/thread.ts maps it for every host";
export const WEBKIT_UNSHAPED: Partial<Record<WebkitMethod, string>> = {
  "thread.read": THREAD_TOOL_JSON,
  "thread.post": THREAD_TOOL_JSON,
  "thread.members": THREAD_TOOL_JSON,
  "thread.setRights": THREAD_TOOL_JSON,
  "thread.agentAdd": THREAD_TOOL_JSON,
  "thread.agentRemove": THREAD_TOOL_JSON,
  "thread.joinCodeCreate": THREAD_TOOL_JSON,
  "thread.joinCodeList": THREAD_TOOL_JSON,
  "thread.joinCodeCancel": THREAD_TOOL_JSON,
  "thread.settings": THREAD_TOOL_JSON,
};

/** The exported document (`bridge-shapes.json`): `ops` and `webkit` map
 * each name to its shape; `$defs` holds the named ones. */
export function bridgeShapesDocument() {
  const sorted = <T>(o: Record<string, T>) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => a.localeCompare(b)));
  return {
    $comment:
      "Generated from apps/cua-spaces-web/src/bridge/contracts/shapes.ts by `pnpm contract:shapes`; do not edit. " +
      "JSON Schema keywords; every $ref is #/$defs/<name>.",
    $defs: sorted(defs.all),
    ops: sorted(OP_SHAPES),
    webkit: sorted(WEBKIT_SHAPES as Record<string, Schema>),
    webkitUnshaped: sorted(WEBKIT_UNSHAPED as Record<string, string>),
  };
}

export const SHAPE_DEFS = defs.all;
