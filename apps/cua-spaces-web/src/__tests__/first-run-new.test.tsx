// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new first run (CUA-3318), phase 1: each screen's words and choices in
// every state the storyboard draws (signed out, signed in, an offline computer, no coding agent, a Mac
// that can't make a separate computer, a sign-in that didn't finish), then the whole run on its real
// route with the wasm core.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { WizardEnv } from "@/bridge/contracts/new-space";
import { firstSpaceOffers } from "@/bridge/new-space";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
import { readFirstRun, resetFirstRunCache, writeFirstRun as writeFirstRunForTest } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { AgentScreen, PermissionsScreen, ReadyScreen, SignInScreen, StepDots, WelcomeScreen } from "@/components/first-run-new/first-run-new";
import { BringTheRestCard, CliPanel } from "@/components/ask-first/bring-the-rest";
import { placeKeys } from "@/components/first-run-new/float-keys";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import type { AppPermissions } from "@/bridge/app-permissions";
import { permissionsStepView } from "@/lib/app-permissions";
import {
  addingView,
  afterSignIn,
  agentView,
  bringRestView,
  chosenAgents,
  dotFor,
  dotStep,
  firstJobDone,
  installedWhere,
  NEW_RUN_SCREENS,
  reachedAfter,
  readyComputers,
  readyView,
  signInView,
  stepDots,
  toggleAgentOff,
  welcomeUsage,
} from "@/lib/first-run-new";
import { addAskTask, resetAskTasks } from "@/lib/ask-first";
import { getDriver, resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const machines = [
  { id: "here", name: "Francesco's MacBook", os: "macos", online: true, current: true },
  { id: "mini", name: "Mac mini", os: "macos", online: true, current: false },
  { id: "pc", name: "Gaming PC", os: "windows", online: false, current: false },
  { id: "build", name: "Build server", os: "linux", online: true, current: false },
  { id: "lent", name: "Lent laptop", os: "macos", online: true, current: false, notSharing: "Its owner stopped sharing it." },
  { id: "phone", name: "iPhone", os: "ios", online: true, current: false, device: true },
];
const noop = () => {};
const text = () => document.body.textContent ?? "";

describe("no step to pick where agents work (Francesco, 8 Nov 2026)", () => {
  it("is not among the setup's screens: welcome, sign-in, then the permissions on a Mac or the agents", () => {
    expect(NEW_RUN_SCREENS).toEqual(["welcome", "signin", "permissions", "agent", "ready"]);
    expect(afterSignIn("macos")).toBe("permissions");
    expect(afterSignIn("windows")).toBe("agent");
    expect(afterSignIn("linux")).toBe("agent");
  });

  it("agents can use this computer and every ready computer in the account, never offline, not-sharing or device ones", () => {
    expect(readyComputers(machines)).toEqual(["this", "Mac mini", "Build server"]);
    expect(readyComputers(null)).toEqual(["this"]);
    expect(readyComputers([machines[0]!])).toEqual(["this"]);
  });
});

describe("1b, adding this computer once signed in", () => {
  it("says this Mac is being added, with nothing to press", () => {
    render(<SignInScreen view={addingView({ os: "macos", state: "adding", error: null })} onTryAgain={noop} onUseCode={noop} onOpen={noop} />);
    expect(screen.getByRole("heading").textContent).toBe("Adding this Mac to your account");
    expect(screen.getByRole("status").textContent).toBe("Adding this Mac");
    expect(screen.queryAllByRole("button")).toEqual([]);
  });

  it("when adding failed, says why with Try again", () => {
    const onTryAgain = vi.fn();
    render(
      <SignInScreen
        view={addingView({ os: "macos", state: "failed", error: "The relay didn't answer." })}
        onTryAgain={onTryAgain}
        onUseCode={noop}
        onOpen={noop}
      />,
    );
    expect(screen.getByRole("heading").textContent).toBe("Couldn't add this Mac to your account");
    expect(text()).toContain("The relay didn't answer.");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onTryAgain).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Use a code from another computer" })).toBeNull();
  });

  it("when the account wants proof, asks to confirm it's you with a code from another computer", () => {
    const onUseCode = vi.fn();
    const message =
      "To add this Mac to your account, enter a code from a computer you already added. On that computer, open Cua Spaces, go to Computers and click Add a computer.";
    render(
      <SignInScreen
        view={addingView({ os: "macos", state: "failed", error: message, needsProof: true })}
        onTryAgain={noop}
        onUseCode={onUseCode}
        onOpen={noop}
      />,
    );
    expect(text()).toContain(`Confirm it's you. ${message}`);
    fireEvent.click(screen.getByRole("button", { name: "Use a code from another computer" }));
    expect(onUseCode).toHaveBeenCalled();
  });
});

describe.skipIf(!wasmBuilt)("what this computer can make, per system (the core's offers the setup reads)", () => {
  const vol = { availableBytes: 500e9, totalBytes: 1000e9, name: "disk" };
  const storage = { reserveBytes: 5e9, lume: vol, qemu: vol, container: vol, pulled: [] };
  const offers = async (os: "macos" | "windows" | "linux", hostArch: string, backends: string[]) => {
    const env = { defaultLocation: "local", localAvailable: backends.length > 0, localBackends: backends, maxCpus: 8, hostArch, storage } as WizardEnv;
    return firstSpaceOffers(await testCore(), env, os).map((o) => [o.id, o.blocked]);
  };

  it("a Mac with Apple silicon: Linux, and macOS", async () => {
    expect(await offers("macos", "arm64", ["lume", "managed"])).toEqual([
      ["linux", null],
      ["macos", null],
    ]);
  });

  it("an Intel Mac: Linux only, never macOS", async () => {
    expect(await offers("macos", "amd64", ["lume", "managed"])).toEqual([["linux", null]]);
  });

  it("a PC: never macOS", async () => {
    for (const backends of [["docker"], ["qemu", "docker"]]) expect((await offers("windows", "amd64", backends)).map(([id]) => id)).not.toContain("macos");
  });

  it("a Linux computer: never macOS", async () => {
    expect((await offers("linux", "amd64", ["docker", "qemu"])).map(([id]) => id)).not.toContain("macos");
    expect(await offers("linux", "arm64", ["docker"])).toEqual([["linux", null]]);
  });

  it("no runtime ready: Linux is offered blocked, so setup leaves the separate computer out", async () => {
    const [[id, blocked]] = (await offers("macos", "arm64", [])) as [[string, string | null]];
    expect(id).toBe("linux");
    expect(blocked).toBeTruthy();
    // Ready's line to try then never asks for one.
    expect(readyView({ os: "macos", agent: "codex", computers: ["this"], canMakeSeparate: !blocked }).command).not.toContain("separate");
  });
});

describe("1b, sign-in", () => {
  it("waits for the browser with Try again, and Use a code instead after a while", () => {
    vi.useFakeTimers();
    try {
      const v = signInView({ kind: "waiting" }, null);
      render(<SignInScreen view={v} onTryAgain={noop} onUseCode={noop} onOpen={noop} />);
      expect(screen.getByRole("heading").textContent).toBe("Finish signing in in your browser");
      expect(text()).toContain("Create your account or sign in there. This window moves on when you're done.");
      expect(screen.getByRole("status").textContent).toBe("Waiting for your browser");
      expect(screen.getByRole("button", { name: "Try again" })).not.toBeNull();
      expect(screen.queryByRole("button", { name: "Use a code instead" })).toBeNull();
      act(() => void vi.advanceTimersByTime(12_000));
      expect(screen.getByRole("button", { name: "Use a code instead" })).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("says 'Sign-in didn't finish' with its reason, and Try again becomes the white button", () => {
    const v = signInView({ kind: "failed", reason: "notConfirmed", message: "x" }, null);
    expect(v).toMatchObject({
      title: "Sign-in didn't finish",
      line: "The browser didn't confirm in time. Try again, or use a code if your browser is on another device.",
      tryAgainPrimary: true,
      waiting: null,
    });
    // Other reasons keep the core's own line.
    expect(signInView({ kind: "failed", reason: "offline", message: "You're offline." }, null).line).toBe("You're offline.");
  });

  it("a session that could not be saved says so on 1b, with Try again and Use a code instead", () => {
    const msg = "Sign-in didn't finish. Cua couldn't save your session on this computer.";
    const v = signInView({ kind: "failed", reason: "couldNotSave", message: msg }, null);
    render(<SignInScreen view={v} onTryAgain={noop} onUseCode={noop} onOpen={noop} />);
    expect(screen.getByRole("heading").textContent).toBe("Sign-in didn't finish");
    expect(text()).toContain(msg);
    expect(screen.getByRole("button", { name: "Try again" })).not.toBeNull();
    expect(screen.getByRole("button", { name: "Use a code instead" })).not.toBeNull();
  });

  it("with a code shows the code and the page to enter it on", () => {
    const onOpen = vi.fn();
    render(
      <SignInScreen
        view={signInView({ kind: "waiting", userCode: "WXYZ-1234" }, "https://cua.ai/device")}
        onTryAgain={noop}
        onUseCode={noop}
        onOpen={onOpen}
      />,
    );
    expect(document.querySelector("[data-new-run-code]")!.textContent).toContain("WXYZ-1234");
    fireEvent.click(screen.getByRole("button", { name: "Open the sign-in page" }));
    expect(onOpen).toHaveBeenCalledWith("https://cua.ai/device");
    expect(screen.queryByRole("button", { name: "Use a code instead" })).toBeNull();
  });
});

describe("screen 3, the coding agents", () => {
  const draw = (found: string[] | null, off: string[] = []) => {
    const onInstall = vi.fn();
    const onPick = vi.fn();
    const onContinue = vi.fn();
    const onLater = vi.fn();
    render(
      <AgentScreen
        view={agentView({ os: "macos", found, off, error: null })}
        onPick={onPick}
        onContinue={onContinue}
        onLater={onLater}
        onInstall={onInstall}
        busy={false}
      />,
    );
    return { onInstall, onPick, onContinue, onLater };
  };

  it("one found: asks which agents you use, says where it is installed, ticked, and Continue works", () => {
    draw(["claude-code", "cursor"]);
    expect(screen.getByRole("heading").textContent).toBe("Which agents do you use?");
    expect(text()).toContain("Cua has no AI of its own. Claude Code does the work, on your own plan.");
    expect(document.querySelector("[data-new-run-agent=claude-code] [data-new-run-agent-where]")!.textContent).toBe("On this Mac");
    expect(document.querySelector("[data-new-run-agent=claude-code] [data-check=on]")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
    expect(chosenAgents(["claude-code"])).toEqual(["claude-code"]);
  });

  it("several found: all ticked, nothing to pick before Continue, and a quiet Choose later", () => {
    const { onPick, onContinue, onLater } = draw(["codex", "claude-code", "hermes"]);
    expect(screen.getByRole("heading").textContent).toBe("Which agents do you use?");
    expect(text()).toContain("Cua has no AI of its own. Claude Code, Codex and Hermes do the work, each on your own plan.");
    expect(document.querySelector("[data-new-run-agent-note]")!.textContent).toBe(
      "You choose the agent each time you start a job. You can change this later in Settings.",
    );
    expect(text()).not.toContain("Pick the coding agent");
    expect(document.querySelectorAll("[data-check=on]").length).toBe(3);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onContinue).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Choose later" }));
    expect(onLater).toHaveBeenCalled();
    fireEvent.click(document.querySelector("[data-new-run-agent=codex]")!);
    expect(onPick).toHaveBeenCalledWith("codex");
    // Connected in the app's order, leaving out the unticked ones.
    expect(chosenAgents(["hermes", "codex", "claude-code"])).toEqual(["claude-code", "codex", "hermes"]);
    expect(chosenAgents(["hermes", "codex", "claude-code"], ["codex"])).toEqual(["claude-code", "hermes"]);
    expect(toggleAgentOff(["codex"], "codex")).toEqual([]);
    expect(toggleAgentOff([], "codex")).toEqual(["codex"]);
  });

  it("says every computer an agent is installed on, this one first", () => {
    expect(installedWhere("macos")).toBe("On this Mac");
    expect(installedWhere("macos", ["Francesco's MacBook Pro"])).toBe("On this Mac and Francesco's MacBook Pro");
    expect(installedWhere("windows", ["Mac mini", "Build server"])).toBe("On this PC, Mac mini and Build server");
    const v = agentView({ os: "macos", found: ["claude-code", "codex"], elsewhere: { codex: ["Mac mini"] }, error: null });
    expect(v.agents.map((a) => [a.id, a.where])).toEqual([
      ["claude-code", "On this Mac"],
      ["codex", "On this Mac and Mac mini"],
    ]);
  });

  it("an unticked agent stays unticked; Continue still works with none ticked", () => {
    draw(["claude-code", "codex"], ["claude-code", "codex"]);
    expect(document.querySelectorAll("[data-check=on]").length).toBe(0);
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
  });

  it("none found: Install Claude Code, or Install Codex instead (the official pages), or Install later", () => {
    const { onInstall, onLater } = draw([]);
    expect(screen.getByRole("heading").textContent).toBe("Install a coding agent");
    expect(text()).toContain("Cua has no AI of its own. We didn't find a coding agent on this Mac. Install Claude Code, then sign in to it once.");
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Install Claude Code" }));
    fireEvent.click(screen.getByRole("button", { name: "Install Codex instead" }));
    expect(onInstall.mock.calls).toEqual([["claude-code"], ["codex"]]);
    fireEvent.click(screen.getByRole("button", { name: "Install later" }));
    expect(onLater).toHaveBeenCalled();
  });
});

describe("screen 4, Ready", () => {
  it("names the agent and the ready computers, with a line to try and Copy", () => {
    const v = readyView({ os: "macos", agent: "claude-code", computers: ["Mac mini", "Build server"] });
    render(<ReadyScreen view={v} agent="claude-code" onDone={noop} />);
    expect(screen.getByRole("heading").textContent).toBe("Claude Code can now use Mac mini and Build server");
    expect(text()).toContain("Try it from your own terminal. Nothing runs until you do.");
    expect(document.querySelector("[data-new-run-command]")!.textContent).toContain('claude "Build this branch on Mac mini"');
    expect(screen.getByRole("button", { name: "Copy" })).not.toBeNull();
    // Phase 1: no "Pick up where you left off".
    expect(text()).not.toContain("Pick up where you left off");
  });

  it("reports Copy, Continue in Cua and Done, and nothing else", () => {
    const v = readyView({ os: "macos", agent: "claude-code", computers: ["this"] });
    const onCopied = vi.fn();
    const onDone = vi.fn();
    render(<ReadyScreen view={v} agent="claude-code" onDone={onDone} onCopied={onCopied} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(onCopied).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: v.done }));
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("names every agent connected, and the line to try uses the first", () => {
    expect(readyView({ os: "macos", agent: "claude-code", agents: ["claude-code", "codex", "hermes"], computers: ["Mac mini"] })).toMatchObject({
      title: "Claude Code, Codex and Hermes can now use Mac mini",
      command: 'claude "Build this branch on Mac mini"',
    });
  });

  it("names up to four computers, then counts the others", () => {
    expect(readyView({ os: "macos", agent: "codex", computers: ["this", "Mac mini", "Build server", "Gaming PC"] }).title).toBe(
      "Codex can now use this Mac, Mac mini, Build server and Gaming PC",
    );
    expect(readyView({ os: "macos", agent: "codex", computers: ["this", "A", "B", "C", "D", "E"] })).toMatchObject({
      title: "Codex can now use this Mac and 5 other computers",
      command: 'codex "Build this branch on A"',
    });
  });

  it("with only this Mac ready, says this Mac", () => {
    expect(readyView({ os: "macos", agent: "codex", computers: ["this"] })).toMatchObject({
      title: "Codex can now use this Mac",
      command: 'codex "Make a separate Linux computer on this Mac and run this repo\'s tests in it"',
    });
  });

  it("never suggests a separate computer where this one can't make it", () => {
    for (const os of ["macos", "windows", "linux"]) {
      const v = readyView({ os, agent: "codex", computers: ["this"], canMakeSeparate: false });
      expect(v.command).not.toContain("separate");
      expect(v.command).toContain("Run this repo's tests on this");
    }
    // Still checking: the usual line.
    expect(readyView({ os: "macos", agent: "codex", computers: ["this"], canMakeSeparate: null }).command).toContain("separate Linux computer");
  });
});

describe("2b, Cua Spaces' permissions (a Mac only)", () => {
  const SCREEN = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
  const AX = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
  const data = (screen: boolean, accessibility: boolean, appName = "Cua Spaces"): AppPermissions => ({
    appName,
    permissions: [
      { id: "screen", granted: screen, settingsUrl: SCREEN },
      { id: "accessibility", granted: accessibility, settingsUrl: AX },
    ],
  });
  const draw = (d: AppPermissions, allow = vi.fn(async () => {}), onContinue = vi.fn()) => {
    const allGranted = d.permissions.every((p) => p.granted);
    render(<PermissionsScreen view={permissionsStepView({ appName: d.appName, allGranted })} perms={{ data: d, allow }} onContinue={onContinue} />);
    return { allow, onContinue };
  };

  it("comes right after the sign-in, on a Mac only, under the first dot", () => {
    expect(afterSignIn("macos")).toBe("permissions");
    expect(afterSignIn("windows")).toBe("agent");
    expect(dotFor("permissions")).toBe(dotFor("signin"));
  });

  it("asks for both for Cua Spaces itself, says Cua Driver uses the same ones, and offers Later", () => {
    const { allow, onContinue } = draw(data(false, false));
    expect(screen.getByRole("heading").textContent).toBe("Let Cua Spaces see and use apps");
    expect(text()).toContain("Agents can then use the apps on this Mac. Your own pointer never moves.");
    expect(text()).not.toContain("Cua Driver uses these same permissions");
    const rows = [...document.querySelectorAll<HTMLElement>("[data-app-permission]")];
    expect(rows.map((r) => r.dataset.appPermission)).toEqual(["screen", "accessibility"]);
    expect(rows[0]!.textContent).toContain("See the screen");
    expect(rows[0]!.textContent).toContain("Screen Recording, given to Cua Spaces");
    expect(rows[1]!.textContent).toContain("Use the mouse and keyboard");
    expect(rows[1]!.textContent).toContain("Accessibility, given to Cua Spaces");
    fireEvent.click(rows[1]!.querySelector("button")!);
    expect(allow).toHaveBeenCalledWith("accessibility");
    // Never blocks: Later goes on.
    const later = screen.getByRole("button", { name: "Later" });
    fireEvent.click(later);
    expect(onContinue).toHaveBeenCalled();
  });

  it("shows a grant as On, and Continue once both are on", () => {
    draw(data(true, false));
    expect(document.querySelector("[data-app-permission=screen]")!.textContent).toContain("On");
    expect(document.querySelector("[data-app-permission=screen] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("on");
    expect(document.querySelector("[data-app-permission=screen] button")).toBeNull();
    expect(screen.getByRole("button", { name: "Later" })).toBeTruthy();
    cleanup();
    draw(data(true, true));
    expect(screen.queryByRole("button", { name: "Allow" })).toBeNull();
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();
    expect(text()).toContain("Both are on.");
  });

  it("names Cua Spaces Local in the Local build, which has its own grants", () => {
    draw(data(false, false, "Cua Spaces Local"));
    expect(screen.getByRole("heading").textContent).toBe("Let Cua Spaces Local see and use apps");
    expect(document.querySelector("[data-app-permission=screen]")!.textContent).toContain("Screen Recording, given to Cua Spaces Local");
    expect(text()).toContain("Turn on Cua Spaces Local there, then come back.");
  });
});

describe("after the first job, on Home: the rest of setup", () => {
  it("is not one of setup's screens: Ready is the last", () => {
    expect(NEW_RUN_SCREENS.at(-1)).toBe("ready");
    expect(NEW_RUN_SCREENS).not.toContain("next");
  });

  it("offers to invite a teammate, add another computer and let agents use apps on this Mac, and Not now", () => {
    const onRow = vi.fn();
    const onDismiss = vi.fn();
    render(<BringTheRestCard view={bringRestView({ os: "macos" })} onRow={onRow} onDismiss={onDismiss} />);
    expect(screen.getByRole("heading").textContent).toBe("Now bring the rest in");
    expect(text()).toContain("Your first job is done.");
    const rows = [...document.querySelectorAll<HTMLElement>("[data-bring-rest-row]")].map((r) => r.textContent);
    expect(rows[0]).toContain("Invite a teammate");
    expect(rows[1]).toContain("Add another computer");
    expect(rows[2]).toContain("Let agents use apps on this Mac");
    fireEvent.click(document.querySelector("[data-bring-rest-row=apps]")!);
    expect(onRow).toHaveBeenCalledWith("apps");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("has no apps row away from a Mac, or on a Mac with no permissions to give", () => {
    expect(bringRestView({ os: "windows" }).rows.map((r) => r.id)).toEqual(["invite", "add-computer"]);
    expect(bringRestView({ os: "macos", apps: false }).rows.map((r) => r.id)).toEqual(["invite", "add-computer"]);
  });

  it("adds Use Cua from Terminal as the last row, only where this app can install the command", () => {
    expect(bringRestView({ os: "macos", cli: true }).rows.map((r) => r.id)).toEqual(["invite", "add-computer", "apps", "cli"]);
    expect(bringRestView({ os: "linux", cli: true }).rows.map((r) => r.id)).toEqual(["invite", "add-computer", "cli"]);
    expect(bringRestView({ os: "windows", cli: false }).rows.map((r) => r.id)).toEqual(["invite", "add-computer"]);
    const row = bringRestView({ os: "windows", cli: true }).rows.at(-1)!;
    expect(row.title).toBe("Use Cua from Terminal");
    for (const word of ["CLI", "PATH", "binary", "shell"]) expect(`${row.title} ${row.sub}`).not.toContain(word);
  });

  it("opens the Terminal row in place: Install where the command is missing, the line to try once it's there", () => {
    const onRow = vi.fn();
    const onInstall = vi.fn();
    const missing = {
      status: "Installs the cua command in ~/.local/bin.",
      installed: false,
      busy: false,
      tryCommand: null,
      error: null,
      primary: { id: "install", label: "Install" },
    };
    const { rerender } = render(
      <BringTheRestCard
        view={bringRestView({ os: "linux", cli: true })}
        open="cli"
        onRow={onRow}
        onDismiss={noop}
        cli={<CliPanel view={missing} onInstall={onInstall} />}
      />,
    );
    fireEvent.click(document.querySelector("[data-bring-rest-row=cli]")!);
    expect(onRow).toHaveBeenCalledWith("cli");
    expect(document.querySelector("[data-bring-rest-cli]")!.getAttribute("data-bring-rest-cli")).toBe("missing");
    fireEvent.click(document.querySelector("[data-bring-rest-cli-install]")!);
    expect(onInstall).toHaveBeenCalledTimes(1);
    const done = { ...missing, status: "Installed in ~/.local/bin.", installed: true, tryCommand: "cua spaces list", primary: null };
    rerender(
      <BringTheRestCard
        view={bringRestView({ os: "linux", cli: true })}
        open="cli"
        onRow={onRow}
        onDismiss={noop}
        cli={<CliPanel view={done} onInstall={onInstall} />}
      />,
    );
    expect(document.querySelector("[data-bring-rest-cli-install]")).toBeNull();
    expect(document.querySelector("[data-bring-rest-cli-try]")!.textContent).toBe("cua spaces list");
  });

  it("waits for a job that finished: a done Home task, or a run since setup ended that finished its turn", () => {
    const since = 1_000;
    expect(firstJobDone({ tasks: [], runs: [] }, since)).toBe(false);
    expect(
      firstJobDone(
        {
          tasks: [
            { state: "working", ending: null },
            { state: "starting", ending: null },
          ],
          runs: [],
        },
        since,
      ),
    ).toBe(false);
    expect(firstJobDone({ tasks: [{ state: "failed", ending: null }], runs: [] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [{ state: "done", ending: null }], runs: [] }, since)).toBe(true);
    expect(firstJobDone({ tasks: [{ state: null, ending: "thrown" }], runs: [] }, since)).toBe(true);
    // Runs from before setup ended don't count; a run still going or one that failed doesn't either.
    expect(firstJobDone({ tasks: [], runs: [{ startedMs: 500, status: "idle" }] }, since)).toBe(false);
    expect(
      firstJobDone(
        {
          tasks: [],
          runs: [
            { startedMs: 2_000, status: "running" },
            { startedMs: 2_000, status: "failed" },
          ],
        },
        since,
      ),
    ).toBe(false);
    expect(firstJobDone({ tasks: [], runs: [{ startedMs: 2_000, status: "idle" }] }, since)).toBe(true);
  });
});

describe("screen 1, usage data", () => {
  const core = {
    notice: "Features you use and errors, linked to your account. Never your files, typing or screen.",
    noticeLinkLabel: "What is collected",
    noticeLinkUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
    usage: { label: "Share usage data to help improve Cua", on: true, enabled: true, help: null },
  };

  it("says what is collected, with a one-tap off and the link, on the first screen", () => {
    const onUsage = vi.fn();
    const onLink = vi.fn();
    render(<WelcomeScreen onAdd={noop} usage={welcomeUsage(core)} onUsage={onUsage} onUsageLink={onLink} />);
    const line = document.querySelector<HTMLElement>("[data-new-run-usage]")!;
    expect(line.getAttribute("data-new-run-usage")).toBe("on");
    expect(line.textContent).toContain("Share usage data to help improve Cua");
    expect(line.textContent).toContain("Never your files, typing or screen.");
    const sw = screen.getByRole("switch", { name: "Share usage data to help improve Cua" });
    fireEvent.click(sw);
    expect(onUsage).toHaveBeenCalledWith(false);
    fireEvent.click(document.querySelector("[data-new-run-usage-link]")!);
    expect(onLink).toHaveBeenCalledTimes(1);
  });

  it("can't be changed when the environment decides, and says who", () => {
    render(<WelcomeScreen onAdd={noop} usage={welcomeUsage({ ...core, usage: { ...core.usage, on: false, enabled: false, help: "Set by DO_NOT_TRACK" } })} />);
    const sw = screen.getByRole("switch", { name: "Share usage data to help improve Cua" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    expect(sw.hasAttribute("disabled") || sw.getAttribute("aria-disabled") === "true" || sw.hasAttribute("data-disabled")).toBe(true);
    expect(document.body.textContent).toContain("Set by DO_NOT_TRACK");
  });

  it("is left out when the core has no notice to show", () => {
    expect(welcomeUsage(null)).toBeNull();
    expect(welcomeUsage({ ...core, notice: null })).toBeNull();
    render(<WelcomeScreen onAdd={noop} usage={null} />);
    expect(document.querySelector("[data-new-run-usage]")).toBeNull();
  });
});

describe("the step dots", () => {
  const S1 = `Step 1 of 2: ${"Combine all your computers into one supercomputer."}`;
  const S2 = "Step 2 of 2: Which agents do you use?";
  it("name each step and go back to any earlier one, but forward only to a step already reached", () => {
    const perms = stepDots("permissions", 0)!;
    expect(perms.map((d) => d.label)).toEqual([S1, S2]);
    expect(perms.map((d) => d.to)).toEqual(["welcome", null]);
    expect(perms.map((d) => d.current)).toEqual([true, false]);
    // Back on screen 1 after reaching the agents: the later step opens again.
    expect(stepDots("welcome", 1)!.map((d) => d.to)).toEqual([null, "agent"]);
    // On the sign-in, the first dot goes back to its start; nothing past it opens (a dot never skips the sign-in).
    expect(stepDots("signin", 0)!.map((d) => d.to)).toEqual(["welcome", null]);
    expect(stepDots("ready", 1)).toBeNull();
  });

  it("count Ready and Next steps as past the last dot, and never go back on how far a run got", () => {
    expect(reachedAfter(0, "signin")).toBe(0);
    expect(reachedAfter(0, "permissions")).toBe(0);
    expect(reachedAfter(0, "agent")).toBe(1);
    expect(reachedAfter(0, "ready")).toBe(1);
    expect(reachedAfter(1, "welcome")).toBe(1);
    // A run saved on the removed step counts as the first.
    expect(dotFor("where")).toBe(0);
  });

  it("move with the arrow keys to the nearest step that opens", () => {
    const dots = stepDots("agent", 1)!;
    expect(dotStep(dots, 1, -1)).toBe(0);
    expect(dotStep(dots, 1, 1)).toBeNull();
    expect(dotStep(stepDots("welcome", 0)!, 0, 1)).toBeNull();
  });

  it("are buttons with their step's name: a click or an arrow key opens a step, a step not reached yet can't be pressed", () => {
    const onGo = vi.fn();
    render(<StepDots dots={stepDots("signin", 0)!} onGo={onGo} />);
    const first = screen.getByRole("button", { name: S1 });
    const second = screen.getByRole("button", { name: S2 });
    expect(first.getAttribute("aria-current")).toBe("step");
    expect(second.hasAttribute("disabled")).toBe(true);
    fireEvent.click(second);
    expect(onGo).not.toHaveBeenCalled();
    fireEvent.click(first);
    expect(onGo).toHaveBeenLastCalledWith(0, false);
    cleanup();
    onGo.mockClear();
    render(<StepDots dots={stepDots("agent", 1)!} onGo={onGo} />);
    fireEvent.keyDown(screen.getByRole("button", { name: S2 }), { key: "ArrowLeft" });
    expect(onGo).toHaveBeenLastCalledWith(0, true);
    onGo.mockClear();
    fireEvent.keyDown(screen.getByRole("button", { name: S2 }), { key: "ArrowRight" });
    expect(onGo).not.toHaveBeenCalled();
  });
});

describe("the keys", () => {
  it("put the Apple, Windows and Linux marks on the product keys and keep the keys right of the copy", () => {
    const keys = placeKeys(1366, 982, 544);
    expect(keys.filter((k) => k.mark).map((k) => [k.src, k.mark])).toEqual([
      ["product-1", "windows"],
      ["product-2", "macos"],
      ["product-3", "linux"],
    ]);
    // Only the soft near key (bottom left, behind the dots) sits left of the copy's edge.
    expect(keys.filter((k) => !k.soft && k.left + k.width * 0.1 < 544).map((k) => k.src)).toEqual([]);
  });
});

/* ---- The real route, with the wasm core ------------------------------------------------- */

beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  resetFirstRunCache();
  resetConnectedAppsState();
  resetAskTasks();
  resetPrepare();
});
afterEach(() => {
  resetPrepare();
});

const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";
const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
const onlyCodex = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "codex" }));
type Calls = { op: string; args: unknown }[];

describe.skipIf(!wasmBuilt)("the new first run on its route", () => {
  it("signed out: Skip for now, this Mac only, the agent found here, Ready, then Home; nothing is made meanwhile", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    const { router } = await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
      answer: (op, out) => (op === "agents.setup" || op === "agents.configure" ? onlyCodex(out as AgentSetupRow[]) : out),
    });
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    expect(document.querySelector("[data-float-keys]")).not.toBeNull();
    // Skip for now goes straight on: there is no step to pick where agents work.
    click("[data-onboarding-later]");
    // On a Mac, Cua Spaces' permissions next: both missing here, so Later goes on without them.
    await waitFor(() => expect(heading()).toBe("Let Cua Spaces see and use apps"), { timeout: 15_000 });
    await waitFor(() => expect(document.querySelectorAll("[data-app-permission]").length).toBe(2));
    expect(calls.some((c) => c.op === "driver.permissions")).toBe(true);
    click("[data-new-run-permissions-next=Later]");
    await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
    expect(document.querySelector("[data-new-run-agent=codex] [data-new-run-agent-where]")!.textContent).toBe("On this Mac");
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
    // Only Cua's tools were waited for; the sign-in is kept after Ready shows, and the chats listed are Codex's,
    // asked for while Codex was being connected.
    const connects = () => calls.filter((c) => c.op === "agents.connect").map((c) => c.args);
    expect(connects()[0]).toEqual({ agents: ["codex"], signIn: false });
    await waitFor(() => expect(connects()).toContainEqual({ agents: ["codex"], signIn: true }));
    const listed = calls.findIndex((c) => c.op === "agents.recentThreads");
    expect(calls[listed]!.args).toEqual({ limit: 4, agents: ["openai-codex"] });
    expect(listed).toBeLessThan(calls.findIndex((c) => c.op === "agents.connect"));
    await waitFor(() => expect(document.querySelectorAll("[data-pick-up-row]").length).toBeGreaterThan(0));
    expect([...document.querySelectorAll<HTMLElement>("[data-pick-up-row]")].every((r) => r.dataset.pickUpRow === "openai-codex")).toBe(true);
    // No Space was made while the first run showed.
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
    // Ready is the last screen: Done ends setup on Home, ready for the first job.
    click("[data-onboarding-primary]");
    await waitFor(() => expect(calls.find((c) => c.op === "session.completeOnboarding")).toBeTruthy());
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    // The rest is asked for once the first job is done, not now.
    expect(readFirstRun().restOffer).toBe("waiting");
    expect(document.querySelector("[data-bring-rest]")).toBeNull();
  });

  it("several agents found: Continue connects all of them at once, with nothing to pick", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    const twoFound = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "codex" || r.agent === "claude-code" }));
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
      answer: (op, out) => (op === "agents.setup" || op === "agents.configure" ? twoFound(out as AgentSetupRow[]) : out),
    });
    writeFirstRunForTest({ newRun: { screen: "agent", reached: 1 } });
    await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Claude Code and Codex can now use this Mac"), { timeout: 15_000 });
    const connect = calls.find((c) => c.op === "agents.connect" || c.op === "agents.configure");
    expect((connect!.args as { agents: string[] }).agents).toEqual(["claude-code", "codex"]);
    // Both agents' chats are read while connecting; the sign-ins are kept after Ready shows.
    const connects = () => calls.filter((c) => c.op === "agents.connect").map((c) => c.args);
    expect(connects()[0]).toEqual({ agents: ["claude-code", "codex"], signIn: false });
    await waitFor(() => expect(connects()).toContainEqual({ agents: ["claude-code", "codex"], signIn: true }));
    expect(calls.find((c) => c.op === "agents.recentThreads")!.args).toEqual({ limit: 4, agents: ["claude-code", "openai-codex"] });
  });

  it("Choose later: setup ends on Home and no agent's settings are touched", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    const { router } = await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    writeFirstRunForTest({ newRun: { screen: "agent", reached: 1 } });
    await waitFor(() => expect(document.querySelector("[data-new-run-agent-later]")).not.toBeNull(), { timeout: 15_000 });
    click("[data-new-run-agent-later]");
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"), { timeout: 15_000 });
    expect(calls.some((c) => c.op === "agents.connect" || c.op === "agents.configure")).toBe(false);
    expect(calls.some((c) => c.op === "session.completeOnboarding")).toBe(true);
  });

  it("a run saved on the old last screen finishes on Home", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ newRun: { screen: "next", reached: 3 } });
    const { router } = await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"), { timeout: 15_000 });
    expect(readFirstRun().restOffer).toBe("waiting");
  });

  it("screen 1 shows the usage line from the core; turning it off is saved before the run moves on", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    const line = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-new-run-usage]");
      if (!el) throw new Error("no usage line");
      return el;
    });
    expect(line.textContent).toContain("Share usage data to help improve Cua");
    expect(line.textContent).toContain("What is collected");
    fireEvent.click(screen.getByRole("switch", { name: "Share usage data to help improve Cua" }));
    await waitFor(() => expect(calls.find((c) => c.op === "settings.set")?.args).toEqual({ key: "telemetry", value: false }));
    // Still on screen 1: nothing moved on.
    expect(heading()).toBe("Combine all your computers into one supercomputer.");
  });

  it("Home asks for the rest only once the first job is done, and Not now puts it away for good", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ restOffer: "waiting", restSinceMs: Date.now() });
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: true } });
    await waitFor(() => expect(document.querySelector("[data-ask-first-home]:not([aria-busy])")).not.toBeNull(), { timeout: 15_000 });
    await new Promise((r) => setTimeout(r, 300));
    // The demo's agent runs all started before setup ended: no job yet.
    expect(document.querySelector("[data-bring-rest]")).toBeNull();
    // A Home task finishes.
    addAskTask({ prompt: "Run the tests", agent: "claude-code", os: "linux", ending: "kept", endedMs: Date.now() });
    await waitFor(() => expect(document.querySelector("[data-bring-rest]")).not.toBeNull(), { timeout: 15_000 });
    expect(readFirstRun().restOffer).toBe("shown");
    expect(document.querySelector("#bring-rest-heading")!.textContent).toBe("Now bring the rest in");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(document.querySelector("[data-bring-rest]")).toBeNull());
    expect(readFirstRun().restOffer).toBe("dismissed");
  });

  it("Home has no Terminal row when the command is already on this computer", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ restOffer: "shown" });
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: true } });
    await waitFor(() => expect(document.querySelector("[data-bring-rest]")).not.toBeNull(), { timeout: 15_000 });
    await new Promise((r) => setTimeout(r, 300));
    expect(document.querySelector("[data-bring-rest-row=cli]")).toBeNull();
  });

  it("Home offers Use Cua from Terminal where the app carries the command, and Install installs it", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    writeFirstRunForTest({ restOffer: "shown" });
    await mountApp("/home", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: true, onboarded: true, cli: "missing" },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    const row = await waitFor(
      () => {
        const el = document.querySelector<HTMLElement>("[data-bring-rest-row=cli]");
        if (!el) throw new Error("no Terminal row");
        return el;
      },
      { timeout: 15_000 },
    );
    expect(row.textContent).toContain("Use Cua from Terminal");
    fireEvent.click(row);
    const install = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-bring-rest-cli-install]");
      if (!el) throw new Error("no Install");
      return el;
    });
    fireEvent.click(install);
    await waitFor(() => expect(calls.some((c) => c.op === "cli.install")).toBe(true));
    await waitFor(() => expect(document.querySelector("[data-bring-rest-cli]")?.getAttribute("data-bring-rest-cli")).toBe("installed"), { timeout: 15_000 });
  });

  // CUA-3551, Mac test 7 Nov (run 2, A2): Accessibility off, quit, relaunch, and Home showed no apps row at all,
  // because the row lived only inside "Now bring the rest in", which waits for the first job.
  const SCREEN_URL = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
  const AX_URL = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
  const axOff = (axOn: () => boolean) => (op: string) =>
    op === "driver.permissions"
      ? {
          appName: "Cua Spaces Local",
          permissions: [
            { id: "screen", granted: true, state: "on", settingsUrl: SCREEN_URL },
            { id: "accessibility", granted: axOn(), state: axOn() ? "on" : "off", settingsUrl: AX_URL },
          ],
        }
      : undefined;
  const status = (id: string) =>
    document.querySelector(`[data-home-apps] [data-app-permission=${id}] [data-permission-status]`)?.getAttribute("data-permission-status") ?? null;

  for (const offer of ["waiting", "dismissed", null] as const) {
    it(`Home shows the apps row at launch while a permission is missing, each switch live (offer ${offer ?? "none"})`, { timeout: 60_000 }, async () => {
      let on = false;
      writeFirstRunForTest({ restOffer: offer, restSinceMs: Date.now() });
      await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: true }, intercept: axOff(() => on) });
      await waitFor(() => expect(document.querySelector("[data-home-apps]")).not.toBeNull(), { timeout: 15_000 });
      expect(document.querySelector("#home-apps-heading")!.textContent).toBe("Let agents use apps on this Mac");
      expect(document.querySelector("[data-home-apps]")!.textContent).toContain("One switch in System Settings is still off.");
      expect(status("screen")).toBe("on");
      expect(status("accessibility")).toBe("off");
      expect(document.querySelector("[data-bring-rest]")).toBeNull();
      // The switch turns on in System Settings: the row says On in place, without a click.
      on = true;
      window.dispatchEvent(new Event("focus"));
      await waitFor(() => expect(status("accessibility")).toBe("on"), { timeout: 15_000 });
      expect(document.querySelector("[data-home-apps-done]")!.textContent).toContain("On");
    });
  }

  it("Home's apps row stays away while both permissions are on", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ restOffer: "waiting", restSinceMs: Date.now() });
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: true }, intercept: axOff(() => true) });
    await waitFor(() => expect(document.querySelector("[data-ask-first-home]:not([aria-busy])")).not.toBeNull(), { timeout: 15_000 });
    await new Promise((r) => setTimeout(r, 300));
    expect(document.querySelector("[data-home-apps]")).toBeNull();
  });

  it("with 'Now bring the rest in' showing, its apps row opens by itself while a permission is missing", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ restOffer: "shown" });
    await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: true }, intercept: axOff(() => false) });
    await waitFor(() => expect(document.querySelector("[data-bring-rest-apps]")).not.toBeNull(), { timeout: 15_000 });
    expect(document.querySelector("[data-bring-rest-row=apps]")!.getAttribute("aria-expanded")).toBe("true");
    expect(
      document.querySelector("[data-bring-rest-apps] [data-app-permission=accessibility] [data-permission-status]")!.getAttribute("data-permission-status"),
    ).toBe("off");
    // One card, not two.
    expect(document.querySelector("[data-home-apps]")).toBeNull();
  });

  it(
    "signed in: Add this computer opens the sign-in, adds this Mac there, then goes straight on; Ready names every ready computer",
    { timeout: 60_000 },
    async () => {
      const calls: Calls = [];
      await mountApp("/onboarding", {
        failsCreates: false,
        listsGhost: false,
        demo: { signedIn: false, onboarded: false, noSpaces: true },
        intercept: (op, args) => (calls.push({ op, args }), undefined),
        answer: (op, out) => (op === "agents.setup" || op === "agents.configure" ? onlyCodex(out as AgentSetupRow[]) : out),
      });
      await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
      click("[data-onboarding-primary]");
      await waitFor(() => expect(heading()).toBe("Finish signing in in your browser"));
      expect(calls.some((c) => c.op === "session.signIn")).toBe(true);
      // No "Pick where your agents work": on a Mac, the permissions come next.
      await waitFor(() => expect(heading()).toBe("Let Cua Spaces see and use apps"), { timeout: 15_000 });
      expect(document.body.textContent).not.toContain("Pick where your agents work");
      click("[data-new-run-permissions-next=Later]");
      await waitFor(() => expect(heading()).toBe("Which agents do you use?"), { timeout: 15_000 });
      click("[data-onboarding-primary]");
      await waitFor(() => expect(document.querySelector("[data-new-run=ready]")).not.toBeNull(), { timeout: 15_000 });
      // The demo account's ready computers are all named (or counted), not only this Mac.
      await waitFor(() => expect(heading()).not.toBe("Codex can now use this Mac"), { timeout: 15_000 });
      expect(heading()).toMatch(/^Codex can now use this Mac(, | and )/);
    },
  );

  it("Add this computer waits for the browser: nothing is set up or registered while it waits", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true, signIn: "wait" },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Finish signing in in your browser"));
    expect(screen.getByRole("status").textContent).toContain("Waiting for your browser");
    await new Promise((r) => setTimeout(r, 500));
    // The browser hasn't confirmed: this computer isn't set up, registered or asked about, and the run stays on 1b.
    expect(calls.some((c) => c.op === "host.setUp")).toBe(false);
    expect(heading()).toBe("Finish signing in in your browser");
    expect(readFirstRun().newRun?.screen).toBe("signin");
  });

  it("an app restarted mid-sign-in starts the sign-in again and sets nothing up until it finishes", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ newRun: { screen: "signin", reached: 0 } });
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true, signIn: "wait" },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Finish signing in in your browser"), { timeout: 15_000 });
    await waitFor(() => expect(calls.some((c) => c.op === "session.signIn")).toBe(true));
    await new Promise((r) => setTimeout(r, 500));
    expect(calls.some((c) => c.op === "host.setUp")).toBe(false);
  });

  it("already signed in (Reset & Show): Add this computer opens no second browser sign-in, adds this Mac, then goes on", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: true, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Let Cua Spaces see and use apps"), { timeout: 15_000 });
    // A browser sign-in started here would finish later, after the run had already gone on (8 Nov, Francesco's test).
    expect(calls.some((c) => c.op === "session.signIn")).toBe(false);
    expect(calls.some((c) => c.op === "host.status")).toBe(true);
  });

  it("a run saved on the removed step goes on from the step after the sign-in", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ newRun: { screen: "where", reached: 1 } });
    await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(document.querySelector("[data-new-run=permissions], [data-new-run=agent]")).not.toBeNull(), { timeout: 15_000 });
    expect(readFirstRun().newRun?.screen).not.toBe("where");
  });

  it("Skip for now on the agents leaves setup for Home, as finishing does", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ newRun: { screen: "agent", reached: 1 } });
    const { router } = await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(document.querySelector("[data-new-run=agent]")).not.toBeNull(), { timeout: 15_000 });
    click("[data-onboarding-later]");
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    expect(readFirstRun().finishedLater).toBe(true);
  });

  it("the step dots go back to screen 1 and forward again to the agents, but never past the sign-in", { timeout: 60_000 }, async () => {
    const S1 = "Step 1 of 2: Combine all your computers into one supercomputer.";
    const S2 = "Step 2 of 2: Which agents do you use?";
    await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: false, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    // The agents aren't reached yet: their dot can't be pressed, so it can't skip the sign-in.
    expect(screen.getByRole("button", { name: S2 }).hasAttribute("disabled")).toBe(true);
    writeFirstRunForTest({ newRun: { screen: "agent", reached: 1 } });
    await waitFor(() => expect(document.querySelector("[data-new-run=agent]")).not.toBeNull(), { timeout: 15_000 });
    fireEvent.click(screen.getByRole("button", { name: S1 }));
    await waitFor(() => expect(heading()).toBe("Combine all your computers into one supercomputer."));
    fireEvent.click(screen.getByRole("button", { name: S2 }));
    await waitFor(() => expect(document.querySelector("[data-new-run=agent]")).not.toBeNull());
  });
});
