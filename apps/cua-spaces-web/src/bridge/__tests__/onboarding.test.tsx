// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import { BridgeProvider, useOnboarding, useSession, useSettings, type OnboardingHook, type SessionHook, type SettingsHook } from "../index";
import {
  firstSpaceImages,
  onboardingInitial,
  onboardingReduce,
  onboardingTelemetry,
  onboardingView,
  permissionRows,
  readSavedOnboarding,
  resetSavedOnboardingCache,
} from "../onboarding";
import { noCore, testCore, wasmBuilt } from "./testCore";

const ctx = { menuBar: false };

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});
afterEach(cleanup);

describe.each([
  ["wasm core", testCore],
  ["no core", async () => noCore],
])("the first run (%s)", (_name, getCore) => {
  it("walks the web pages in the core's order, past the native-only ones", async () => {
    const core = await getCore();
    let s = onboardingInitial(core, null);
    expect(onboardingView(core, s).dots.map((d) => d.label)).toEqual(["Welcome", "Sign in", "AI agents", "This machine", "Done"]);
    s = onboardingReduce(core, s, { type: "start" }, ctx);
    expect(s.step).toBe("signin");
    expect(onboardingView(core, s).canSkip).toBe(true);
    s = onboardingReduce(core, s, { type: "signin-done" }, ctx);
    expect(s.step).toBe("agents");
    s = onboardingReduce(core, s, { type: "agents-done", configured: ["Codex"] }, ctx);
    expect(s.step).toBe("mode");
    s = onboardingReduce(core, s, { type: "back" }, ctx);
    expect(s.step).toBe("agents");
    s = onboardingReduce(core, s, { type: "agents-done", configured: ["Codex"] }, ctx);
    s = onboardingReduce(core, s, { type: "mode-chosen", mode: "client" }, ctx);
    expect(s.step).toBe("done");
    const v = onboardingView(core, s);
    expect(v.title).toBe("You're all set");
    expect(v.summary.map((f) => f.label)).toEqual(["Account", "AI agents", "This machine"]);
    // Where the app stays: the menu bar on a Mac, the tray on Windows and Linux.
    expect(onboardingView(core, s, "macos").lede).toBe("Cua Spaces is in your menu bar.");
    expect(onboardingView(core, s, "windows").lede).toBe("Cua Spaces is in your system tray.");
    expect(onboardingView(core, s, "linux").lede).toBe("Cua Spaces is in your system tray.");
  });

  it("keeps the presentation setting the machine has", async () => {
    const core = await getCore();
    let s = onboardingReduce(core, onboardingInitial(core, null), { type: "start" }, ctx);
    s = onboardingReduce(core, s, { type: "signin-done" }, { menuBar: true });
    s = onboardingReduce(core, s, { type: "agents-done", configured: [] }, { menuBar: true });
    expect(s.menuBar).toBe(true);
  });

  it("shows the usage switch from the setting, and locks it when the environment decides", async () => {
    const core = await getCore();
    let s = onboardingInitial(core, null);
    s = onboardingReduce(core, s, { type: "telemetry-loaded", telemetry: { enabled: false, lockedBy: null } }, ctx);
    expect(onboardingView(core, s).usage).toMatchObject({ on: false, enabled: true });
    s = onboardingReduce(core, s, { type: "usage-data-toggled", on: true }, ctx);
    expect(onboardingView(core, s).usage?.on).toBe(true);

    const locked = onboardingTelemetry({
      enabled: false,
      source: "env DO_NOT_TRACK",
      sourceKind: "do_not_track",
      noticeShown: true,
      noticeText: "",
      docsUrl: "",
    });
    s = onboardingReduce(core, onboardingInitial(core, null), { type: "telemetry-loaded", telemetry: locked }, ctx);
    s = onboardingReduce(core, s, { type: "usage-data-toggled", on: true }, ctx);
    expect(onboardingView(core, s).usage).toMatchObject({ on: false, enabled: false, help: "Set by env DO_NOT_TRACK" });
  });

  it("lists only the permissions still to grant, and one image per OS", async () => {
    const core = await getCore();
    const rows = permissionRows(core, [
      { id: "screen-recording", label: "Screen Recording", instructions: "Turn it on", settingsUrl: "x-apple.systempreferences:a" },
      { id: "accessibility", label: "Accessibility", granted: true },
    ]);
    expect(rows).toEqual([{ id: "screen-recording", title: "Screen Recording", help: "Turn it on", settingsUrl: "x-apple.systempreferences:a" }]);
    expect(firstSpaceImages(core).map((i) => i.os)).toEqual(["macos", "linux", "windows"]);
  });
});

interface Probe {
  flow: OnboardingHook;
  session: SessionHook;
  settings: SettingsHook;
}

function mount(core: CoreClient, mode?: "electron") {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 6, signedIn: false, onboarded: false });
  const calls: [string, unknown][] = [];
  // The Electron shell answers through the same contract; only its mode differs here.
  const adapter = Object.assign(Object.create(demo) as typeof demo, {
    mode: mode ?? demo.mode,
    call: ((op, args) => {
      calls.push([op, args]);
      return demo.call(op, args);
    }) as typeof demo.call,
  });
  const probe = {} as Probe;
  function P() {
    probe.flow = useOnboarding();
    probe.session = useSession();
    probe.settings = useSettings();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core} storeOptions={{ tickMs: 5 }}>
      <P />
    </BridgeProvider>,
  );
  return { probe, adapter, calls };
}

describe.skipIf(!wasmBuilt)("useOnboarding over the demo adapter", () => {
  it("saves progress, follows sign-in, writes the usage switch and finishes", async () => {
    const { probe, adapter } = mount(await testCore());
    await waitFor(() => expect(probe.flow.view?.usage).toBeTruthy());
    expect(probe.flow.view?.step).toBe("welcome");

    act(() => probe.flow.send({ type: "usage-data-toggled", on: false }));
    await waitFor(() => expect(adapter.state.settings.telemetry).toBe(false));

    act(() => probe.flow.send({ type: "start" }));
    expect(readSavedOnboarding().state?.step).toBe("signin");

    await act(async () => {
      await probe.session.signIn();
    });
    await waitFor(() => expect(probe.flow.state?.identity).toBe(adapter.state.identity));
    expect(probe.flow.signInText).toBe(`Signed in as ${adapter.state.identity}.`);

    act(() => probe.flow.skip());
    expect(readSavedOnboarding()).toMatchObject({ skipped: true, state: { step: "signin" } });

    act(() => probe.flow.send({ type: "signin-done" }));
    await waitFor(() => expect(probe.flow.permissions.map((p) => p.title)).toEqual(["Screen Recording", "Accessibility"]));
    expect(probe.flow.hostConfigured).toBe(false);
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    expect(readSavedOnboarding().state?.step).toBe("mode");
    act(() => probe.flow.send({ type: "mode-chosen", mode: "client" }));
    await act(async () => {
      await probe.flow.finish();
    });
    expect(adapter.state.onboarding).toMatchObject({ completed: true, mode: "client" });
    expect(readSavedOnboarding().state).toBeNull();
  });

  it("drops the account when signed out", async () => {
    const { probe } = mount(await testCore());
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    await act(async () => {
      await probe.session.signIn();
    });
    await waitFor(() => expect(probe.flow.state?.identity).toBeTruthy());
    await act(async () => {
      await probe.session.signOut();
    });
    await waitFor(() => expect(probe.flow.state?.identity).toBeNull());
  });
});

describe.skipIf(!wasmBuilt)("Done's Launch at login", () => {
  const toDone = async (probe: Probe) => {
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    act(() => probe.flow.send({ type: "mode-chosen", mode: "client" }));
    await waitFor(() => expect(probe.flow.view?.step).toBe("done"));
  };

  it("shows where the host applies it (Electron), and finishes with the choice", async () => {
    const { probe, calls } = mount(await testCore(), "electron");
    await toDone(probe);
    expect(probe.flow.launchAtLogin).toMatchObject({ checked: true });
    expect(probe.flow.launchAtLogin?.label).toBeTruthy();
    act(() => probe.flow.send({ type: "launch-at-login-toggled", on: false }));
    await waitFor(() => expect(probe.flow.launchAtLogin?.checked).toBe(false));
    await act(async () => {
      await probe.flow.finish();
    });
    expect(calls.filter(([op]) => op === "session.completeOnboarding")).toEqual([["session.completeOnboarding", { mode: "client", launchAtLogin: false }]]);
  });

  it("is not offered where the host does not apply it", async () => {
    const { probe, calls } = mount(await testCore());
    await toDone(probe);
    expect(probe.flow.launchAtLogin).toBeNull();
    await act(async () => {
      await probe.flow.finish();
    });
    expect(calls.filter(([op]) => op === "session.completeOnboarding")).toEqual([["session.completeOnboarding", { mode: "client" }]]);
  });
});
