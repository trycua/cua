// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The 0.10.0 first run on its real route with the wasm core: connect the
// agent, then "Your agent can use this ..." with This computer as the first
// Space; the extra Space never blocks Done and asks the host to notify when
// it is ready; a fix left turns Done into Finish later and Home keeps the
// to-do list.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { HostStatus } from "@/bridge/contracts/host";
import type { NewSpaceOptions } from "@/bridge/contracts/new-space";
import { readFirstRun, resetFirstRunCache, writeFirstRun } from "@/bridge/first-run";
import { CLOSED_NEW_SPACE, updateNewSpaceSession } from "@/bridge/new-space";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  updateNewSpaceSession(() => CLOSED_NEW_SPACE);
});
afterEach(cleanup);

type Calls = { op: string; args: unknown }[];

async function mount(opts: { host?: (h: HostStatus) => HostStatus; local?: (l: NonNullable<NewSpaceOptions["local"]>) => NewSpaceOptions["local"] } = {}) {
  await testCore();
  const calls: Calls = [];
  await mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    intercept: (op, args) => {
      calls.push({ op, args });
      return undefined;
    },
    answer: (op, out) => {
      if (op === "host.status" && opts.host) return opts.host(out as HostStatus);
      if (op === "spaces.createOptions" && opts.local) {
        const o = out as NewSpaceOptions;
        return { ...o, local: o.local ? opts.local(o.local) : o.local };
      }
      return out;
    },
  });
  return calls;
}

const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";
const primary = () => document.querySelector<HTMLButtonElement>("[data-first-run-primary]");

describe.skipIf(!wasmBuilt)("the first run (real route, wasm core)", () => {
  it("connects the agent, then says it can use this computer, with Done on", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("A shared Space for every agent."));
    const connect = await screen.findByRole("button", { name: /^Connect / });
    fireEvent.click(connect);
    await waitFor(() => expect(calls.find((c) => c.op === "agents.configure")).toBeTruthy());
    expect((calls.find((c) => c.op === "agents.configure")!.args as { agents: string[] }).agents).toHaveLength(1);
    await waitFor(() => expect(heading()).toMatch(/^Your agent can use this (Mac|PC|computer)\.$/), { timeout: 5000 });
    expect(primary()!.disabled).toBe(false);
    expect(primary()!.textContent).toBe("Done");
    expect(document.querySelector('[data-first-run-row="computer"]')!.textContent).toMatch(/is your first Space/);
    // Nothing in the app's own words calls a computer a VM or a container.
    expect(document.body.textContent).not.toMatch(/\bVM\b|virtual machine|container|daemon|MCP/i);
  });

  it("makes the extra Space in the background, asks the host to notify, and keeps Done on", async () => {
    writeFirstRun({ step: "setup", agent: "claude-code", setupStartedMs: Date.now() });
    const calls = await mount({ local: (l) => ({ ...l, backends: ["docker"], macosImage: null, hostArch: "amd64" }) });
    const linux = await waitFor(() => {
      const b = document.querySelector<HTMLButtonElement>('[data-first-run-extra-choice="linux"]');
      expect(b && !b.disabled).toBe(true);
      return b!;
    }, { timeout: 8000 });
    // An Intel Mac or a PC can't run macOS: it isn't offered.
    expect(document.querySelector('[data-first-run-extra-choice="macos"]')).toBeNull();
    fireEvent.click(linux);
    await waitFor(() => expect(calls.find((c) => c.op === "spaces.create")).toBeTruthy());
    const create = calls.find((c) => c.op === "spaces.create")!.args as { config: { kind: string }; notifyWhenReady?: boolean };
    expect(create.notifyWhenReady).toBe(true);
    expect(create.config.kind).toBe("container");
    expect(readFirstRun().extraPendingId).toBeTruthy();
    await waitFor(() => expect(primary()!.disabled).toBe(false));
  });

  it("names the fix, turns Done into Finish later, and keeps a to-do list on Home", async () => {
    writeFirstRun({ step: "setup", agent: "claude-code", setupStartedMs: Date.now() });
    const calls = await mount({
      host: (h) => ({ ...h, permissions: [{ id: "accessibility", label: "Accessibility", granted: false, settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" }] }),
    });
    await waitFor(() => expect(heading()).toBe("One step left."), { timeout: 5000 });
    expect(document.querySelector('[data-first-run-row="computer"]')!.textContent).toMatch(/^Let Cua control this/);
    fireEvent.click(document.querySelector('[data-first-run-action="open-settings"]')!);
    await waitFor(() => expect(calls.find((c) => c.op === "host.openSettings")).toBeTruthy());
    expect(primary()!.textContent).toBe("Finish later");
    fireEvent.click(primary()!);
    await waitFor(() => expect(calls.find((c) => c.op === "session.completeOnboarding")).toBeTruthy());
    await waitFor(() => expect(document.querySelector("[data-first-run-todos]")).toBeTruthy(), { timeout: 5000 });
    expect(document.querySelector('[data-first-run-todo="computer"]')!.textContent).toMatch(/Let Cua control/);
  });

  it("offers the host's runtime fix when nothing can be made here", async () => {
    writeFirstRun({ step: "setup", agent: "claude-code", setupStartedMs: Date.now() });
    await mount({
      local: (l) => ({
        ...l,
        available: false,
        backends: [],
        error: "Local Spaces on Windows run in Docker Desktop, which isn't installed yet.",
        fix: { action: "install-docker", label: "Install Docker Desktop", note: "About 600 MB.", busy: false, termsUrl: "https://www.docker.com/legal/" },
      }),
    });
    expect(await screen.findByRole("button", { name: "Install Docker Desktop" }, { timeout: 8000 })).toBeTruthy();
    // Done never waits for an extra Space.
    await waitFor(() => expect(primary()!.disabled).toBe(false));
  });
});
