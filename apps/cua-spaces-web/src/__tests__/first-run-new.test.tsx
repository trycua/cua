// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new first run (CUA-3318), phase 1: each screen's words and choices in
// every state the storyboard draws (signed out, signed in, an offline computer, no coding agent, a Mac
// that can't make a separate computer, a sign-in that didn't finish), then the whole run on its real
// route with the wasm core.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { WizardEnv } from "@/bridge/contracts/new-space";
import { firstSpaceOffers } from "@/bridge/new-space";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
import { readFirstRun, resetFirstRunCache, writeFirstRun as writeFirstRunForTest } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { AgentScreen, PermissionsScreen, ReadyScreen, SignInScreen, StepDots, WelcomeScreen, WhereScreen } from "@/components/first-run-new/first-run-new";
import { BringTheRestCard, CliPanel } from "@/components/ask-first/bring-the-rest";
import { placeKeys } from "@/components/first-run-new/float-keys";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import type { AppPermissions } from "@/bridge/app-permissions";
import { permissionsStepView } from "@/lib/app-permissions";
import { afterWhere, agentView, bringRestView, chosenAgent, dotFor, dotStep, firstJobDone, NEW_RUN_SCREENS, reachedAfter, readyView, signInView, stepDots, togglePick, welcomeUsage, whereView, type WhereInput } from "@/lib/first-run-new";
import { addAskTask, resetAskTasks } from "@/lib/ask-first";
import { getDriver, resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const NOW = 1_800_000_000_000;
const base: WhereInput = {
  os: "macos",
  signedIn: true,
  thisState: "added",
  addedInRun: true,
  addError: null,
  model: "MacBook Pro",
  machines: [
    { id: "here", name: "Francesco's MacBook", os: "macos", online: true, current: true },
    { id: "mini", name: "Mac mini", os: "macos", online: true, current: false },
    { id: "pc", name: "Gaming PC", os: "windows", online: false, lastSeen: NOW / 1000 - 2 * 3600, current: false },
    { id: "build", name: "Build server", os: "linux", online: true, current: false },
    { id: "phone", name: "iPhone", os: "ios", online: true, current: false, device: true },
  ],
  picks: ["this"],
  canMakeSeparate: true,
  separateBlocked: null,
  separate: { phase: "none", name: null, error: null },
  now: NOW,
};
const noop = () => {};
const drawWhere = (input: WhereInput, handlers: Partial<Parameters<typeof WhereScreen>[0]> = {}) =>
  render(<WhereScreen view={whereView(input)} onToggle={noop} onAddThis={noop} onSignIn={noop} onAddAnother={noop} onSeparate={noop} onContinue={noop} {...handlers} />);
const text = () => document.body.textContent ?? "";

describe("screen 2, signed in", () => {
  it("says this Mac was added just now, lists the account's other computers and Add another computer you own", () => {
    drawWhere(base);
    expect(screen.getByRole("heading").textContent).toBe("Pick where your agents work");
    expect(text()).toContain("You're signed in, and this Mac is now in your account. Your agents can start here, or on your other computers while you keep working on this one.");
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("Added to your account just now");
    expect(document.querySelector("[data-new-run-computer=this]")!.textContent).toContain("MacBook Pro");
    expect(text()).toContain("Other computers in your account");
    // This computer and devices that are not computers are not in the list.
    const ids = [...document.querySelectorAll<HTMLElement>("[data-new-run-computer]")].map((e) => e.dataset.newRunComputer);
    expect(ids).toEqual(["this", "mini", "pc", "build"]);
    expect(document.querySelector("[data-new-run-computer=mini]")!.textContent).toContain("macOS · ready");
    expect(text()).toContain("Add another computer you own");
    expect(text()).toContain("Install Cua Spaces on it and sign in. It shows up here.");
    expect(text()).toContain("Or make a separate computer on this Mac");
    expect(text()).toContain("A Linux computer just for your agents, away from your files. About 2 minutes.");
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
  });

  it("with no other computers says so", () => {
    drawWhere({ ...base, machines: [base.machines![0]!] });
    expect(document.querySelector("[data-new-run-none]")!.textContent).toBe("None yet. Computers you or your team sign in on show up here.");
  });

  it("a computer already in the account says so, not 'just now'", () => {
    drawWhere({ ...base, addedInRun: false });
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("In your account");
  });

  it("when adding this Mac failed, says so with Try again", () => {
    const onAddThis = vi.fn();
    drawWhere({ ...base, thisState: "failed", addError: "The relay didn't answer." }, { onAddThis });
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("Couldn't add it to your account. The relay didn't answer.");
    fireEvent.click(document.querySelector("[data-new-run-add-this=retry-add]")!);
    expect(onAddThis).toHaveBeenCalledWith("retry-add");
  });

  it("when the account wants proof, asks to confirm it's you with a code from another computer", () => {
    const onAddThis = vi.fn();
    const message = "To add this Mac to your account, enter a code from a computer you already added. On that computer, open Cua Spaces, go to Computers and click Add a computer.";
    drawWhere({ ...base, thisState: "failed", addError: message, addNeedsProof: true }, { onAddThis });
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe(`Confirm it's you. ${message}`);
    expect(document.querySelector("[data-new-run-add-this=retry-add]")).toBeNull();
    const button = document.querySelector<HTMLElement>("[data-new-run-add-this=use-code]")!;
    expect(button.textContent).toBe("Use a code from another computer");
    fireEvent.click(button);
    expect(onAddThis).toHaveBeenCalledWith("use-code");
  });
});

describe("screen 2, an offline computer", () => {
  it("is greyed with 'Offline, last seen 2 hours ago', has no check circle and can't be picked", () => {
    const onToggle = vi.fn();
    drawWhere(base, { onToggle });
    const pc = document.querySelector<HTMLElement>("[data-new-run-computer=pc]")!;
    expect(pc.textContent).toContain("Offline, last seen 2 hours ago");
    expect(pc.getAttribute("aria-disabled")).toBe("true");
    expect(pc.className).toContain("opacity-40");
    expect(pc.querySelector("[data-check]")).toBeNull();
    fireEvent.click(pc);
    expect(onToggle).not.toHaveBeenCalled();
    // The online ones can be picked.
    fireEvent.click(document.querySelector("[data-new-run-computer=mini]")!);
    expect(onToggle).toHaveBeenCalledWith("mini");
  });

  it("is never added to the picks, even when asked", () => {
    const rows = whereView(base).others!;
    expect(togglePick(["this"], "pc", rows)).toEqual(["this"]);
    expect(togglePick(["this"], "mini", rows)).toEqual(["this", "mini"]);
    expect(togglePick(["this", "mini"], "mini", rows)).toEqual(["this"]);
  });
});

describe("screen 2, a long list", () => {
  const many = (n: number) => Array.from({ length: n }, (_, i) => ({ id: `off-${i}`, name: `Old box ${i + 1}`, os: "linux", online: false, lastSeen: NOW / 1000 - 86400, current: false }));
  const long = { ...base, machines: [...base.machines!, ...many(13)] };

  it("folds offline computers into one row with their count, so the online ones and Continue stay in view", () => {
    drawWhere(long);
    const fold = document.querySelector<HTMLElement>("[data-new-run-offline-fold]")!;
    // Gaming PC and the 13 old boxes.
    expect(fold.textContent).toContain("14 offline");
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    const ids = () => [...document.querySelectorAll<HTMLElement>("[data-new-run-computer]")].map((e) => e.dataset.newRunComputer);
    expect(ids()).toEqual(["this", "mini", "build"]);
    fireEvent.click(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    expect(ids()).toHaveLength(3 + 14);
    expect(document.querySelector("[data-new-run-computer=pc]")!.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(fold);
    expect(ids()).toEqual(["this", "mini", "build"]);
  });

  it("counts whatever is offline", () => {
    expect(whereView({ ...base, machines: [...base.machines!, ...many(2)] }).offlineFold?.label).toBe("3 offline");
    // One offline computer shows as itself.
    expect(whereView(base).offlineFold).toBeNull();
    expect(whereView({ ...base, machines: base.machines!.map((m) => ({ ...m, online: true })) }).offlineFold).toBeNull();
  });

  it("keeps Continue in a footer that sticks to the bottom of the window", () => {
    drawWhere(long);
    const footer = document.querySelector<HTMLElement>("[data-new-run-footer]")!;
    expect(footer.className).toContain("sticky");
    expect(footer.className).toContain("bottom-0");
    expect(footer.contains(screen.getByRole("button", { name: "Continue" }))).toBe(true);
    // No background of its own (5 Nov: it drew a black box over the stars and cut the keys); the list fades instead.
    expect(footer.className).not.toMatch(/\bbg-|from-\[/);
    expect(footer.contains(document.querySelector("[data-new-run-content]"))).toBe(false);
  });
});

describe("screen 2, signed out (after Skip for now)", () => {
  it("shows this Mac not added yet with Add, 'Sign in to see your other computers', and no Add another computer", () => {
    const onAddThis = vi.fn();
    const onSignIn = vi.fn();
    drawWhere({ ...base, signedIn: false, thisState: "signed-out" }, { onAddThis, onSignIn });
    expect(text()).toContain("Your agents can work on this Mac without an account. Add this Mac to an account when you want them on your other computers too.");
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("not added yet");
    expect(document.querySelector("[data-new-run-sign-in-line]")!.textContent).toBe("Sign in to see your other computers.");
    expect(text()).not.toContain("Other computers in your account");
    expect(text()).not.toContain("Add another computer you own");
    expect(text()).not.toContain("Mac mini");
    // The separate computer needs no account: still offered.
    expect(document.querySelector("[data-new-run-separate-open]")).not.toBeNull();
    fireEvent.click(document.querySelector("[data-new-run-add-this=add]")!);
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(onAddThis).toHaveBeenCalled();
    expect(onSignIn).toHaveBeenCalled();
  });
});

describe("screen 2, this Mac can't make a separate computer", () => {
  it("leaves the option out when the host says so before anyone asks (offer only what it can run)", () => {
    for (const os of ["macos", "windows", "linux"]) {
      expect(whereView({ ...base, os, canMakeSeparate: false, separateBlocked: "Needs Apple silicon." }).separate).toBeNull();
      expect(whereView({ ...base, os, canMakeSeparate: true }).separate?.state).toBe("offer");
    }
    drawWhere({ ...base, canMakeSeparate: false, separateBlocked: "Needs Apple silicon." });
    expect(document.querySelector("[data-new-run-separate]")).toBeNull();
    expect(document.body.textContent).not.toContain("Or make a separate computer");
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
  });

  it("once a create was refused, greys only that option, with one plain sentence; this Mac and the other computers still work", () => {
    drawWhere({ ...base, canMakeSeparate: false, separateRefused: true, separateBlocked: "Needs Apple silicon." });
    const sep = document.querySelector<HTMLElement>("[data-new-run-separate]")!;
    expect(sep.dataset.disabled).toBe("");
    expect(sep.textContent).toContain("This Mac can't make one. Older Intel Macs can't, for example. Your agents can still work on this Mac or on another computer you add.");
    expect(document.querySelector("[data-new-run-separate-open]")).toBeNull();
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
    expect(document.querySelector("[data-new-run-computer=mini]")!.getAttribute("aria-disabled")).toBeNull();
  });

  it("on a PC, says the host's reason once a create was refused", () => {
    const v = whereView({ ...base, os: "windows", canMakeSeparate: false, separateRefused: true, separateBlocked: "Docker Desktop isn't running." });
    expect(v.separate?.sub).toBe("Docker Desktop isn't running. Your agents can still work on this PC or on another computer you add.");
  });

  it("explains the Local Network ask before the create, then follows it", () => {
    expect(whereView({ ...base, separate: { phase: "explain", name: null, error: null } }).separate).toMatchObject({
      state: "explain",
      action: "Make it",
      sub: "If macOS asks to let Cua Spaces find devices on your local network, choose Allow. Cua uses it to reach the new computer.",
    });
    expect(whereView({ ...base, separate: { phase: "making", name: "Linux 1", error: null } }).separate?.sub).toBe("Making Linux 1 now. You can continue; it keeps going.");
    expect(whereView({ ...base, separate: { phase: "failed", name: "Linux 1", error: "Not enough memory." } }).separate).toMatchObject({ sub: "Couldn't make it. Not enough memory.", action: "Try again" });
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
    expect(whereView({ ...base, canMakeSeparate: !blocked }).separate).toBeNull();
  });
});

describe("1b, sign-in", () => {
  it("waits for the browser with Try again and Use a code instead", () => {
    const v = signInView({ kind: "waiting" }, null);
    render(<SignInScreen view={v} onTryAgain={noop} onUseCode={noop} onOpen={noop} />);
    expect(screen.getByRole("heading").textContent).toBe("Finish signing in in your browser");
    expect(text()).toContain("Cua opened in your browser. Create your account there, or sign in if you already have one. This window moves on by itself when you're done.");
    expect(screen.getByRole("status").textContent).toBe("Waiting for your browser");
    expect(screen.getByRole("button", { name: "Try again" })).not.toBeNull();
    expect(screen.getByRole("button", { name: "Use a code instead" })).not.toBeNull();
  });

  it("says 'Sign-in didn't finish' with its reason, and Try again becomes the white button", () => {
    const v = signInView({ kind: "failed", reason: "notConfirmed", message: "x" }, null);
    expect(v).toMatchObject({ title: "Sign-in didn't finish", line: "The browser didn't confirm in time. Try again, or use a code if your browser is on another device.", tryAgainPrimary: true, waiting: null });
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
    render(<SignInScreen view={signInView({ kind: "waiting", userCode: "WXYZ-1234" }, "https://cua.ai/device")} onTryAgain={noop} onUseCode={noop} onOpen={onOpen} />);
    expect(document.querySelector("[data-new-run-code]")!.textContent).toContain("WXYZ-1234");
    fireEvent.click(screen.getByRole("button", { name: "Open the sign-in page" }));
    expect(onOpen).toHaveBeenCalledWith("https://cua.ai/device");
    expect(screen.queryByRole("button", { name: "Use a code instead" })).toBeNull();
  });
});

describe("screen 3, the coding agent", () => {
  const draw = (found: string[] | null, picked: string | null = null) => {
    const onInstall = vi.fn();
    const onPick = vi.fn();
    render(<AgentScreen view={agentView({ os: "macos", found, picked, error: null })} onPick={onPick} onContinue={noop} onInstall={onInstall} busy={false} />);
    return { onInstall, onPick };
  };

  it("one found: it is picked", () => {
    draw(["claude-code", "cursor"]);
    expect(screen.getByRole("heading").textContent).toBe("Your agents will use Claude Code");
    expect(text()).toContain("We found Claude Code on this Mac. Your agents will use it to do the work while you keep working on other things.");
    expect(text()).toContain("Found on this Mac");
    expect(document.querySelector("[data-new-run-agent=claude-code]")!.textContent).toContain("By Anthropic");
    expect(document.querySelector("[data-new-run-agent=claude-code] [data-check=on]")).not.toBeNull();
    expect(chosenAgent(["claude-code"], null)).toBe("claude-code");
  });

  it("several found: nothing is picked for them, and Continue waits", () => {
    const { onPick } = draw(["claude-code", "codex"]);
    expect(screen.getByRole("heading").textContent).toBe("Pick the coding agent to use");
    expect(text()).toContain("We found two on this Mac.");
    expect(document.querySelectorAll("[data-check=on]").length).toBe(0);
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(document.querySelector("[data-new-run-agent=codex]")!);
    expect(onPick).toHaveBeenCalledWith("codex");
    expect(chosenAgent(["claude-code", "codex"], null)).toBeNull();
    expect(chosenAgent(["claude-code", "codex"], "codex")).toBe("codex");
  });

  it("none found: Install Claude Code, or Install Codex instead (the official pages)", () => {
    const { onInstall } = draw([]);
    expect(screen.getByRole("heading").textContent).toBe("Install a coding agent");
    expect(text()).toContain("Cua doesn't come with its own, and we didn't find one on this Mac. Install Claude Code, then sign in to it once.");
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Install Claude Code" }));
    fireEvent.click(screen.getByRole("button", { name: "Install Codex instead" }));
    expect(onInstall.mock.calls).toEqual([["claude-code"], ["codex"]]);
  });
});

describe("screen 4, Ready", () => {
  it("names the agent and the picked computers, with a line to try and Copy", () => {
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

  it("with only this Mac picked, says this Mac", () => {
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

  it("comes after picking where agents work, on a Mac only, under the same dot", () => {
    expect(afterWhere("macos")).toBe("permissions");
    expect(afterWhere("windows")).toBe("agent");
    expect(afterWhere("linux")).toBe("agent");
    expect(dotFor("permissions")).toBe(dotFor("where"));
  });

  it("asks for both for Cua Spaces itself, says Cua Driver uses the same ones, and offers Later", () => {
    const { allow, onContinue } = draw(data(false, false));
    expect(screen.getByRole("heading").textContent).toBe("Let Cua Spaces see and use apps");
    expect(text()).toContain("This lets your agents work in the apps on this Mac, in the background.");
    expect(text()).toContain("Cua Driver uses these same permissions, so there's no second app to allow.");
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
    const missing = { status: "Installs the cua command in ~/.local/bin.", installed: false, busy: false, tryCommand: null, error: null, primary: { id: "install", label: "Install" } };
    const { rerender } = render(<BringTheRestCard view={bringRestView({ os: "linux", cli: true })} open="cli" onRow={onRow} onDismiss={noop} cli={<CliPanel view={missing} onInstall={onInstall} />} />);
    fireEvent.click(document.querySelector("[data-bring-rest-row=cli]")!);
    expect(onRow).toHaveBeenCalledWith("cli");
    expect(document.querySelector("[data-bring-rest-cli]")!.getAttribute("data-bring-rest-cli")).toBe("missing");
    fireEvent.click(document.querySelector("[data-bring-rest-cli-install]")!);
    expect(onInstall).toHaveBeenCalledTimes(1);
    const done = { ...missing, status: "Installed in ~/.local/bin.", installed: true, tryCommand: "cua spaces list", primary: null };
    rerender(<BringTheRestCard view={bringRestView({ os: "linux", cli: true })} open="cli" onRow={onRow} onDismiss={noop} cli={<CliPanel view={done} onInstall={onInstall} />} />);
    expect(document.querySelector("[data-bring-rest-cli-install]")).toBeNull();
    expect(document.querySelector("[data-bring-rest-cli-try]")!.textContent).toBe("cua spaces list");
  });

  it("waits for a job that finished: a done Home task, or a run since setup ended that finished its turn", () => {
    const since = 1_000;
    expect(firstJobDone({ tasks: [], runs: [] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [{ state: "working", ending: null }, { state: "starting", ending: null }], runs: [] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [{ state: "failed", ending: null }], runs: [] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [{ state: "done", ending: null }], runs: [] }, since)).toBe(true);
    expect(firstJobDone({ tasks: [{ state: null, ending: "thrown" }], runs: [] }, since)).toBe(true);
    // Runs from before setup ended don't count; a run still going or one that failed doesn't either.
    expect(firstJobDone({ tasks: [], runs: [{ startedMs: 500, status: "idle" }] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [], runs: [{ startedMs: 2_000, status: "running" }, { startedMs: 2_000, status: "failed" }] }, since)).toBe(false);
    expect(firstJobDone({ tasks: [], runs: [{ startedMs: 2_000, status: "idle" }] }, since)).toBe(true);
  });
});

describe("screen 1, usage data", () => {
  const core = {
    notice: "Cua collects usage data, like which features you use and when something goes wrong. Never your files, what you type or your screen.",
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
    expect(line.textContent).toContain("Never your files, what you type or your screen.");
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
  it("name each step and go back to any earlier one, but forward only to a step already reached", () => {
    const where = stepDots("where", 1)!;
    expect(where.map((d) => d.label)).toEqual(["Step 1 of 3: Combine your existing computers into one supercomputer.", "Step 2 of 3: Pick where your agents work", "Step 3 of 3: Pick your coding agent"]);
    expect(where.map((d) => d.to)).toEqual(["welcome", null, null]);
    expect(where.map((d) => d.current)).toEqual([false, true, false]);
    // Back on screen 1 after reaching screen 3: both later steps open again.
    expect(stepDots("welcome", 2)!.map((d) => d.to)).toEqual([null, "where", "agent"]);
    // On the sign-in, the first dot goes back to its start; nothing past it opens (a dot never skips the sign-in).
    expect(stepDots("signin", 0)!.map((d) => d.to)).toEqual(["welcome", null, null]);
    expect(stepDots("ready", 2)).toBeNull();
  });

  it("count Ready and Next steps as past the last dot, and never go back on how far a run got", () => {
    expect(reachedAfter(0, "signin")).toBe(0);
    expect(reachedAfter(0, "where")).toBe(1);
    expect(reachedAfter(1, "ready")).toBe(2);
    expect(reachedAfter(2, "welcome")).toBe(2);
  });

  it("move with the arrow keys to the nearest step that opens", () => {
    const dots = stepDots("agent", 2)!;
    expect(dotStep(dots, 2, -1)).toBe(1);
    expect(dotStep(dots, 2, 1)).toBeNull();
    expect(dotStep(stepDots("welcome", 0)!, 0, 1)).toBeNull();
  });

  it("are buttons with their step's name: a click or an arrow key opens a step, a step not reached yet can't be pressed", () => {
    const onGo = vi.fn();
    render(<StepDots dots={stepDots("where", 1)!} onGo={onGo} />);
    const first = screen.getByRole("button", { name: "Step 1 of 3: Combine your existing computers into one supercomputer." });
    const second = screen.getByRole("button", { name: "Step 2 of 3: Pick where your agents work" });
    const third = screen.getByRole("button", { name: "Step 3 of 3: Pick your coding agent" });
    expect(second.getAttribute("aria-current")).toBe("step");
    expect(third.hasAttribute("disabled")).toBe(true);
    fireEvent.click(third);
    expect(onGo).not.toHaveBeenCalled();
    fireEvent.click(first);
    expect(onGo).toHaveBeenLastCalledWith(0, false);
    fireEvent.keyDown(second, { key: "ArrowLeft" });
    expect(onGo).toHaveBeenLastCalledWith(0, true);
    onGo.mockClear();
    fireEvent.keyDown(second, { key: "ArrowRight" });
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
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
    expect(document.querySelector("[data-float-keys]")).not.toBeNull();
    click("[data-onboarding-later]");
    await waitFor(() => expect(heading()).toBe("Pick where your agents work"));
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("not added yet");
    click("[data-onboarding-primary]");
    // On a Mac, Cua Spaces' permissions next: both missing here, so Later goes on without them.
    await waitFor(() => expect(heading()).toBe("Let Cua Spaces see and use apps"), { timeout: 15_000 });
    await waitFor(() => expect(document.querySelectorAll("[data-app-permission]").length).toBe(2));
    expect(calls.some((c) => c.op === "driver.permissions")).toBe(true);
    click("[data-new-run-permissions-next=Later]");
    await waitFor(() => expect(heading()).toBe("Your agents will use Codex"), { timeout: 15_000 });
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
    expect(calls.some((c) => c.op === "agents.connect" || c.op === "agents.configure")).toBe(true);
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
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
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
    expect(heading()).toBe("Combine your existing computers into one supercomputer.");
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
  const status = (id: string) => document.querySelector(`[data-home-apps] [data-app-permission=${id}] [data-permission-status]`)?.getAttribute("data-permission-status") ?? null;

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
    expect(document.querySelector("[data-bring-rest-apps] [data-app-permission=accessibility] [data-permission-status]")!.getAttribute("data-permission-status")).toBe("off");
    // One card, not two.
    expect(document.querySelector("[data-home-apps]")).toBeNull();
  });

  it("signed in: Add this computer opens the sign-in, then this Mac was added just now and the account's computers are listed", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Finish signing in in your browser"));
    expect(calls.some((c) => c.op === "session.signIn")).toBe(true);
    await waitFor(() => expect(heading()).toBe("Pick where your agents work"), { timeout: 15_000 });
    await waitFor(() => expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("Added to your account just now"), { timeout: 15_000 });
    expect(document.body.textContent).toContain("Other computers in your account");
    expect(document.body.textContent).toContain("Add another computer you own");
    expect(readFirstRun().newRun?.addedInRun).toBe(true);
  });
  it("a Mac that can't run virtual computers: the explainer, the create, then only the separate-computer option turns grey", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true, failCreates: "unsupported" },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
    click("[data-onboarding-later]");
    await waitFor(() => expect(document.querySelector("[data-new-run-separate-open]")).not.toBeNull(), { timeout: 15_000 });
    click("[data-new-run-separate-open]");
    // Our one line comes first; nothing is made until "Make it".
    await waitFor(() => expect(document.querySelector("[data-new-run-separate=explain]")).not.toBeNull());
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    click("[data-new-run-separate-action]");
    await waitFor(() => expect(document.querySelector("[data-new-run-separate]")!.textContent).toContain("This Mac can't make one."), { timeout: 15_000 });
    expect((document.querySelector("[data-new-run-separate]") as HTMLElement).dataset.disabled).toBe("");
    expect(document.body.textContent).not.toMatch(/local runtime:|exit status/i);
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
  });

  it("Skip for now after screen 2 leaves setup for Home, as finishing does", { timeout: 60_000 }, async () => {
    writeFirstRunForTest({ newRun: { screen: "agent", reached: 2 } });
    const { router } = await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: true, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(document.querySelector("[data-new-run=agent]")).not.toBeNull(), { timeout: 15_000 });
    click("[data-onboarding-later]");
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
    expect(readFirstRun().finishedLater).toBe(true);
  });

  it("the step dots go back to screen 1 and forward again to screen 2, but never past the sign-in", { timeout: 60_000 }, async () => {
    await mountApp("/onboarding", { failsCreates: false, listsGhost: false, demo: { signedIn: false, onboarded: false, noSpaces: true } });
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
    // Screen 2 isn't reached yet: its dot can't be pressed, so it can't skip the sign-in.
    expect(screen.getByRole("button", { name: "Step 2 of 3: Pick where your agents work" }).hasAttribute("disabled")).toBe(true);
    click("[data-onboarding-later]");
    await waitFor(() => expect(heading()).toBe("Pick where your agents work"));
    fireEvent.click(screen.getByRole("button", { name: "Step 1 of 3: Combine your existing computers into one supercomputer." }));
    await waitFor(() => expect(heading()).toBe("Combine your existing computers into one supercomputer."));
    fireEvent.click(screen.getByRole("button", { name: "Step 2 of 3: Pick where your agents work" }));
    await waitFor(() => expect(heading()).toBe("Pick where your agents work"));
    expect(screen.getByRole("button", { name: "Step 3 of 3: Pick your coding agent" }).hasAttribute("disabled")).toBe(true);
  });
});
