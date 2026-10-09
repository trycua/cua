// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import {
  BridgeProvider,
  useBridge,
  useKeyvault,
  useMachines,
  useSession,
  useSettings,
  useSpaces,
  type KeyvaultHook,
  type MachinesHook,
  type SessionHook,
  type SettingsHook,
  type SpacesHook,
} from "../index";
import { noCore, testCore } from "./testCore";

afterEach(cleanup);

interface Hooks {
  bridge: ReturnType<typeof useBridge>;
  spaces: SpacesHook;
  machines: MachinesHook;
  settings: SettingsHook;
  keyvault: KeyvaultHook;
  session: SessionHook;
}

function mount(core: CoreClient, options: Parameters<typeof createDemoAdapter>[0] = {}) {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 6, ...options });
  const hooks = {} as Hooks;
  function Probe() {
    hooks.bridge = useBridge();
    hooks.spaces = useSpaces();
    hooks.machines = useMachines();
    hooks.settings = useSettings();
    hooks.keyvault = useKeyvault();
    hooks.session = useSession();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core} storeOptions={{ tickMs: 5 }}>
      <Probe />
    </BridgeProvider>,
  );
  return { hooks, adapter };
}

const loaded = (h: Hooks) =>
  waitFor(() => {
    expect(h.spaces.data).toHaveLength(6);
    expect(h.machines.data).toHaveLength(4);
    expect(h.settings.data).toBeDefined();
    expect(h.keyvault.data).toBeDefined();
    expect(h.session.data).toBeDefined();
  });

describe.each([
  ["wasm core", testCore],
  ["no core", async () => noCore],
])("hooks over the demo adapter (%s)", (_name, getCore) => {
  it("start loading, then show the demo data", async () => {
    const { hooks } = mount(await getCore());
    expect(hooks.spaces.isLoading).toBe(true);
    expect(hooks.bridge.mode).toBe("demo");
    await loaded(hooks);
    expect(hooks.spaces.isLoading).toBe(false);
    expect(hooks.machines.data?.map((m) => [m.name, m.spaceIds.length])).toEqual([
      ["This Mac", 2],
      ["Mac mini", 2],
      ["Linux box", 2],
      ["Studio PC", 0],
    ]);
    expect(hooks.keyvault.data?.groups.map((g) => g.app)).toContain("Google Chrome");
    expect(hooks.session.data?.signedIn).toBe(true);
  });

  it("stops and starts a Space", async () => {
    const { hooks } = mount(await getCore(), { stepMs: 80 });
    await loaded(hooks);
    let stop!: Promise<void>;
    act(() => {
      stop = hooks.spaces.stopSpace("relay:linux-box/ubuntu-build");
    });
    await waitFor(() => expect(hooks.spaces.data?.find((s) => s.id.endsWith("ubuntu-build"))?.power?.turningOn).toBe(false));
    await act(() => stop);
    expect(hooks.spaces.data?.find((s) => s.id.endsWith("ubuntu-build"))?.status).toBe("suspended");
    await act(() => hooks.spaces.startSpace("relay:linux-box/ubuntu-build"));
    expect(hooks.spaces.data?.find((s) => s.id.endsWith("ubuntu-build"))?.status).toBe("running");
  });

  it("shows a create's progress, then the new Space", async () => {
    const { hooks } = mount(await getCore(), { stepMs: 80 });
    await loaded(hooks);
    let create!: Promise<unknown>;
    act(() => {
      create = hooks.spaces.createSpace({ image: "ubuntu-xfce", name: "scratch", on: "host:mac-mini" });
    });
    await waitFor(() => {
      const pending = hooks.spaces.data?.find((s) => s.id.startsWith("pending:"));
      expect(pending?.status).toBe("provisioning");
      expect(pending?.progress?.permille).toBeGreaterThan(0);
    });
    await act(async () => {
      await create;
    });
    expect(hooks.spaces.data?.some((s) => s.id.startsWith("pending:"))).toBe(false);
    expect(hooks.spaces.data?.find((s) => s.id === "relay:mac-mini/scratch")?.status).toBe("running");
    expect(hooks.machines.data?.find((m) => m.id === "mac-mini")?.spaceIds).toContain("relay:mac-mini/scratch");
  });

  it("updates a setting", async () => {
    const { hooks } = mount(await getCore());
    await loaded(hooks);
    await act(() => hooks.settings.updateSetting("theme", "dark"));
    expect(hooks.settings.data?.values.theme).toBe("dark");
  });

  it("locks and unlocks Keyvault items", async () => {
    const { hooks } = mount(await getCore());
    await loaded(hooks);
    await act(() => hooks.keyvault.unlockItems(["kv-linear", "kv-figma"]));
    const item = (id: string) => hooks.keyvault.data?.overview.items.find((i) => i.id === id);
    expect(item("kv-linear")?.policy.unattended).toBe(true);
    await act(() => hooks.keyvault.lockItem("kv-linear"));
    expect(item("kv-linear")?.policy.unattended).toBe(false);
    expect(item("kv-figma")?.policy.unattended).toBe(true);
  });

  it("signs in through the browser", async () => {
    const { hooks } = mount(await getCore(), { signedIn: false });
    await loaded(hooks);
    expect(hooks.session.data?.signedIn).toBe(false);
    await act(async () => {
      await hooks.session.signIn();
    });
    expect(hooks.session.data?.signIn.kind).toBe("waiting");
    await waitFor(() => expect(hooks.session.data?.signedIn).toBe(true));
    expect(hooks.session.data?.identity).toBe("ada@example.com");
    expect(hooks.session.data?.signIn.kind).toBe("idle");
  });
});

describe("with the wasm core", () => {
  it("exposes the core and its views", async () => {
    const core = await testCore();
    if (core.status !== "ready") return;
    const { hooks } = mount(core);
    await loaded(hooks);
    expect(hooks.bridge.core.status).toBe("ready");
    expect(hooks.settings.data?.page?.sections.map((s) => s.id)).toContain("account");
    expect(hooks.keyvault.data?.views?.page.pendingCount).toBe(1);
  });

  it("refuses a malformed image before calling the host", async () => {
    const core = await testCore();
    if (core.status !== "ready") return;
    const { hooks } = mount(core);
    await loaded(hooks);
    await expect(hooks.spaces.createSpace({ image: "not an image!" })).rejects.toThrow(/image reference/i);
    expect(hooks.spaces.data).toHaveLength(6);
  });
});

it("hooks need a provider", () => {
  function Bare() {
    useSpaces();
    return null;
  }
  expect(() => render(<Bare />)).toThrow(/BridgeProvider/);
});
