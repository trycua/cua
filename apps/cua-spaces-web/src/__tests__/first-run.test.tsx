// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agent-first first run on its real route with the wasm core: Welcome,
// Pick your agent (the agents found here, most preferred first, one Connect
// each and none connected by itself), Add your other computers (an
// optional sign-in), then Try it in the lead agent. With no agent, a
// one-click first Space that never blocks Done and asks the host to notify.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AgentSetupRow, ConnectionReport } from "@/bridge/contracts/agents";
import type { HostStatus } from "@/bridge/contracts/host";
import type { NewSpaceOptions } from "@/bridge/contracts/new-space";
import { readFirstRun, resetFirstRunCache, useFirstRun, writeFirstRun } from "@/bridge/first-run";
import { CLOSED_NEW_SPACE, updateNewSpaceSession } from "@/bridge/new-space";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { connectedAppsState, resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  updateNewSpaceSession(() => CLOSED_NEW_SPACE);
  resetConnectedAppsState();
});
afterEach(cleanup);

type Calls = { op: string; args: unknown }[];

async function mount(opts: { signedIn?: boolean; host?: (h: HostStatus) => HostStatus; local?: (l: NonNullable<NewSpaceOptions["local"]>) => NewSpaceOptions["local"]; agents?: (rows: AgentSetupRow[]) => AgentSetupRow[]; cli?: "missing" | "local"; demo?: object } = {}) {
  await testCore();
  const calls: Calls = [];
  let lastRows: AgentSetupRow[] | null = null;
  await mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    // A first run: signed out, and Cua in no agent's settings yet.
    demo: { signedIn: opts.signedIn ?? false, onboarded: false, cli: opts.cli, ...opts.demo },
    intercept: (op, args) => {
      calls.push({ op, args });
      return undefined;
    },
    answer: (op, out) => {
      if (op === "host.status" && opts.host) return opts.host(out as HostStatus);
      // The agents found here as the test sets them up (Connect still adds Cua to them).
      if ((op === "agents.setup" || op === "agents.signIn" || op === "agents.configure") && opts.agents) {
        lastRows = opts.agents(out as AgentSetupRow[]);
        return lastRows;
      }
      // The connection status agrees with the rows the test set up (it is what says "connected").
      if (op === "agents.status" && opts.agents && lastRows) {
        const rows = lastRows;
        const report = out as ConnectionReport;
        return {
          ...report,
          agents: report.agents.map((a) => {
            const r = rows.find((x) => x.agent === a.id);
            if (!r) return a;
            if (!r.installed) return { ...a, installed: false, state: "not_installed" as const, problem: null };
            return r.configured ? { ...a, installed: true, mcp: "ok" as const, state: "connected" as const, problem: null } : { ...a, installed: true, state: "not_connected" as const };
          }),
        };
      }
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

/** The one question Pick your agent asks (or its answer once every app is ready). */
const ASK = /^Let .+ use Cua\?$|^.+ can use Cua\.$/;
const onlyCodex = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "codex" }));
const agents = () => [...document.querySelectorAll<HTMLElement>("[data-first-run-agent]")].map((e) => e.dataset.firstRunAgent);
const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
/** No agent on this computer. */
const noAgents = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: false, configured: false }));

describe.skipIf(!wasmBuilt)("the first run (real route, wasm core)", () => {
  it("asks once, naming the apps found, and connects nothing until Set up", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Let your 6 coding apps use Cua?"));
    // The apps found here, most preferred first; nothing connected by itself, no per-app buttons or file paths.
    await waitFor(() => expect(agents()).toEqual(["hermes", "claude-code", "codex", "cursor", "openclaw", "vscode"]));
    expect([...document.querySelectorAll<HTMLElement>("[data-first-run-agent]")].every((e) => e.dataset.connected === "false")).toBe(true);
    expect(document.querySelector("[data-first-run-connect]")).toBeNull();
    expect(document.body.textContent).not.toMatch(/\.hermes|config\.yaml|MCP/);
    expect(calls.some((c) => c.op === "agents.configure")).toBe(false);
    // The consent says it covers apps installed later, and how to undo.
    expect(document.querySelector("[data-first-run-consent]")!.textContent).toMatch(/Apps you install later are set up the same way, with a notice and Undo\./);
    expect(connectedAppsState().auto).toBeNull();
    click("[data-first-run-set-up]");
    // The same Connect as Settings, the CLI and MCP: Cua's tools and the app's sign-in, one app at a time.
    await waitFor(() => expect(calls.filter((c) => c.op === "agents.connect").map((c) => (c.args as { agents: string[] }).agents)).toHaveLength(5));
    await waitFor(() => expect(heading()).toBe("Your 6 coding apps can use Cua."));
    expect(document.querySelector("[data-first-run-prompt]")!.textContent).toBe("Reproduce the crash from issue #4821 on a fresh Windows 11 Space and show me a recording.");
    expect(document.body.textContent).toMatch(/restart it first/);
    expect(connectedAppsState().auto).toBe(true);
    expect(connectedAppsState().known).toEqual(expect.arrayContaining(["hermes", "claude-code", "codex", "cursor", "openclaw", "vscode"]));
    expect(readFirstRun().picked).toEqual(expect.arrayContaining(["codex", "claude-code"]));
  });

  it("Not now moves on and sets up nothing, and Cua never sets up later installs on its own", async () => {
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    expect(calls.some((c) => c.op === "agents.configure")).toBe(false);
    expect(connectedAppsState().auto).toBe(false);
  });

  it("says plainly which app needs a hand, with Try again and the text to paste", { timeout: 30_000 }, async () => {
    await mount({ demo: { agentsReadOnly: ["cursor"] }, agents: (rows) => rows.map((r) => ({ ...r, installed: ["codex", "cursor"].includes(r.agent) })) });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Let OpenAI Codex and Cursor use Cua?"));
    click("[data-first-run-set-up]");
    await waitFor(() => expect(heading()).toBe("OpenAI Codex can use Cua. Cursor needs a hand."), { timeout: 8000 });
    const row = document.querySelector<HTMLElement>('[data-first-run-agent="cursor"]')!;
    await waitFor(() => expect(row.textContent).toContain("Couldn't set up Cursor. Cua can't change this file because it is read-only or belongs to another user."), { timeout: 8000 });
    expect(document.body.textContent).not.toMatch(/os error/);
    expect(row.querySelector("[data-first-run-retry]")).toBeTruthy();
    expect(document.querySelector('[data-first-run-agent="codex"]')!.textContent).toContain("Ready");
    fireEvent.click(screen.getByRole("button", { name: "Add it by hand" }));
    const hand = await waitFor(() => {
      const el = document.querySelector("[data-first-run-by-hand]");
      expect(el, document.body.textContent ?? "").not.toBeNull();
      return el!;
    });
    expect(hand.textContent).toContain("mcpServers");
    // Continue is still there: Cursor can wait.
    expect(document.querySelector("[data-onboarding-primary]")!.textContent).toBe("Continue");
  });

  it("walks Welcome, the question, Connect your devices and Try it, connecting only what is asked", async () => {
    const calls = await mount({ agents: onlyCodex });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Let OpenAI Codex use Cua?"));
    click("[data-first-run-set-up]");
    await waitFor(() => expect(calls.find((c) => c.op === "agents.connect")?.args).toEqual({ agents: ["codex"], signIn: true }));
    await waitFor(() => expect(heading()).toBe("OpenAI Codex can use Cua."));
    expect(readFirstRun().picked).toEqual(["codex"]);
    expect(calls.filter((c) => c.op === "agents.connect")).toHaveLength(1);
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    expect(document.querySelector("[data-first-run-devices]")!.textContent).toMatch(/Your other computers.*Your cloud/);
    expect(document.body.textContent).not.toMatch(/phone/i);
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    expect(calls.some((c) => c.op === "session.signIn")).toBe(false);
    const prompt = document.querySelector<HTMLTextAreaElement>("textarea[data-first-run-prompt]")!;
    expect(prompt.readOnly).toBe(true);
    expect(prompt.value).toMatch(/^Make a Linux Space on this (Mac|PC|computer) and show its desktop in a new window\.$/);
    expect(document.querySelector("[data-first-run-copy]")).toBeTruthy();
    // A Codex user never sees Claude or its key first.
    expect(document.body.textContent).not.toMatch(/Claude|Anthropic/);
    expect(document.body.textContent).not.toMatch(/\bVM\b|virtual machine|container|daemon|MCP|\$|Pro\b/i);
    // The last button names the choice until the agent used Cua (CUA-2267).
    expect(primary()!.textContent).toBe("Skip, I'll try it later");
    expect(primary()!.disabled).toBe(false);
    fireEvent.click(primary()!);
    await waitFor(() => expect(calls.find((c) => c.op === "session.completeOnboarding")).toBeTruthy());
  });

  it("counts each screen viewed and finished, a skipped one, and Skip for now, as fixed words only", async () => {
    const calls = await mount();
    const funnel = () =>
      calls
        .filter((c) => c.op === "telemetry.track")
        .flatMap((c) => (c.args as { signals: { type: string; page: string; action: string; choice: string }[] }).signals)
        .filter((s) => s.type === "onboarding-page" && s.action.startsWith("step_"))
        .map((s) => `${s.page} ${s.action}${s.choice === "none" ? "" : ` ${s.choice}`}`);
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(document.querySelector("[data-step]")?.getAttribute("data-step")).toBe("setup"));
    click("[data-onboarding-later]");
    await new Promise((r) => setTimeout(r, 300));
    await waitFor(() =>
      expect(funnel()).toEqual([
        "welcome step_viewed",
        "welcome step_completed",
        "connect_agent step_viewed",
        "connect_agent step_skipped step",
        "sign_in step_viewed",
        "sign_in step_skipped step",
        "create_space step_viewed",
        "create_space step_skipped later",
      ]),
    );
  });

  it("sends no funnel event while usage data is off", async () => {
    const calls: Calls = [];
    await testCore();
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false },
      intercept: (op, args) => {
        calls.push({ op, args });
        return undefined;
      },
      answer: (op, out) => (op === "settings.get" ? { ...(out as object), telemetry: { ...(out as { telemetry: object }).telemetry, enabled: false } } : out),
    });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.some((c) => c.op === "telemetry.track")).toBe(false);
  });

  it("Back works from every step after Welcome, all the way to Welcome, keeping what was picked", async () => {
    const calls = await mount({ agents: onlyCodex });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    expect(document.querySelector("[data-first-run-back]")).toBeNull();
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    click("[data-first-run-set-up]");
    await waitFor(() => expect(readFirstRun().picked).toEqual(["codex"]));
    await waitFor(() => expect(heading()).toBe("OpenAI Codex can use Cua."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    // Try it → Connect your devices → Pick your agent → Welcome.
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    expect(document.querySelector<HTMLElement>('[data-first-run-agent="codex"]')!.dataset.connected).toBe("true");
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    // Going back only navigates: nothing was connected again or undone.
    expect(readFirstRun().picked).toEqual(["codex"]);
    expect(calls.filter((c) => c.op === "agents.connect")).toHaveLength(1);
  });

  it("Back sits above the heading, apart from the choices; Tab goes primary, secondary, Back; Esc goes back", async () => {
    writeFirstRun({ step: "devices" });
    await mount();
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    const step = document.querySelector<HTMLElement>('[data-first-run="devices"]')!;
    const back = step.querySelector<HTMLElement>("[data-first-run-back]")!;
    // Drawn first (above the heading), but last in the page's order.
    expect(back.className).toContain("order-first");
    const order = Array.from(step.querySelectorAll<HTMLElement>("[data-onboarding-primary], [data-first-run-secondary], [data-first-run-back]"));
    expect(order.map((el) => (el.matches("[data-onboarding-primary]") ? "primary" : el.matches("[data-first-run-back]") ? "back" : "secondary"))).toEqual(["primary", "secondary", "back"]);
    // "Not now" is a button of its own, described by the note under it.
    const secondary = step.querySelector<HTMLElement>("[data-first-run-secondary]")!;
    expect(screen.getByRole("button", { name: "Not now" })).toBe(secondary);
    expect(document.getElementById(secondary.getAttribute("aria-describedby")!)?.textContent).toBe("Not now keeps Cua on this Mac only. You can sign in later.");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(heading()).toMatch(ASK));
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    // Welcome has no Back, so Esc stays.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(heading()).toBe("Your computers, shared by your team and its agents.");
  });

  it("shows no sign-ins or keys: those live in Settings, Connectors", async () => {
    writeFirstRun({ step: "connect" });
    await mount({ agents: (rows) => rows.map((r) => ({ ...r, installed: r.agent === "codex", configured: r.agent === "codex" })) });
    await waitFor(() => expect(heading()).toBe("OpenAI Codex can use Cua."));
    expect(document.querySelector("[data-first-run-credentials]")).toBeNull();
    expect(document.body.textContent).not.toMatch(/ChatGPT sign-in|API key|Sign-ins and keys/);
  });

  it("Sign in starts the one sign-in; Not now skips it", async () => {
    writeFirstRun({ step: "devices" });
    const calls = await mount();
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    expect(document.body.textContent).toMatch(/You can sign in later\./);
    click('[data-first-run-devices-primary="sign-in"]');
    await waitFor(() => expect(calls.some((c) => c.op === "session.signIn")).toBe(true));
  });

  it("a disk-image install offers the Cua CLI after Pick your agent: the exact file, then Install only when pressed", async () => {
    const calls = await mount({ cli: "missing" });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    // The install plan is read: the run has the CLI's screen.
    await waitFor(() => expect(document.querySelector("ol[aria-label]")!.getAttribute("aria-label")).toBe("Step 2 of 5"));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Use Cua from the Terminal."));
    const facts = [...document.querySelectorAll<HTMLElement>("[data-first-run-cli-fact]")].map((e) => e.textContent);
    expect(facts).toEqual(["Installs~/.local/bin/cua", "Adds to PATH~/.local/bin, in ~/.zshrc"]);
    expect(document.querySelector("[data-step]")!.getAttribute("data-step")).toBe("cli");
    expect(document.querySelector("ol[aria-label]")!.getAttribute("aria-label")).toBe("Step 3 of 5");
    // Nothing is written before Install.
    expect(calls.some((c) => c.op === "cli.install")).toBe(false);
    click('[data-first-run-cli-primary="install"]');
    await waitFor(() => expect(heading()).toBe("Cua is ready in Terminal."));
    expect(calls.find((c) => c.op === "cli.install")?.args).toEqual({ modifyPath: true });
    expect(document.querySelector("[data-first-run-cli-done]")!.textContent).toMatch(/cua spaces ls/);
    await waitFor(() => expect(document.querySelector('[data-first-run-cli-primary="continue"]')!.textContent).toBe("Continue"));
    click('[data-first-run-cli-primary="continue"]');
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    // Back shows the result again.
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Cua is ready in Terminal."));
  });

  it("Not now leaves the CLI as it is; with the CLI already installed there is no such screen", async () => {
    const calls = await mount({ cli: "missing" });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    // The install plan is read: the run has the CLI's screen.
    await waitFor(() => expect(document.querySelector("ol[aria-label]")!.getAttribute("aria-label")).toBe("Step 2 of 5"));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Use Cua from the Terminal."));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
    expect(calls.some((c) => c.op === "cli.install")).toBe(false);
    cleanup();
    localStorage.clear();
    resetSavedOnboardingCache();
    resetFirstRunCache();
    await mount();
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toMatch(ASK));
    click("[data-first-run-secondary]");
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
  });

  it("a signed-in Mac that shares its screen asks for two permissions in its own words, and Allow opens the guide", async () => {
    const SR = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
    const AX = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
    let missing = [
      { id: "screen-recording", label: "Screen Recording", settingsUrl: SR },
      { id: "accessibility", label: "Accessibility", settingsUrl: AX },
    ];
    writeFirstRun({ step: "devices", picked: ["codex"] });
    const calls = await mount({
      signedIn: true,
      host: (h) => ({ ...h, configured: true, shareDesktop: true, appName: "Cua Spaces Local", permissions: missing }),
    });
    await waitFor(() => expect(heading()).toBe("This Mac is in your account."));
    // The check row adds to the headline instead of repeating it, and no jargon.
    const body = document.body.textContent ?? "";
    expect(body.match(/in your account/g)?.length).toBe(1); // the headline only
    expect(body).toMatch(/Ready to share/);
    expect(body).not.toMatch(/\bpool\b/i);
    // "Skip for now" is a visible ghost pill (border, readable text, hover), not a faint link.
    const skip = document.querySelector<HTMLElement>("[data-onboarding-later]")!;
    expect(skip.className).toMatch(/\brounded-full\b/);
    expect(skip.className).toMatch(/\bborder\b/);
    expect(skip.className).toMatch(/text-foreground\/90/);
    expect(skip.className).toMatch(/hover:bg-foreground\/10/);
    expect(skip.className).not.toMatch(/text-muted-foreground/);
    click('[data-first-run-devices-primary="continue"]');
    await waitFor(() => expect(heading()).toBe("Share this Mac's screen."));
    expect(document.body.textContent).toMatch(/Allow two permissions so your team and agents can see and use this Mac\./);
    expect(document.body.textContent).toMatch(/turn on Cua Spaces Local\./);
    // Nothing is asked before a click.
    expect(calls.some((c) => c.op === "host.openSettings")).toBe(false);
    const rows = () => [...document.querySelectorAll<HTMLElement>("[data-first-run-permissions] [data-first-run-status]")].map((e) => `${e.dataset.firstRunStatus}:${e.dataset.state}`);
    await waitFor(() => expect(rows()).toEqual(["screen-recording:fix", "accessibility:fix"]));
    click('[data-first-run-status="screen-recording"] [data-first-run-action="allow"]');
    await waitFor(() => expect(calls.find((c) => c.op === "host.openSettings")?.args).toEqual({ url: SR }));
    // Granted in System Settings: a check, live; Continue once both are.
    missing = [];
    fireEvent.focus(window);
    await waitFor(() => expect(rows()).toEqual(["screen-recording:ok", "accessibility:ok"]));
    click('[data-first-run-permissions-primary="continue"]');
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    click("[data-first-run-back]");
    await waitFor(() => expect(heading()).toBe("Share this Mac's screen."));
  });

  it("a Mac that runs macOS Spaces also asks for Local Network on the permissions screen, and checks it there", async () => {
    const LN = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
    let granted = false;
    writeFirstRun({ step: "permissions", picked: ["codex"] });
    const calls = await mount({
      signedIn: true,
      host: (h) => ({ ...h, configured: true, shareDesktop: true, permissions: [{ id: "local-network", label: "Local Network", settingsUrl: LN, granted }] }),
    });
    await waitFor(() => expect(heading()).toBe("Share this Mac's screen."));
    expect(document.body.textContent).toMatch(/Allow three permissions so your team and agents can see and use this Mac, and so macOS Spaces can run on it\./);
    const rows = () => [...document.querySelectorAll<HTMLElement>("[data-first-run-permissions] [data-first-run-status]")].map((e) => `${e.dataset.firstRunStatus}:${e.dataset.state}`);
    await waitFor(() => expect(rows()).toEqual(["screen-recording:ok", "accessibility:ok", "local-network:fix"]));
    // The page asks the host to check it (the daemon's own connect is what makes macOS ask).
    expect(calls.some((c) => c.op === "host.status" && (c.args as { localNetwork?: boolean } | undefined)?.localNetwork === true)).toBe(true);
    click('[data-first-run-status="local-network"] [data-first-run-action="allow"]');
    await waitFor(() => expect(calls.find((c) => c.op === "host.openSettings")?.args).toEqual({ url: LN }));
    // Allowed on this Mac: a check, live.
    granted = true;
    fireEvent.focus(window);
    await waitFor(() => expect(rows()).toEqual(["screen-recording:ok", "accessibility:ok", "local-network:ok"]));
  });

  it("Not now on the permissions screen goes on; a Mac kept for Spaces only never sees it", async () => {
    writeFirstRun({ step: "permissions", picked: ["codex"] });
    let share = true;
    const calls = await mount({
      signedIn: true,
      host: (h) => ({ ...h, configured: true, shareDesktop: share, permissions: [{ id: "accessibility", label: "Accessibility" }] }),
    });
    await waitFor(() => expect(heading()).toBe("Share this Mac's screen."));
    click('[data-first-run-permissions-secondary="not-now"]');
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
    expect(calls.some((c) => c.op === "host.openSettings")).toBe(false);
    share = false;
    writeFirstRun({ step: "permissions" });
    fireEvent.focus(window);
    await waitFor(() => expect(heading()).toBe("Try it in OpenAI Codex."));
  });

  it("with no agent: install one or use the CLI, then a one-click first Space", async () => {
    const calls = await mount({ agents: noAgents, local: (l) => ({ ...l, backends: ["docker"], macosImage: null, hostArch: "amd64" }) });
    await waitFor(() => expect(heading()).toBe("Your computers, shared by your team and its agents."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("We didn\u2019t find a coding app yet."));
    expect(agents()).toEqual([]);
    const missing = [...document.querySelectorAll<HTMLElement>("[data-first-run-missing]")].map((e) => e.dataset.firstRunMissing);
    expect(missing).toEqual(["claude-code", "codex", "cursor"]);
    expect(screen.getAllByRole("button", { name: /Download/ })).toHaveLength(3);
    expect(document.body.textContent).toMatch(/Spaces work without a coding app\./);
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continue without one" }));
    await waitFor(() => expect(heading()).toBe("Add your other computers."));
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
    await waitFor(() => expect(heading()).toMatch(ASK));
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
