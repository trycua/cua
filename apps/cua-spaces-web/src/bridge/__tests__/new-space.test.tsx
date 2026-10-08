// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import { DEMO_NEW_SPACE_OPTIONS, demoMacHostOptions } from "../adapters/demo/new-space";
import type { DataAdapter } from "../adapter";
import type { CoreClient } from "../core";
import { BridgeProvider, useConnectCloud, useFirstSpaceOffers, useNewSpaceRequests, useNewSpaceWizard, useSettings, useSpaces, type ConnectCloudHook, type FirstSpaceHook, type NewSpaceWizardHook, type SettingsHook, type SpacesHook } from "../index";
import { connectedClouds, firstSpaceOffers, readNewSpaceSession, requestNewSpace, resetNewSpaceSession, sizeText, wizardEnv, wizardInitial, wizardOffered, wizardReduce, wizardView } from "../new-space";
import { noCore, testCore, wasmBuilt } from "./testCore";

beforeEach(resetNewSpaceSession);
afterEach(cleanup);

interface Hooks {
  first: FirstSpaceHook;
  wizard: NewSpaceWizardHook;
  connect: ConnectCloudHook;
  spaces: SpacesHook;
  settings: SettingsHook;
}

function mount(core: CoreClient, stepMs = 4, options: Parameters<typeof createDemoAdapter>[0] = {}, wrap?: (a: DataAdapter) => DataAdapter) {
  const base = createDemoAdapter({ latencyMs: 1, stepMs, ...options });
  const adapter = wrap ? wrap(base) : base;
  const hooks = {} as Hooks;
  function Probe() {
    hooks.wizard = useNewSpaceWizard();
    hooks.first = useFirstSpaceOffers();
    hooks.connect = useConnectCloud();
    hooks.spaces = useSpaces();
    hooks.settings = useSettings();
    useNewSpaceRequests();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core} storeOptions={{ tickMs: 5 }}>
      <Probe />
    </BridgeProvider>,
  );
  return { hooks, adapter };
}

describe("where the wizard runs", () => {
  it("needs the core, and a host that answers its options", async () => {
    expect(wizardOffered(noCore, "demo")).toBe(false);
    if (!wasmBuilt) return;
    const core = await testCore();
    expect(wizardOffered(core, "demo")).toBe(true);
    expect(wizardOffered(core, "tauri")).toBe(true);
    // The SwiftUI app answers the options too (its native sheet's env).
    expect(wizardOffered(core, "webkit")).toBe(true);
  });
});

describe.skipIf(!wasmBuilt)("New Space (wasm core)", () => {
  it("builds the wizard's env from what the bridge has", async () => {
    const core = await testCore();
    const machines = [
      { id: "this-mac", name: "This Mac", via: "local", online: true, os: "macos", current: true, limits: [] },
      { id: "mac-mini", name: "Mac mini", via: "relay", online: true, os: "macos", current: false, limits: [] },
    ];
    const env = wizardEnv(core, { options: DEMO_NEW_SPACE_OPTIONS, clouds: null, defaultLocation: "host:mac-mini", cloudAvailable: true, machines });
    expect(env).toMatchObject({ defaultLocation: "host", cloudAvailable: true, localAvailable: true, localBackends: ["docker", "lume"], maxCpus: 10, hostArch: "arm64" });
    expect(env.hosts!.map((h) => h.id)).toEqual(["mac-mini"]);
    expect(env.clouds).toEqual([]);

    const none = wizardEnv(core, { options: { ...DEMO_NEW_SPACE_OPTIONS, local: { available: false, backends: [], error: "no runtime", macosImage: null } }, clouds: null, defaultLocation: "local", cloudAvailable: false, machines: [] });
    expect(none).toMatchObject({ localAvailable: false, localReason: "no runtime" });
  });

  it("reads connected clouds, and a cloud word as the default, from cloud_status", async () => {
    const core = await testCore();
    const adapter = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    await adapter.call("clouds.connect", { target: { provider: "aws", region: "us-west-2" }, makeDefault: true });
    const status = await adapter.call("clouds.status", {});
    expect(connectedClouds(core, status).map((c) => [c.name, c.label])).toEqual([["aws", "AWS · us-west-2"]]);
    const settings = await adapter.call("settings.get", {});
    expect(settings.values.defaultLocation).toBe("aws");
    const env = wizardEnv(core, { options: DEMO_NEW_SPACE_OPTIONS, clouds: status, defaultLocation: "aws", cloudAvailable: true, machines: [] });
    expect(env.defaultLocation).toBe("yours");
    adapter.dispose?.();
  });

  it("walks System, Resources, Options and Summary, then creates through the creates flow", async () => {
    const { hooks } = mount(await testCore(), 40);
    await waitFor(() => expect(hooks.spaces.data).toHaveLength(6));
    act(() => hooks.wizard.show());
    await waitFor(() => expect(hooks.wizard.view?.step).toBe(0));
    expect(hooks.wizard.view!.placements[0]).toMatchObject({ id: "local", label: "This Mac", selected: true });
    // Your machines follow This Mac; the offline one can't be chosen.
    await waitFor(() => expect(hooks.wizard.view!.placements.map((p) => [p.id, p.enabled])).toContainEqual(["host:studio-pc", false]));

    act(() => hooks.wizard.send({ type: "choose-os", os: "macos" }));
    act(() => hooks.wizard.send({ type: "next" }));
    expect(hooks.wizard.view!.step).toBe(1);
    // Lume offers a GPU here (the demo's gpu_support).
    expect(hooks.wizard.view!.gpu).toMatchObject({ enabled: true, on: false });
    act(() => hooks.wizard.send({ type: "set-gpu", on: true }));
    act(() => hooks.wizard.send({ type: "set-cpus", cpus: 6 }));
    expect(hooks.wizard.view!.cpusText).toBe("6 cores");
    act(() => hooks.wizard.send({ type: "next" }));
    act(() => hooks.wizard.send({ type: "set-name", name: "release-mac" }));
    act(() => hooks.wizard.send({ type: "next" }));
    const v = hooks.wizard.view!;
    expect(v.step).toBe(3);
    expect(v.plan).toMatchObject({ placement: "local", cpus: 6, name: "release-mac", gpu: "paravirtual" });

    let created: Promise<unknown> | null = null;
    act(() => {
      created = hooks.wizard.create();
    });
    expect(hooks.wizard.open).toBe(false);
    // The tile shows at once, creating.
    await waitFor(() => expect(hooks.spaces.data!.find((s) => s.id.startsWith("pending:"))).toMatchObject({ name: "release-mac", status: "provisioning" }));
    await act(async () => {
      await created;
    });
    expect(hooks.spaces.data!.find((s) => s.id === "local:release-mac")?.status).toBe("running");
  });

  it("opens when the host asks, on the machine it names once that machine is offered", async () => {
    const { hooks } = mount(await testCore());
    act(() => requestNewSpace("host:mac-mini"));
    await waitFor(() => expect(hooks.wizard.view?.placementId).toBe("host:mac-mini"));
    expect(hooks.wizard.open).toBe(true);
    act(() => hooks.wizard.close());
    act(() => requestNewSpace(null));
    await waitFor(() => expect(hooks.wizard.view?.placementId).toBe("local"));
  });

  it("connects a cloud from the sheet, then offers it in Run on as the default", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.settings.data).toBeDefined());
    act(() => hooks.wizard.show());
    act(() => hooks.connect.show());
    await waitFor(() => expect(hooks.connect.view?.rows.map((r) => r.id)).toEqual(["aws", "gcp", "modal", "none"]));
    expect(hooks.connect.view!.rows.find((r) => r.id === "aws")).toMatchObject({ found: true, selected: true });

    await act(() => hooks.connect.send({ type: "set-value", text: "us-west-2" }));
    await act(() => hooks.connect.send({ type: "test" }));
    await waitFor(() => expect(hooks.connect.view!.checks.every((c) => c.ok)).toBe(true));
    expect(hooks.connect.view!.result).toContain("Nothing was created");

    await act(() => hooks.connect.send({ type: "set-make-default", on: true }));
    await act(() => hooks.connect.send({ type: "connect" }));
    await waitFor(() => expect(hooks.connect.open).toBe(false));
    await waitFor(() => expect(hooks.settings.data!.values.defaultLocation).toBe("aws"));

    await waitFor(() => expect(hooks.wizard.env.clouds!.map((c) => c.name)).toEqual(["aws"]));
    act(() => hooks.wizard.send({ type: "sync-default", location: hooks.wizard.env.defaultLocation }));
    const aws = hooks.wizard.view!.placements.find((p) => p.id === "aws");
    expect(aws).toMatchObject({ label: "AWS · us-west-2", group: "clouds", selected: true });
  });

  it("No cloud closes the sheet without a request", async () => {
    const { hooks } = mount(await testCore());
    act(() => hooks.connect.show());
    await waitFor(() => expect(hooks.connect.view).not.toBeNull());
    await act(() => hooks.connect.send({ type: "select", name: "none" }));
    await act(() => hooks.connect.send({ type: "connect" }));
    expect(hooks.connect.open).toBe(false);
  });

  it("Connect by address adds the Space, and says why when it can't", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.spaces.data).toHaveLength(6));
    act(() => hooks.wizard.show());
    act(() => hooks.wizard.send({ type: "show-address" }));
    expect(hooks.wizard.view!.mode).toBe("address");
    act(() => hooks.wizard.send({ type: "set-address", url: "studio.local" }));
    await act(() => hooks.wizard.submitAddress());
    await waitFor(() => expect(hooks.wizard.view?.address.error).toBeTruthy());
    expect(hooks.wizard.open).toBe(true);

    act(() => hooks.wizard.send({ type: "set-address", url: "studio.local:7400" }));
    await act(() => hooks.wizard.submitAddress());
    expect(hooks.wizard.open).toBe(false);
    await waitFor(() => expect(hooks.spaces.data!.map((s) => s.id)).toContain("direct:studio-local"));
  });
});

describe.skipIf(!wasmBuilt)("First Space in one click (wasm core)", () => {
  const env = (core: CoreClient, options = DEMO_NEW_SPACE_OPTIONS) =>
    wizardEnv(core, { options, clouds: null, defaultLocation: "cloud", cloudAvailable: true, machines: [] });

  it("offers Linux first with its size and time, then macOS with its real download", async () => {
    const core = await testCore();
    const fresh = { ...DEMO_NEW_SPACE_OPTIONS, local: { ...DEMO_NEW_SPACE_OPTIONS.local!, storage: { ...DEMO_NEW_SPACE_OPTIONS.local!.storage!, pulled: [] } } };
    const [linux, macos, ...rest] = firstSpaceOffers(core, env(core, fresh));
    expect(rest).toEqual([]);
    expect(linux).toMatchObject({ os: "linux", image: "ghcr.io/trycua/linux:24.04", time: "about 2 min", blocked: null });
    expect(linux!.size).toMatch(/^1\.\d GB download$/);
    // On this Mac, whatever Settings' default says.
    expect(linux!.args).toMatchObject({ image: "ghcr.io/trycua/linux:24.04", on: "local" });
    expect(macos).toMatchObject({ os: "macos", image: "ghcr.io/trycua/macos:26", time: "10 to 30 min the first time", blocked: null });
    expect(macos!.size).toMatch(/^2\d GB download$/);
    expect(macos!.args).toMatchObject({ on: "local", kind: "vm" });
    expect(macos!.args.gpu).toBeUndefined();
    // A pulled image says so, and is quicker.
    const [cached] = firstSpaceOffers(core, env(core));
    expect(cached).toMatchObject({ size: "Already downloaded", time: "under a minute" });
  });

  it("names the real cause when this Mac has no room, instead of a create that fails", async () => {
    const core = await testCore();
    const GB = 1024 ** 3;
    const storage = { ...DEMO_NEW_SPACE_OPTIONS.local!.storage!, pulled: [], lume: { availableBytes: 20 * GB, totalBytes: 494 * GB, name: "Macintosh HD" } };
    const tight = { ...DEMO_NEW_SPACE_OPTIONS, local: { ...DEMO_NEW_SPACE_OPTIONS.local!, storage } };
    const [linux, macos] = firstSpaceOffers(core, env(core, tight));
    expect(linux!.blocked).toBeNull();
    expect(macos!.blocked).toMatch(/^Not enough space on Macintosh HD: needs \d+ GB, 20 GB available\.$/);
  });

  it("offers Linux in one click to a signed-out Mac with no Spaces (it needs no sign-in)", async () => {
    const core = await testCore();
    const { hooks } = mount(core, 4, { signedIn: false, onboarded: true, noSpaces: true });
    await waitFor(() => expect(hooks.first.offers.map((o) => o.os)).toEqual(["linux", "macos"]));
    expect(hooks.first.offers[0]!.blocked).toBeNull();
  });

  it("asks the host again for options it did not give, so the offer still shows", async () => {
    const core = await testCore();
    let failures = 1;
    const { hooks } = mount(core, 4, { noSpaces: true }, (a) =>
      Object.assign(Object.create(a) as DataAdapter, {
        call: ((op: string, args: unknown) => {
          if (op === "spaces.createOptions" && failures-- > 0) return Promise.reject(new Error("the app did not answer"));
          return (a.call as (o: string, x: unknown) => Promise<unknown>).call(a, op, args);
        }) as DataAdapter["call"],
      }),
    );
    await new Promise((r) => setTimeout(r, 300));
    expect(hooks.first.offers).toEqual([]);
    await waitFor(() => expect(hooks.first.offers.length).toBe(2), { timeout: 9000 });
  }, 15000);

  it("offers nothing without the core, and sizes read like the wizard's", async () => {
    expect(firstSpaceOffers(noCore, env(await testCore()))).toEqual([]);
    expect(sizeText(1.11 * 1024 ** 3)).toBe("1.1 GB");
    expect(sizeText(22.29 * 1024 ** 3)).toBe("22 GB");
    expect(sizeText(450 * 1024 ** 2)).toBe("450 MB");
  });
});

describe.skipIf(!wasmBuilt)("On the SwiftUI host's answer (wasm core)", () => {
  const env = (core: CoreClient, options = demoMacHostOptions("local", 2, false)) =>
    wizardEnv(core, { options, clouds: null, defaultLocation: "cloud", cloudAvailable: true, machines: [] });

  it("offers both first Spaces, and the Resources step says what is free", async () => {
    const core = await testCore();
    const e = env(core);
    // The host's own env, as it sent it.
    expect(e.storage?.container?.name).toBe("Macintosh HD");
    const [linux, macos, ...rest] = firstSpaceOffers(core, e);
    expect(rest).toEqual([]);
    expect(linux).toMatchObject({ os: "linux", image: "ghcr.io/trycua/linux:24.04", size: "Already downloaded", blocked: null });
    expect(macos).toMatchObject({ os: "macos", image: "ghcr.io/trycua/macos:26", blocked: null });
    expect(macos!.size).toMatch(/^2\d GB download$/);
    let state = wizardReduce(core, wizardInitial(core, e), { type: "choose-placement", on: "local" }, e);
    state = wizardReduce(core, state, { type: "next" }, e);
    const resources = wizardView(core, state, e);
    expect(resources.step).toBe(1);
    expect(resources.resourceFacts.find((f) => f.id === "available")).toMatchObject({ label: "Available", value: "212 GB on Macintosh HD" });
  });

  it("offers Linux while the host is still starting, then asks again once it is ready", async () => {
    const core = await testCore();
    const { hooks, adapter } = mount(core, 4, { noSpaces: true, keychain: true, macHost: true });
    const asked = () => readNewSpaceSession().options;
    // The launch is at the Keychain: what the host knows, with no storage yet.
    await waitFor(() => expect(asked()?.pending).toBe(true));
    await waitFor(() => expect(hooks.first.offers.map((o) => o.os)).toEqual(["linux", "macos"]));
    expect(hooks.first.offers[0]!.size).not.toBe("Already downloaded");
    expect(hooks.first.macosVmsRunning).toBeNull();
    // Access is given; the host is ready and says so; the page asks again.
    await act(async () => {
      await adapter.call("startup.act", { action: "allowAccess" });
    });
    // Before the offer's own retry (5 s): the ready event asks again.
    await waitFor(() => expect(asked()?.pending).toBeUndefined(), { timeout: 3000 });
    expect(asked()?.env?.storage?.container?.name).toBe("Macintosh HD");
    await waitFor(() => expect(hooks.first.offers[0]!.size).toBe("Already downloaded"));
    expect(hooks.first.macosVmsRunning).toBe(2);
    expect(hooks.wizard.macosVmsRunning).toBe(2);
  }, 15000);

  it("counts the Mac's macOS VMs that aren't Spaces toward Apple's limit, in the core's words", async () => {
    const core = await testCore();
    expect(core.call("wizard.macosLimit", { spacesBusy: 0, vmsRunning: 2 })).toBe(
      "This Mac is already running 2 macOS virtual machines, the most Apple's macOS license allows at once. Stop one, then create this one.",
    );
    expect(core.call("wizard.macosLimit", { spacesBusy: 0, vmsRunning: 1 })).toBeNull();
    expect(core.call("wizard.macosLimit", { spacesBusy: 2, vmsRunning: null })).toMatch(/^This Mac is already running 2 macOS Spaces/);
  });

  it("offers the built-in Lume when only this Mac's own is chosen and missing, and runs on it once switched", async () => {
    const core = await testCore();
    const chosen: unknown[] = [];
    let lume = "system";
    // This Mac without its own Lume, set to use only its own.
    const options = () => {
      const o = demoMacHostOptions("local", 0, false);
      const backends = lume === "system" ? ["qemu", "container"] : ["lume", "qemu", "container"];
      return { ...o, local: { ...o.local!, backends }, env: { ...o.env!, localBackends: backends, lumeSource: lume } };
    };
    const { hooks } = mount(core, 4, { noSpaces: true, macHost: true }, (a) =>
      Object.assign(Object.create(a) as DataAdapter, {
        call: ((op: string, args: { row: string; option: string }) => {
          if (op === "spaces.createOptions") return Promise.resolve(options());
          if (op === "settings.choose") {
            chosen.push(args);
            lume = args.option;
            return a.call("settings.get", {});
          }
          return (a.call as (o: string, x: unknown) => Promise<unknown>).call(a, op, args);
        }) as DataAdapter["call"],
      }),
    );
    await waitFor(() => expect((readNewSpaceSession().options?.env as { lumeSource?: string } | undefined)?.lumeSource).toBe("system"));
    act(() => hooks.wizard.show("local"));
    await waitFor(() => expect(hooks.wizard.view).toBeTruthy());
    act(() => hooks.wizard.send({ type: "choose-os", os: "macos" }));
    act(() => hooks.wizard.send({ type: "choose-placement", on: "local" }));
    const change = await waitFor(() => {
      const c = hooks.wizard.view?.runtimeSwitch;
      if (!c) throw new Error("no runtime switch");
      return c;
    });
    expect(change).toMatchObject({ setting: "runtime.lume", value: "builtin", label: "Use built-in Lume" });
    expect(hooks.wizard.view?.placementError).toBeTruthy();

    await act(() => hooks.wizard.applyRuntimeSwitch(change));
    // Settings, Runtimes' own row, then this Mac's runtimes read again.
    expect(chosen).toEqual([{ row: "macos-runtime", option: "builtin" }]);
    await waitFor(() => expect(hooks.wizard.view?.runtimeSwitch ?? null).toBeNull());
    expect(hooks.wizard.view?.placementError ?? null).toBeNull();
  });

  it("never lets a starting host's answer replace a full one", async () => {
    const core = await testCore();
    let pending = false;
    const { hooks } = mount(core, 4, { noSpaces: true, macHost: true }, (a) =>
      Object.assign(Object.create(a) as DataAdapter, {
        call: ((op: string, args: unknown) =>
          op === "spaces.createOptions" && pending
            ? Promise.resolve(demoMacHostOptions("local", 0, true))
            : (a.call as (o: string, x: unknown) => Promise<unknown>).call(a, op, args)) as DataAdapter["call"],
      }),
    );
    await waitFor(() => expect(readNewSpaceSession().options?.macosVmsRunning).toBe(2));
    pending = true;
    act(() => hooks.wizard.show());
    await new Promise((r) => setTimeout(r, 50));
    expect(readNewSpaceSession().options?.pending).toBeUndefined();
    expect(readNewSpaceSession().options?.env?.storage).toBeTruthy();
  });
});

describe.skipIf(!wasmBuilt)("A create that fails (wasm core)", () => {
  it("keeps its row with the reason, then Remove clears it here and on the host", async () => {
    const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
    const deleted: string[] = [];
    const reason = "lume API 500: The number of VMs exceeds the system limit";
    const adapter: DataAdapter = {
      mode: demo.mode,
      subscribe: (l) => demo.subscribe(l),
      call: ((op: string, args: never) => {
        if (op === "spaces.create") return Promise.reject(new Error(reason));
        if (op === "spaces.delete") deleted.push((args as { spaceId: string }).spaceId);
        return demo.call(op as never, args);
      }) as DataAdapter["call"],
    };
    const hooks = {} as { spaces: SpacesHook };
    function Probe() {
      hooks.spaces = useSpaces();
      return null;
    }
    render(
      <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
        <Probe />
      </BridgeProvider>,
    );
    await waitFor(() => expect(hooks.spaces.data).toHaveLength(6));
    await act(async () => {
      await hooks.spaces.createSpace({ image: "ghcr.io/trycua/macos:26", os: "macos", on: "local" }).catch(() => {});
    });
    const row = hooks.spaces.data!.find((s) => s.id.startsWith("pending:"))!;
    expect(row.status).toBe("suspended");
    // Lume's words become the cause in plain words (the core's failure text).
    expect(row.progress?.error).toMatch(/^This Mac is already running two macOS VMs/);
    await act(() => hooks.spaces.dismissCreate(row.id));
    expect(hooks.spaces.data!.some((s) => s.id === row.id)).toBe(false);
    // The host forgets its own row for the create too (no ghost after a reload).
    expect(deleted).toEqual([row.id]);
  });
});

describe.skipIf(!wasmBuilt)("A create on one of your machines (wasm core)", () => {
  it("names that machine while it runs, and maps its row to the Space it became", async () => {
    const { hooks } = mount(await testCore(), 40);
    await waitFor(() => expect(hooks.spaces.data).toHaveLength(6));
    await waitFor(() => expect(hooks.wizard.env.hosts?.length).toBeGreaterThan(0));
    let created: Promise<{ id: string }> | null = null;
    act(() => {
      created = hooks.spaces.createSpace({ image: "ghcr.io/trycua/linux:24.04", os: "linux", on: "host:mac-mini" });
    });
    const row = await waitFor(() => {
      const r = hooks.spaces.data!.find((s) => s.id.startsWith("pending:"));
      expect(r).toBeDefined();
      return r!;
    });
    expect(row).toMatchObject({ provider: "relay", hostName: "Mac mini" });
    expect(row.detail.startsWith("Another machine")).toBe(true);
    let space: { id: string } | null = null;
    await act(async () => {
      space = await created;
    });
    expect(space!.id.startsWith("relay:")).toBe(true);
    expect(hooks.spaces.createdId(row.id)).toBe(space!.id);
    expect(hooks.spaces.createdId("pending:unknown")).toBeUndefined();
  });
});

describe.skipIf(!wasmBuilt)("A create the host runs (wasm core)", () => {
  it("lists as Failed with the host's reason, not as a stopped Space", async () => {
    const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
    const error = "This Mac is already running two macOS VMs, the most Apple's macOS license allows at once.";
    const adapter: DataAdapter = {
      mode: demo.mode,
      subscribe: (l) => demo.subscribe(l),
      call: (async (op: string, args: never) => {
        const out = await demo.call(op as never, args);
        if (op !== "spaces.list") return out;
        const ghost = { id: "pending:host1", name: "macOS", provider: "local", os: "macos", spacesdVersion: "", features: [], reachable: false, powerState: "suspended", hostProgress: { phase: "booting", permille: 900, label: "Failed", error } };
        return [...(out as unknown[]), ghost];
      }) as DataAdapter["call"],
    };
    const hooks = {} as { spaces: SpacesHook };
    function Probe() {
      hooks.spaces = useSpaces();
      return null;
    }
    render(
      <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
        <Probe />
      </BridgeProvider>,
    );
    await waitFor(() => expect(hooks.spaces.data).toHaveLength(7));
    expect(hooks.spaces.data!.find((s) => s.id === "pending:host1")).toMatchObject({ status: "suspended", detail: error, progress: { error } });
  });
});

describe("New Space without the core", () => {
  it("is not offered, and opening it does nothing", async () => {
    const { hooks } = mount(noCore);
    expect(hooks.wizard.offered).toBe(false);
    act(() => hooks.wizard.show());
    expect(hooks.wizard.open).toBe(false);
    expect(hooks.wizard.view).toBeNull();
  });
});
