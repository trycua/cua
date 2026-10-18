// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Teleport and the Keyvault on the real app core (`pnpm native`), over fixture
// Spaces, a simulated broker and a simulated SDK teleport handle (the
// SwiftUI app's TeleportBridgeTests, the Keyvault parts of WebUIHostTests
// and the Keyvault model tests): the bridge's methods answer in the shapes
// the page reads (bridge-shapes.json), the caches stay bounded, the consent
// reaches the SDK whole, the prompts decide what runs, and nothing ever
// carries a secret value or a passphrase to the page. Skipped when this
// machine's native directory was not built.
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { validate, type Schema } from "../../cua-spaces-web/src/bridge/contracts/schema";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import type { OpArgs, OpName } from "../../cua-spaces-web/src/bridge/protocol";
import { createBridge, type Bridge } from "../src/bridge";
import type { BridgeEvent } from "../src/bridge/host";
import { AppModel } from "../src/model/app-model";
import { feedNotch } from "../src/notch-feed";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { KeyvaultModel, type CredentialAnswer, type UnlockAnswer } from "../src/model/keyvault";
import { StartupModel } from "../src/model/startup";
import { MAX_ENTRIES, TeleportCache } from "../src/model/teleport";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureDevices, fixtureHost } from "./fixtures";
import {
  FakeKeyvault,
  FakeSpace,
  FakeTeleport,
  JPEG,
  NOW,
  PNG,
  SVG,
  asSpace,
  asTeleport,
  entry,
  parityOverview,
  unlockOverview,
  withCopy,
} from "./vault-fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

const SHAPES = JSON.parse(readFileSync(path.resolve(__dirname, "../../cua-spaces-web/src/bridge/contracts/bridge-shapes.json"), "utf8")) as {
  webkit: Record<string, Schema>;
  $defs: Record<string, Schema>;
};
const json = (v: unknown) => JSON.parse(JSON.stringify({ v })).v as unknown;
const shapeProblems = (method: string, answer: unknown) => validate(SHAPES.webkit[method]!, json(answer), SHAPES.$defs).map((e) => `${method} ${e}`);

const ELECTRON_ADAPTER = path.resolve(__dirname, "../../cua-spaces-web/src/bridge/adapters/electron.ts");
interface PageAdapter {
  call(op: string, args: unknown): Promise<unknown>;
  dispose?(): void;
}
const pageAdapter = async (win: unknown) =>
  ((await import(/* @vite-ignore */ ELECTRON_ADAPTER)) as { createElectronAdapter(win: unknown): PageAdapter }).createElectronAdapter(win);

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

describe.skipIf(!built)("Teleport and the Keyvault on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let backend: FixtureSpacesBackend;
  let fake: FakeKeyvault;
  let sdk: FakeTeleport;
  let space: FakeSpace;
  let model: AppModel;
  let bridge: Bridge;
  let cache: TeleportCache;
  let events: BridgeEvent[];
  /** What the prompts were asked, and how the user answers them. */
  let asked: { unlock: string[]; delete: string[]; passphrase: string[]; popOut: string[]; opened: string[] };
  let answers: { unlock: UnlockAnswer; delete: boolean; passphrase: CredentialAnswer };
  let servicesIn = true;
  let hasPrompts = true;
  let platform: NodeJS.Platform = "darwin";
  let activations = 0;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-vault-native-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  /** A model and bridge over `overview`. */
  async function setUp(overview = parityOverview(native), client: boolean = true) {
    backend = new FixtureSpacesBackend(native);
    sdk = new FakeTeleport();
    space = new FakeSpace();
    backend.teleport = asTeleport(sdk);
    backend.space = asSpace(space);
    fake = new FakeKeyvault(native, overview);
    const keyvault = new KeyvaultModel(native, client ? fake : null, () => NOW);
    const settingsPath = path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json");
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath,
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account: new FixtureAccount("ada@example.com"),
      telemetry: new FixtureTelemetry(),
      keyvault,
      servicesIn: () => servicesIn,
      cpus: 8,
    });
    asked = { unlock: [], delete: [], passphrase: [], popOut: [], opened: [] };
    answers = { unlock: "allow", delete: true, passphrase: { passphrase: "a long enough passphrase", confirm: "a long enough passphrase" } };
    cache = new TeleportCache();
    bridge = createBridge({
      model,
      teleportCache: cache,
      supervisor: null,
      version: "1.2.3",
      platform,
      env: {},
      ui: {
        openSpace: (id, name) => asked.opened.push(`${id}|${name}`),
        setBackground: () => {},
        activate: () => void (activations += 1),
        ...(hasPrompts
          ? {
              askUnlock: async (_w, prompt) => {
                asked.unlock.push(`${prompt.title} | ${prompt.subject}`);
                return answers.unlock;
              },
              askDelete: async (_w, confirm) => {
                asked.delete.push(`${confirm.title} | ${confirm.message}`);
                return answers.delete;
              },
              askPassphrase: async (_w, form) => {
                asked.passphrase.push(`${form.mode} ${form.method}`);
                return answers.passphrase;
              },
            }
          : {}),
        pip: {
          open: (spec) => {
            asked.popOut.push(`${spec.spaceId}|${spec.spaceName}|${spec.key}|${spec.title}`);
          },
          close: () => {},
        },
      },
    });
    events = [];
    bridge.events.subscribe((e) => events.push(e));
    await model.refresh();
    await model.keyvault.refresh();
    await tick(5);
    events.length = 0;
  }

  beforeEach(async () => {
    servicesIn = true;
    hasPrompts = true;
    platform = "darwin";
    activations = 0;
    await setUp();
  });

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args, { window: "win-1" });
  const code = (method: string, args: Record<string, unknown> = {}) => bridge.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.code));
  const message = (method: string, args: Record<string, unknown> = {}) => bridge.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.message));
  const kinds = () => fake.commands.map((c) => c.tag);
  const shapes = async (method: string, args: Record<string, unknown> = {}) => {
    const answer = await call(method, args);
    expect(shapeProblems(method, answer)).toEqual([]);
    return answer as any;
  };

  // MARK: Keyvault

  describe("Keyvault", () => {
    it("answers the page, its list and the redacted overview: names and policy, no blob", async () => {
      const kv = await shapes("keyvault.get");
      expect(kv).toMatchObject({ availability: "ready", busy: false, error: null, dismissed: [] });
      expect(kv.page.ready).toBe(true);
      expect(kv.overview.items).toHaveLength(8);
      for (const item of kv.overview.items) expect(item).not.toHaveProperty("blob");
      expect(kv.overview.items[0]).toMatchObject({ id: "gh-ada", providerId: "chrome", kind: "cookie" });
      // The broker has no value to give: no item anywhere carries a blob.
      expect(JSON.stringify(kv)).not.toMatch(/"blob":"/);
    });

    it("reads the broker again when asked, but not while the app is launching", async () => {
      let reads = 0;
      const read = fake.overview.bind(fake);
      fake.overview = async () => (reads++, read());
      await call("keyvault.get");
      expect(reads).toBe(1);
      servicesIn = false;
      await call("keyvault.get");
      expect(reads).toBe(1);
    });

    it("is unavailable, not an error, with no broker to ask", async () => {
      await setUp(parityOverview(native), false);
      const kv = await shapes("keyvault.get");
      expect(kv.availability).toBe("not_running");
      expect(kv.overview.items).toEqual([]);
      expect(await code("keyvault.setDisabled", { disabled: true })).toBeNull();
      expect(kinds()).toEqual([]);
    });

    it("approves what the user ticked, denies, and revokes: each goes to the broker", async () => {
      const grant = (await call("keyvault.approve", { requestId: "req-1", items: null })) as { requestId: string };
      expect(grant.requestId).toBe("req-1");
      expect(fake.commands.at(-1)).toMatchObject({ tag: "Approve", inner: { requestId: "req-1", items: undefined } });
      expect(shapeProblems("keyvault.approve", grant)).toEqual([]);

      await call("keyvault.deny", { requestId: "req-2" });
      expect(fake.commands.at(-1)).toMatchObject({ tag: "Deny", inner: { requestId: "req-2" } });
      expect(model.keyvault.overview.pending).toEqual([]);
      // A request that is no longer waiting.
      expect(await code("keyvault.approve", { requestId: "req-1", items: ["x"] })).toBe("not_found");
      expect(await code("keyvault.deny", {})).toBe("bad_args");

      expect(await call("keyvault.revokeGrant", { id: "grant-1" })).toBe(1);
      expect(fake.commands.at(-1)).toMatchObject({ tag: "RevokeGrant", inner: { id: "grant-1" } });
      expect(await code("keyvault.revokeGrant", {})).toBe("bad_args");
    });

    it("approves only the ticked items", async () => {
      await call("keyvault.approve", { requestId: "req-1", items: ["gh-ada"] });
      expect(fake.commands.at(-1)).toMatchObject({ tag: "Approve", inner: { requestId: "req-1", items: ["gh-ada"] } });
      expect(model.keyvault.overview.grants.find((g) => g.requestId === "req-1")?.items).toEqual(["gh-ada"]);
    });

    it("fails an approval the broker refused, in the broker's words", async () => {
      // The SDK's message names its kind ("invalid argument: ..."); the page gets the broker's sentence.
      fake.failWith = "CuaError.InvalidArgument: invalid argument: that item is gone";
      const reply = await bridge.registry.dispatch({ id: "a", method: "keyvault.approve", args: { requestId: "req-1", items: null } });
      expect(reply).toMatchObject({ ok: false, error: { code: "failed", message: "that item is gone" } });
      // The request is still waiting.
      expect(model.keyvault.overview.pending.map((p) => p.id)).toContain("req-1");
    });

    it("locks and unlocks items: Allow asks first, in the core's words, then the daemon asks for presence once for the batch", async () => {
      const kv = await shapes("keyvault.unlock", { ids: ["gh-ada", "gh-bob"] });
      expect(asked.unlock).toEqual([expect.stringContaining("2 items")]);
      expect(kinds()).toEqual(["SetLocked"]);
      expect(fake.commands[0]).toMatchObject({ inner: { itemIds: ["gh-ada", "gh-bob"], locked: false } });
      expect(kv.overview.items.find((i: any) => i.id === "gh-bob").policy.unattended).toBe(true);

      await shapes("keyvault.lock", { ids: ["gh-ada"] });
      expect(fake.commands.at(-1)).toMatchObject({ tag: "SetLocked", inner: { itemIds: ["gh-ada"], locked: true } });
      expect(asked.unlock).toHaveLength(1);
    });

    it("names the one item in the unlock prompt", async () => {
      await call("keyvault.unlock", { ids: ["gh-ada"], name: "user_session" });
      expect(asked.unlock[0]).toContain("user_session");
    });

    it("unlocks nothing when the user denies, and says cancelled", async () => {
      answers.unlock = "deny";
      expect(await code("keyvault.unlock", { ids: ["gh-ada"] })).toBe("cancelled");
      expect(kinds()).toEqual([]);
      // No prompts at all in the host: the same.
      hasPrompts = false;
      await setUp();
      expect(await code("keyvault.unlock", { ids: ["gh-ada"] })).toBe("cancelled");
      expect(kinds()).toEqual([]);
    });

    it("remembers Never ask again in the vault, then unlocks without asking", async () => {
      answers.unlock = "neverAskAgain";
      await call("keyvault.unlock", { ids: ["gh-ada"] });
      expect(kinds()).toEqual(["SetSkipUnlockPrompt", "SetLocked"]);
      expect(fake.commands[0]).toMatchObject({ inner: { on: true } });
      expect(model.keyvault.unlockPromptShows).toBe(false);
      await call("keyvault.unlock", { ids: ["gh-bob"] });
      expect(asked.unlock).toHaveLength(1);
      expect(kinds()).toEqual(["SetSkipUnlockPrompt", "SetLocked", "SetLocked"]);
    });

    it("refuses empty ids and bad arguments", async () => {
      expect(await code("keyvault.unlock", { ids: [] })).toBe("bad_args");
      expect(await code("keyvault.unlock", {})).toBe("bad_args");
      expect(await code("keyvault.delete", { ids: [] })).toBe("bad_args");
      expect(await code("keyvault.setDisabled", { disabled: "yes" })).toBe("bad_args");
      expect(await call("keyvault.lock", { ids: [] })).toMatchObject({ availability: "ready" });
      expect(kinds()).toEqual([]);
    });

    it("never lets an identity provider be left unlocked", async () => {
      await call("keyvault.unlock", { ids: ["idp"] });
      expect(model.keyvault.overview.items.find((i) => i.id === "idp")?.policy.unattended).toBe(false);
    });

    it("turns the kill switch on and off", async () => {
      const kv = (await call("keyvault.setDisabled", { disabled: true })) as { page: { disabled: boolean } };
      expect(fake.commands.at(-1)).toMatchObject({ tag: "SetDisabled", inner: { disabled: true } });
      expect(kv.page.disabled).toBe(true);
      expect(((await call("keyvault.setDisabled", { disabled: false })) as { page: { disabled: boolean } }).page.disabled).toBe(false);
    });

    it("asks before deleting, naming the Spaces whose copies are wiped too; declined, nothing is deleted", async () => {
      await setUp(parityOverview(native, withCopy));
      answers.delete = false;
      expect(await code("keyvault.delete", { ids: ["gh-ada", "gh-bob"] })).toBe("cancelled");
      expect(kinds()).toEqual([]);
      expect(model.keyvault.deleteConfirm).toBeNull();
      expect(asked.delete[0]).toContain("Delete 2 items?");
      expect(asked.delete[0]).toContain("Copies delivered to a Space will be wiped as well.");

      answers.delete = true;
      const kv = await shapes("keyvault.delete", { ids: ["gh-ada", "gh-bob"] });
      expect(fake.commands.at(-1)).toMatchObject({ tag: "DeleteItems", inner: { itemIds: ["gh-ada", "gh-bob"] } });
      expect(fake.current.deliveries.every((d) => d.wiped)).toBe(true);
      expect(kv.overview.items.map((i: any) => i.id)).not.toContain("gh-ada");
    });

    it("deletes nothing without a prompt to ask", async () => {
      hasPrompts = false;
      await setUp();
      expect(await code("keyvault.delete", { ids: ["gh-ada"] })).toBe("cancelled");
      expect(kinds()).toEqual([]);
    });

    it("dismisses copies from the notch only, and remembers it", async () => {
      await setUp(parityOverview(native, withCopy));
      expect(model.keyvault.notchLabel).not.toBeNull();
      const kv = await shapes("keyvault.dismiss", { imports: ["imp-1"] });
      expect(kv.dismissed).toEqual(["imp-1"]);
      expect(model.keyvault.notchLabel).toBeNull();
      expect(model.settings.dismissedAccess).toEqual(["imp-1"]);
      expect(kinds()).toEqual([]);
      // The Spaces list still says Signed in.
      expect(model.keyvault.signedIn(model.spaces)).toContain("local:aurora");
      expect(model.keyvault.signedIn(model.spaces, true)).not.toContain("local:aurora");
      // Again: nothing new.
      await call("keyvault.dismiss", { imports: ["imp-1"] });
      expect(model.keyvault.dismissed).toEqual(["imp-1"]);
    });

    it("forgets a dismissal once its copy is wiped", async () => {
      await setUp(parityOverview(native, withCopy));
      await call("keyvault.dismiss", { imports: ["imp-1"] });
      fake.current.deliveries[0]!.wiped = true;
      await call("keyvault.get");
      expect(model.keyvault.dismissed).toEqual([]);
      expect(model.settings.dismissedAccess).toEqual([]);
    });

    it("shows the sign-ins live in a Space on the notch, less the dismissed ones", async () => {
      await setUp(parityOverview(native, withCopy));
      expect(model.keyvault.sharingLabel).not.toBeNull();
      expect(model.keyvault.notchLabel).toBe(model.keyvault.sharingLabel);
      expect(model.keyvault.signedIn(model.spaces, true)).toContain("local:aurora");
      const space = model.spaces.find((s) => s.id === "local:aurora")!;
      expect(model.keyvault.accessKey(space)).not.toBeNull();
    });

    it("runs an Access row's command, and nothing else", async () => {
      await call("keyvault.run", { command: { type: "release", target: "local:aurora" } });
      await call("keyvault.run", { command: { type: "remove-rule", id: "r1" } });
      await call("keyvault.run", { command: { type: "revoke-grant", id: "g1" } });
      expect(fake.commands).toMatchObject([
        { tag: "Release", inner: { target: "local:aurora" } },
        { tag: "RemoveRule", inner: { id: "r1" } },
        { tag: "RevokeGrant", inner: { id: "g1" } },
      ]);
      for (const bad of [{ type: "delete-items", itemIds: ["gh-ada"] }, { type: "set-disabled", disabled: true }, {}]) {
        expect(await code("keyvault.run", { command: bad })).toBe("bad_args");
      }
      expect(await code("keyvault.run", {})).toBe("bad_args");
      expect(fake.commands).toHaveLength(3);
    });

    it("shows the items' names behind the daemon's presence check", async () => {
      await setUp(unlockOverview(native, "ready"));
      fake.current.namesVisible = false;
      const kv = await shapes("keyvault.showItems");
      expect(kinds()).toEqual(["Browse"]);
      expect(kv.overview.namesVisible).toBe(true);
    });

    it("brings the app forward before the system asks for presence, except on a Mac (Touch ID's sheet needs no help)", async () => {
      const asks = async () => {
        activations = 0;
        await call("keyvault.approve", { requestId: "req-1", items: null });
        await call("keyvault.unlock", { ids: ["gh-ada"] });
        await call("keyvault.showItems");
        await call("keyvault.setDisabled", { disabled: true });
        await call("teleport.sites", { providerId: "chrome" });
        return activations;
      };
      expect(await asks()).toBe(0);
      for (const other of ["win32", "linux"] as const) {
        platform = other;
        await setUp();
        expect(await asks()).toBe(5);
      }
      // Things that never ask do not move the window.
      activations = 0;
      await call("keyvault.lock", { ids: ["gh-ada"] });
      await call("keyvault.get");
      await call("keyvault.dismiss", { imports: ["imp-1"] });
      expect(activations).toBe(0);
    });

    describe("set up and unlock", () => {
      it("sets up with the OS key store: the daemon confirms, and the recovery key comes back once", async () => {
        await setUp(unlockOverview(native, "setup-touch-id"));
        expect(model.keyvault.page.canSetup).toBe(true);
        const r = await shapes("keyvault.setup");
        expect(r).toEqual({ recoveryKey: "WXYZ-2345" });
        expect(kinds()).toEqual(["Setup"]);
        expect(model.keyvault.overview.availability).toBe("ready");
        // Set up already: nothing to set up.
        expect(await code("keyvault.setup")).toBe("failed");
        expect(asked.passphrase).toEqual([]);
      });

      it("sets up with a passphrase typed in this app's form, which goes to the broker and nowhere else", async () => {
        await setUp(unlockOverview(native, "setup-passphrase"));
        const r = await shapes("keyvault.setup");
        expect(r).toEqual({ recoveryKey: "ABCDE-FGHJK" });
        expect(asked.passphrase).toEqual(["setup passphrase"]);
        expect(fake.passphrases).toEqual([{ setup: true, passphrase: "a long enough passphrase" }]);
        expect(JSON.stringify([r, await call("keyvault.get"), events])).not.toContain("a long enough passphrase");
        // The recovery key is shown once.
        expect(model.keyvault.recoveryKey).toBe("ABCDE-FGHJK");
      });

      it("keeps a too-short or unmatched passphrase from the broker", async () => {
        await setUp(unlockOverview(native, "setup-passphrase"));
        answers.passphrase = { passphrase: "short", confirm: "short" };
        expect(await code("keyvault.setup")).toBe("failed");
        answers.passphrase = { passphrase: "a long enough passphrase", confirm: "another long passphrase" };
        expect(await code("keyvault.setup")).toBe("failed");
        expect(fake.passphrases).toEqual([]);
      });

      it("says cancelled when the passphrase form is closed", async () => {
        await setUp(unlockOverview(native, "setup-passphrase"));
        answers.passphrase = null;
        expect(await code("keyvault.setup")).toBe("cancelled");
        await setUp(unlockOverview(native, "unlock-passphrase"));
        answers.passphrase = null;
        expect(await code("keyvault.unlockVault")).toBe("cancelled");
        expect(fake.passphrases).toEqual([]);
      });

      it("keeps a passphrase out of the page without a form of this app's to type it in: native_only", async () => {
        hasPrompts = false;
        await setUp(unlockOverview(native, "setup-passphrase"));
        expect(await code("keyvault.setup")).toBe("native_only");
        await setUp(unlockOverview(native, "unlock-passphrase"));
        expect(await code("keyvault.unlockVault")).toBe("native_only");
      });

      it("unlocks the vault with the OS key store", async () => {
        await setUp(unlockOverview(native, "unlock-touch-id"));
        expect(model.keyvault.page.canUnlock).toBe(true);
        await shapes("keyvault.unlockVault").catch(() => {});
        expect(kinds()).toEqual(["Unlock"]);
        expect(model.keyvault.overview.availability).toBe("ready");
        expect(asked.passphrase).toEqual([]);
      });

      it("unlocks the vault with a passphrase typed in this app's form", async () => {
        await setUp(unlockOverview(native, "unlock-passphrase"));
        await call("keyvault.unlockVault");
        expect(asked.passphrase).toEqual(["unlock passphrase"]);
        expect(fake.passphrases).toEqual([{ setup: false, passphrase: "a long enough passphrase" }]);
        expect(model.keyvault.overview.availability).toBe("ready");
      });

      it("fails a wrong passphrase in the broker's words", async () => {
        await setUp(unlockOverview(native, "unlock-passphrase"));
        fake.unlockWithPassphrase = async () => {
          throw Object.assign(new Error("CuaError.PermissionDenied: the credential does not unlock this vault"), { [Symbol.for("typeName")]: "CuaError", tag: "PermissionDenied" });
        };
        const reply = await bridge.registry.dispatch({ id: "u", method: "keyvault.unlockVault", args: {} });
        expect(reply).toMatchObject({ ok: false, error: { code: "failed", message: "the credential does not unlock this vault" } });
      });

      it("has nothing to unlock when the vault is open", async () => {
        await call("keyvault.unlockVault");
        expect(kinds()).toEqual([]);
      });
    });

    it("tells the page when the Keyvault changed, once, and not when nothing did", async () => {
      await call("keyvault.get");
      await tick(5);
      expect(events.filter((e) => e.event === "keyvault.changed")).toEqual([]);
      await call("keyvault.dismiss", { imports: ["imp-x"] });
      await tick(5);
      expect(events.filter((e) => e.event === "keyvault.changed")).toEqual([{ event: "keyvault.changed" }]);
      events.length = 0;
      fake.current.pending = [];
      await model.keyvault.refresh();
      await tick(5);
      expect(events.filter((e) => e.event === "keyvault.changed")).toHaveLength(1);
    });

    it("refreshes on a timer, so access is never silent", async () => {
      let reads = 0;
      const read = fake.overview.bind(fake);
      fake.overview = async () => (reads++, read());
      const stop = model.keyvault.startPoll(5);
      try {
        const deadline = Date.now() + 2000;
        while (reads < 3 && Date.now() < deadline) await tick(5);
        expect(reads).toBeGreaterThanOrEqual(3);
      } finally {
        stop();
      }
    });

    it("feeds the notch the sign-ins live in a Space, and Dismiss hides them there and nowhere else", async () => {
      await setUp(parityOverview(native, withCopy));
      const sent: [string | null | undefined, string[] | undefined][] = [];
      const stop = feedNotch({ setSpaces: () => {}, setShown: () => {}, setKeyvault: (label, signedIn) => void sent.push([label, signedIn]), setActivity: () => {} }, model);
      expect(sent.at(-1)![0]).toBe(model.keyvault.sharingLabel);
      expect(sent.at(-1)![1]).toContain("local:aurora");
      await call("keyvault.dismiss", { imports: ["imp-1"] });
      expect(sent.at(-1)).toEqual([null, []]);
      // The page still sees the live copy (Dismiss revokes and wipes nothing).
      expect(model.keyvault.sharingLabel).not.toBeNull();
      expect(model.keyvault.overview.deliveries.some((d) => !d.wiped)).toBe(true);
      stop();
    });

    it("tells the notch when sign-ins go live", async () => {
      const seen: (string | null)[] = [];
      model.keyvault.onSharing = (label) => seen.push(label);
      fake.current = parityOverview(native, withCopy);
      await model.keyvault.refresh();
      expect(seen.at(-1)).toBe(model.keyvault.sharingLabel);
      expect(seen.at(-1)).not.toBeNull();
    });
  });

  // MARK: Teleport

  describe("Teleport", () => {
    const aurora = "local:aurora";

    it("reads the catalog for the Space, and keeps only the latest read", async () => {
      const entries = await shapes("teleport.catalog", { spaceId: aurora });
      expect(entries.map((e: any) => e.id)).toEqual(["slack", "chrome"]);
      expect(sdk.catalogCalls).toEqual([{ roots: undefined, spaceOs: "linux", spaceArch: "arm64", recentsPath: undefined }]);
      expect(cache.entryCount).toBe(2);
      sdk.entries = [entry("figma", "Figma")];
      await call("teleport.catalog", { spaceId: aurora });
      expect(cache.entryCount).toBe(1);
      expect(cache.entry("slack")).toBeUndefined();
    });

    it("needs a reachable Space", async () => {
      backend.teleport = null;
      expect(await code("teleport.catalog", { spaceId: aurora })).toBe("unsupported");
      expect(await message("teleport.catalog", { spaceId: aurora })).toBe("Teleport needs a reachable Space");
      expect(await code("teleport.entryForPath", { path: "/Applications/Slack.app" })).toBe("unsupported");
      expect(await code("teleport.catalog", {})).toBe("bad_args");
    });

    it("joins an app picked from a window to the catalog's entries", async () => {
      const found = await shapes("teleport.entryForPath", { path: "/Applications/Slack.app" });
      expect(found.id).toBe("slack");
      expect(cache.entry("slack")).toBeDefined();
      expect(await code("teleport.entryForPath", { path: "/Applications/Nope.app" })).toBe("failed");
      expect(await code("teleport.entryForPath", {})).toBe("bad_args");
    });

    it("keeps the entries it holds bounded", () => {
      const c = new TeleportCache();
      for (let i = 0; i < MAX_ENTRIES + 10; i++) c.addEntry(entry(`app-${i}`, `App ${i}`));
      expect(c.entryCount).toBe(MAX_ENTRIES);
      expect(c.entry("app-0")).toBeUndefined();
      expect(c.entry(`app-${MAX_ENTRIES + 9}`)).toBeDefined();
      c.setCatalog([entry("a", "A")]);
      expect(c.entryCount).toBe(1);
    });

    it("lists this machine's windows, front to back, as the picker's records", async () => {
      const windows = await shapes("teleport.windows");
      expect(windows).toEqual([{ windowId: 41, appId: "", appName: "Slack", windowTitle: "general", supported: true, bundlePath: "/Applications/Slack.app" }]);
    });

    it("lists no windows where the SDK has none (not a Mac, or no permission)", async () => {
      sdk.windows = new Error("CuaError.Unsupported: window listing is not available on this OS");
      expect(await shapes("teleport.windows")).toEqual([]);
      backend.teleport = null;
      expect(await call("teleport.windows")).toEqual([]);
    });

    it("lists the Space's windows", async () => {
      const windows = await shapes("teleport.remoteWindows", { spaceId: aurora });
      expect(windows[0]).toMatchObject({ id: "w-1", appName: "Firefox", title: "Mozilla Firefox", visible: true, appId: "org.mozilla.firefox", widthPx: 1280, heightPx: 720, pid: 321 });
      expect(windows[0].targetEpoch).toBe(7);
      // No bounds and no pid: none reported.
      expect(windows[1]).toMatchObject({ id: "w-2", widthPx: null, heightPx: null, pid: null });
    });

    it("draws icons and previews as data URLs: this machine's from the SDK, the Space's through its icon cache", async () => {
      expect(await shapes("teleport.icon", { spaceId: aurora, icon: { kind: "host", path: "/Applications/Slack.app" } })).toBe(`data:image/png;base64,${Buffer.from(PNG).toString("base64")}`);
      expect(await call("teleport.icon", { spaceId: aurora, icon: { kind: "host", path: "/Applications/Other.app" } })).toBeNull();
      backend.guestIcons.set("Firefox", SVG);
      expect(await shapes("teleport.icon", { spaceId: aurora, icon: { kind: "guest", appName: "Firefox", appId: "org.mozilla.firefox", pid: 321 } })).toBe(
        `data:image/svg+xml;base64,${Buffer.from(SVG).toString("base64")}`,
      );
      expect(backend.iconRequests.at(-1)).toEqual([{ appName: "Firefox", appId: "org.mozilla.firefox", pid: 321 }]);
      expect(await call("teleport.icon", { spaceId: aurora, icon: { kind: "guest", appName: "Gone" } })).toBeNull();
      expect(backend.iconRequests.at(-1)).toEqual([{ appName: "Gone", appId: "", pid: 0 }]);
      expect(await call("teleport.icon", { spaceId: aurora, icon: { kind: "other" } })).toBeNull();
      expect(await code("teleport.icon", { spaceId: aurora })).toBe("bad_args");
    });

    it("captures this machine's window and the Space's, never the screen", async () => {
      expect(await shapes("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "host-window", windowId: 41 } })).toMatch(/^data:image\/png;base64,/);
      expect(await call("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "host-window", windowId: 9 } })).toBeNull();
      expect(await code("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "host-window" } })).toBe("bad_args");
      expect(await code("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "host-window", windowId: -1 } })).toBe("bad_args");

      expect(await shapes("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "guest-window", windowId: "w-1", epoch: 7 } })).toMatch(/^data:image\/jpeg;base64,/);
      expect(space.thumbnailCalls).toEqual([{ windowId: "w-1", epoch: 7n, max: 480 }]);
      expect(await call("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "guest-window", windowId: "w-2" } })).toBeNull();
      expect(space.thumbnailCalls.at(-1)).toEqual({ windowId: "w-2", epoch: 0n, max: 480 });
      expect(await call("teleport.thumbnail", { spaceId: aurora, thumbnail: { kind: "nothing" } })).toBeNull();
    });

    describe("plan and run", () => {
      const choose = async (id = "slack") => {
        await call("teleport.catalog", { spaceId: aurora });
        return (await call("teleport.plan", { spaceId: aurora, entry: { id }, move: "app_with_state", files: ["/tmp/a.txt"], sensitiveGroups: ["sign_ins", "history", "nonsense"] })) as { json: string; app: { providerId: string | null } };
      };
      const run = (planJson: string, consent: Record<string, unknown> = { approved: true, acknowledgeSensitive: true }, runId = "run-1") =>
        call("teleport.run", { spaceId: aurora, plan: { json: planJson }, consent, runId });

      it("plans from the entry the catalog read, with the page's choices", async () => {
        const plan = await choose();
        expect(shapeProblems("teleport.plan", plan)).toEqual([]);
        expect(sdk.planCalls[0]!.app.id).toBe("slack");
        expect(sdk.planCalls[0]!.options).toEqual({
          moves: "appWithState",
          files: ["/tmp/a.txt"],
          stateItems: undefined,
          sensitiveGroups: ["signIns", "history"],
          scope: undefined,
          launch: true,
        });
        expect(cache.plan(aurora)?.json).toBe(plan.json);
        expect(cache.planCount).toBe(1);
      });

      it("maps the move words, and the app alone for anything else", async () => {
        await call("teleport.catalog", { spaceId: aurora });
        for (const [word, move] of [["app_only", "appOnly"], ["app_with_files", "appWithFiles"], ["app_with_state", "appWithState"], ["bogus", "appOnly"]]) {
          await call("teleport.plan", { spaceId: aurora, entry: { id: "slack" }, move: word, files: [], sensitiveGroups: [] });
          expect((sdk.planCalls.at(-1)!.options as { moves: string }).moves).toBe(move);
        }
        expect(await code("teleport.plan", { spaceId: aurora, entry: { id: "slack" } })).toBe("bad_args");
      });

      it("asks for the catalog again before planning an app it does not hold", async () => {
        expect(await code("teleport.plan", { spaceId: aurora, entry: { id: "slack" }, move: "app_only", files: [], sensitiveGroups: [] })).toBe("not_found");
        expect(await message("teleport.plan", { spaceId: aurora, entry: { id: "slack" }, move: "app_only" })).toBe("Read the catalog again");
        expect(await code("teleport.plan", { spaceId: aurora, move: "app_only" })).toBe("bad_args");
      });

      it("keeps one plan per Space, the latest", async () => {
        const first = await choose();
        const second = await choose("chrome");
        expect(second.json).not.toBe(first.json);
        expect(cache.planCount).toBe(1);
        // The earlier plan is not the one the host holds.
        expect(await code("teleport.run", { spaceId: aurora, plan: { json: first.json }, consent: { approved: true }, runId: "r" })).toBe("not_found");
        expect(cache.plan(aurora)?.json).toBe(second.json);
      });

      it("runs the plan: progress as teleport.progress events, then the report, and the plan is spent", async () => {
        const plan = await choose();
        const report = await run(plan.json);
        expect(shapeProblems("teleport.run", report)).toEqual([]);
        expect(report).toEqual({ appId: "slack", installed: ["slack"], sent: [], imported: ["Cookies"], skipped: [], launched: true });
        const progress = events.filter((e) => e.event === "teleport.progress").map((e) => e.payload as { runId: string; event: { step: number; kind: string; phase: string; doneBytes: number } });
        expect(progress.map((p) => [p.runId, p.event.step, p.event.kind, p.event.phase])).toEqual([
          ["run-1", 0, "install", "finished"],
          ["run-1", 1, "state", "progress"],
        ]);
        expect(progress[1]!.event.doneBytes).toBe(5);
        expect(cache.planCount).toBe(0);
        expect(await code("teleport.run", { spaceId: aurora, plan: { json: plan.json }, consent: { approved: true }, runId: "r2" })).toBe("not_found");
      });

      it("hands the SDK the consent whole: Save to Keyvault, the sites, the exclusions and the items sent from the Keyvault", async () => {
        const plan = await choose();
        await run(plan.json, {
          approved: true,
          acknowledgeSensitive: true,
          saveToKeyvault: true,
          acknowledgeRelayPlaintext: true,
          cookieDomains: ["github.com"],
          exclude: ["Default/Bookmarks"],
          fromVault: ["a", "b"],
          includePasswords: true,
        });
        expect(sdk.runs[0]!.consent).toEqual({
          approved: true,
          acknowledgeSensitive: true,
          saveToKeyvault: true,
          acknowledgeRelayPlaintext: true,
          cookieDomains: ["github.com"],
          exclude: ["Default/Bookmarks"],
          fromVault: ["a", "b"],
          includePasswords: true,
        });
        const again = await choose();
        await run(again.json, { approved: true, acknowledgeSensitive: true });
        expect(sdk.runs[1]!.consent).toEqual({
          approved: true,
          acknowledgeSensitive: true,
          saveToKeyvault: false,
          acknowledgeRelayPlaintext: false,
          cookieDomains: undefined,
          exclude: [],
          fromVault: undefined,
          includePasswords: false,
        });
      });

      it("refuses a run for a plan it does not hold, or with arguments missing", async () => {
        await choose();
        expect(await code("teleport.run", { spaceId: aurora, plan: { json: "{}" }, consent: {}, runId: "r" })).toBe("not_found");
        expect(await message("teleport.run", { spaceId: aurora, plan: { json: "{}" }, consent: {}, runId: "r" })).toBe("Plan the teleport again");
        expect(await code("teleport.run", { spaceId: aurora, consent: {}, runId: "r" })).toBe("bad_args");
        expect(sdk.runs).toEqual([]);
      });

      it("spends the plan when the arguments after it are wrong, and when the run fails", async () => {
        const plan = await choose();
        expect(await code("teleport.run", { spaceId: aurora, plan: { json: plan.json }, consent: { approved: true } })).toBe("bad_args");
        expect(cache.planCount).toBe(0);
        const again = await choose();
        sdk.runError = Object.assign(new Error("CuaError.Runtime: the Space went away"), { [Symbol.for("typeName")]: "CuaError", tag: "Runtime" });
        const reply = await bridge.registry.dispatch({ id: "r", method: "teleport.run", args: { spaceId: aurora, plan: { json: again.json }, consent: { approved: true }, runId: "r" } });
        expect(reply).toMatchObject({ ok: false, error: { code: "failed", message: "the Space went away" } });
        expect(cache.planCount).toBe(0);
        expect(model.activity.transfer).toBe(false);
      });

      it("shows the transfer on the notch while it runs, and only then", async () => {
        const plan = await choose();
        let release = () => {};
        sdk.hold = new Promise<void>((r) => (release = r));
        const changes: string[] = [];
        model.activity.subscribe(() => changes.push(`${model.activity.transfer}`));
        const running = run(plan.json);
        await tick(5);
        expect(model.activity.transfer).toBe(true);
        release();
        await running;
        expect(model.activity.transfer).toBe(false);
        expect(changes).toEqual(["true", "false"]);
      });

      it("remembers the sites sent for the next review: per app and Space, never from the Keyvault", async () => {
        expect(await call("teleport.remembered", { providerId: "chrome", spaceId: aurora })).toBeNull();
        const plan = await choose("chrome");
        await run(plan.json, { approved: true, acknowledgeSensitive: true, cookieDomains: ["github.com", "linear.app"] });
        const kept = await call("teleport.remembered", { providerId: "chrome", spaceId: aurora });
        expect(kept).toEqual(["github.com", "linear.app"]);
        expect(shapeProblems("teleport.remembered", kept)).toEqual([]);
        // The same key the native review reads.
        const name = model.spaces.find((s) => s.id === aurora)!.name;
        expect(native.appReviewRemembered(model.settings.teleportChoices, native.appReviewRememberKey("chrome", name))).toEqual(kept);
        // From the Keyvault, or with no site choice (an app that has none), nothing changes.
        const second = await choose("chrome");
        await run(second.json, { approved: true, cookieDomains: ["x.com"], fromVault: ["a"] });
        const third = await choose("slack");
        await run(third.json, { approved: true, cookieDomains: null });
        expect(await call("teleport.remembered", { providerId: "slack", spaceId: aurora })).toBeNull();
        expect(await call("teleport.remembered", { providerId: "chrome", spaceId: aurora })).toEqual(kept);
        expect(await code("teleport.remembered", { spaceId: aurora })).toBe("bad_args");
      });
    });

    it("counts a browser's sites from the Keyvault, never values", async () => {
      const inventory = await shapes("teleport.sites", { providerId: "chrome" });
      expect(inventory.domains[0]).toMatchObject({ domain: "github.com", cookies: 12 });
      expect(fake.inventoryAsks).toEqual([{ app: "chrome", profile: undefined }]);
      expect(await code("teleport.sites", {})).toBe("bad_args");
      await setUp(parityOverview(native), false);
      expect(await code("teleport.sites", { providerId: "chrome" })).toBe("unsupported");
    });

    it("streams one of the Space's windows onto this machine", async () => {
      expect(await shapes("teleport.streamWindow", { spaceId: aurora, windowId: "w-1" })).toBeNull();
      const name = model.spaces.find((s) => s.id === aurora)!.name;
      // The Stream section's picture-in-picture panel for that window.
      expect(asked.popOut).toEqual([`${aurora}|${name}|window:w-1|cua@space: ~`]);
      expect(await code("teleport.streamWindow", { spaceId: aurora, windowId: "w-9" })).toBe("not_found");
      expect(await code("teleport.streamWindow", { spaceId: "local:nope", windowId: "w-1" })).toBe("not_found");
      expect(await code("teleport.streamWindow", { spaceId: aurora })).toBe("bad_args");
    });
  });

  // MARK: The page's adapter

  it("reads every answer into the page's contract through the Electron adapter", async () => {
    await setUp(parityOverview(native, withCopy));
    const win = {
      cuaDesktop: { invoke: (_channel: string, request: unknown) => bridge.registry.dispatch(request, { window: "win-1" }), platform: "darwin" },
      open: () => null,
    };
    const adapter = await pageAdapter(win);
    const SPACE = "local:aurora";
    // The calls in the order the page makes them.
    const calls: [OpName, unknown][] = [
      ["keyvault.overview", {}],
      ["keyvault.showItems", {}],
      ["keyvault.setUnattended", { itemIds: ["gh-ada"], unattended: true }],
      ["keyvault.setUnattended", { itemIds: ["gh-ada"], unattended: false }],
      ["keyvault.setDisabled", { disabled: false }],
      ["keyvault.approve", { requestId: "req-1", items: null }],
      ["keyvault.deny", { requestId: "req-2" }],
      ["keyvault.revokeGrant", { id: "grant-1" }],
      ["keyvault.run", { command: { type: "revoke-grant", id: "grant-0" } }],
      ["keyvault.dismiss", { imports: ["imp-1"] }],
      ["keyvault.delete", { itemIds: ["gh-bob"] }],
      ["keyvault.unlock", { passphrase: null }],
      ["teleport.catalog", { spaceId: SPACE }],
      ["teleport.entryForPath", { path: "/Applications/Slack.app" }],
      ["teleport.windows", {}],
      ["teleport.remoteWindows", { spaceId: SPACE }],
      ["teleport.icon", { spaceId: SPACE, icon: { kind: "host", path: "/Applications/Slack.app" } }],
      ["teleport.thumbnail", { spaceId: SPACE, thumbnail: { kind: "host-window", windowId: 41 } }],
      ["teleport.sites", { providerId: "chrome" }],
      ["teleport.remembered", { providerId: "chrome", spaceId: SPACE }],
      ["teleport.streamWindow", { spaceId: SPACE, spaceName: "Aurora", windowId: "w-1", appName: "Firefox", title: "Mozilla Firefox" }],
    ];
    const problems: string[] = [];
    for (const [op, args] of calls) {
      try {
        const result = await adapter.call(op, args as OpArgs<typeof op>);
        problems.push(...validate(OP_SHAPES[op], result, SHAPE_DEFS).map((e) => `${op} ${e}`));
      } catch (e) {
        problems.push(`${op} failed: ${(e as Error).message}`);
      }
    }
    // plan and run need the entry and plan the page holds from the steps before.
    const entries = (await adapter.call("teleport.catalog", { spaceId: SPACE })) as { id: string }[];
    const plan = await adapter.call("teleport.plan", { spaceId: SPACE, entry: entries[0], move: "app_only", files: [], sensitiveGroups: [] } as never);
    problems.push(...validate(OP_SHAPES["teleport.plan"], plan, SHAPE_DEFS).map((e) => `teleport.plan ${e}`));
    const report = await adapter.call("teleport.run", { spaceId: SPACE, plan, consent: { approved: true, acknowledgeSensitive: false }, runId: "r" } as never);
    problems.push(...validate(OP_SHAPES["teleport.run"], report, SHAPE_DEFS).map((e) => `teleport.run ${e}`));
    adapter.dispose?.();
    expect(problems).toEqual([]);
  });
});
