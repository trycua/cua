// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agent-first first run on its real route with the wasm core: Welcome,
// Pick your agent (the agents found here, most preferred first, one Connect
// each and none connected by itself), Connect your other devices (an
// optional sign-in), then Try it in the lead agent. With no agent, a
// one-click first Space that never blocks Done and asks the host to notify.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
import type { HostStatus } from "@/bridge/contracts/host";
import type { NewSpaceOptions } from "@/bridge/contracts/new-space";
import { readFirstRun, resetFirstRunCache, useFirstRun, writeFirstRun } from "@/bridge/first-run";
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
  await mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    // A first run: signed out, and Cua in no agent's settings yet.
    demo: { signedIn: false, onboarded: false },
    intercept: (op, args) => {
      calls.push({ op, args });
      return undefined;
    },
    answer: (op, out) => {
      if (op === "host.status" && opts.host) return opts.host(out as HostStatus);
      // The agents found here as the test sets them up (Connect still adds Cua to them).
      if ((op === "agents.setup" || op === "agents.signIn" || op === "agents.configure") && opts.agents) return opts.agents(out as AgentSetupRow[]);
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

describe("first-run detection on window focus", () => {
  it("refreshes connected agents without waiting for polling and stops after unmount", async () => {
    writeFirstRun({ step: "setup", agent: "claude-code" });
    const demo = createDemoAdapter({ latencyMs: 0 });
    let reads = 0;
    let latest: AgentSetupRow[] = [];
    const adapter: typeof demo = {
      ...demo,
      call: (async (op, args) => {
        if (op === "agents.setup") {
          reads++;
          return [{ agent: "claude-code", name: "Claude Code", installed: reads < 3, configured: true, detail: "MCP configured", skillsInstalled: 1, skillsTotal: 1, mcpConfig: ".claude.json", skillsDir: ".claude/skills" }];
        }
        return demo.call(op, args);
      }) as typeof demo.call,
    };
    function Probe() {
      latest = useFirstRun(true, 60_000).agents;
      return null;
    }
    const mounted = render(<BridgeProvider adapter={adapter} core={noCore}><Probe /></BridgeProvider>);
    await waitFor(() => expect(latest[0]?.installed).toBe(true));
    const before = reads;
    fireEvent.focus(window);
    await waitFor(() => expect(reads).toBe(before + 1));
    fireEvent.focus(window);
    await waitFor(() => expect(reads).toBe(before + 2));
    await waitFor(() => expect(latest[0]?.installed).toBe(false));
    mounted.unmount();
    const stopped = reads;
    await act(async () => { fireEvent.focus(window); });
    expect(reads).toBe(stopped);
    demo.dispose?.();
  });
});

const agents = () => [...document.querySelectorAll<HTMLElement>("[data-first-run-agent]")].map((e) => e.dataset.firstRunAgent);
const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
/** No agent on this computer. */
const noAgents = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: false, configured: false }));

describe.skipIf(!wasmBuilt)("the first run (real route, wasm core)", () => {
  it("walks Welcome, Pick your agent, Connect your devices and Try it, connecting only what is clicked", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Your agents get their own computer."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Pick your agent."));
    // The agents found here, most preferred first; nothing connected by itself.
    await waitFor(() => expect(agents()).toEqual(["hermes", "claude-code", "codex", "cursor", "openclaw"]));
    expect([...document.querySelectorAll<HTMLElement>("[data-first-run-agent]")].every((e) => e.dataset.connected === "false")).toBe(true);
    expect(calls.some((c) => c.op === "agents.configure")).toBe(false);
    // Each row names the file Connect changes.
    expect(document.querySelector('[data-first-run-agent="hermes"]')!.textContent).toMatch(/~\/\.hermes\/config\.yaml/);
    click('[data-first-run-connect="codex"]');
    await waitFor(() => expect(calls.find((c) => c.op === "agents.configure")?.args).toEqual({ agents: ["codex"] }));
    await waitFor(() => expect(document.querySelector<HTMLElement>('[data-first-run-agent="codex"]')!.dataset.connected).toBe("true"));
    expect(readFirstRun().picked).toEqual(["codex"]);
    expect(calls.filter((c) => c.op === "agents.configure")).toHaveLength(1);
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Connect your other devices."));
    expect(document.querySelector("[data-first-run-devices]")!.textContent).toMatch(/Your other computers.*Your cloud/);
    expect(document.body.textContent).not.toMatch(/phone/i);
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    expect(calls.some((c) => c.op === "session.signIn")).toBe(false);
    const prompt = document.querySelector<HTMLTextAreaElement>("textarea[data-first-run-prompt]")!;
    expect(prompt.readOnly).toBe(true);
    expect(prompt.value).toMatch(/^Make a Linux Space on this (Mac|PC|computer) and stream the desktop in a new window\.$/);
    expect(document.querySelector("[data-first-run-copy]")).toBeTruthy();
    // A Codex user never sees Claude or its key first.
    expect(document.body.textContent).not.toMatch(/Claude|Anthropic/);
    expect(document.body.textContent).not.toMatch(/\bVM\b|virtual machine|container|daemon|MCP|\$|Pro\b/i);
    expect(primary()!.textContent).toBe("Done");
    expect(primary()!.disabled).toBe(false);
    fireEvent.click(primary()!);
    await waitFor(() => expect(calls.find((c) => c.op === "session.completeOnboarding")).toBeTruthy());
  });

  it("Back works from every step after Welcome, all the way to Welcome, keeping what was picked", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Your agents get their own computer."));
    expect(document.querySelector("[data-first-run-back]")).toBeNull();
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Pick your agent."));
    click('[data-first-run-connect="codex"]');
    await waitFor(() => expect(readFirstRun().picked).toEqual(["codex"]));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Connect your other devices."));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    // Try it → Connect your devices → Pick your agent → Welcome.
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Connect your other devices."));
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Pick your agent."));
    expect(document.querySelector<HTMLElement>('[data-first-run-agent="codex"]')!.dataset.connected).toBe("true");
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Your agents get their own computer."));
    // Going back only navigates: nothing was connected again or undone.
    expect(readFirstRun().picked).toEqual(["codex"]);
    expect(calls.filter((c) => c.op === "agents.configure")).toHaveLength(1);
  });

  it("Codex: its ChatGPT sign-in and OpenAI key follow it, and the sign-in starts on this computer", async () => {
    writeFirstRun({ step: "connect" });
    const calls = await mount({ agents: (rows) => rows.map((r) => ({ ...r, installed: r.agent === "codex" })) });
    await waitFor(() => expect(agents()).toEqual(["codex"]));
    expect(document.querySelector("[data-first-run-credentials]")).toBeNull();
    click('[data-first-run-connect="codex"]');
    const signIn = await waitFor(() => {
      const r = document.querySelector('[data-first-run-credential="sign-in:codex"]');
      expect(r).toBeTruthy();
      return r!;
    });
    expect(signIn.textContent).toMatch(/ChatGPT sign-in/);
    expect(signIn.textContent).toMatch(/uses it on this (Mac|PC|computer)/);
    expect(document.querySelector("[data-first-run-credentials-note]")!.textContent).toMatch(/^They stay on this (Mac|PC|computer)\.$/);
    expect(document.querySelector('[data-first-run-credential="key:anthropic"]')).toBeNull();
    fireEvent.click(signIn.querySelector('[data-first-run-credential-action="agent-sign-in"]')!);
    await waitFor(() => expect(calls.find((c) => c.op === "agents.signIn")?.args).toEqual({ agent: "codex" }));
  });

  it("Sign in to connect starts the one sign-in; Not now skips it", async () => {
    writeFirstRun({ step: "devices" });
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Connect your other devices."));
    expect(document.body.textContent).toMatch(/works without it\./);
    click('[data-first-run-devices-primary="sign-in"]');
    await waitFor(() => expect(calls.some((c) => c.op === "session.signIn")).toBe(true));
  });

  it("with no agent: install one or use the CLI, then a one-click first Space", async () => {
    const calls = await mount({ agents: noAgents, local: (l) => ({ ...l, backends: ["docker"], macosImage: null, hostArch: "amd64" }) });
    await waitFor(() => expect(heading()).toBe("Your agents get their own computer."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Connect an agent."));
    expect(agents()).toEqual([]);
    const alts = [...document.querySelectorAll<HTMLElement>("[data-first-run-alternative]")].map((e) => e.dataset.firstRunAlternative);
    expect(alts).toEqual(["install", "cli"]);
    expect(document.querySelector('[data-first-run-alternative="cli"]')!.textContent).toMatch(/cua spaces create linux/);
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Connect your other devices."));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Try a first Space."));
    const linux = await waitFor(() => {
      const b = document.querySelector<HTMLButtonElement>('[data-first-run-extra-choice="linux"]');
      expect(b && !b.disabled).toBe(true);
      return b!;
    }, { timeout: 8000 });
    // An Intel Mac or a PC can't run macOS: it isn't offered.
    expect(document.querySelector('[data-first-run-extra-choice="macos"]')).toBeNull();
    // Nothing starts before the click.
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    fireEvent.click(linux);
    await waitFor(() => expect(calls.find((c) => c.op === "spaces.create")).toBeTruthy());
    const create = calls.find((c) => c.op === "spaces.create")!.args as { config: { kind: string }; notifyWhenReady?: boolean };
    expect(create.notifyWhenReady).toBe(true);
    expect(create.config.kind).toBe("container");
    expect(readFirstRun().extraPendingId).toBeTruthy();
    await waitFor(() => expect(primary()!.disabled).toBe(false));
  });

  it("says in plain words that usage data is on, and turns it off with one tap", async () => {
    writeFirstRun({ step: "connect" });
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Pick your agent."));
    const note = await waitFor(() => {
      const n = document.querySelector("[data-first-run-usage]");
      expect(n).toBeTruthy();
      return n!;
    });
    expect(note.textContent).toMatch(/Share usage data to help improve Cua/);
    expect(note.textContent).not.toMatch(/SPACES_CP_LINK_USAGE|telemetry|opaque/i);
    const sw = note.querySelector<HTMLElement>('[role="switch"]')!;
    expect(sw.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(sw);
    await waitFor(() => expect(calls.some((c) => /telemetry|settings/.test(c.op))).toBe(true));
  });

  it("offers the host's runtime fix when nothing can be made here", async () => {
    writeFirstRun({ step: "setup", setupStartedMs: Date.now() });
    await mount({
      agents: noAgents,
      local: (l) => ({
        ...l,
        available: false,
        backends: [],
        error: "Local Spaces on Windows run in Docker Desktop, which isn't installed yet.",
        fix: { action: "install-docker", label: "Install Docker Desktop", note: "About 600 MB.", busy: false, termsUrl: "https://www.docker.com/legal/" },
      }),
    });
    expect(await screen.findByRole("button", { name: "Install Docker Desktop" }, { timeout: 8000 })).toBeTruthy();
    // Done never waits for a first Space.
    await waitFor(() => expect(primary()!.disabled).toBe(false));
  });

  it("an older saved run resumes on the matching screen", async () => {
    writeFirstRun({ step: "setup", agent: "claude-code", setupStartedMs: Date.now() });
    await mount();
    await waitFor(() => expect(heading()).toBe("Try it in Claude Code."));
  });
});
