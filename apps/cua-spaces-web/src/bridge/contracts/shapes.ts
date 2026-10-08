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

import type { AgentEventsPage, AgentSetupRow, PersistentAgent, SpaceAgentRun } from "./agents";
import type { DeviceInput, DevicesInput, EnrollResult, MachineAccessNotice } from "./devices";
import type {
  CancelOutcome,
  DaemonStatus,
  DefaultLocation,
  FleetStatus,
  HostStatus,
  MachineRow,
  OnboardingState,
  SignInStart,
  SpacePowerReport,
  TelemetryView,
} from "./host";
import type { KeyvaultOverview, KvGrant } from "./keyvault";
import type {
  CloudCheckWire,
  CloudPricing,
  CloudProviderWire,
  CloudTestWire,
  ConnectedCloud,
  GpuChoice,
  LocalStatus,
  LocalStorage,
  NewSpaceOptions,
  StorageVolume,
  WizardEnv,
} from "./new-space";
import type { NotificationInput } from "./notifications";
import type { AboutInput, DriveCheckInput, DriveStorageInput, LoginItemReport, StorageInput } from "./settings";
import type { ShareEntry } from "./share";
import type { SpaceProgress, SpaceRow } from "./spaces";
import type { CatalogEntry, ConsentItem, KvDomainCount, KvInventory, OpenWindow, PlanStep, RemoteWindow, RunReport, TeleportPlan } from "./teleport";
import type { AgentSetupOutcome, DriveGrantInput, DriveMountInput, DriveRequestInput, Experiments, VolumeOverview } from "./volume";
import type { AgentKeysReport } from "../ops/agent-keys";
import type { SentFileInfo, SpaceThumbnail, SpaceUsage, SpaceWindows } from "../ops/space-detail";
import type { StartupState } from "../ops/startup";
import type { StreamTicket } from "../ops/stream";
import type { HostSettings, OpName, SessionSnapshot, SettingsSnapshot, SettingsValues } from "../protocol";
import type { WebkitMethod, WkHostState, WkMachines, WkSession, WkSettings, WkSettingsRow, WkSpace, WkSpaces } from "../webkit-protocol";
import { Defs, any, bool, dict, list, nul, nullable, num, obj, object, oneOf, opt, optNull, str, union, type Schema } from "./schema";

const defs = new Defs();
const ref = (name: string, s: Schema) => defs.ref(name, s);
const strings = list(str);
const limits = list(object({ resource: str, used: num, limit: num, reason: str }));
const os = oneOf("macos", "windows", "linux", "unknown");

/* ---- Spaces and machines --------------------------------------------------- */

const SpaceProgressShape = ref(
  "SpaceProgress",
  obj<SpaceProgress>()({ phase: str, permille: num, label: str, error: opt(str), transfer: opt(str), cancellable: opt(bool), cancelling: opt(bool) }),
);

const SpaceRowShape = ref(
  "SpaceRow",
  obj<SpaceRow>()({
    id: str,
    hostProgress: opt(SpaceProgressShape),
    name: str,
    provider: oneOf("cloud", "local", "direct", "relay"),
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
    cloud: opt(str),
    cloudPlace: opt(str),
    cloudDelete: opt(str),
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
  }),
);

/* ---- Session and settings --------------------------------------------------- */

const SessionShape = ref(
  "SessionSnapshot",
  obj<SessionSnapshot>()({
    fleet: obj<FleetStatus>()({
      configured: bool,
      authMode: oneOf("user", "client-credentials", "static-token", "none"),
      baseUrl: str,
      tokenUrl: str,
      clientId: opt(str),
      identity: opt(str),
      namespaces: opt(strings),
      probeError: opt(str),
    }),
    onboarding: obj<OnboardingState>()({
      completed: bool,
      mode: optNull(oneOf("client", "host")),
      installerMode: optNull(oneOf("client", "host")),
    }),
    daemon: nullable(
      obj<DaemonStatus>()({ connected: bool, version: opt(str), socketPath: opt(str), loopbackUrl: opt(str), error: opt(str) }),
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
    }),
    defaultLocation: nullable(obj<DefaultLocation>()({ value: str, source: oneOf("env", "config", "default"), env: opt(str), path: str })),
    telemetry: nullable(
      obj<TelemetryView>()({ enabled: bool, source: str, sourceKind: str, noticeShown: bool, noticeText: str, docsUrl: str }),
    ),
    hostSettings: opt(
      obj<HostSettings>()({ lumeSource: optNull(str), linuxSource: optNull(str), autoConnect: optNull(bool), keyvaultAutoWipe: optNull(bool) }),
    ),
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
  obj<NotificationInput>()({ id: str, atMs: num, agent: optNull(str), kind: str, title: str, body: str, read: opt(bool) }),
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

/* ---- New Space ------------------------------------------------------------- */

const StorageVolumeShape = obj<StorageVolume>()({ availableBytes: num, totalBytes: num, name: str });
const LocalStorageShape = ref(
  "LocalStorage",
  obj<LocalStorage>()({
    reserveBytes: num,
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
const PricingShape = ref("CloudPricing", obj<CloudPricing>()({ vcpuHourUsd: num, memoryGibHourUsd: num }));
const LocalStatusShape = ref(
  "LocalStatus",
  obj<LocalStatus>()({
    available: bool,
    backends: strings,
    containerImage: opt(str),
    macosImage: optNull(str),
    error: nullable(str),
    hostArch: opt(str),
    storage: optNull(LocalStorageShape),
  }),
);
const WizardEnvShape = ref(
  "WizardEnv",
  obj<WizardEnv>()({
    defaultLocation: oneOf("cloud", "local", "yours", "host"),
    cloudAvailable: bool,
    localAvailable: bool,
    localReason: optNull(str),
    localBackends: optNull(strings),
    maxCpus: num,
    hostArch: optNull(str),
    storage: optNull(LocalStorageShape),
    cloudPricing: optNull(PricingShape),
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
  }),
);
const createOptions = (strict: boolean) =>
  obj<NewSpaceOptions>()({
    local: strict ? LocalStatusShape : nullable(LocalStatusShape),
    gpus: nullable(list(GpuShape)),
    cloudPricing: nullable(PricingShape),
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
const shares = list(ShareShape);

export const OP_SHAPES: Record<OpName, Schema> = {
  "spaces.list": list(SpaceRowShape),
  "spaces.create": SpaceRowShape,
  "spaces.cancelCreate": obj<CancelOutcome>()({ id: str, state: oneOf("cancelled", "not_creating", "already_created"), message: str }),
  "spaces.setPower": obj<SpacePowerReport>()({ space: str, state: str, power: str, message: str }),
  "spaces.delete": str,
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
  "agents.pause": nul,
  "agents.resume": nul,
  "agents.setup": agentSetupRows,
  "agents.configure": agentSetupRows,

  "spaces.createOptions": NewSpaceOptionsShape,
  "spaces.add": SpaceRowShape,
  "clouds.status": object({ default_on: opt(str), providers: list(CloudProviderShape) }),
  "clouds.test": obj<CloudTestWire>()({ provider: str, ok: bool, account: opt(str), checks: list(CloudCheckShape) }),
  "clouds.connect": CloudProviderShape,

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

  "spaces.openStream": obj<StreamTicket>()({ wsUrl: str, expiresAt: nullable(str) }),

  "about.get": AboutShape,
  "about.set": AboutShape,
  "about.checkNow": AboutShape,
  "experiments.get": ExperimentsShape,
  "experiments.set": ExperimentsShape,
  "loginItem.get": LoginItemShape,
  "loginItem.set": LoginItemShape,
  "loginItem.openSettings": nul,
  "devices.get": DevicesShape,
  "devices.enroll": obj<EnrollResult>()({ enrolled: bool, code: nullable(str) }),
  "devices.checkEnrolled": bool,
  "devices.approve": nul,
  "devices.rename": nul,
  "devices.revoke": nul,
  "devices.confirmMachine": nul,
  "storage.get": StorageShape,
  "storage.run": nullable(DriveCheckShape),
  "notifications.list": list(NotificationShape),
  "notifications.markAllRead": nul,

  "telemetry.track": nul,
  "spaces.usage": nullable(
    obj<SpaceUsage>()({ memoryUsed: num, memoryTotal: num, memoryLimited: bool, diskUsed: num, diskTotal: num, diskLimited: bool }),
  ),
  "spaces.windows": obj<SpaceWindows>()({ windows: list(RemoteWindowShape), display: nullable(object({ widthPx: num, heightPx: num })) }),
  "stream.pip": strings,
  "spaces.thumbnail": nullable(obj<SpaceThumbnail>()({ url: str, capturedAtMs: num })),
  "spaces.chooseFiles": strings,
  "spaces.droppedFiles": strings,
  "spaces.sendFiles": list(obj<SentFileInfo>()({ name: str, dest: str, bytes: num })),
  "host.setUp": HostStatusShape,
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
  }),
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
    provider: optNull(oneOf("cloud", "local", "direct", "relay")),
    sdk: optNull(object({ features: strings, spacesdVersion: str, reachable: bool, error: optNull(str) })),
    osName: optNull(str),
    osPrettyName: optNull(str),
    image: optNull(str),
    imageDigest: optNull(str),
    kind: optNull(oneOf("container", "vm")),
    arch: optNull(str),
    host: optNull(str),
    hostName: optNull(str),
    power: optNull(object({ control: str, off: bool, turningOn: optNull(bool), error: optNull(str) })),
    cloud: optNull(str),
    cloudPlace: optNull(str),
    cloudDelete: optNull(str),
    progress: optNull(SpaceProgressShape),
  }),
);

const WkSpacesShape = ref(
  "WkSpaces",
  obj<WkSpaces>()({ loaded: bool, selectedId: nullable(str), spaces: list(object({ space: WkSpaceShape, deleting: bool })), rosterError: optNull(str) }),
);

const WkSessionShape = ref(
  "WkSession",
  obj<WkSession>()({
    identity: nullable(str),
    signedIn: bool,
    cloudConfigured: bool,
    signIn: union(oneOf("idle", "starting"), object({ type: str, userCode: optNull(str), message: opt(str) })),
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
              options: list(object({ id: str, label: str, active: bool })),
            }),
          ),
        }),
      ),
    }),
    updateChannel: optNull(oneOf("stable", "beta")),
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
  "spaces.list": WkSpacesShape,
  "spaces.setPower": WkSpacesShape,
  "spaces.delete": WkSpacesShape,
  "spaces.open": nul,
  // The page's wizard runs on `env`, and reads `local`.
  "spaces.createOptions": ref("WkCreateOptions", createOptions(true)),
  "spaces.create": WkSpaceShape,
  "spaces.add": WkSpaceShape,
  "spaces.cancelCreate": OP_SHAPES["spaces.cancelCreate"],
  "machines.list": WkMachinesShape,
  "host.status": nullable(WkHostShape),
  "host.setUp": nullable(WkHostShape),
  "host.action": nullable(WkHostShape),
  "agents.list": OP_SHAPES["agents.list"],
  "agents.runs": OP_SHAPES["agents.runs"],
  "agents.events": OP_SHAPES["agents.events"],
  "agents.pause": any,
  "agents.resume": any,
  "agents.setup": agentSetupRows,
  "agents.configure": agentSetupRows,
  "agents.setupDriver": OP_SHAPES["agents.setupDriver"],
  "agentKeys.list": AgentKeysWire,
  "agentKeys.set": AgentKeysWire,
  "agentKeys.remove": AgentKeysWire,
  "clouds.status": nullable(OP_SHAPES["clouds.status"]),
  "clouds.test": OP_SHAPES["clouds.test"],
  "clouds.connect": OP_SHAPES["clouds.connect"],
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
  "loginItem.get": LoginItemShape,
  "loginItem.set": LoginItemShape,
  "loginItem.openSettings": nul,
  "devices.get": DevicesShape,
  "devices.enroll": OP_SHAPES["devices.enroll"],
  "devices.checkEnrolled": bool,
  "devices.approve": nul,
  "devices.rename": nul,
  "devices.revoke": nul,
  "devices.confirmMachine": nul,
  "storage.get": StorageShape,
  "storage.run": nullable(DriveCheckShape),
  "notifications.list": list(NotificationShape),
  "notifications.markAllRead": nul,
  "telemetry.track": nul,
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
  "startup.get": StartupShape,
  "startup.act": StartupShape,
};

/** Methods whose answer has no shape here, and why (none today: keep the
 * table so a new method can say why before it gets one). */
export const WEBKIT_UNSHAPED: Partial<Record<WebkitMethod, string>> = {};

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
