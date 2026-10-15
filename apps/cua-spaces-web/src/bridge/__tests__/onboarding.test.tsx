// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import { BridgeProvider, useOnboarding, useSession, useSettings, type OnboardingHook, type SessionHook, type SettingsHook } from "../index";
import type { HostOs } from "../host-os";
import {
  firstSpaceImages,
  hasNotch,
  onboardingInitial,
  onboardingReduce,
  onboardingTelemetry,
  onboardingView,
  passedSteps,
  permissionRows,
  readSavedOnboarding,
  resetSavedOnboardingCache,
} from "../onboarding";
import { noCore, testCore, wasmBuilt } from "./testCore";

// Off a Mac: no notch, so the flow passes through Menu bar.
const ctx = { menuBar: false, os: "linux" as HostOs };

/** The host the hooks see (`hostOs()` reads the browser's platform). */
const onPlatform = (platform: string) => vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the Menu bar page per host", () => {
  it.each([
    ["macos", true],
    ["windows", false],
    ["linux", false],
  ] as const)("%s: notch %s", (os, notch) => {
    expect(hasNotch(os)).toBe(notch);
    expect(passedSteps(os)).toEqual(notch ? [] : ["presentation"]);
  });

  it("follows the host's platform (the Mac app's web view, Electron on each system)", () => {
    onPlatform("MacIntel");
    expect(hasNotch()).toBe(true);
    onPlatform("Win32");
    expect(hasNotch()).toBe(false);
    onPlatform("Linux x86_64");
    expect(hasNotch()).toBe(false);
  });
});

describe.each([
  ["wasm core", testCore],
  ["no core", async () => noCore],
])("the first run (%s)", (_name, getCore) => {
  it("walks the web pages in the core's order, past the native-only ones", async () => {
    const core = await getCore();
    let s = onboardingInitial(core, null);
    expect(onboardingView(core, s, "linux").dots.map((d) => d.label)).toEqual(["Welcome", "Sign in", "AI agents", "This machine", "Done"]);
    s = onboardingReduce(core, s, { type: "start" }, ctx);
    expect(s.step).toBe("signin");
    expect(onboardingView(core, s, "linux").canSkip).toBe(true);
    expect(onboardingView(core, s, "linux").lede).toBe("Sign in for account-based access to your machines and Spaces across devices; skip to continue without it.");
    s = onboardingReduce(core, s, { type: "signin-done" }, ctx);
    expect(s.step).toBe("agents");
    s = onboardingReduce(core, s, { type: "agents-done", configured: ["Codex"] }, ctx);
    expect(s.step).toBe("mode");
    s = onboardingReduce(core, s, { type: "back" }, ctx);
    expect(s.step).toBe("agents");
    s = onboardingReduce(core, s, { type: "agents-done", configured: ["Codex"] }, ctx);
    s = onboardingReduce(core, s, { type: "mode-chosen", mode: "client" }, ctx);
    expect(s.step).toBe("done");
    const v = onboardingView(core, s, "linux");
    expect(v.title).toBe("You're all set");
    expect(v.summary.map((f) => f.label)).toEqual(["Account", "AI agents", "This machine"]);
    expect(onboardingView(core, s, "windows").summary.map((f) => f.label)).toEqual(["Account", "AI agents", "This machine"]);
    // Where the app stays: the menu bar on a Mac, the tray on Windows and Linux.
    expect(onboardingView(core, s, "macos").lede).toBe("Cua Spaces is in your menu bar.");
    expect(onboardingView(core, s, "windows").lede).toBe("Cua Spaces is in your system tray.");
    expect(onboardingView(core, s, "linux").lede).toBe("Cua Spaces is in your system tray.");
  });

  it("keeps the presentation setting the machine has", async () => {
    const core = await getCore();
    let s = onboardingReduce(core, onboardingInitial(core, null), { type: "start" }, ctx);
    s = onboardingReduce(core, s, { type: "signin-done" }, { ...ctx, menuBar: true });
    s = onboardingReduce(core, s, { type: "agents-done", configured: [] }, { ...ctx, menuBar: true });
    expect(s.menuBar).toBe(true);
  });

  it("asks where the app shows on a Mac, and Done says it", async () => {
    const core = await getCore();
    const mac = { menuBar: false, os: "macos" as HostOs };
    let s = onboardingInitial(core, null);
    expect(onboardingView(core, s, "macos").dots.map((d) => d.label)).toEqual(["Welcome", "Sign in", "AI agents", "Menu bar", "This machine", "Done"]);
    s = onboardingReduce(core, s, { type: "start" }, mac);
    s = onboardingReduce(core, s, { type: "signin-done" }, mac);
    s = onboardingReduce(core, s, { type: "agents-done", configured: [] }, mac);
    expect(s.step).toBe("presentation");
    let v = onboardingView(core, s, "macos");
    expect(v.title).toBe("Where should Cua Spaces show up?");
    expect(v.lede).toBe("You can change this in Settings.");
    expect(v.presentations?.map((c) => [c.id, c.title, c.selected])).toEqual([
      ["onboarding-notch", "Notch and menu bar", true],
      ["onboarding-menu-bar", "Menu bar only", false],
    ]);
    s = onboardingReduce(core, s, { type: "presentation-picked", menuBar: true }, mac);
    expect(onboardingView(core, s, "macos").presentations?.find((c) => c.selected)?.title).toBe("Menu bar only");
    s = onboardingReduce(core, s, { type: "presentation-done" }, { ...mac, menuBar: true });
    expect(s.step).toBe("mode");
    s = onboardingReduce(core, s, { type: "back" }, { ...mac, menuBar: true });
    expect(s.step).toBe("presentation");
    s = onboardingReduce(core, s, { type: "presentation-done" }, { ...mac, menuBar: true });
    s = onboardingReduce(core, s, { type: "mode-chosen", mode: "client" }, { ...mac, menuBar: true });
    v = onboardingView(core, s, "macos");
    expect(v.summary.map((f) => f.label)).toEqual(["Account", "AI agents", "Shows in", "This machine"]);
    expect(v.summary.find((f) => f.label === "Shows in")?.value).toBe("Menu bar only");
  });

  it("starts the Menu bar choice from the setting now", async () => {
    const core = await getCore();
    let s = onboardingInitial(core, null);
    s = onboardingReduce(core, s, { type: "start" }, { menuBar: false, os: "macos" });
    s = onboardingReduce(core, s, { type: "signin-done" }, { menuBar: false, os: "macos" });
    // Changed in Settings since the flow started.
    s = onboardingReduce(core, s, { type: "agents-done", configured: [] }, { menuBar: true, os: "macos" });
    expect(s).toMatchObject({ step: "presentation", menuBar: true });
  });

  it("shows the usage switch from the setting, and locks it when the environment decides", async () => {
    const core = await getCore();
    let s = onboardingInitial(core, null);
    s = onboardingReduce(core, s, { type: "telemetry-loaded", telemetry: { enabled: false, lockedBy: null } }, ctx);
    expect(onboardingView(core, s, "linux").usage).toMatchObject({ on: false, enabled: true });
    s = onboardingReduce(core, s, { type: "usage-data-toggled", on: true }, ctx);
    expect(onboardingView(core, s, "linux").usage?.on).toBe(true);

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
    expect(onboardingView(core, s, "linux").usage).toMatchObject({ on: false, enabled: false, help: "Set by env DO_NOT_TRACK" });
  });

  it("lists only the permissions still to grant, and one image per OS", async () => {
    const core = await getCore();
    const rows = permissionRows(core, [
      { id: "screen-recording", label: "Screen Recording", instructions: "Turn it on", settingsUrl: "x-apple.systempreferences:a" },
      { id: "accessibility", label: "Accessibility", granted: true },
    ]);
    expect(rows).toEqual([{ id: "screen-recording", title: "Screen Recording", help: "Turn it on", settingsUrl: "x-apple.systempreferences:a" }]);
    expect(firstSpaceImages(core, "macos").map((i) => i.os)).toEqual(["macos", "linux", "windows"]);
  });
});

describe("the first Space's images per host", () => {
  it.each([
    ["macos", ["macos", "linux", "windows"]],
    ["linux", ["linux", "windows"]],
    ["windows", ["linux", "windows"]],
  ] as const)("on %s", (os, expected) => {
    expect(firstSpaceImages(noCore, os).map((i) => i.os)).toEqual(expected);
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
  beforeEach(() => onPlatform("Linux x86_64"));

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

  it("stops the pending browser sign-in when Sign in is skipped", async () => {
    const { probe } = mount(await testCore());
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    await act(async () => {
      void probe.session.signIn().catch(() => {});
    });
    await waitFor(() => expect(["starting", "waiting"]).toContain(probe.session.data?.signIn.kind));
    act(() => probe.flow.send({ type: "signin-done" }));
    await waitFor(() => expect(probe.session.data?.signIn.kind).toBe("idle"));
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

  it("on a Mac, the Menu bar pick writes the notch setting Settings shows", async () => {
    onPlatform("MacIntel");
    const { probe, adapter, calls } = mount(await testCore());
    await waitFor(() => expect(probe.flow.state && probe.settings.data).toBeTruthy());
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    expect(probe.flow.view?.step).toBe("presentation");
    act(() => probe.flow.send({ type: "presentation-picked", menuBar: true }));
    await waitFor(() => expect(adapter.state.settings.menuBar).toBe(true));
    expect(calls).toContainEqual(["settings.set", { key: "menuBar", value: true }]);
    expect(probe.flow.view?.presentations?.find((c) => c.selected)?.id).toBe("onboarding-menu-bar");
    act(() => probe.flow.send({ type: "presentation-done" }));
    expect(probe.flow.view?.step).toBe("mode");
    act(() => probe.flow.send({ type: "mode-chosen", mode: "client" }));
    expect(probe.flow.view?.summary).toContainEqual({ label: "Shows in", value: "Menu bar only" });
  });
});

describe.skipIf(!wasmBuilt)("Done's Launch at login", () => {
  beforeEach(() => onPlatform("Linux x86_64"));
  const toDone = async (probe: Probe) => {
    await waitFor(() => expect(probe.flow.state).toBeTruthy());
    act(() => probe.flow.send({ type: 'start' }));
    act(() => probe.flow.send({ type: 'signin-done' }));
    act(() => probe.flow.send({ type: 'agents-done', configured: [] }));
    act(() => probe.flow.send({ type: 'presentation-done' }));
    act(() => probe.flow.send({ type: 'mode-chosen', mode: 'client' }));
    await waitFor(() => expect(probe.flow.view?.step).toBe('done'));
  };

  it('shows where the host applies it (Electron), and finishes with the choice', async () => {
    const { probe, calls } = mount(await testCore(), 'electron');
    await toDone(probe);
    expect(probe.flow.launchAtLogin).toMatchObject({ checked: false });
    expect(probe.flow.launchAtLogin?.label).toBeTruthy();
    await act(async () => {
      await probe.flow.finish();
    });
    expect(calls.filter(([op]) => op === 'session.completeOnboarding')).toEqual([
      ['session.completeOnboarding', { mode: 'client', launchAtLogin: false }],
    ]);
  });

  it('is not offered where the host does not apply it', async () => {
    const { probe, calls } = mount(await testCore());
    await toDone(probe);
    expect(probe.flow.launchAtLogin).toBeNull();
    await act(async () => {
      await probe.flow.finish();
    });
    expect(calls.filter(([op]) => op === 'session.completeOnboarding')).toEqual([
      ['session.completeOnboarding', { mode: 'client' }],
    ]);
  });
});
