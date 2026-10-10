// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's detail, its drop well, sharing and New Space's address form and
// clouds on the real app core (`pnpm native`), over fixture Spaces (the
// SwiftUI app's BridgeContractTests, CreateOptionsTests and the Files and
// Thumbnails routes). Skipped when this machine's native directory was not
// built.
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { createBridge, type Bridge } from "../src/bridge";
import { dropOnSpace, feedNotch, notchThumbnail, openSpaceFromNotch, type NotchFeedUi } from "../src/notch-feed";
import { spaceStreams } from "../src/bridge/space-detail";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel } from "../src/model/startup";
import { StreamRows, type PipSpec } from "../src/model/streams";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureDevices, fixtureHost } from "./fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

describe.skipIf(!built)("a Space's detail, files, sharing and clouds on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let backend: FixtureSpacesBackend;
  let telemetry: FixtureTelemetry;
  let model: AppModel;
  let bridge: Bridge;
  let picked: string[] | null;
  let pipSpecs: PipSpec[];
  let pipsOpen: Map<string, () => void>;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-bridge-spaces-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  beforeEach(async () => {
    backend = new FixtureSpacesBackend(native);
    telemetry = new FixtureTelemetry();
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath: path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json"),
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account: new FixtureAccount("ada@example.com"),
      telemetry,
      servicesIn: () => true,
      cpus: 8,
    });
    picked = ["/Users/ada/Desktop/report.pdf", "/Users/ada/Desktop/data"];
    pipSpecs = [];
    pipsOpen = new Map();
    bridge = createBridge({
      model,
      supervisor: null,
      version: "1.2.3",
      platform: "darwin",
      env: {},
      ui: {
        openSpace: () => {},
        setBackground: () => {},
        activate: () => {},
        chooseFiles: async () => picked,
        pip: {
          open: (spec, closed) => {
            pipSpecs.push(spec);
            pipsOpen.set(spec.key, closed);
          },
          close: (_id, key) => pipsOpen.delete(key),
        },
      },
    });
    await model.refresh();
  });

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args, { window: "win-1" });
  const code = (method: string, args: Record<string, unknown> = {}) =>
    bridge.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.code));
  const aurora = "local:aurora";

  describe("the Stream section", () => {
    it("lists the windows as the core's remote windows, with the display", async () => {
      expect(await call("spaces.windows", { spaceId: aurora })).toEqual({
        windows: [
          { id: "w-1", appName: "Terminal", title: "cua@space: ~", visible: true, appId: "org.gnome.Terminal", targetEpoch: 3, widthPx: 1280, heightPx: 800, pid: 412 },
          { id: "w-2", appName: "Thunar", title: "", visible: true, appId: "", targetEpoch: 1, widthPx: null, heightPx: null, pid: null },
        ],
        display: { widthPx: 1920, heightPx: 1080 },
        open: [],
      });
    });

    it("keeps what was listed when a read fails, and a hung one reads as failed", async () => {
      await call("spaces.windows", { spaceId: aurora });
      backend.windowsError = new Error("guest gone");
      const again = (await call("spaces.windows", { spaceId: aurora })) as { windows: unknown[] };
      expect(again.windows).toHaveLength(2);
      expect(spaceStreams).toBeTypeOf("function");
      const saved = StreamRows.windowsTimeout;
      StreamRows.windowsTimeout = 0.01;
      try {
        const rows = new StreamRows(native, () => new Promise(() => {}), async () => null);
        await rows.refresh();
        expect([rows.windows, rows.failed]).toEqual([[], true]);
      } finally {
        StreamRows.windowsTimeout = saved;
      }
    });

    it("pops a window and the desktop out, lists the open rows, and pops them back in", async () => {
      const desktop = native.appStreamDesktopRowId();
      expect(await call("stream.pip", { spaceId: aurora, command: { type: "open", row: "w-1" } })).toEqual(["w-1"]);
      expect(pipSpecs[0]).toMatchObject({ spaceId: aurora, os: "linux", key: "window:w-1", title: "cua@space: ~", source: { kind: "window", window: { id: "w-1", epoch: 3 } } });
      expect(await call("stream.pip", { spaceId: aurora, command: { type: "open", row: desktop } })).toEqual([desktop, "w-1"]);
      expect(await call("stream.pip", { spaceId: aurora, command: { type: "close", row: desktop } })).toEqual(["w-1"]);
      // The window list says which panels are open (the viewer and the detail read it).
      expect(((await call("spaces.windows", { spaceId: aurora })) as { open: string[] }).open).toEqual(["w-1"]);
      // The panel's own close button.
      pipsOpen.get("window:w-1")!();
      expect(((await call("spaces.windows", { spaceId: aurora })) as { open: string[] }).open).toEqual([]);
      expect(await call("stream.pip", { spaceId: aurora, command: { type: "close", row: "w-1" } })).toEqual([]);
      expect(await code("stream.pip", { spaceId: aurora, command: { type: "open", row: "w-9" } })).toBe("not_found");
      expect(await code("stream.pip", { spaceId: aurora, command: { type: "open" } })).toBe("bad_args");
      expect(await code("stream.pip", { spaceId: "local:nope", command: { type: "open", row: "w-1" } })).toBe("not_found");
    });

    it("closes a Space's panels when it leaves the list", async () => {
      await call("stream.pip", { spaceId: aurora, command: { type: "open", row: "w-1" } });
      expect(pipsOpen.size).toBe(1);
      backend.rowsNow = backend.rowsNow.filter((r) => r.id !== aurora);
      await model.refresh();
      expect(pipsOpen.size).toBe(0);
    });
  });

  describe("thumbnails", () => {
    it("asks for one no older than the background interval, and answers a data URL", async () => {
      expect(await call("spaces.thumbnail", { spaceId: aurora })).toBeNull();
      backend.thumbnails.set(aurora, { image: Uint8Array.of(0xff, 0xd8, 0xff), format: "jpeg", width: 320, height: 200, capturedAtMs: 1_700_000_000_000 });
      expect(await call("spaces.thumbnail", { spaceId: aurora, maxAgeMs: 15_000 })).toEqual({ url: "data:image/jpeg;base64,/9j/", capturedAtMs: 1_700_000_000_000 });
      const policy = native.appThumbnailPolicy();
      expect(backend.thumbnailCalls.filter(([id]) => id === aurora).map(([, ms]) => ms).slice(-2)).toEqual([Number(policy.backgroundIntervalMs), 15_000]);
      expect(await code("spaces.thumbnail", { spaceId: "local:nope" })).toBe("not_found");
    });

    it("warms running Spaces from the cache after a list read, and forgets a deleted one", async () => {
      expect(backend.thumbnailCalls).toContainEqual([aurora, null]);
      backend.thumbnails.set(aurora, { image: Uint8Array.of(1), format: "png", width: 1, height: 1, capturedAtMs: 1 });
      await call("spaces.thumbnail", { spaceId: aurora });
      expect(model.thumbnails.get(aurora)).not.toBeNull();
      model.delete(model.spaces.find((s) => s.id === aurora)!);
      expect(model.thumbnails.get(aurora)).toBeNull();
    });
  });

  describe("the drop well", () => {
    it("sends files picked with the native picker, and records it", async () => {
      expect(await call("spaces.chooseFiles")).toEqual(picked);
      const sent = await call("spaces.sendFiles", { spaceId: aurora, paths: ["/Users/ada/Desktop/report.pdf"] });
      expect(sent).toEqual([{ name: "report.pdf", dest: "/home/cua/Desktop/report.pdf", bytes: 1024 }]);
      expect(backend.sent).toEqual([{ id: aurora, paths: ["/Users/ada/Desktop/report.pdf"] }]);
      expect(telemetry.recorded.some((s) => JSON.stringify(s).includes("file_send"))).toBe(true);
      picked = null;
      expect(await call("spaces.chooseFiles")).toEqual([]);
    });

    it("offers the drop's files by name, and sends only paths offered here", async () => {
      const paths = ["/Users/ada/notes.txt", "/Users/ada/secret.key"];
      expect(await call("spaces.droppedFiles", { names: ["notes.txt"], paths })).toEqual(["/Users/ada/notes.txt"]);
      expect(await code("spaces.sendFiles", { spaceId: aurora, paths: ["/Users/ada/secret.key"] })).toBe("forbidden");
      expect(await code("spaces.sendFiles", { spaceId: aurora, paths: ["/etc/passwd"] })).toBe("forbidden");
      expect(await code("spaces.sendFiles", { spaceId: "local:nope", paths: ["/Users/ada/notes.txt"] })).toBe("not_found");
      expect(await code("spaces.sendFiles", { spaceId: aurora, paths: [] })).toBe("bad_args");
      expect(await code("spaces.droppedFiles", {})).toBe("bad_args");
      // Nothing dropped on the window: nothing to offer.
      expect(await call("spaces.droppedFiles", { names: ["notes.txt"] })).toEqual([]);
      expect(backend.sent).toEqual([]);
    });

    it("says so where the build has no picker", async () => {
      const bare = createBridge({ model, supervisor: null, version: "1", platform: "linux", env: {}, ui: { openSpace: () => {}, setBackground: () => {}, activate: () => {} } });
      expect(await bare.registry.dispatch({ id: "1", method: "spaces.chooseFiles", args: {} })).toMatchObject({ ok: false, error: { code: "unsupported" } });
      expect(await bare.registry.dispatch({ id: "2", method: "stream.pip", args: { spaceId: aurora, command: { type: "open", row: "w-1" } } })).toMatchObject({
        ok: false,
        error: { code: "unsupported" },
      });
    });
  });

  describe("sharing", () => {
    it("lists, shares view only and unshares, answering who it is shared with", async () => {
      expect(await call("sharing.list", { spaceId: aurora })).toEqual([]);
      expect(await call("sharing.share", { spaceId: aurora, who: "bo@example.com", role: "viewer" })).toEqual([{ who: "bo@example.com", role: "viewer", connected: false }]);
      expect(await call("sharing.share", { spaceId: aurora, who: "bo@example.com", role: "editor" })).toEqual([{ who: "bo@example.com", role: "editor", connected: false }]);
      expect(await call("sharing.unshare", { spaceId: aurora, who: "bo@example.com" })).toEqual([]);
      expect(await code("sharing.share", { spaceId: aurora, who: "bo@example.com" })).toBe("bad_args");
      expect(await code("sharing.unshare", { spaceId: aurora })).toBe("bad_args");
    });
  });

  describe("New Space", () => {
    it("adds a Space by address and answers its row", async () => {
      const row = (await call("spaces.add", { url: "http://studio.local:7400", token: "", name: "studio" })) as { id: string; name: string };
      expect(row).toMatchObject({ id: "direct:studio.local:7400", name: "studio" });
      expect(backend.added).toEqual([{ url: "http://studio.local:7400", token: null, name: "studio" }]);
      expect(await code("spaces.add", {})).toBe("bad_args");
    });

    it("says when an added Space is not listed yet", async () => {
      backend.add = async () => {};
      expect(await code("spaces.add", { url: "http://studio.local:7400" })).toBe("failed");
    });

    it("connects a cloud, makes it the default, and New Space offers it", async () => {
      expect(await call("clouds.status")).toMatchObject({ default_on: "local", providers: [{ name: "aws", connected: false }] });
      expect(await call("clouds.test", { target: { provider: "aws", region: "us-west-2" } })).toMatchObject({ provider: "aws", ok: true });
      const row = await call("clouds.connect", { target: { provider: "aws", region: "us-west-2" }, makeDefault: true });
      expect(row).toMatchObject({ name: "aws", connected: true });
      expect(backend.toolCalls.map(([t, a]) => [t, a])).toEqual(
        expect.arrayContaining([
          ["cloud_test", { provider: "aws", region: "us-west-2" }],
          ["cloud_connect", { provider: "aws", region: "us-west-2", make_default: true }],
        ]),
      );
      const options = (await call("spaces.createOptions")) as { env: { clouds: { name: string }[]; defaultLocation: string } };
      expect(options.env.clouds.map((c) => c.name)).toEqual(["aws"]);
      expect(options.env.defaultLocation).toBe("yours");
      expect(await code("clouds.test", {})).toBe("bad_args");
    });

    it("creates in your cloud and on one of your machines through the same create", async () => {
      for (const [on, name] of [
        ["aws", "in-aws"],
        ["host:m1", "on-studio"],
      ]) {
        await call("spaces.create", { pendingId: `pending:${name}`, config: { image: "ghcr.io/trycua/linux:24.04", kind: "vm", on, name }, os: "linux" });
      }
      expect(backend.created.map((a) => [a.on, a.name, a.kind])).toEqual([
        ["aws", "in-aws", native.AppSpaceKind.Vm],
        ["host:m1", "on-studio", native.AppSpaceKind.Vm],
      ]);
    });
  });

  describe("the notch", () => {
    const ui = () => {
      const shown: string[] = [];
      const notes: [string, string][] = [];
      const dropped: (string[] | undefined)[] = [];
      const u: NotchFeedUi = { showSpace: (id, names) => (shown.push(id), dropped.push(names)), notify: (t, b) => notes.push([t, b]) };
      return { u, shown, notes, dropped };
    };

    it("feeds the Spaces, the setting and the activity, now and on every change", async () => {
      const calls: string[] = [];
      const stop = feedNotch(
        {
          setSpaces: (spaces) => calls.push(`spaces:${spaces.length}`),
          setShown: (shown) => calls.push(`shown:${shown}`),
          setKeyvault: (label, signedIn) => calls.push(`keyvault:${label ?? "none"}:${signedIn?.length ?? 0}`),
          setActivity: (hotspot, transfer) => calls.push(`activity:${hotspot}:${transfer ? "transfer" : "none"}`),
        },
        model,
      );
      expect(calls).toEqual([`spaces:${model.spaces.length}`, "shown:true", "keyvault:none:0", "activity:false:none"]);
      calls.length = 0;
      const end = model.activity.begin();
      model.activity.setHotspot(true);
      end();
      expect(calls).toEqual(["activity:false:transfer", "activity:true:transfer", "activity:true:none"]);
      calls.length = 0;
      model.settings = { ...model.settings, menuBar: true };
      model.saveSettings();
      expect(calls).toContain("shown:false");
      stop();
      calls.length = 0;
      model.activity.setHotspot(false);
      expect(calls).toEqual([]);
    });

    it("gives a tile the store's thumbnail, no older than the open interval", async () => {
      backend.thumbnails.set(aurora, { image: Uint8Array.of(7), format: "png", width: 1, height: 1, capturedAtMs: 5 });
      expect(await notchThumbnail(model)(aurora)).toEqual(Uint8Array.of(7));
      expect(backend.thumbnailCalls.at(-1)).toEqual([aurora, Number(native.appThumbnailPolicy().openIntervalMs)]);
      expect(await notchThumbnail(model)("local:nope")).toBeNull();
    });

    it("selects and shows a Space on a tile's click", () => {
      const { u, shown } = ui();
      openSpaceFromNotch(model, u, aurora);
      expect(model.selectedSpaceId).toBe(aurora);
      expect(shown).toEqual([aurora]);
    });

    it("sends files dropped on a tile with the indicator on, and says what landed", async () => {
      const { u, shown, notes } = ui();
      const seen: boolean[] = [];
      model.activity.subscribe(() => seen.push(model.activity.transfer));
      await dropOnSpace(model, u, aurora, ["/Users/ada/notes.txt"]);
      expect(shown).toEqual([aurora]);
      expect(backend.sent).toEqual([{ id: aurora, paths: ["/Users/ada/notes.txt"] }]);
      expect(seen).toEqual([true, false]);
      expect(notes).toHaveLength(1);
      expect(notes[0]![0]).toBe(`Sent to ${model.spaces.find((s) => s.id === aurora)!.name}`);
      expect(notes[0]![1]).toContain("notes.txt");
      // The dropped file joins the offered paths, as a drop on the well does.
      expect(await call("spaces.sendFiles", { spaceId: aurora, paths: ["/Users/ada/notes.txt"] })).toHaveLength(1);
    });

    it("says why a drop could not be sent, and sends nothing for an app alone", async () => {
      const { u, notes } = ui();
      backend.sendFiles = async () => {
        throw new Error("no room in the Space");
      };
      await dropOnSpace(model, u, aurora, ["/Users/ada/a.txt", "/Users/ada/b.txt"]);
      expect(notes).toEqual([["Couldn't send 2 files to " + model.spaces.find((s) => s.id === aurora)!.name, "no room in the Space"]]);
      expect(model.activity.transfer).toBe(false);
      expect(notes).toHaveLength(1);
    });

    it("opens the Teleport review for an app dropped on a tile, the files beside it going with it", async () => {
      const { u, shown, notes, dropped } = ui();
      await dropOnSpace(model, u, aurora, ["/Users/ada/notes.txt", "/Applications/Notes.app/"]);
      // The page is told the names, as a drop on the well tells it, and asks the host for the paths of that drop.
      expect(shown).toEqual([aurora]);
      expect(dropped).toEqual([["notes.txt", "Notes.app"]]);
      expect(await call("spaces.droppedFiles", { names: ["notes.txt", "Notes.app"] })).toEqual(["/Users/ada/notes.txt", "/Applications/Notes.app/"]);
      // Nothing is sent: the review decides.
      expect(backend.sent).toEqual([]);
      expect(notes).toEqual([]);
      expect(model.selectedSpaceId).toBe(aurora);
    });
  });
});
