// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import { BridgeProvider, useOnboarding, type OnboardingHook } from "../index";
import { readSavedOnboarding, resetSavedOnboardingCache } from "../onboarding";
import { useDriverSetup, useExperimentFlags, useOnboardingVolume, useVolume, type DriverSetup, type VolumeHook } from "../volume";
import type { Experiments } from "../contracts/volume";
import { testCore, wasmBuilt } from "./testCore";

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
});
afterEach(cleanup);

describe("the demo host's Cua Volume", () => {
  it("lists, approves, mounts and resolves", async () => {
    const a = createDemoAdapter({ latencyMs: 0, stepMs: 1, onboarded: false });
    expect(await a.call("experiments.get", {})).toMatchObject({ cuaVolume: true });
    const before = await a.call("volume.overview", {});
    expect(before.mount?.state).toBe("off");
    expect(before.requests).toHaveLength(1);
    await a.call("volume.approve", { id: before.requests[0]!.id });
    const after = await a.call("volume.overview", {});
    expect(after.requests).toEqual([]);
    expect(after.grants.map((g) => g.principal)).toContain("agent:researcher");
    expect((await a.call("volume.mount", {})).path).toBe("/Users/ada/Cua Volume");
    await a.call("volume.resolve", { path: "public/plan.md" });
    expect((await a.call("volume.overview", {})).sync?.conflicts).toEqual([]);
    const check = await a.call("volume.storageSet", {
      update: { backend: "s3", s3: { region: "us-east-1", bucket: "b", root: "", path_style: false }, access_key_id: "k", secret_access_key: "s", dry_run: true },
    });
    expect(check).toMatchObject({ ok: true, applied: false });
    expect((await a.call("volume.storage", {}))?.backend).toBe("fs");
    a.dispose?.();
  });
});

interface Probe {
  volume: VolumeHook;
  experiments: Experiments | null;
  driver: DriverSetup;
}

function mountPage(core: CoreClient) {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 5 });
  const probe = {} as Probe;
  function P() {
    probe.volume = useVolume();
    probe.experiments = useExperimentFlags();
    probe.driver = useDriverSetup();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <P />
    </BridgeProvider>,
  );
  return { probe, adapter };
}

describe.skipIf(!wasmBuilt)("the Volume page over the demo host", () => {
  it("draws the core's page and runs what it asks for", async () => {
    const { probe, adapter } = mountPage(await testCore());
    await waitFor(() => expect(probe.volume.view?.busy).toBe(false));
    expect(probe.experiments?.cuaVolume).toBe(true);
    const v = probe.volume.view!;
    expect(v.openLabel).toBe("Open in Finder");
    expect(v.mountLine).toBe("In Finder at ~/Cua Volume");
    expect(v.requests[0]?.text).toBe("agent:researcher asks to read agents/writer/outputs/");
    expect(v.devices.map((d) => d.text)).toEqual(["This Mac (this device)", "Linux box"]);
    expect(v.conflicts[0]?.openLabel).toBe("Open");

    act(() => probe.volume.act({ type: "approve", id: v.requests[0]!.id }));
    expect(probe.volume.view?.busy).toBe(true);
    await waitFor(() => expect(probe.volume.view?.busy).toBe(false));
    expect(probe.volume.view?.requests).toEqual([]);
    expect(probe.volume.view?.grants.map((g) => g.text)).toContain("agent:researcher can read agents/writer/outputs/");
    expect(adapter.state.volume.requests).toEqual([]);

    act(() => probe.volume.act({ type: "resolve", path: "public/plan.md" }));
    await waitFor(() => expect(probe.volume.view?.conflicts).toEqual([]));

    const summaries = await probe.driver.setUp([{ id: "codex", name: "OpenAI Codex" }]);
    expect(summaries[0]?.line).toBe("OpenAI Codex: done");
  });
});

function mountOnboarding(core: CoreClient) {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 5, signedIn: false, onboarded: false });
  const probe = {} as { flow: OnboardingHook };
  function P() {
    const flow = useOnboarding();
    useOnboardingVolume(flow);
    probe.flow = flow;
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <P />
    </BridgeProvider>,
  );
  return { probe, adapter };
}

describe.skipIf(!wasmBuilt)("the first run's Cua Volume page", () => {
  it("shows with the experiment on, mounts when ticked, then moves on", async () => {
    const { probe, adapter } = mountOnboarding(await testCore());
    await waitFor(() => expect(probe.flow.state?.experiments).toMatchObject({ cuaVolume: true }));
    await waitFor(() => expect(probe.flow.state?.driveChecked).toBe(true));
    act(() => probe.flow.send({ type: "start" }));
    act(() => probe.flow.send({ type: "signin-done" }));
    act(() => probe.flow.send({ type: "agents-done", configured: [] }));
    expect(probe.flow.view?.step).toBe("drive");
    const card = probe.flow.view!.drive!;
    expect(card.label).toBe("Add Cua Volume to Finder");
    expect(card.storageOptions.find((o) => o.active)?.label).toBe("This Mac");
    expect(card.storedIn).toBe("Stored in ~/.cua/volume/data");

    act(() => probe.flow.send({ type: "drive-toggled", on: true }));
    act(() => probe.flow.send({ type: "drive-continue" }));
    await waitFor(() => expect(readSavedOnboarding().state?.step).toBe("mode"));
    expect(adapter.state.volume.mounted).toBe(true);
    act(() => probe.flow.send({ type: "mode-chosen", mode: "client" }));
    expect(probe.flow.view?.summary.find((f) => f.label === "Cua Volume")?.value).toBe("In Finder");
  });
});
