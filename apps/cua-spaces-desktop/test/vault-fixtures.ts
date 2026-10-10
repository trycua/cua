// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A simulated Keyvault broker, Teleport handle and Space for the bridge's
// tests (the SwiftUI app's FakeKeyvault and KeyvaultFixtures): fixture
// metadata only, and no secret value anywhere (the broker has none to give).
// Nothing here touches a keychain, an app, a window or the network.
import { readFileSync } from "node:fs";
import * as path from "node:path";
import type { Native } from "../src/native/load";
import type {
  KeyvaultClientLike,
  KeyvaultOverview,
  KvCommand,
  KvInventory,
  KvOutcome,
  SpaceAppIcon,
  SpaceAppIconRequest,
  SpaceLike,
  SpaceWindow,
  TeleportCatalogEntry,
  TeleportConsent,
  TeleportLike,
  TeleportPlan,
  TeleportRunEvent,
  TeleportRunListener,
  TeleportWindow,
} from "../src/native/generated/index";

const parity = JSON.parse(readFileSync(path.resolve(__dirname, "../../../libs/cua/crates/cua-spaces-app-core/parity/keyvault-approve-deny.json"), "utf8")) as { overview: unknown };

/** The methods and page operations vault-native.test.ts covers (the other bridge tests leave them to it). */
export const VAULT_METHODS = [
  "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows", "teleport.icon", "teleport.thumbnail",
  "teleport.plan", "teleport.run", "teleport.sites", "teleport.remembered", "teleport.streamWindow",
  "keyvault.get", "keyvault.lock", "keyvault.unlock", "keyvault.unlockVault", "keyvault.setup", "keyvault.setDisabled",
  "keyvault.approve", "keyvault.deny", "keyvault.revokeGrant", "keyvault.showItems", "keyvault.delete", "keyvault.run", "keyvault.dismiss",
];
export const VAULT_OPS = [
  "teleport.catalog", "teleport.entryForPath", "teleport.windows", "teleport.remoteWindows", "teleport.icon", "teleport.thumbnail",
  "teleport.plan", "teleport.run", "teleport.sites", "teleport.remembered", "teleport.streamWindow",
  "keyvault.overview", "keyvault.unlock", "keyvault.setUnattended", "keyvault.setDisabled", "keyvault.approve", "keyvault.deny",
  "keyvault.revokeGrant", "keyvault.setup", "keyvault.showItems", "keyvault.delete", "keyvault.run", "keyvault.dismiss",
] as const;

/** The clock the Keyvault fixtures are read at (Unix ms; just after the shared parity fixture's "now"). */
export const NOW = 1_800_000_000_000;

/** The shared parity fixture's overview (the Swift tests load the same file): eight items, two requests waiting, two grants, a rule and a live copy. */
export function parityOverview(native: Native, change: (wire: any) => void = () => {}): KeyvaultOverview {
  const wire = structuredClone(parity.overview) as any;
  change(wire);
  return native.kvOverviewFromJson(JSON.stringify(wire));
}

const unlockCases = JSON.parse(readFileSync(path.resolve(__dirname, "../../../libs/cua/crates/cua-spaces-app-core/parity/keyvault-unlock.json"), "utf8")) as {
  cases: { name: string; overview: unknown }[];
};

/** The setup and unlock forms' broker states (`setup-touch-id`, `setup-passphrase`, `unlock-touch-id`, `unlock-passphrase`, `ready`). */
export function unlockOverview(native: Native, name: string): KeyvaultOverview {
  const found = unlockCases.cases.find((c) => c.name === name);
  if (!found) throw new Error(`no unlock case ${name}`);
  return native.kvOverviewFromJson(JSON.stringify(found.overview));
}

/** A live copy of two of the GitHub items in Aurora (so deleting them wipes it). */
export const withCopy = (wire: any) => {
  wire.deliveries = [
    { import_id: "imp-1", target: "local:aurora", provider_id: "chrome", items: ["gh-ada", "gh-bob"], caller_fp: "fp", delivered_ms: NOW - 600_000, expires_ms: 0, wiped: false },
  ];
};

/** A broker stand-in: serves an overview, records the commands the page sends and applies what the real broker would. */
export class FakeKeyvault implements KeyvaultClientLike {
  commands: KvCommand[] = [];
  /** Passphrases the user typed (test fixtures, never real secrets). */
  passphrases: { setup: boolean; passphrase: string }[] = [];
  inventoryResult: KvInventory = {
    providerId: "chrome",
    appDisplay: "Google Chrome",
    domains: [{ domain: "github.com", cookies: 12, sessionCookies: 4, localStorage: 3, passwords: 2, signin: true, identityProvider: false, unavailable: 0, unavailableReason: "" }],
    notes: [],
  };
  inventoryAsks: { app: string; profile: string | undefined }[] = [];
  /** Makes the next command fail with this message. */
  failWith: string | null = null;

  constructor(
    private readonly native: Native,
    public current: KeyvaultOverview,
  ) {}

  async overview(): Promise<KeyvaultOverview> {
    return structuredClone(this.current);
  }

  async favicons() {
    return [];
  }

  async inventory(app: string, profile: string | undefined): Promise<KvInventory> {
    this.inventoryAsks.push({ app, profile });
    return this.inventoryResult;
  }

  async lock(): Promise<void> {}

  async setupWithPassphrase(passphrase: string): Promise<string | undefined> {
    this.passphrases.push({ setup: true, passphrase });
    this.current.availability = "ready";
    this.current.status!.initialized = true;
    this.current.status!.unlocked = true;
    return "ABCDE-FGHJK";
  }

  async unlockWithPassphrase(passphrase: string): Promise<void> {
    this.passphrases.push({ setup: false, passphrase });
    this.current.availability = "ready";
    this.current.status!.unlocked = true;
  }

  async execute(command: KvCommand): Promise<KvOutcome> {
    this.commands.push(command);
    if (this.failWith) {
      const message = this.failWith;
      this.failWith = null;
      // As the SDK's errors come out of the bindings: `CuaError.<Kind>: <words>`.
      const kind = /^CuaError\.(\w+)/.exec(message)?.[1] ?? "Runtime";
      throw Object.assign(new Error(message), { [Symbol.for("typeName")]: "CuaError", tag: kind });
    }
    const O = this.native.KvOutcome;
    const o = this.current;
    switch (command.tag) {
      case "SetDisabled":
        o.status!.disabled = command.inner.disabled;
        break;
      case "SetAutoWipe":
        o.status!.autoWipe = command.inner.on;
        break;
      case "SetSkipUnlockPrompt":
        o.status!.skipUnlockPrompt = command.inner.on;
        break;
      case "Approve": {
        const { requestId, items } = command.inner;
        const asked = o.pending.find((p) => p.id === requestId);
        o.pending = o.pending.filter((p) => p.id !== requestId);
        const grant = {
          id: `grant-${requestId}`,
          requestId,
          callerFp: asked?.callerFp ?? "",
          callerDisplay: asked?.callerDisplay ?? "",
          items: items ?? asked?.items.map((i) => i.id) ?? [],
          targets: asked?.request.targets ?? [],
          actions: [],
          createdMs: BigInt(NOW),
          notAfterMs: BigInt(NOW + 3_600_000),
          usesLeft: undefined,
          revoked: false,
          agent: undefined,
        };
        o.grants.push(grant);
        return new O.Granted({ grant });
      }
      case "Deny":
        o.pending = o.pending.filter((p) => p.id !== command.inner.requestId);
        break;
      case "RevokeGrant": {
        let count = 0;
        for (const g of o.grants) {
          if ((command.inner.id === "*" || g.id === command.inner.id) && !g.revoked) {
            g.revoked = true;
            count += 1;
          }
        }
        return new O.Revoked({ count });
      }
      case "SetLocked": {
        // The broker's rule: an identity provider always asks.
        const { itemIds, locked } = command.inner;
        const changed: string[] = [];
        const skipped: string[] = [];
        for (const item of o.items.filter((i) => itemIds.includes(i.id))) {
          if (!locked && item.identityProvider) {
            skipped.push(item.id);
            continue;
          }
          if (item.policy.unattended === locked) changed.push(item.id);
          item.policy.unattended = !locked;
        }
        return new O.Locked({ changed, skipped });
      }
      case "SetUnattended": {
        const { itemIds, unattended } = command.inner;
        for (const item of o.items.filter((i) => itemIds.includes(i.id))) item.policy.unattended = unattended;
        break;
      }
      case "DeleteItems": {
        const ids = command.inner.itemIds;
        o.items = o.items.filter((i) => !ids.includes(i.id));
        o.itemsTotal = o.items.length;
        const wiped = o.deliveries.filter((d) => d.items.some((i) => ids.includes(i))).map((d) => d.importId);
        for (const d of o.deliveries) if (wiped.includes(d.importId)) d.wiped = true;
        return new O.Deleted({ count: ids.length, wiped });
      }
      case "Setup":
        o.availability = "ready";
        o.status!.initialized = true;
        o.status!.unlocked = true;
        return new O.RecoveryKey({ key: "WXYZ-2345" });
      case "Unlock":
        o.availability = "ready";
        o.status!.unlocked = true;
        break;
      case "Browse":
        o.namesVisible = true;
        return new O.Browsing({ untilMs: BigInt(NOW + 300_000) });
    }
    return new O.Done();
  }
}

// MARK: Teleport

export function entry(id: string, name: string, over: Partial<TeleportCatalogEntry> = {}): TeleportCatalogEntry {
  return {
    id,
    name,
    hostPath: `/Applications/${name}.app`,
    hostAppId: `com.example.${id}`,
    version: "1.0",
    capability: "full" as TeleportCatalogEntry["capability"],
    reason: undefined,
    moves: ["appOnly", "appWithState"] as TeleportCatalogEntry["moves"],
    providerId: id,
    sensitiveGroups: [],
    installSource: "manifest",
    installId: id,
    installVersion: "1.0.0",
    launchBin: id,
    lastUsedMs: undefined,
    json: JSON.stringify({ id }),
    ...over,
  };
}

export function plan(app: TeleportCatalogEntry, json = `{"space_id":"local:aurora","n":1}`): TeleportPlan {
  return {
    app,
    spaceId: "local:aurora",
    moves: "appWithState" as TeleportPlan["moves"],
    steps: [{ kind: "install", summary: `Install ${app.name}` }],
    consent: [{ kind: "install" as TeleportPlan["consent"][number]["kind"], key: app.id, label: app.name, detail: "pinned", bytes: 0n, sensitive: false }],
    sensitive: false,
    totalBytes: 0n,
    warnings: [],
    relayUnsealed: false,
    json,
  };
}

export const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3]);
export const JPEG = new Uint8Array([0xff, 0xd8, 0xff, 4]);
export const SVG = new TextEncoder().encode("<svg/>");

/** The SDK's teleport handle over fixture apps and windows. */
export class FakeTeleport {
  catalogCalls: unknown[] = [];
  planCalls: { app: TeleportCatalogEntry; options: unknown }[] = [];
  runs: { plan: TeleportPlan; consent: TeleportConsent }[] = [];
  entries: TeleportCatalogEntry[] = [entry("slack", "Slack"), entry("chrome", "Google Chrome", { providerId: "chrome" })];
  windows: TeleportWindow[] | Error = [{ windowId: 41, pid: 100n, appName: "Slack", title: "general", bundlePath: "/Applications/Slack.app" }];
  /** Events a run reports, in order. */
  events: TeleportRunEvent[] = [
    { step: 0, steps: 2, kind: "install", phase: "finished", detail: "", doneBytes: 0n, totalBytes: 0n },
    { step: 1, steps: 2, kind: "state", phase: "progress", detail: "", doneBytes: 5n, totalBytes: 10n },
  ];
  runError: Error | null = null;
  /** Holds a run until released. */
  hold: Promise<void> | null = null;
  thumbnails = new Map<number, Uint8Array>([[41, PNG]]);

  async catalog(options: unknown) {
    this.catalogCalls.push(options);
    return this.entries;
  }
  async spaceHint() {
    return { roots: undefined, spaceOs: "linux", spaceArch: "arm64", recentsPath: undefined };
  }
  catalogEntryForPath(path: string) {
    const found = this.entries.find((e) => e.hostPath === path);
    if (!found) throw new Error(`CuaError.NotFound: no app at ${path}`);
    return found;
  }
  listWindows(): TeleportWindow[] {
    if (this.windows instanceof Error) throw this.windows;
    return this.windows;
  }
  appIconPng(path: string) {
    return path.endsWith("Slack.app") ? PNG.buffer.slice(0) : undefined;
  }
  captureWindowThumbnail(windowId: number) {
    return this.thumbnails.get(windowId)?.buffer.slice(0);
  }
  async plan(app: TeleportCatalogEntry, _space: SpaceLike, options: unknown) {
    this.planCalls.push({ app, options });
    return plan(app, `{"space_id":"local:aurora","n":${this.planCalls.length}}`);
  }
  async run(p: TeleportPlan, _space: SpaceLike, consent: TeleportConsent, listener: TeleportRunListener | undefined) {
    this.runs.push({ plan: p, consent });
    for (const e of this.events) listener?.onEvent(e);
    if (this.hold) await this.hold;
    if (this.runError) throw this.runError;
    return { appId: p.app.id, installed: [p.app.id], sent: [], imported: ["Cookies"], skipped: [], launched: true };
  }
}

/** A Space with windows, previews and icons. */
export class FakeSpace {
  list: SpaceWindow[] = [
    {
      windowId: "w-1",
      epoch: 7n,
      title: "Mozilla Firefox",
      appName: "Firefox",
      appId: "org.mozilla.firefox",
      pid: 321,
      bounds: [0, 0, 1280, 720],
      onScreen: true,
      focused: true,
      available: true,
      limitation: "",
    },
    { windowId: "w-2", epoch: 1n, title: "Notes", appName: "Notes", appId: "", pid: 0, bounds: [], onScreen: false, focused: false, available: true, limitation: "" },
  ];
  thumbnailCalls: { windowId: string; epoch: bigint; max: number }[] = [];
  async windowThumbnail(windowId: string, epoch: bigint, max: number) {
    this.thumbnailCalls.push({ windowId, epoch, max });
    return windowId === "w-1" ? JPEG.buffer.slice(0) : undefined;
  }
  async appIcons(requests: SpaceAppIconRequest[]): Promise<(SpaceAppIcon | undefined)[]> {
    return requests.map(() => undefined);
  }
}

/** `FakeSpace` as the SDK's `Space` handle. */
export function asSpace(space: FakeSpace): SpaceLike {
  return {
    windows: async () => space.list,
    windowThumbnail: (windowId: string, epoch: bigint, max: number) => space.windowThumbnail(windowId, epoch, max),
    appIcons: (requests: SpaceAppIconRequest[]) => space.appIcons(requests),
  } as unknown as SpaceLike;
}

export const asTeleport = (t: FakeTeleport) => t as unknown as TeleportLike;
