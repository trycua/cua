// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The 0.10.0 first run on its real route with the wasm core: pick any
// agents (or none), then "Your agent can use this ..." with This computer as the first
// Space; the extra Space never blocks Done and asks the host to notify when
// it is ready; a fix left turns Done into Finish later and Home keeps the
// to-do list.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
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

async function mount(opts: { host?: (h: HostStatus) => HostStatus; local?: (l: NonNullable<NewSpaceOptions["local"]>) => NewSpaceOptions["local"]; agents?: (rows: AgentSetupRow[]) => AgentSetupRow[] } = {}) {
  await testCore();
  const calls: Calls = [];
  let configured = false;
  await mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    intercept: (op, args) => {
      calls.push({ op, args });
      return undefined;
    },
    answer: (op, out) => {
      if (op === "host.status" && opts.host) return opts.host(out as HostStatus);
      // The agents as the test sets them up, until Continue adds Cua to them.
      if (op === "agents.configure") configured = true;
      if ((op === "agents.setup" || op === "agents.signIn") && opts.agents && !configured) return opts.agents(out as AgentSetupRow[]);
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
  it("connects the ticked agents, then says they can use this computer, with Done on", async () => {
    // Nothing set up yet: every installed agent is ticked.
    const calls = await mount({ agents: (rows) => rows.map((r) => ({ ...r, configured: false })) });
    await waitFor(() => expect(heading()).toBe("A shared Space for every agent."));
    expect(document.querySelector("[data-first-run-intro]")!.textContent).toBe("Pick any, or none.");
    const go = await waitFor(() => {
      const b = document.querySelector<HTMLButtonElement>("[data-first-run-continue]");
      expect(b?.dataset.firstRunContinue).toBeTruthy();
      return b!;
    });
    const ticked = go.dataset.firstRunContinue!.split(",");
    expect(ticked.length).toBeGreaterThan(1);
    fireEvent.click(go);
    await waitFor(() => expect(calls.find((c) => c.op === "agents.configure")).toBeTruthy());
    expect((calls.find((c) => c.op === "agents.configure")!.args as { agents: string[] }).agents).toEqual(ticked);
    await waitFor(() => expect(heading()).toMatch(/^Your agents can use this (Mac|PC|computer)\.$/), { timeout: 5000 });
    expect(primary()!.disabled).toBe(false);
    expect(primary()!.textContent).toBe("Done");
    expect(document.querySelector('[data-first-run-row="computer"]')!.textContent).toMatch(/is your first Space/);
    // Nothing in the app's own words calls a computer a VM or a container.
    expect(document.body.textContent).not.toMatch(/\bVM\b|virtual machine|container|daemon|MCP/i);
  });

  it("takes none: nothing is configured and step 2 has no agent row or request", async () => {
    const calls = await mount({ agents: (rows) => rows.map((r) => ({ ...r, configured: false })) });
    await waitFor(() => expect(document.querySelectorAll('[data-first-run-pick][data-checked="true"]').length).toBeGreaterThan(0));
    for (const row of document.querySelectorAll<HTMLElement>('[data-first-run-pick][data-checked="true"]')) {
      fireEvent.click(row.querySelector('[data-slot="checkbox"]')!);
    }
    await waitFor(() => expect(document.querySelector('[data-first-run-pick][data-checked="true"]')).toBeNull());
    expect(document.querySelector("[data-first-run-credentials]")).toBeNull();
    fireEvent.click(document.querySelector<HTMLButtonElement>("[data-first-run-continue]")!);
    await waitFor(() => expect(heading()).toMatch(/^This (Mac|PC|computer) is ready\.$|^Checking your setup\.$/), { timeout: 5000 });
    await waitFor(() => expect(heading()).toMatch(/^This (Mac|PC|computer) is ready\.$/), { timeout: 5000 });
    expect(calls.some((c) => c.op === "agents.configure")).toBe(false);
    expect(document.querySelector('[data-first-run-row="agent"]')).toBeNull();
    expect(document.querySelector("[data-first-run-copy]")).toBeNull();
    expect(readFirstRun().picked).toEqual([]);
  });

  it("Codex only: adds Cua to Codex, and its ChatGPT sign-in starts on this computer", async () => {
    const calls = await mount({ agents: (rows) => rows.map((r) => ({ ...r, configured: false })) });
    await waitFor(() => expect(document.querySelector('[data-first-run-pick="codex"][data-checked="true"]')).toBeTruthy());
    for (const row of document.querySelectorAll<HTMLElement>('[data-first-run-pick][data-checked="true"]')) {
      if (row.dataset.firstRunPick !== "codex") fireEvent.click(row.querySelector('[data-slot="checkbox"]')!);
    }
    await waitFor(() => expect(document.querySelector<HTMLElement>("[data-first-run-continue]")!.dataset.firstRunContinue).toBe("codex"));
    // The sign-in Codex uses, and where it stays.
    const signIn = document.querySelector('[data-first-run-credential="sign-in:codex"]')!;
    expect(signIn.textContent).toMatch(/ChatGPT sign-in/);
    expect(signIn.textContent).toMatch(/uses it on this (Mac|PC|computer)/);
    expect(document.querySelector("[data-first-run-credentials-note]")!.textContent).toMatch(/^They stay on this (Mac|PC|computer)\. Other computers keep their own\.$/);
    expect(document.querySelector('[data-first-run-credential="key:anthropic"]')).toBeNull();
    fireEvent.click(signIn.querySelector('[data-first-run-credential-action="agent-sign-in"]')!);
    await waitFor(() => expect(calls.find((c) => c.op === "agents.signIn")?.args).toEqual({ agent: "codex" }));
    await waitFor(() => expect(signIn.querySelector("[data-first-run-credential-status]")?.textContent).toBe("Signed in"), { timeout: 8000 });
    fireEvent.click(document.querySelector<HTMLButtonElement>("[data-first-run-continue]")!);
    await waitFor(() => expect((calls.find((c) => c.op === "agents.configure")?.args as { agents: string[] } | undefined)?.agents).toEqual(["codex"]));
    await waitFor(() => expect(readFirstRun().step).toBe("setup"));
    expect(readFirstRun().picked).toEqual(["codex"]);
  });

  it("says in plain words that usage data is on and linked to the account, and turns it off with one tap", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("A shared Space for every agent."));
    const note = await waitFor(() => {
      const n = document.querySelector("[data-first-run-usage]");
      expect(n).toBeTruthy();
      return n!;
    });
    expect(note.textContent).toMatch(/linked to your account through a random ID/);
    expect(note.textContent).toMatch(/Never your files, what you type or your screen/);
    expect(note.textContent).not.toMatch(/SPACES_CP_LINK_USAGE|telemetry|opaque/i);
    expect(note.textContent).toMatch(/Share usage data to help improve Cua/);
    const sw = note.querySelector<HTMLElement>('[role="switch"]')!;
    expect(sw.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(sw);
    await waitFor(() => expect(calls.some((c) => /telemetry|settings/.test(c.op))).toBe(true));
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
