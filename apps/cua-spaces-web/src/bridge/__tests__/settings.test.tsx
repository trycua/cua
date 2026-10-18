// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import {
  BridgeProvider,
  useAbout,
  useDevices,
  useExperiments,
  useLoginItem,
  useNotifications,
  useSpaces,
  useStorageSettings,
  type AboutHook,
  type DevicesHook,
  type ExperimentsHook,
  type LoginItemHook,
  type NotificationsHook,
  type SpacesHook,
  type StorageHook,
} from "../index";
import { replayFlow } from "../parity";
import { testCore, wasmBuilt } from "./testCore";

afterEach(cleanup);

interface Hooks {
  about: AboutHook;
  devices: DevicesHook;
  experiments: ExperimentsHook;
  login: LoginItemHook;
  storage: StorageHook;
  notifications: NotificationsHook;
  spaces: SpacesHook;
}

function mount(core: CoreClient) {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 6 });
  const hooks = {} as Hooks;
  function Probe() {
    hooks.about = useAbout();
    hooks.devices = useDevices();
    hooks.experiments = useExperiments();
    hooks.login = useLoginItem();
    hooks.storage = useStorageSettings();
    hooks.notifications = useNotifications();
    hooks.spaces = useSpaces();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <Probe />
    </BridgeProvider>,
  );
  return { hooks, adapter };
}

describe.skipIf(!wasmBuilt)("Settings and Notifications through the core", () => {
  it("replays the Settings and Notifications parity flows through the bridge", async () => {
    const core = await testCore();
    for (const name of ["about", "devices", "drive-storage", "experiments", "launch-at-login", "notifications"]) {
      const replay = replayFlow(core, name);
      expect(replay.bridged.length, name).toBeGreaterThan(0);
      expect(replay.checkpoints.length, name).toBeGreaterThan(0);
      expect(replay.transcript, name).toEqual(replay.golden);
    }
  });

  it("draws About from the host's version and changes the update controls", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.about.data?.view?.versionLine).toBe("Version 0.6.0 (0.6.0.142)"));
    expect(hooks.about.data?.view?.links.map((l) => l.id)).toEqual(["acknowledgements", "privacy", "terms", "issue"]);
    await hooks.about.setAbout({ autoCheck: false });
    await waitFor(() => expect(hooks.about.data?.view?.updates?.autoInstallEnabled).toBe(false));
    await hooks.about.setAbout({ channel: "beta" });
    await waitFor(() => expect(hooks.about.data?.view?.updates?.channels.find((c) => c.active)?.id).toBe("beta"));
  });

  it("turns an experiment on, and Cua Volume changes the launch-at-login line", async () => {
    const { hooks } = mount(await testCore());
    // The demo starts with Cua Volume on (the Volume page's demo); start from off.
    await waitFor(() => expect(hooks.experiments.data?.experiments).toBeDefined());
    await hooks.experiments.chooseExperiment("experiment:cua_volume", "off");
    await waitFor(() => expect(hooks.login.data?.rows?.map((r) => r.id)).toEqual(["launch-at-login", "launch-at-login-note"]));
    await waitFor(() => expect(hooks.login.data?.rows?.[1]?.label).toBe("Keeps your Spaces and agents available after a restart."));
    await hooks.experiments.chooseExperiment("experiment:cua_volume", "on");
    await waitFor(() => expect(hooks.experiments.data?.experiments.cuaVolume).toBe(true));
    await waitFor(() => expect(hooks.login.data?.rows?.[1]?.label).toBe("Keeps your Spaces, agents and Cua Volume available after a restart."));
  });

  it("switches launch at login off and says what stops", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.login.data?.input.status).toBe("enabled"));
    await hooks.login.setLaunchAtLogin(false);
    await waitFor(() => expect(hooks.login.data?.input.status).toBe("notRegistered"));
    expect(hooks.login.data?.rows?.[1]?.label).toBe("This machine runs persistent agents, which stop after a restart until you open Cua Spaces.");
  });

  it("lists the account's devices, approves the waiting one by code and revokes another", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.devices.data?.view?.rows.map((r) => r.id)).toEqual(["dev_mac", "dev_old", "dev_studio", "dev_work"]));
    expect(hooks.devices.data?.view?.approvals.map((a) => a.deviceId).sort()).toEqual(["dev_old", "dev_work"]);
    hooks.devices.openApproval("dev_work");
    await waitFor(() => expect(hooks.devices.data?.approve?.view?.canApprove).toBe(false));
    hooks.devices.setApprovalCode("k7qx m2rp");
    await waitFor(() => expect(hooks.devices.data?.approve?.view?.code).toBe("K7QX-M2RP"));
    await hooks.devices.approve();
    await waitFor(() => expect(hooks.devices.data?.view?.rows.find((r) => r.id === "dev_work")?.subtitle).toMatch(/^Enrolled/));
    expect(hooks.devices.data?.approve).toBeNull();
    await hooks.devices.revoke("dev_studio");
    await waitFor(() => expect(hooks.devices.data?.view?.rows.find((r) => r.id === "dev_studio")?.subtitle).toBe("Revoked"));
  });

  it("enrolls with a one-time code in the enroll sheet", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.devices.data?.view).toBeTruthy());
    hooks.devices.startEnroll();
    await waitFor(() => expect(hooks.devices.data?.enroll?.view?.options.map((o) => o.method)).toEqual(["sign-in", "approve"]));
    await hooks.devices.chooseEnroll("approve");
    await waitFor(() => expect(hooks.devices.data?.enroll?.state.phase).toBe("enrolled"));
    expect(hooks.devices.data?.enroll?.view?.closeLabel).toBe("Done");
  });

  it("runs Storage's requests: the cache limit, then Clear cache", async () => {
    const { hooks, adapter } = mount(await testCore());
    await waitFor(() => expect(hooks.storage.data?.section?.rows.map((r) => r.id)).toContain("cache-limit"));
    hooks.storage.choose("cache-limit", String(5 * 1024 ** 3));
    await waitFor(() => expect(hooks.storage.data?.section?.rows.find((r) => r.id === "cache")?.value).toBe("1.2 GB of 5 GB"));
    hooks.storage.press("cache");
    await waitFor(() => expect(hooks.storage.data?.section?.rows.find((r) => r.id === "cache")?.value).toBe("0 KB of 5 GB"));
    adapter.dispose?.();
  });

  it("lists notifications and marks them read", async () => {
    const { hooks } = mount(await testCore());
    await waitFor(() => expect(hooks.notifications.data?.unread).toBe(2));
    expect(hooks.notifications.data?.view?.rows[0]?.text).toBe("ada: Your research is ready.");
    expect(hooks.notifications.data?.view?.markAllLabel).toBe("Mark all read");
    await hooks.notifications.markAllRead();
    await waitFor(() => expect(hooks.notifications.data?.unread).toBe(0));
    expect(hooks.notifications.data?.view?.markAllLabel).toBeNull();
  });

  it("lists a Space that became ready and a create that failed, with why", async () => {
    const { hooks, adapter } = mount(await testCore());
    const call = adapter.call.bind(adapter);
    adapter.call = ((op: string, args: never) =>
      op === "spaces.create" && (args as { config: { name?: string } }).config.name === "boom"
        ? Promise.reject(new Error("The number of virtual machines exceeds the limit."))
        : call(op as never, args)) as typeof adapter.call;
    await waitFor(() => expect(hooks.notifications.data?.unread).toBe(2));
    await hooks.spaces.createSpace({ image: "ubuntu-xfce", name: "qa-gui-linux-local", os: "linux" });
    await expect(hooks.spaces.createSpace({ image: "macos-tahoe", name: "boom", os: "macos" })).rejects.toThrow(/exceeds the limit/);
    await waitFor(() => expect(hooks.notifications.data?.unread).toBe(4));
    const texts = hooks.notifications.data!.view!.rows.map((r) => r.text);
    expect(texts).toContain("qa-gui-linux-local is ready");
    // The reason in the failed row's words (the core names the macOS limit).
    expect(texts.find((t) => t.startsWith("Couldn't create boom: "))).toMatch(/This Mac is already running/);
    await hooks.notifications.markAllRead();
    await waitFor(() => expect(hooks.notifications.data?.unread).toBe(0));
  });
});
