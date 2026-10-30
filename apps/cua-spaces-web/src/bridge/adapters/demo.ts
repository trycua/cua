// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host: realistic data in memory with simulated latency and state
 * changes (creates step through the SDK's phases, power takes a moment,
 * sign-in finishes in the "browser"). Used when no host is detected.
 */

import type { RiskyRequest } from "../ops/nudges";
import { Emitter, HostError, UnsupportedOperationError, type DataAdapter } from "../adapter";
import type { AgentSetupRow, ConnectionReport, PersistentAgent } from "../contracts/agents";
import type { MachineRow, OnboardingState } from "../contracts/host";
import type { KvItem } from "../contracts/keyvault";
import type { UpdateState } from "../contracts/settings";
import type { BulkResult } from "../contracts/spaces-page";
import type { CreatingElsewhere, SpaceOs, SpaceRow } from "../contracts/spaces";
import {
  DEFAULT_SETTINGS,
  type HostEvent,
  type OpArgs,
  type OpName,
  type OpResult,
  type HostSettings,
  type SessionSnapshot,
  type SettingsSnapshot,
  type SettingsValues,
} from "../protocol";
import {
  DEMO_IDENTITY,
  DEMO_MACHINES,
  demoMachineRows,
  demoKeyvaultItems,
  demoKeyvaultOverview,
  demoDesktopRows,
  demoSpaceRows,
  demoTelemetry,
} from "./demo-data";
import { demoNewSpaceState, newSpaceDemoHandlers, type DemoClouds, type DemoNewSpaceState } from "./demo/new-space";
import { demoShareHandlers } from "./demo/share";
import { demoThreadHandlers } from "./demo/thread";
import { demoAgentDetailHandlers } from "./demo/agent-detail";
import { demoAgentKeysHandlers, demoKeyStore, type DemoKeyStore } from "./demo/agent-keys";
import { demoKeyvaultManageHandlers, demoKeyvaultManageState, withDemoCopies, type DemoKeyvaultManageState } from "./demo/keyvault-manage";
import { demoKeyvaultSetupHandlers, demoKeyvaultSetupState, demoNoVaultOverview, type DemoKeyvaultSetupState } from "./demo/keyvault-setup";
import { unsupportedStreamOps } from "../ops/stream";
import { unsupportedPresenceOps } from "../ops/presence";
import { unsupportedRuntimeFixOps } from "../ops/runtime-fix";
import type { CliInstallPlan } from "../contracts/first-run";
import { unsupportedChecklistOps } from "../ops/checklist";
import { demoTeleportHandlers } from "./demo/teleport";
import { createDemoSettings } from "./demo/settings";
import { demoChooseRuntime, demoRuntimesInput, demoRuntimesState, type DemoRuntimes } from "./demo/runtimes";
import { demoAgentSetup, demoConnections, demoPersistentAgents, demoReply, demoRunRecords, demoStartedRun, runAt, writtenEvents, type DemoRunRecord } from "./demo-agents";
import { demoVolumeHandlers, demoVolumeState, type DemoVolumeState } from "./demo/volume";
import { demoHostSetupHandlers, demoHostSetupState, type DemoHostSetupState } from "./demo/host-setup";
import { demoSpaceDetailHandlers, demoSpaceDetailState, type DemoSpaceDetailState } from "./demo/space-detail";
import { demoTelemetryHandlers, demoTelemetryState, type DemoTelemetryState } from "./demo/telemetry";
import { demoLinksHandlers, demoLinksState, type DemoLinksState } from "./demo/links";
import { demoResourcesHandlers, demoResourcesState, type DemoResourcesState } from "./demo/resources";
import { demoRelayUsageHandlers, demoRelayUsageNamed, demoRelayUsageState, type DemoRelayUsageState } from "./demo/relay-usage";
import { demoStartupHandlers, demoStartupState, type DemoStartupState } from "./demo/startup";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./demo/platform";

export interface DemoOptions {
  /** Round-trip latency of every call (ms). */
  latencyMs?: number;
  /** Time per create phase and per power change (ms). */
  stepMs?: number;
  /** Clock (tests). */
  now?: () => number;
  signedIn?: boolean;
  onboarded?: boolean;
  keyvaultLocked?: boolean;
  /** Start with no Keyvault (`?demo=novault`). */
  noVault?: boolean;
  /** Machines that are only signed in (`?demo=signed-in-machines`): rows in the one list, "Signed in, not sharing", and the two that wait to be added. */
  signedInMachines?: boolean;
  /** Saved credentials the provider refuses (`?demo=rejected-key`: the OpenAI key): a check marks them as needing a reconnect. */
  rejectedKeys?: string[];
  /** Claude Code's sign-in on this Mac: imported already (`?demo=claude-signed-in`), imported and run out (`?demo=claude-expired`),
   * or none to import (`?demo=claude-none`: Connect finds nothing and offers the ways on). Default: there is one, not imported yet. */
  claudeLogin?: "signed-in" | "expired" | "none";
  /** `?demo=chatgpt-signed-in`: a ChatGPT sign-in is saved for Codex (no OpenAI API key unless `no-keys` is left off). */
  chatgptLogin?: boolean;
  /** Which connections are saved: `none` (`?demo=no-keys`, an empty Connections list) or `several` (`?demo=connections`: Anthropic, OpenRouter, a custom endpoint and a DeepSeek key the provider refuses). Default: an OpenAI key. */
  keys?: "none" | "several";
  /** Start with no Spaces (the first-Space empty state). */
  noSpaces?: boolean;
  /** `?demo=desktops`: the Linux box also shares its desktop (a Space row of its own, `relay:linux-box`). */
  desktops?: boolean;
  /** This many more Spaces on the list, for scale (`?demo=scale-1000`;
   * scripts/scale-bench.mjs). */
  extraSpaces?: number;
  /** An account in no team (`?demo=no-team`): Share explains that sharing is for teams. */
  noTeam?: boolean;
  /** A team with no free computers (`?demo=team-full`): adding a machine to it is refused, as the control plane does at one computer per seat. Simulated: the demo has no control plane. */
  teamFull?: boolean;
  /** Start at the macOS Keychain prompt (the startup screen). */
  keychain?: boolean;
  /** Answer New Space's options as the SwiftUI host does (`demo/new-space.ts`). */
  macHost?: boolean;
  /** Start with AWS and Hetzner connected: `connected` (`?demo=clouds`), or with one thing wrong:
   * `budget` (AWS's budget is used), `untrusted` (AWS lost its trust), `token` (Hetzner's token was
   * refused) or `approval` (an agent's AWS link waits for approval). */
  clouds?: DemoClouds;
  /** 14 more computers on the account, for a long list (`?demo=clouds-many`). */
  manyMachines?: boolean;
  /** This device is not enrolled on the relay yet (the enroll bar above every page). */
  unenrolled?: boolean;
  /** The relay allowance: `idle` (free, no live session, so prompts can show),
   * `warn` (80% of the hours), `limit` (used up), `pro` or `teams`. */
  usage?: "idle" | "warn" | "limit" | "pro" | "teams";
  /** A connection status to answer `agents.status` with as is (a real
   * `cua connect status --json`, for screenshots of real states); none: the
   * demo's own agents decide it. */
  connections?: ConnectionReport;
  /** Which agents are on this computer (`?demo=agents-none`, `?demo=agents-codex`); all of them by default. `unset`: all found, none set up for Cua yet, without the first-run onboarding. */
  agents?: "none" | "codex" | "unset";
  /** Coding agents whose config file Cua can't write (setup fails with a permission error), by id. */
  agentsReadOnly?: string[];
  /** This Mac's free disk: `low` (`?demo=low-disk`) or `full` (`?demo=disk-full`). */
  disk?: "low" | "full";
  /** The machine the demo runs on: a Mac on Apple silicon unless set (the
   * Electron shell passes its own system and architecture). */
  platform?: DemoPlatform;
  /** Where the updater is at start (`?demo=update-ready`, `?demo=update-downloading`, `?demo=update-error`). */
  update?: Partial<UpdateState>;
  /** A This Mac prompt to show (`?demo=nudge-second-agent`, `?demo=nudge-risky`). */
  nudge?: "second-agent" | "risky";
  /** Creates fail as a real host's do: `runtime-once` (the first create's
   * built-in Linux runtime does not start; later attempts work,
   * `?demo=create-fail-runtime`) or `disk` (every create is short of disk
   * space, `?demo=create-fail-disk`). */
  failCreates?: "runtime-once" | "disk";
  /** The CLI the demo host reports: the released app's `cua` on PATH unless
   * set; `local` is a Local build's (`?demo=cli-local`), `missing` one
   * not installed (`?demo=cli-missing`). */
  cli?: "local" | "missing";
  /** A daemon without the Space thread (`?demo=nothread`): the Space page keeps its old layout. */
  noThread?: boolean;
  /** You in the demo's thread as someone the Space is shared with: `guest` (`?demo=thread-guest`, view only) or `member` (`?demo=thread-member`); the owner by default. */
  threadRole?: "member" | "guest";
  /** Extra scenes in the demo's thread: `?demo=thread-lookalike` (CUA-2262). */
  threadLookalike?: boolean;
  /** `?demo=thread-auth-failed`: Claude and Hermes answer "hi" with "not connected" (CUA-2638). */
  threadAuthFailed?: boolean;
  /** `?demo=thread-paused`: agents wake each other until the daemon pauses agent to agent messages (CUA-2757). */
  threadPaused?: boolean;
  /** `?demo=thread-stranger`: an agent whose owner is not in the Space (its card opens their profile in place). */
  threadStranger?: boolean;
  /** Join codes: redeemed this long after they are made (null: never, `?demo=thread-join-slow`). */
  joinRedeemMs?: number | null;
  /** Join codes expire this long after they are made (`?demo=thread-join-expire`). */
  joinExpireMs?: number;
  /** Linus's OpenClaw asks to join (`?demo=thread-request`). */
  threadRequest?: boolean;
  /** Another of your computers is creating a Space (`?demo=creating-elsewhere`):
   * the Mac mini, a macOS Space 40% of the way (CUA-2349). */
  creatingElsewhere?: boolean;
}

/** What the Mac mini is creating (`?demo=creating-elsewhere`). */
export const DEMO_CREATING_ELSEWHERE: CreatingElsewhere[] = [
  { machineId: "mac-mini", machineName: "Mac mini", name: "browser-qa", os: "macos", phase: "pulling", percent: 40 },
];

/** The SDK's words for the failures `failCreates` plays. */
export const DEMO_CREATE_FAILURES = {
  runtime: "local runtime: the built-in Linux runtime did not start: exiting, status 1",
  disk: "not enough disk space to pull ghcr.io/trycua/macos:26: it needs about 30.0 GB and cua keeps 5.0 GB free, but only 12.4 GB is available on /Users/me/.lume (run `cua cache prune` to free space, or lower CUA_DISK_MIN_FREE)",
} as const;

const DEMO_NOTES = "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3";

/** `?demo=fresh` (signed out, first run), `?demo=no-team`, `?demo=team-full`, `?demo=slow` (slow answers), `?demo=locked` (Keyvault locked), `?demo=novault` (no Keyvault yet),
 * `?demo=empty` (no Spaces yet), `?demo=nothread` (a daemon without the Space thread), `?demo=thread-lookalike` (a look-alike agent in the thread), `?demo=thread-request` (an agent asks to join), `?demo=thread-join-slow` / `?demo=thread-join-expire` (join codes never redeemed / expiring), `?demo=keychain` (the startup screen),
 * `?demo=mac-host` (New Space options and the Keyvault's Settings rows in the
 * SwiftUI host's shape), `?demo=unenrolled` (this device not enrolled),
 * `?demo=agents-readonly` (Cua can't write Hermes's config),
 * `?demo=claude-signed-in` / `claude-expired` / `claude-none` (Claude Code's sign-in imported, run out, or not there to import),
 * `?demo=update-ready` (an update is downloaded), `?demo=update-downloading`, `?demo=update-error`,
 * `?demo=clouds` (AWS and Hetzner connected; `clouds-budget`, `clouds-untrusted`,
 * `clouds-token`, `clouds-approval`: one thing wrong with one of them;
 * `clouds-cli`: AWS linked through the AWS CLI's sign-in, offering OIDC;
 * `clouds-limits`: an agent asked to raise AWS's limits; `clouds-revoked`:
 * AWS was disconnected on another device while machines still run),
 * `?demo=low-disk` / `?demo=disk-full` (This Mac almost out of space: New
 * Space and the first Space offers see it too), and `?demo=intel-mac` /
 * `?demo=linux-pc` (an Intel Mac, an x64 Linux PC with QEMU), and
 * `?demo=desktops` (the Linux box shares its desktop, a shared desktop row beside the Spaces), and
 * `?demo=creating-elsewhere` (the Mac mini is creating a Space). */
/** The account's signed-in machines that share nothing, as a host lists them (`?demo=signed-in-machines`): one enrolled, and the demo devices that wait. */
function demoSignedInRows(nowMs: number): MachineRow[] {
  const t = Math.floor(nowMs / 1000);
  return [
    { id: "dev_laptop", name: "Ana's laptop", via: "relay", online: false, os: "macos", device: true, deviceState: "enrolled", lastSeen: t - 2 * 3600, limits: [] },
    { id: "dev_work", name: "Work laptop", via: "relay", online: false, os: "windows", device: true, deviceState: "pending", lastSeen: t - 120, limits: [] },
    { id: "dev_old", name: "Old laptop", via: "relay", online: false, os: "linux", device: true, deviceState: "expired", lastSeen: t - 33 * 86_400, limits: [] },
  ];
}

/** `count` plain Spaces for scale: alternating systems, places and states,
 * each with its own id, name and time (`?demo=scale-N`). */
export function demoScaleRows(now: number, count: number): SpaceRow[] {
  const oses = ["linux", "macos", "windows"] as const;
  return Array.from({ length: count }, (_, i) => {
    const os = oses[i % 3]!;
    const on = i % 4 !== 0;
    return {
      id: `local:scale-${i}`,
      name: `scale-${String(i).padStart(4, "0")}`,
      provider: "local",
      spacesdVersion: "0.6.0",
      features: [],
      addedAt: new Date(now - i * 60_000).toISOString(),
      os,
      osName: os === "linux" ? "Ubuntu" : os === "macos" ? "macOS" : "Windows",
      kind: os === "macos" ? "vm" : "container",
      arch: "arm64",
      reachable: true,
      power: "suspend",
      powerState: on ? "running" : "stopped",
    } as SpaceRow;
  });
}

export function demoOptionsFromSearch(search: string): DemoOptions {
  const flags = new Set((new URLSearchParams(search).get("demo") ?? "").split(",").filter(Boolean));
  return {
    // `?demo=slow`: every answer takes a few seconds (the first run's "Checking" in screenshots).
    ...(flags.has("slow") ? { latencyMs: 4000 } : {}),
    signedIn: !flags.has("fresh"),
    onboarded: !flags.has("fresh"),
    keyvaultLocked: flags.has("locked"),
    noVault: flags.has("novault"),
    ...(flags.has("signed-in-machines") ? { signedInMachines: true } : {}),
    ...(flags.has("rejected-key") ? { rejectedKeys: ["OPENAI_API_KEY"] } : {}),
    ...(flags.has("no-keys") ? { keys: "none" as const } : flags.has("connections") ? { keys: "several" as const, rejectedKeys: ["DEEPSEEK_API_KEY"] } : {}),
    ...(flags.has("claude-expired") ? { claudeLogin: "expired" as const } : flags.has("claude-signed-in") ? { claudeLogin: "signed-in" as const } : flags.has("claude-none") ? { claudeLogin: "none" as const } : {}),
    ...(flags.has("chatgpt-signed-in") ? { chatgptLogin: true } : {}),
    noSpaces: flags.has("empty"),
    desktops: flags.has("desktops"),
    ...(() => {
      const scale = [...flags].find((f) => /^scale-\d+$/.test(f));
      return scale ? { extraSpaces: Math.min(5000, Number(scale.slice(6))) } : {};
    })(),
    noTeam: flags.has("no-team"),
    teamFull: flags.has("team-full"),
    noThread: flags.has("nothread"),
    ...(flags.has("thread-guest") ? { threadRole: "guest" as const } : flags.has("thread-member") ? { threadRole: "member" as const } : {}),
    threadLookalike: flags.has("thread-lookalike"),
    threadAuthFailed: flags.has("thread-auth-failed"),
    threadStranger: flags.has("thread-stranger"),
    threadPaused: flags.has("thread-paused"),
    threadRequest: flags.has("thread-request"),
    ...(flags.has("thread-join-slow") || flags.has("thread-join-expire") ? { joinRedeemMs: null } : {}),
    ...(flags.has("thread-join-expire") ? { joinExpireMs: 6_000 } : {}),
    keychain: flags.has("keychain"),
    macHost: flags.has("mac-host"),
    clouds: flags.has("clouds-budget")
      ? "budget"
      : flags.has("clouds-untrusted")
        ? "untrusted"
        : flags.has("clouds-token")
          ? "token"
          : flags.has("clouds-cli")
            ? "cli"
            : flags.has("clouds-limits")
              ? "limits"
              : flags.has("clouds-revoked")
                ? "revoked"
                : flags.has("clouds-approval")
            ? "approval"
            : flags.has("clouds") || flags.has("clouds-many")
              ? "connected"
              : undefined,
    manyMachines: flags.has("clouds-many"),
    unenrolled: flags.has("unenrolled"),
    usage: (["idle", "warn", "limit", "pro", "teams"] as const).find((u) => flags.has(`usage-${u}`)),
    agents: (["none", "codex"] as const).find((a) => flags.has(`agents-${a}`)),
    // A captured `cua connect status --json` (screenshots of real states).
    connections: (globalThis as { __CUA_DEMO_CONNECTIONS?: ConnectionReport }).__CUA_DEMO_CONNECTIONS,
    // `?demo=agents-readonly`: Hermes's config file can't be written, so setting it up fails.
    ...(flags.has("agents-readonly") ? { agentsReadOnly: ["hermes"] } : {}),
    nudge: (["second-agent", "risky"] as const).find((n) => flags.has(`nudge-${n}`)),
    disk: flags.has("disk-full") ? "full" : flags.has("low-disk") ? "low" : undefined,
    cli: flags.has("cli-local") ? "local" : flags.has("cli-missing") ? "missing" : undefined,
    creatingElsewhere: flags.has("creating-elsewhere"),
    failCreates: flags.has("create-fail-runtime") ? "runtime-once" : flags.has("create-fail-disk") ? "disk" : undefined,
    ...(flags.has("intel-mac") ? { platform: { os: "macos" as const, arch: "amd64" as const } } : {}),
    ...(flags.has("linux-pc") ? { platform: { os: "linux" as const, arch: "amd64" as const } } : {}),
    ...(flags.has("update-ready")
      ? { update: { status: "downloaded" as const, version: "0.9.3", percent: 100, notesUrl: DEMO_NOTES } }
      : flags.has("update-downloading")
        ? { update: { status: "downloading" as const, version: "0.9.3", percent: 42, notesUrl: DEMO_NOTES } }
        : flags.has("update-error")
          ? { update: { status: "error" as const, message: "The network connection was lost." } }
          : {}),
  };
}

/** The SwiftUI host's Keyvault settings (`?demo=mac-host`). */
const DEMO_MAC_HOST_SETTINGS: HostSettings = {
  keyvaultAutoWipe: false,
  keyvaultUnlockPrompt: true,
  keyvaultSiteIcons: true,
  keyvaultProtection: [
    { label: "Touch ID", value: "Asked by the Cua daemon to widen access" },
    { label: "Unlock", value: "Mac login" },
    { label: "This app", value: "Verified as Cua" },
  ],
};

const DEMO_KEYVAULT_ROWS: Partial<Record<string, "keyvaultAutoWipe" | "keyvaultUnlockPrompt" | "keyvaultSiteIcons">> = {
  "keyvault-auto-wipe": "keyvaultAutoWipe",
  "keyvault-unlock-prompt": "keyvaultUnlockPrompt",
  "keyvault-site-icons": "keyvaultSiteIcons",
};

/** Simulated: one more of the account's computers (`?demo=clouds-many`). */
function demoExtraMachine(i: number): MachineRow {
  const n = String(i + 1).padStart(2, "0");
  const os = (["macos", "linux", "windows"] as const)[i % 3]!;
  return { id: `build-${n}`, name: `build-${os === "macos" ? "mini" : os === "linux" ? "box" : "pc"}-${n}`, via: "relay", online: i % 5 !== 4, os, limits: [] };
}

const CREATE_PHASES = ["preparing", "pulling", "creating", "booting", "waiting_for_services", "connecting"];

type Handler<K extends OpName> = (args: OpArgs<K>) => Promise<OpResult<K>> | OpResult<K>;

export function createDemoAdapter(options: DemoOptions = {}): DataAdapter & { readonly state: DemoState } {
  const latency = options.latencyMs ?? 120;
  const step = options.stepMs ?? 700;
  const now = options.now ?? Date.now;
  const events = new Emitter();
  const timers = new Set<ReturnType<typeof setTimeout>>();

  const state: DemoState = {
    rows: options.noSpaces ? [] : [...demoSpaceRows(now()), ...(options.desktops ? demoDesktopRows(now()) : []), ...demoScaleRows(now(), options.extraSpaces ?? 0)],
    items: demoKeyvaultItems(now()),
    keyvaultLocked: Boolean(options.keyvaultLocked),
    keyvaultDisabled: false,
    pendingRequests: true,
    revokedGrants: new Set(),
    // Kept as the Mac and Electron apps keep the core's settings: tips and the Pro banner's close.
    settings: { ...DEFAULT_SETTINGS, launchAtLogin: true, updateChannel: "stable", tips: { off: false, done: [] }, notchPinned: [] },
    identity: options.signedIn === false ? undefined : DEMO_IDENTITY,
    onboarding: { completed: options.onboarded !== false, mode: options.onboarded === false ? null : "client" },
    creates: new Map(),
    agents: demoPersistentAgents(now()),
    // A named allowance is for showing the prompts, which wait for no agent run.
    runs: options.usage ? [] : demoRunRecords(now()),
    imported: [],
    keyStore: demoKeyStore(now, options.rejectedKeys ?? [], options.claudeLogin, options.keys, options.chatgptLogin),
    readOnlyAgents: options.agentsReadOnly ?? [],
    agentSetup: demoAgentSetup(options.onboarded === false || options.agents === "unset", options.agents === "none" ? [] : options.agents === "codex" ? ["codex"] : null),
    platform: options.platform ?? MAC_DEMO_PLATFORM,
    paired: [],
    renamed: {},
    removed: [],
    hostSettings: options.macHost ? DEMO_MAC_HOST_SETTINGS : undefined,
    runtimes: demoRuntimesState(),
    volume: demoVolumeState(now(), options.onboarded !== false, options.platform),
    ...demoTelemetryState(),
    ...demoRelayUsageState(),
    links: demoLinksState(),
    ...demoResourcesState(options.disk ?? "healthy"),
    ...(options.usage ? { relayUsage: demoRelayUsageNamed(options.usage) } : {}),
    ...demoSpaceDetailState(),
    ...demoHostSetupState(now(), options.onboarded === false),
    ...demoStartupState(Boolean(options.keychain)),
    ...demoNewSpaceState(Boolean(options.macHost), options.clouds ?? null),
    ...demoKeyvaultSetupState(!options.noVault),
    ...demoKeyvaultManageState(),
  };

  if (state.platform.os !== "macos") state.host = { ...state.host, name: demoMachine(state.platform).name };
  // Claude Code has been on the desktop for a while; Codex just started.
  if (options.nudge === "second-agent") {
    const t = now();
    const agent = (name: string, id: string) => `${DEMO_IDENTITY} (acct-ada) \u00b7 agent ${name} (${id})`;
    state.host = {
      ...state.host,
      recentAccess: [
        { atMs: t - 5_000, via: "relay", who: agent("Codex", "agent:codex-5120"), what: "DriverService" },
        { atMs: t - 20_000, via: "relay", who: agent("Claude Code", "agent:claude-code-4242"), what: "ComputerService" },
        { atMs: t - 12 * 60_000, via: "relay", who: agent("Claude Code", "agent:claude-code-4242"), what: "DriverService" },
        ...(state.host.recentAccess ?? []),
      ],
    };
  }
  let risky: RiskyRequest[] =
    options.nudge === "risky" ? [{ id: "risky-4242-1", agent: "Claude Code", action: "run an installer", atMs: now(), expiresMs: now() + 30_000 }] : [];

  const later = (ms: number, f: () => void) => {
    const t = setTimeout(() => {
      timers.delete(t);
      f();
    }, ms);
    timers.add(t);
    return t;
  };
  const wait = (ms: number) => new Promise<void>((r) => later(ms, r));
  const emit = (e: HostEvent) => events.emit(e);
  const findRow = (id: string) => {
    const row = state.rows.find((r) => r.id === id);
    if (!row) throw new HostError(`no Space ${id}`, "not_found");
    return row;
  };
  const replaceRow = (id: string, patch: Partial<SpaceRow>) => {
    state.rows = state.rows.map((r) => (r.id === id ? { ...r, ...patch } : r));
  };
  // The bundled CLI (`?demo=cli-missing`: a disk-image install, none on PATH yet).
  const cliName = options.cli === "local" ? "cua-local" : "cua";
  let cliInstalled = options.cli !== "missing";
  const cliPlan = (): CliInstallPlan => {
    const target = `/Users/me/.local/bin/${cliName}`;
    return {
      source: `/Applications/Cua Spaces.app/Contents/MacOS/cua`,
      target,
      binDir: "/Users/me/.local/bin",
      method: "symlink",
      installed: cliInstalled,
      upToDate: cliInstalled,
      onPath: cliInstalled,
      pathProfile: cliInstalled ? null : "/Users/me/.zshrc",
      pathLine: cliInstalled ? null : 'export PATH="/Users/me/.local/bin:$PATH"',
      found: cliInstalled ? target : null,
    };
  };
  const settingsSnapshot = (): SettingsSnapshot => ({
    values: { ...state.settings },
    defaultLocation: { value: state.settings.defaultLocation, source: "config", path: "~/.cua/config.toml" },
    telemetry: demoTelemetry(state.settings.telemetry),
    hostSettings: { ...state.hostSettings, runtimes: demoRuntimesInput(state.runtimes) },
    // The macOS hosts register ⌘⇧Space; the Windows and Linux shells have none.
    globalShortcut: state.platform.os === "macos" ? { label: state.settings.hotkey, registered: true } : null,
    // What a host's CLI install plan names (`CuaChannel::cli_name`): Local's is `cua-local`.
    cli: { name: cliName, installed: cliInstalled },
  });
  const overview = () => {
    if (!state.keyvaultSetUp) return demoNoVaultOverview(now());
    const o = demoKeyvaultOverview(now(), state.items, state.keyvaultLocked);
    if (o.status) o.status.disabled = state.keyvaultDisabled;
    if (!state.pendingRequests) {
      o.pending = [];
      if (o.status) o.status.pending = 0;
    }
    o.grants = o.grants.filter((g) => !state.revokedGrants.has(g.id));
    return withDemoCopies(o, state, now());
  };
  const session = (): SessionSnapshot => ({
    account: {
      configured: Boolean(state.identity),
      authMode: state.identity ? "user" : "none",
      identity: state.identity,
    },
    onboarding: { ...state.onboarding },
    daemon: { connected: true, version: "0.6.0", socketPath: "~/.cua/daemon.sock" },
  });

  const findAgent = (name: string) => {
    const agent = state.agents.find((a) => a.name === name);
    if (!agent) throw new HostError(`no persistent agent named ${name}`, "not_found");
    return agent;
  };
  const replaceAgent = (name: string, patch: Partial<PersistentAgent>) => {
    state.agents = state.agents.map((a) => (a.name === name ? { ...a, ...patch } : a));
  };

  const ctx = { state, now, wait, emit, findRow, stepMs: step };
  // Creates asked for so far (`failCreates: "runtime-once"` fails the first).
  let createAttempts = 0;
  const handlers: { [K in OpName]: Handler<K> } = {
    "spaces.list": () => state.rows.map((r) => ({ ...r })),

    "spaces.create": async ({ config, pendingId }) => {
      const os: SpaceOs = /mac/i.test(config.image ?? "") ? "macos" : /win/i.test(config.image ?? "") ? "windows" : "linux";
      const on = config.on ?? state.settings.defaultLocation;
      const host = on.startsWith("host:") ? DEMO_MACHINES.find((m) => m.id === on.slice(5)) : undefined;
      const base = (config.name?.trim() || `${os === "macos" ? "mac" : os}-${Math.random().toString(36).slice(2, 6)}`)
        .toLowerCase()
        .replace(/[^a-z0-9-]+/g, "-");
      const id = host ? `relay:${host.id}/${base}` : `local:${base}`;
      const job = { cancelled: false };
      state.creates.set(pendingId, job);
      try {
        // A failure the options ask for, after the create starts preparing.
        const failure =
          options.failCreates === "disk" ? DEMO_CREATE_FAILURES.disk : options.failCreates === "runtime-once" && createAttempts++ === 0 ? DEMO_CREATE_FAILURES.runtime : null;
        if (failure) {
          for (const fraction of [0, 0.3]) {
            emit({ type: "spaces.createProgress", progress: { pendingId, phase: "preparing", fraction, detail: "preparing", space: id } });
            await wait(step / 2);
          }
          throw new HostError(failure, "runtime");
        }
        for (const phase of CREATE_PHASES) {
          for (const fraction of [0, 0.5]) {
            if (job.cancelled) throw new HostError("cancelled: the create was stopped and what it made was removed", "cancelled");
            emit({
              type: "spaces.createProgress",
              progress: {
                pendingId,
                phase,
                fraction,
                detail: phase,
                space: id,
                ...(phase === "pulling" ? { bytesDone: fraction * 4.2e9, bytesTotal: 4.2e9, bytesPerSecond: 85e6 } : {}),
              },
            });
            await wait(step / 2);
          }
        }
        if (job.cancelled) throw new HostError("cancelled: the create was stopped and what it made was removed", "cancelled");
        const row: SpaceRow = {
          id,
          name: base,
          provider: host ? "relay" : "local",
          spacesdVersion: "0.6.0",
          features: ["desktop_stream", "window_stream", "audio.desktop"],
          addedAt: new Date(now()).toISOString(),
          os,
          image: config.image,
          kind: os === "linux" ? "container" : "vm",
          arch: "arm64",
          reachable: true,
          host: host?.id,
          hostName: host?.name,
          power: os === "linux" ? "stop" : "suspend",
          powerState: "running",
        };
        state.rows = [...state.rows, row];
        emit({ type: "spaces.changed" });
        return row;
      } finally {
        state.creates.delete(pendingId);
      }
    },

    "spaces.cancelCreate": async ({ pendingId }) => {
      const job = state.creates.get(pendingId);
      if (!job) return { id: pendingId, state: "not_creating", message: "Nothing was being created." };
      job.cancelled = true;
      return { id: pendingId, state: "cancelled", message: "Stopped. Nothing was left behind." };
    },

    "spaces.setPower": async ({ spaceId, on }) => {
      const row = findRow(spaceId);
      if (!row.power) throw new HostError("This Space can't be turned off.", "unsupported");
      await wait(step * 2);
      const next = on ? "running" : row.power === "stop" ? "stopped" : "suspended";
      replaceRow(spaceId, { powerState: next, reachable: on });
      emit({ type: "spaces.changed" });
      return { space: spaceId, state: next, power: row.power, message: on ? "Started" : row.power === "stop" ? "Stopped" : "Suspended" };
    },

    "spaces.delete": async ({ spaceId }) => {
      findRow(spaceId);
      await wait(step * 2);
      state.rows = state.rows.filter((r) => r.id !== spaceId);
      emit({ type: "spaces.changed" });
      return spaceId;
    },

    "spaces.deleteMany": async ({ ids }) => {
      const results: BulkResult[] = [];
      for (const id of ids) {
        const row = state.rows.find((r) => r.id === id);
        const theirs = row?.sharing && row.sharing.role !== "owner";
        if (!row) results.push({ id, ok: false, error: `No Space ${id}.` });
        else if (theirs) results.push({ id, ok: false, error: `Couldn't delete ${row.name}: it isn't yours.` });
        else {
          await wait(step);
          state.rows = state.rows.filter((r) => r.id !== id);
          results.push({ id, ok: true });
          emit({ type: "spaces.changed" });
        }
      }
      return { results };
    },

    "spaces.leave": async ({ ids }) => {
      const results: BulkResult[] = [];
      for (const id of ids) {
        const row = state.rows.find((r) => r.id === id);
        if (!row) results.push({ id, ok: false, error: `Couldn't leave ${id}: it isn't listed.` });
        else if (!row.sharing || row.sharing.role === "owner") results.push({ id, ok: false, error: `Couldn't leave ${row.name}: it's yours; delete it instead.` });
        else {
          await wait(step);
          state.rows = state.rows.filter((r) => r.id !== id);
          results.push({ id, ok: true });
          emit({ type: "spaces.changed" });
        }
      }
      return { results };
    },

    "spaces.rename": async ({ spaceId, name }) => {
      findRow(spaceId);
      const next = name.trim();
      if (!/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(next)) {
        throw new HostError("Use lowercase letters, digits and dashes in a Space's name, starting and ending with a letter or digit.", "invalid_argument");
      }
      if (state.rows.some((r) => r.id !== spaceId && r.name === next)) {
        throw new HostError(`Another Space is already called ${next}. Pick another name.`, "invalid_argument");
      }
      await wait(step);
      replaceRow(spaceId, { name: next });
      emit({ type: "spaces.changed" });
      return null;
    },

    // There is no window to open in the browser; the Space only has to exist.
    "spaces.open": ({ spaceId }) => {
      findRow(spaceId);
      return null;
    },

    "machines.list": () =>
      [
        ...demoMachineRows(now(), state.host, state.platform),
        ...(options.manyMachines ? Array.from({ length: 14 }, (_, i) => demoExtraMachine(i)) : []),
        ...state.paired.map((m) => ({ ...m, limits: [...m.limits] })),
        ...(options.signedInMachines ? demoSignedInRows(now()) : []),
      ]
        .filter((m) => !state.removed.includes(m.id))
        .map((m) => (state.renamed[m.id] ? { ...m, name: state.renamed[m.id]! } : m)),
    "machines.rename": async ({ id, name }) => {
      await wait(step / 2);
      state.renamed = { ...state.renamed, [id]: name };
      emit({ type: "machines.changed" });
      return null;
    },
    // The demo machines are drawn by the core from their rows, with no
    // daemon to ask: nothing to check.
    "machines.checkHealth": () => null,
    "machines.remove": async ({ machine, device }) => {
      await wait(step / 2);
      state.removed = [...state.removed, ...[machine, device].filter((x): x is string => Boolean(x))];
      emit({ type: "machines.changed" });
      return null;
    },
    "host.status": () => ({ ...state.host }),

    "settings.get": () => settingsSnapshot(),
    "settings.set": ({ key, value }) => {
      state.settings = { ...state.settings, [key]: value } as SettingsValues;
      emit({ type: "settings.changed" });
      return settingsSnapshot();
    },
    // The host's own rows: only the Keyvault's switches, as `?demo=mac-host`.
    "settings.choose": async ({ row, option }) => {
      const key = DEMO_KEYVAULT_ROWS[row];
      if (key && state.hostSettings) state.hostSettings = { ...state.hostSettings, [key]: option === "on" };
      // Settings, Advanced's runtime rows: the kind's source or its path.
      if (demoChooseRuntime(state.runtimes, row, option)) await wait(step);
      // An AI agents row's button (the SwiftUI host's `press`): set it up, or remove what cua added.
      const agent = row.startsWith("agent:") ? state.agentSetup.find((r) => `agent:${r.agent}` === row && r.installed) : undefined;
      if (agent) {
        await wait(step);
        const on = !agent.configured;
        state.agentSetup = state.agentSetup.map((r) =>
          r === agent ? { ...r, configured: on, skillsInstalled: on ? r.skillsTotal : 0, detail: on ? "Skills and MCP server set up" : "Not set up" } : r,
        );
      }
      return settingsSnapshot();
    },

    "keyvault.overview": () => overview(),
    "keyvault.unlock": async ({ passphrase }) => {
      if (passphrase !== undefined && passphrase !== null && passphrase.length < 4) {
        throw new HostError("That passphrase doesn't unlock this Keyvault.", "bad_passphrase");
      }
      await wait(step);
      state.keyvaultLocked = false;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.setUnattended": ({ itemIds, unattended }) => {
      const idp = state.items.find((i) => itemIds.includes(i.id) && i.identity_provider);
      if (unattended && idp) throw new HostError(`${idp.domain ?? idp.app_display} signs you in elsewhere, so it never runs unattended.`, "identity_provider");
      state.items = state.items.map((i): KvItem =>
        itemIds.includes(i.id) ? { ...i, policy: { ...i.policy, unattended }, rev: i.rev + 1, updated_ms: now() } : i,
      );
      emit({ type: "keyvault.changed" });
      return state.items.filter((i) => itemIds.includes(i.id));
    },
    "keyvault.setDisabled": ({ disabled }) => {
      state.keyvaultDisabled = disabled;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.approve": ({ requestId, items }) => {
      state.pendingRequests = false;
      emit({ type: "keyvault.changed" });
      return {
        id: `grant-${requestId}`,
        request_id: requestId,
        caller_fp: "fp-claude",
        caller_display: "Claude Code",
        items: items ?? ["kv-gh-session", "kv-gh-sess", "kv-gh-theme"],
        targets: ["local:design-review"],
        actions: ["deliver"],
        created_ms: now(),
        not_after_ms: now() + 3_600_000,
        revoked: false,
      };
    },
    "keyvault.deny": () => {
      state.pendingRequests = false;
      emit({ type: "keyvault.changed" });
      return null;
    },
    "keyvault.revokeGrant": ({ id }) => {
      state.revokedGrants.add(id);
      emit({ type: "keyvault.changed" });
      return 1;
    },

    "session.get": () => session(),
    "session.signIn": () => {
      later(step * 3, () => {
        state.identity = DEMO_IDENTITY;
        emit({ type: "session.signedIn", identity: DEMO_IDENTITY });
      });
      return { method: "browser", verificationUri: "https://cua.ai/signin" };
    },
    "session.signOut": () => {
      state.identity = undefined;
      emit({ type: "session.signedOut" });
      return null;
    },
    "session.completeOnboarding": ({ mode }) => {
      state.onboarding = { ...state.onboarding, completed: true, mode };
      return null;
    },
    "session.openExternal": ({ url }) => {
      globalThis.window?.open?.(url, "_blank", "noopener");
      return null;
    },

    "agents.list": () => state.agents.map((a) => ({ ...a })),
    "agents.runs": ({ spaceId }) => {
      const row = findRow(spaceId);
      if (row.powerState !== "running" || !row.reachable) throw new HostError(`${row.name} is not running`, "space_off");
      return state.runs
        .filter((r) => r.space === spaceId)
        .map((r) => runAt(r, now()))
        .sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
    },
    "agents.events": ({ spaceId, runId, cursor, max }) => {
      const rec = state.runs.find((r) => r.space === spaceId && r.run.runId === runId);
      if (!rec) throw new HostError(`no run ${runId} in ${spaceId}`, "not_found");
      const written = writtenEvents(rec, now());
      const after = written.filter((e) => e.seq > cursor);
      const page = after.slice(0, max ?? 100);
      const run = runAt(rec, now());
      return {
        run_id: runId,
        status: run.status,
        phase: run.phase,
        events: page,
        cursor: page.length ? page[page.length - 1]!.seq : cursor,
        caught_up: page.length === after.length,
      };
    },
    "agents.start": async ({ spaceId, agent, prompt, repo }) => {
      // This Mac's own Space id (its host service shares it on the relay) runs too, like the core's.
      const own = state.host.configured && state.host.sharing && state.host.machineId ? `relay:${state.host.machineId}` : null;
      if (spaceId !== own) {
        const row = findRow(spaceId);
        if (row.powerState !== "running" || !row.reachable) throw new HostError(`${row.name} is not running`, "space_off");
      }
      await wait(step);
      const runId = `run-${Math.random().toString(16).slice(2, 8)}`;
      state.runs.push(demoStartedRun(spaceId, runId, agent, prompt, now(), repo));
      emit({ type: "agents.changed" });
      return { run_id: runId, agent, space: spaceId };
    },
    "agents.message": async ({ spaceId, runId, text }) => {
      const rec = state.runs.find((r) => r.space === spaceId && r.run.runId === runId);
      if (!rec) throw new HostError(`no run ${runId} in ${spaceId}`, "not_found");
      const busy = runAt(rec, now()).status === "running";
      demoReply(rec, text, now());
      emit({ type: "agents.changed" });
      const run = runAt(rec, now());
      return { delivered: true, run_id: runId, status: run.status, phase: run.phase, reason: busy ? "queued after the current turn" : "" };
    },
    "agents.pause": async ({ name }) => {
      const agent = findAgent(name);
      await wait(step);
      const rec = state.runs.find((r) => r.run.runId === agent.runId);
      if (rec && rec.live && rec.stoppedAt === undefined && writtenEvents(rec, now()).length < rec.events.length) {
        // Pausing stops the run: what was not written yet never will be.
        const t = now();
        const written = writtenEvents(rec, t);
        const last = written[written.length - 1];
        rec.events = [
          ...written,
          { seq: (last?.seq ?? 0) + 1, ts_ms: t, turn: last?.turn ?? 0, kind: "turn_ended", stop_reason: "cancelled", category: "activity", summary: `Turn ${last?.turn ?? 0} ended (cancelled)` },
        ];
        rec.stoppedAt = t;
      }
      const local = agent.space.startsWith("local:");
      replaceAgent(name, { paused: true, runId: null, spaceState: local ? "suspended" : "released", savedMs: now() });
      emit({ type: "agents.changed" });
      return null;
    },
    "agents.resume": async ({ name }) => {
      findAgent(name);
      await wait(step);
      replaceAgent(name, { paused: false, spaceState: "running" });
      emit({ type: "agents.changed" });
      return null;
    },
    "agents.setup": () => state.agentSetup.map((r) => ({ ...r })),
    "agents.status": () => options.connections ?? demoConnections(state.agentSetup, state.imported, now()),
    "agents.connect": async ({ agents, signIn, skills, mcp }) => {
      await wait(step);
      // Only Cua's skills or only its MCP server (an app's page's switches).
      const part = skills === false || mcp === false;
      // `?demo=agents-readonly`: an app whose config Cua can't write fails, as with `agents.configure`.
      state.agentSetup = state.agentSetup.map((r) =>
        r.installed && agents.includes(r.agent)
          ? state.readOnlyAgents.includes(r.agent)
            ? { ...r, configured: false, skillsInstalled: 0, detail: "Permission denied (os error 13)" }
            : part
              ? { ...r, configured: false, skillsInstalled: skills === false ? 0 : r.skillsTotal, detail: skills === false ? "MCP server set up" : "Skills set up" }
              : { ...r, configured: true, skillsInstalled: r.skillsTotal, detail: "Skills and MCP server set up" }
          : r,
      );
      // Claude Code's own sign-in, read from where Claude Code keeps it (macOS asks to allow it), then its
      // access token is saved with the keys. `?demo=claude-none`: there is nothing to read.
      const claudeFound = options.claudeLogin !== "none";
      if (signIn && agents.includes("claude-code") && claudeFound) {
        await wait(step * 6);
        state.keyStore.keys = [
          ...state.keyStore.keys.filter((k) => k.env !== "CLAUDE_CODE_OAUTH_TOKEN"),
          { provider: "other", env: "CLAUDE_CODE_OAUTH_TOKEN", last4: "", addedMs: now(), label: "Signed in as ada@example.com (Claude Max)" },
        ];
        emit({ type: "agents.changed" });
      }
      return agents.map((agent) => {
        const row = state.agentSetup.find((r) => r.agent === agent);
        if (row && state.readOnlyAgents.includes(agent)) {
          return { agent, tools_ok: false, tools_line: `Couldn't add Cua to ${row.mcpConfig ?? agent}: Permission denied (os error 13)`, sign_in: null };
        }
        const copyable = agent === "hermes" || (agent === "claude-code" && claudeFound) || (agent === "codex" && Boolean(row?.signedIn));
        if (signIn && copyable && !state.imported.includes(agent)) state.imported = [...state.imported, agent];
        return {
          agent,
          tools_ok: Boolean(row?.installed),
          tools_line: row?.installed ? null : `Couldn't add Cua to ${row?.mcpConfig ?? agent}`,
          sign_in: !signIn || !["hermes", "claude-code", "codex"].includes(agent)
            ? null
            : copyable
              ? { imported: { source: agent, env: "", label: "", expires_ms: null, added_ms: now() }, refused: null, line: null }
              : agent === "claude-code"
                ? { imported: null, refused: "not_signed_in", line: "Claude Code isn't signed in on This Mac. Run `claude auth login` in Terminal, then try again." }
                : { imported: null, refused: "not_signed_in", line: "No ChatGPT sign-in on This Mac yet." },
        };
      });
    },
    // Codex's device-code sign-in made for Cua: a code to show, then approved a moment later.
    "agents.signInStart": async ({ agent }) => {
      await wait(step);
      if (agent !== "codex") throw new HostError(`${agent} has no device sign-in`);
      return { id: `codex-${now()}`, url: "https://auth.openai.com/codex/device", code: "QX7P-4M2KD" };
    },
    "agents.signInWait": async () => {
      await wait(step * 20);
      state.keyStore.keys = [...state.keyStore.keys.filter((k) => k.env !== "CODEX_AUTH_JSON"), { provider: "other", env: "CODEX_AUTH_JSON", last4: "", addedMs: now() }];
      emit({ type: "agents.changed" });
      return { imported: { source: "codex", env: "CODEX_AUTH_JSON", label: "Signed in with ChatGPT", expires_ms: null, added_ms: now() }, refused: null, line: null };
    },
    "agents.disconnect": async ({ agents, signIn, skills, mcp }) => {
      await wait(step);
      // Some parts only (an app's page's switches): what stays keeps its state.
      const part = skills !== undefined || mcp !== undefined;
      state.agentSetup = state.agentSetup.map((r) => {
        if (!agents.includes(r.agent)) return r;
        if (!part) return { ...r, configured: false, skillsInstalled: 0, detail: "Not set up" };
        const hasSkills = skills ? false : r.skillsInstalled > 0;
        const hasMcp = mcp ? false : r.configured || /MCP/.test(r.detail);
        return { ...r, configured: false, skillsInstalled: hasSkills ? r.skillsTotal : 0, detail: hasSkills && hasMcp ? "Skills and MCP server set up" : hasSkills ? "Skills set up" : hasMcp ? "MCP server set up" : "Not set up" };
      });
      const removed = signIn ? state.imported.filter((a) => agents.includes(a)) : [];
      if (signIn && agents.includes("claude-code")) state.keyStore.keys = state.keyStore.keys.filter((k) => k.env !== "CLAUDE_CODE_OAUTH_TOKEN");
      state.imported = state.imported.filter((a) => !removed.includes(a));
      return { removed_sign_ins: removed };
    },
    // The demo's browser sign-in finishes a moment later.
    "agents.signIn": async ({ agent }) => {
      const row = state.agentSetup.find((r) => r.agent === agent);
      if (!row?.signIn) throw new Error(`${row?.name ?? agent} has no sign-in Cua can start`);
      if (!row.installed) throw new Error(`${row.name} isn't installed on this machine`);
      setTimeout(() => {
        state.agentSetup = state.agentSetup.map((r) => (r.agent === agent ? { ...r, signedIn: true } : r));
      }, step * 40);
      return state.agentSetup.map((r) => ({ ...r }));
    },
    "agents.configure": async ({ agents }) => {
      await wait(step);
      const wanted = (r: AgentSetupRow) => r.installed && (agents === null || agents.includes(r.agent));
      state.agentSetup = state.agentSetup.map((r) =>
        wanted(r)
          ? state.readOnlyAgents.includes(r.agent)
            ? { ...r, configured: false, skillsInstalled: 0, detail: "Permission denied (os error 13)" }
            : { ...r, configured: true, skillsInstalled: r.skillsTotal, detail: "Skills and MCP server set up" }
          : r,
      );
      // The demo plays the agent's first request a little later: a line in this computer's access log.
      const used = state.agentSetup.find(wanted);
      if (used && agents?.length === 1) {
        setTimeout(() => {
          const line = { atMs: now(), via: "token", who: used.name, what: "MCP" };
          state.host = { ...state.host, recentAccess: [line, ...(state.host.recentAccess ?? [])] };
        }, step * 30);
      }
      return state.agentSetup.map((r) => ({ ...r }));
    },
    ...newSpaceDemoHandlers({ state, wait, emit, step, now }),
    ...demoTeleportHandlers({ wait, emit, now, stepMs: step }),
    ...demoShareHandlers({ wait, stepMs: step, teamFull: options.teamFull }),
    ...demoAgentDetailHandlers({ wait, stepMs: step, now }),
    ...demoAgentKeysHandlers({ wait, stepMs: step, now, store: state.keyStore }),
    ...demoVolumeHandlers({ state: state.volume, now, wait, step, emit, platform: state.platform }),
    ...createDemoSettings({ now, wait, emit, stepMs: step, platform: state.platform, unenrolled: options.unenrolled, update: options.update, experiments: state.volume.experiments, signedIn: () => Boolean(state.identity), settings: () => state.settings, setSettings: (p) => ((state.settings = { ...state.settings, ...p }), emit({ type: "settings.changed" })), addMachine: (m) => ((state.paired = [...state.paired, m]), emit({ type: "machines.changed" })) }),
    ...demoTelemetryHandlers(ctx),
    ...(options.noTeam ? { "teams.list": () => ({ teams: [], currentTeamId: null }) } : {}),
    ...demoRelayUsageHandlers(ctx),
    ...demoLinksHandlers(ctx),
    ...demoThreadHandlers({ now, wait, stepMs: step, enabled: !options.noThread, role: options.threadRole ?? "owner", lookalike: options.threadLookalike ?? false, authFailed: options.threadAuthFailed ?? false, stranger: options.threadStranger ?? false, paused: options.threadPaused ?? false,
      joinRedeemMs: options.joinRedeemMs,
      joinExpireMs: options.joinExpireMs,
      request: options.threadRequest ?? false,
    }),
    "nudges.pending": () => risky.filter((r) => r.expiresMs > now()).map((r) => ({ ...r })),
    "nudges.answer": ({ id }) => {
      risky = risky.filter((r) => r.id !== id);
      return null;
    },
    ...demoResourcesHandlers(ctx),
    ...demoSpaceDetailHandlers(ctx),
    ...demoHostSetupHandlers(ctx),
    ...demoKeyvaultSetupHandlers(ctx),
    ...demoKeyvaultManageHandlers(ctx, overview),
    ...demoStartupHandlers(ctx),
    ...unsupportedStreamOps("demo"),
    ...unsupportedChecklistOps("demo"),
    ...unsupportedPresenceOps("demo"),
    ...unsupportedRuntimeFixOps("demo"),
    "cli.plan": () => cliPlan(),
    "cli.install": async () => {
      await wait(step);
      cliInstalled = true;
      return cliPlan();
    },
  };

  return {
    mode: "demo",
    state,
    async call<K extends OpName>(op: K, args: OpArgs<K>): Promise<OpResult<K>> {
      const handler = handlers[op] as Handler<K> | undefined;
      if (!handler) throw new UnsupportedOperationError("demo", op);
      await wait(latency);
      return handler(args);
    },
    subscribe: (l) => events.subscribe(l),
    ...(options.creatingElsewhere ? { creatingElsewhere: () => DEMO_CREATING_ELSEWHERE } : {}),
    dispose() {
      for (const t of timers) clearTimeout(t);
      timers.clear();
      events.clear();
    },
  };
}

interface DemoLinksStateField {
  links: DemoLinksState;
}

export interface DemoState extends DemoLinksStateField, DemoTelemetryState, DemoRelayUsageState, DemoResourcesState, DemoSpaceDetailState, DemoHostSetupState, DemoStartupState, DemoNewSpaceState, DemoKeyvaultSetupState, DemoKeyvaultManageState {
  rows: SpaceRow[];
  items: KvItem[];
  keyvaultLocked: boolean;
  keyvaultDisabled: boolean;
  pendingRequests: boolean;
  revokedGrants: Set<string>;
  settings: SettingsValues;
  identity: string | undefined;
  onboarding: OnboardingState;
  creates: Map<string, { cancelled: boolean }>;
  agents: PersistentAgent[];
  runs: DemoRunRecord[];
  agentSetup: AgentSetupRow[];
  /** Agents whose sign-in the demo "copied" to Keyvault. */
  imported: string[];
  /** Saved keys and sign-ins (Settings, Connections; Claude Code's Connect adds its token here). */
  keyStore: DemoKeyStore;
  readOnlyAgents: string[];
  volume: DemoVolumeState;
  platform: DemoPlatform;
  /** Machines added with a pairing code ("Add a computer"). */
  paired: MachineRow[];
  /** Machines renamed on the Machines page (`machines.rename`), by id. */
  renamed: Record<string, string>;
  /** Machines removed from the account (`machines.remove`). */
  removed: string[];
  /** `?demo=mac-host`: the Keyvault's switches the SwiftUI host lays out in Settings. */
  hostSettings?: HostSettings;
  /** Settings, Advanced's runtimes (every demo host has them). */
  runtimes: DemoRuntimes;
}
