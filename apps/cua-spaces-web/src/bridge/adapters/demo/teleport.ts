// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's "Teleport an app": a small catalog, this machine's and the
 * Space's windows, plans shaped like the SDK's, a browser's sites with
 * counts, and a run that steps through its plan. Nothing is read from this
 * machine and nothing is sent.
 */

import { HostError } from "../../adapter";
import type {
  CatalogEntry,
  ConsentItem,
  KvInventory,
  OpenWindow,
  RemoteWindow,
  SensitiveGroup,
  TeleportMove,
  TeleportPlan,
} from "../../contracts/teleport";
import type { TeleportHandlers, TeleportHostEvent } from "../../ops/teleport";

const HOUR = 3_600_000;

function entry(e: Partial<CatalogEntry> & Pick<CatalogEntry, "id" | "name" | "capability" | "moves">): CatalogEntry {
  return {
    hostPath: `/Applications/${e.name}.app`,
    hostAppId: null,
    version: null,
    reason: null,
    providerId: null,
    sensitiveGroups: [],
    installSource: null,
    installId: null,
    installVersion: null,
    launchBin: null,
    lastUsedMs: null,
    json: JSON.stringify({ id: e.id }),
    ...e,
  };
}

export function demoCatalog(now: number): CatalogEntry[] {
  return [
    entry({
      id: "slack",
      name: "Slack",
      hostAppId: "com.tinyspeck.slackmacgap",
      version: "4.41",
      capability: "full",
      moves: ["app_only", "app_with_state"],
      providerId: "slack",
      installSource: "manifest",
      installId: "slack",
      installVersion: "4.41.105",
      launchBin: "slack",
      lastUsedMs: now - 2 * HOUR,
    }),
    entry({
      id: "com.google.Chrome",
      name: "Google Chrome",
      hostAppId: "com.google.Chrome",
      version: "141.0",
      capability: "full",
      moves: ["app_only", "app_with_state"],
      providerId: "chrome",
      sensitiveGroups: ["sign_ins", "passwords", "history"],
      installSource: "manifest",
      installId: "google-chrome",
      installVersion: "141.0.7390.54",
      launchBin: "google-chrome",
      lastUsedMs: now - 20 * 60_000,
    }),
    entry({
      id: "code",
      name: "Visual Studio Code",
      hostAppId: "com.microsoft.VSCode",
      version: "1.99",
      capability: "install_only",
      moves: ["app_only", "app_with_files"],
      installSource: "manifest",
      installId: "vscode",
      installVersion: "1.99.0",
      launchBin: "code",
    }),
    entry({
      id: "firefox",
      name: "Firefox",
      hostAppId: "org.mozilla.firefox",
      version: "131.0",
      capability: "full",
      moves: ["app_only", "app_with_state"],
      providerId: "firefox",
      sensitiveGroups: ["sign_ins", "history"],
      installSource: "manifest",
      installId: "firefox",
      installVersion: "131.0.2",
      launchBin: "firefox",
    }),
    entry({
      id: "obsidian",
      name: "Obsidian",
      hostAppId: "md.obsidian",
      version: "1.7",
      capability: "install_only",
      moves: ["app_only", "app_with_files"],
      installSource: "manifest",
      installId: "obsidian",
      installVersion: "1.7.4",
      launchBin: "obsidian",
    }),
    entry({ id: "figma", name: "Figma", hostAppId: "com.figma.Desktop", capability: "unsupported", moves: [], reason: "no Linux build" }),
    entry({ id: "keynote", name: "Keynote", hostAppId: "com.apple.iWork.Keynote", capability: "unsupported", moves: [], reason: "macOS only" }),
  ];
}

export const DEMO_OPEN_WINDOWS: OpenWindow[] = [
  { windowId: 41, appId: "com.tinyspeck.slackmacgap", appName: "Slack", windowTitle: "general - Acme", supported: true, bundlePath: "/Applications/Slack.app" },
  { windowId: 38, appId: "com.google.Chrome", appName: "Google Chrome", windowTitle: "Pull requests · trycua/cua", supported: true, bundlePath: "/Applications/Google Chrome.app" },
  { windowId: 12, appId: "com.microsoft.VSCode", appName: "Code", windowTitle: "main.rs - cua", supported: true, bundlePath: "/Applications/Visual Studio Code.app" },
  { windowId: 7, appId: "com.apple.finder", appName: "Finder", windowTitle: "Downloads", supported: false },
];

export const DEMO_REMOTE_WINDOWS: RemoteWindow[] = [
  { id: "w-screen", appName: "cua driver", title: "Screen", visible: true, appId: "cua-driver", targetEpoch: 1 },
  { id: "w-1", appName: "Firefox", title: "Mozilla Firefox", visible: true, appId: "firefox", targetEpoch: 3, pid: 812 },
  { id: "w-2", appName: "xterm", title: "ada@design-review: ~", visible: true, appId: "xterm", targetEpoch: 1, pid: 904 },
];

/** Google Chrome's sites, with counts only. */
export const DEMO_CHROME_SITES: KvInventory = {
  provider_id: "chrome",
  app_display: "Google Chrome",
  domains: [
    { domain: "github.com", cookies: 14, session_cookies: 3, local_storage: 6, passwords: 1, signin: true },
    { domain: "linear.app", cookies: 9, local_storage: 4, signin: true },
    { domain: "accounts.google.com", cookies: 22, passwords: 2, signin: true, identity_provider: true },
    { domain: "vercel.com", cookies: 7, local_storage: 2, signin: true },
    { domain: "news.ycombinator.com", cookies: 2 },
    { domain: "docs.rs", cookies: 1, local_storage: 3 },
    {
      domain: "figma.com",
      cookies: 0,
      signin: true,
      unavailable: 5,
      unavailable_reason: "Chrome protects them with app-bound encryption",
    },
  ],
};

const MB = 1_048_576;

function consentFor(e: CatalogEntry, move: TeleportMove, files: string[], groups: SensitiveGroup[]): ConsentItem[] {
  const items: ConsentItem[] = [];
  if (e.installSource === "manifest" && e.installId) {
    items.push({ kind: "install", key: e.installId, label: `${e.name} ${e.installVersion ?? ""}`.trim(), detail: "pinned, sha256 verified", bytes: 0, sensitive: false });
  }
  for (const f of files) items.push({ kind: "file", key: f, label: f.split("/").pop() ?? f, detail: f, bytes: 240_000, sensitive: false });
  if (move !== "app_with_state") return items;
  if (e.providerId === "slack") {
    items.push({ kind: "state", key: "Local Storage", label: "Workspace settings", detail: "3 items", bytes: 1280, sensitive: false });
    items.push({ kind: "secret", key: "Cookies", label: "Sign-in cookies", detail: "12 cookies", bytes: 15 * MB, sensitive: true });
    return items;
  }
  const profile = e.providerId === "firefox" ? ".mozilla/firefox/default" : ".config/google-chrome/Default";
  items.push({ kind: "state", key: "tabs.json", label: "Open tabs", detail: "18 tabs", bytes: 24_576, sensitive: false });
  items.push({ kind: "state", key: `${profile}/Preferences`, label: "Preferences", detail: "", bytes: 8192, sensitive: false });
  if (groups.includes("sign_ins")) {
    items.push({ kind: "secret", key: `${profile}/Cookies`, label: "Cookies", detail: "60 cookies", bytes: 486_400, sensitive: true });
  }
  if (groups.includes("history")) {
    items.push({ kind: "secret", key: `${profile}/History`, label: "Browsing history", detail: "4,920 pages, 42 bookmarks", bytes: 262_144, sensitive: true });
  }
  return items;
}

export interface DemoTeleportDeps {
  wait(ms: number): Promise<void>;
  emit(e: TeleportHostEvent): void;
  now(): number;
  /** Time per run step (ms). */
  stepMs: number;
}

export function demoTeleportHandlers({ wait, emit, now, stepMs }: DemoTeleportDeps): TeleportHandlers {
  const catalog = () => demoCatalog(now());
  const find = (id: string) => {
    const e = catalog().find((x) => x.id === id);
    if (!e) throw new HostError(`no app ${id}`, "not_found");
    return e;
  };
  return {
    "teleport.catalog": async () => catalog(),
    "teleport.entryForPath": async ({ path }) => {
      const e = catalog().find((x) => x.hostPath === path);
      if (!e) throw new HostError(`${path} is not an app Teleport knows`, "not_found");
      return e;
    },
    "teleport.windows": async () => DEMO_OPEN_WINDOWS.map((w) => ({ ...w })),
    "teleport.remoteWindows": async () => DEMO_REMOTE_WINDOWS.map((w) => ({ ...w })),
    "teleport.icon": async () => null,
    "teleport.thumbnail": async () => null,
    "teleport.plan": async ({ spaceId, entry: e, move, files, sensitiveGroups }) => {
      await wait(stepMs);
      const app = find(e.id);
      const consent = consentFor(app, move, files, sensitiveGroups);
      const plan: TeleportPlan = {
        app,
        spaceId,
        moves: move,
        steps: [
          ...(consent.some((c) => c.kind === "install") ? [{ kind: "install", summary: `Install ${app.installId} (pinned, verified)` }] : []),
          ...(files.length ? [{ kind: "files", summary: `Send ${files.length} item(s) to ~/Downloads/${app.launchBin}` }] : []),
          ...(move === "app_with_state" ? [{ kind: "state", summary: `Import ${consent.filter((c) => c.kind === "state" || c.kind === "secret").length} ${app.providerId} item(s)` }] : []),
          { kind: "launch", summary: `Open ${app.launchBin ?? app.name}` },
        ],
        consent,
        sensitive: consent.some((c) => c.sensitive),
        totalBytes: consent.reduce((n, c) => n + c.bytes, 0),
        warnings: move === "app_with_state" && app.providerId === "slack" ? ["Slack signs out on this Mac while the Space holds the session."] : [],
        relayUnsealed: false,
        json: JSON.stringify({ space_id: spaceId, app: app.id }),
      };
      return plan;
    },
    "teleport.run": async ({ plan, runId }) => {
      const steps = plan.steps.length;
      for (const [i, s] of plan.steps.entries()) {
        const total = s.kind === "state" || s.kind === "files" ? plan.totalBytes : 0;
        emit({ type: "teleport.progress", runId, event: { step: i, steps, kind: s.kind, phase: "started", detail: s.summary, doneBytes: 0, totalBytes: total } });
        await wait(stepMs / 2);
        if (total) {
          emit({ type: "teleport.progress", runId, event: { step: i, steps, kind: s.kind, phase: "progress", detail: s.kind === "state" ? "Uploading" : s.summary, doneBytes: Math.round(total / 2), totalBytes: total } });
          await wait(stepMs / 2);
        }
        emit({ type: "teleport.progress", runId, event: { step: i, steps, kind: s.kind, phase: "finished", detail: "", doneBytes: total, totalBytes: total } });
      }
      return {
        appId: plan.app.id,
        installed: plan.consent.filter((c) => c.kind === "install").map((c) => c.key),
        sent: plan.consent.filter((c) => c.kind === "file").map((c) => c.key),
        imported: plan.consent.filter((c) => c.kind === "state" || c.kind === "secret").map((c) => c.key),
        skipped: [],
        launched: true,
      };
    },
    "teleport.sites": async ({ providerId }) => {
      if (providerId !== "chrome") throw new HostError(`no sites saved for ${providerId}`, "not_found");
      return structuredClone(DEMO_CHROME_SITES);
    },
    "teleport.remembered": async () => null,
    "teleport.streamWindow": async () => null,
  };
}
