// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Demo fixtures, in the hosts' wire shapes (so they go through the same
 * app-core mapping as live data). Four machines (This Mac, Mac mini, Linux
 * box over the relay, and an offline Windows PC added directly), six Spaces across macOS, Windows and Linux, and Keyvault items in
 * five apps. All synthetic; example.com identities only.
 */

import type { HostStatus, MachineRow, TelemetryView } from "../contracts/host";
import type { KeyvaultOverview, KvItem, KvStatus } from "../contracts/keyvault";
import type { SpaceRow } from "../contracts/spaces";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./demo/platform";

export const DEMO_IDENTITY = "ada@example.com";

export const DEMO_MACHINES: MachineRow[] = [
  { id: "this-mac", name: "This Mac", via: "local", online: true, os: "macos", current: true, model: "MacBook Pro", arch: "aarch64", limits: [] },
  {
    id: "mac-mini",
    name: "Mac mini",
    via: "relay",
    online: true,
    os: "macos",
    model: "Mac mini",
    limits: [{ resource: "macos-vms", used: 1, limit: 2, reason: "macOS runs at most two VMs per Mac." }],
  },
  { id: "linux-box", name: "Linux box", via: "relay", online: true, os: "linux", model: "ThinkCentre M90q", limits: [] },
  {
    id: "studio-pc",
    name: "Studio PC",
    via: "direct",
    online: false,
    os: "windows",
    model: "Windows 11 Pro",
    detail: "Direct at 192.168.1.40:8443",
    limits: [],
  },
];

/** The machine list as the demo host answers `machines.list`: this Mac
 * carries its host status, the others when they were last seen. */
export function demoMachineRows(now: number, host: HostStatus = demoHostStatus(now), platform: DemoPlatform = MAC_DEMO_PLATFORM): MachineRow[] {
  const here = demoMachine(platform);
  const seen = (minutesAgo: number) => Math.floor((now - minutesAgo * 60_000) / 1000);
  const lastSeen: Record<string, number> = { "mac-mini": seen(2), "linux-box": seen(8), "studio-pc": seen(2 * 24 * 60 + 30) };
  return DEMO_MACHINES.map((m) => ({
    ...m,
    // This machine is the one the demo runs on.
    ...(m.current && platform.os !== "macos" ? { name: here.name, os: platform.os, model: here.model, arch: here.rowArch } : {}),
    limits: [...m.limits],
    lastSeen: lastSeen[m.id],
    host: m.current ? { ...host } : undefined,
  }));
}

const STREAMS = ["desktop_stream", "window_stream", "audio.desktop", "relay_attach"];

/** `minutesAgo` → RFC 3339, relative to `now`. */
const at = (now: number, minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();

export function demoSpaceRows(now: number): SpaceRow[] {
  return [
    {
      id: "local:design-review",
      name: "design-review",
      provider: "local",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 12),
      os: "macos",
      osName: "macOS",
      osPrettyName: "macOS Tahoe 26.0",
      image: "macos-tahoe",
      kind: "vm",
      arch: "arm64",
      reachable: true,
      power: "suspend",
      powerState: "running",
    },
    {
      id: "local:agent-sandbox",
      name: "agent-sandbox",
      provider: "local",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 95),
      os: "linux",
      osName: "Ubuntu",
      osPrettyName: "Ubuntu 24.04.3 LTS",
      image: "ubuntu-xfce",
      kind: "container",
      arch: "arm64",
      reachable: false,
      power: "stop",
      powerState: "stopped",
    },
    {
      id: "relay:mac-mini/release-checks",
      name: "release-checks",
      provider: "relay",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 240),
      os: "macos",
      osName: "macOS",
      osPrettyName: "macOS Sequoia 15.6",
      image: "macos-sequoia",
      kind: "vm",
      arch: "arm64",
      reachable: false,
      host: "mac-mini",
      hostName: "Mac mini",
      power: "suspend",
      powerState: "suspended",
    },
    {
      id: "relay:mac-mini/qa-windows",
      name: "qa-windows",
      provider: "relay",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 30),
      os: "windows",
      osName: "Windows",
      osPrettyName: "Windows 11 Pro 24H2",
      image: "windows-11",
      kind: "vm",
      arch: "arm64",
      reachable: true,
      host: "mac-mini",
      hostName: "Mac mini",
      power: "stop",
      powerState: "running",
    },
    {
      id: "relay:linux-box/ubuntu-build",
      name: "ubuntu-build",
      provider: "relay",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 4),
      os: "linux",
      osName: "Ubuntu",
      osPrettyName: "Ubuntu 24.04.3 LTS",
      image: "ubuntu-xfce",
      kind: "container",
      arch: "amd64",
      reachable: true,
      host: "linux-box",
      hostName: "Linux box",
      power: "stop",
      powerState: "running",
    },
    {
      id: "relay:linux-box/windows-legacy",
      name: "windows-legacy",
      provider: "relay",
      spacesdVersion: "0.6.0",
      features: STREAMS,
      addedAt: at(now, 1440),
      os: "windows",
      osName: "Windows",
      osPrettyName: "Windows 10 Enterprise LTSC",
      image: "windows-10",
      kind: "vm",
      arch: "amd64",
      reachable: false,
      host: "linux-box",
      hostName: "Linux box",
      power: "suspend",
      powerState: "suspended",
    },
  ];
}

/** "This Mac" as a host. A first run (`?demo=fresh`) is not set up yet and
 * still has macOS panes to grant. */
export function demoHostStatus(now = Date.now(), firstRun = false): HostStatus {
  if (firstRun) {
    return {
      configured: false,
      sharing: false,
      service: { installed: false, running: false, kind: "launchd" },
      clients: [],
      permissions: [
        {
          id: "screen-recording",
          label: "Screen Recording",
          instructions: "Turn on Cua Spaces so you can see and stream this desktop.",
          settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
          granted: false,
        },
        {
          id: "accessibility",
          label: "Accessibility",
          instructions: "Turn on Cua Spaces so agents can click and type in this desktop.",
          settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
          granted: false,
        },
      ],
    };
  }
  return {
    configured: true,
    mode: "relay",
    relayUrl: "https://relay.cua.ai",
    machineId: "this-mac",
    name: "This Mac",
    sharing: true,
    service: { installed: true, running: true, kind: "launchd" },
    online: true,
    clients: [{ id: "acct-ada", email: DEMO_IDENTITY, name: "Ada's iPad", streams: 1 }],
    permissions: [],
    shareDesktop: true,
    provideSpaces: true,
    maxSpaces: 4,
    maxMacosVms: 2,
    recentAccess: [
      { atMs: now - 4 * 60_000, via: "relay", who: DEMO_IDENTITY, what: "DesktopService" },
      { atMs: now - 52 * 60_000, via: "relay", who: DEMO_IDENTITY, what: "MCP" },
    ],
  };
}

export function demoTelemetry(enabled: boolean): TelemetryView {
  return {
    enabled,
    source: "config ~/.cua/config.toml",
    sourceKind: "config",
    noticeShown: true,
    noticeText: "Cua sends anonymous usage data to improve the app. No content, names or addresses.",
    docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
  };
}

/* ---- Keyvault ------------------------------------------------------------ */

interface ItemSpec {
  id: string;
  kind: KvItem["kind"];
  provider: string;
  app: string;
  key: string;
  domain?: string;
  path?: string;
  unattended?: boolean;
  idp?: boolean;
  session?: boolean;
  bytes?: number;
  ageMin?: number;
}

function item(now: number, s: ItemSpec): KvItem {
  return {
    id: s.id,
    kind: s.kind,
    provider_id: s.provider,
    app_display: s.app,
    domain: s.domain,
    key: s.key,
    path: s.path,
    source: s.kind === "file" ? `${s.app}.app` : `${s.app} · Default profile`,
    session: Boolean(s.session),
    bytes: s.bytes ?? 256,
    identity_provider: Boolean(s.idp),
    policy: { allowed_targets: [], ttl_secs: 3600, unattended: Boolean(s.unattended) },
    created_ms: now - 6 * 86_400_000,
    updated_ms: now - (s.ageMin ?? 60) * 60_000,
    rev: 1,
    record_digest: `sha256:${s.id}`,
  };
}

/** One item per secret, in five apps (Google Chrome, Safari, Slack, Notion, Linear). */
export function demoKeyvaultItems(now: number): KvItem[] {
  const chrome = { provider: "chrome", app: "Google Chrome" } as const;
  const safari = { provider: "safari", app: "Safari" } as const;
  return [
    item(now, { id: "kv-gh-session", kind: "cookie", ...chrome, domain: "github.com", key: "user_session", unattended: true, ageMin: 12 }),
    item(now, { id: "kv-gh-sess", kind: "cookie", ...chrome, domain: "github.com", key: "_gh_sess", unattended: true, session: true, ageMin: 12 }),
    item(now, { id: "kv-gh-theme", kind: "local_storage", ...chrome, domain: "github.com", key: "color-mode", ageMin: 12 }),
    item(now, { id: "kv-linear", kind: "cookie", ...chrome, domain: "linear.app", key: "session", ageMin: 40 }),
    item(now, { id: "kv-google", kind: "cookie", ...chrome, domain: "google.com", key: "SID", idp: true, ageMin: 300 }),
    item(now, { id: "kv-figma", kind: "cookie", ...safari, domain: "figma.com", key: "figma.authn", ageMin: 90 }),
    item(now, { id: "kv-apple", kind: "password", ...safari, domain: "developer.apple.com", key: DEMO_IDENTITY, ageMin: 2880 }),
    item(now, {
      id: "kv-slack",
      kind: "file",
      provider: "slack",
      app: "Slack",
      key: "Cookies",
      path: "~/Library/Application Support/Slack/Cookies",
      unattended: true,
      bytes: 44_000,
      ageMin: 25,
    }),
    item(now, {
      id: "kv-notion",
      kind: "file",
      provider: "notion",
      app: "Notion",
      key: "Cookies",
      path: "~/Library/Application Support/Notion/Partitions/notion/Cookies",
      bytes: 31_000,
      ageMin: 600,
    }),
    item(now, {
      id: "kv-linear-app",
      kind: "file",
      provider: "linear",
      app: "Linear",
      key: "Local Storage",
      path: "~/Library/Application Support/Linear/Local Storage/leveldb",
      bytes: 12_000,
      ageMin: 1500,
    }),
  ];
}

export function demoKeyvaultOverview(now: number, items: KvItem[], locked: boolean): KeyvaultOverview {
  const status: KvStatus = {
    version: "0.6.0",
    initialized: true,
    unlocked: !locked,
    disabled: false,
    caller_first_party: true,
    caller_display: "Cua Spaces",
    items: items.length,
    pending: locked ? 0 : 1,
    unlock_policy: "presence",
    os_protector_available: true,
    passphrase_available: true,
    unlock_protectors: ["macos-keychain", "passphrase"],
  };
  const empty = { grants: [], rules: [], deliveries: [], audit: [], partialErrors: [] };
  if (locked) {
    return { availability: "locked", message: "The Keyvault is locked.", status, serverVerified: true, items: [], namesVisible: false, itemsTotal: items.length, pending: [], ...empty };
  }
  const caller = {
    pid: 4242,
    uid: 501,
    path: "/usr/local/bin/claude",
    signing: { kind: "signed" as const, team_id: "Q6L2SF6YDW", identifier: "com.anthropic.claude-code", cdhash: "9f2c" },
    first_party: false,
    os_verified: true,
    launched_by: "Terminal",
    verified_name: "Claude Code",
  };
  const github = items.filter((i) => i.domain === "github.com");
  return {
    availability: "ready",
    status,
    serverVerified: true,
    items,
    namesVisible: true,
    itemsTotal: items.length,
    pending: github.length
      ? [
          {
            id: "req-1",
            caller,
            caller_fp: "fp-claude",
            caller_display: "Claude Code",
            request: {
              selectors: [{ kind: "site", app: "chrome", site: "github.com" }],
              targets: ["local:design-review"],
              actions: ["deliver"],
              duration_secs: 3600,
              reason: "Open the pull request in the design-review Space",
              agent: "claude-code",
            },
            items: github,
            needs_import: [],
            created_ms: now - 45_000,
          },
        ]
      : [],
    grants: [
      {
        id: "grant-1",
        request_id: "req-0",
        caller_fp: "fp-codex",
        caller_display: "Codex",
        items: ["kv-linear"],
        targets: ["relay:linux-box/ubuntu-build"],
        actions: ["deliver"],
        created_ms: now - 20 * 60_000,
        not_after_ms: now + 40 * 60_000,
        uses_left: null,
        revoked: false,
        agent: "codex",
      },
    ],
    rules: [],
    deliveries: [],
    audit: [
      { seq: 1, ts_ms: now - 20 * 60_000, kind: "consent.request", actor: "Codex", caller_fp: "fp-codex", item: "kv-linear", decision: "ok" },
      { seq: 2, ts_ms: now - 20 * 60_000 + 8_000, kind: "consent.allow", actor: "you", caller_fp: "fp-codex", item: "kv-linear", target: "relay:linux-box/ubuntu-build", decision: "allow" },
      { seq: 3, ts_ms: now - 19 * 60_000, kind: "teleport.deliver", actor: "Codex", caller_fp: "fp-codex", item: "kv-linear", target: "relay:linux-box/ubuntu-build", decision: "ok" },
      { seq: 4, ts_ms: now - 45_000, kind: "consent.request", actor: "Claude Code", caller_fp: "fp-claude", item: "kv-gh-session", decision: "ok" },
    ],
    auditVerification: { ok: true, entries: 4, unauthenticated: 0 },
    partialErrors: [],
  };
}
