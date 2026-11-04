// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new first run (Labs, "Try the new setup"; CUA-3318), phase 1: each screen's words and choices in
// every state the storyboard draws (signed out, signed in, an offline computer, no coding agent, a Mac
// that can't make a separate computer, a sign-in that didn't finish), then the whole run on its real
// route with the wasm core.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { AgentSetupRow } from "@/bridge/contracts/agents";
import { readFirstRun, resetFirstRunCache } from "@/bridge/first-run";
import { resetSavedOnboardingCache } from "@/bridge/onboarding";
import { AgentScreen, NextScreen, ReadyScreen, SignInScreen, WhereScreen } from "@/components/first-run-new/first-run-new";
import { placeKeys } from "@/components/first-run-new/float-keys";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { agentView, chosenAgent, nextView, readyView, signInView, togglePick, whereView, type WhereInput } from "@/lib/first-run-new";
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
    expect(onAddThis).toHaveBeenCalled();
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
  it("greys only that option, with one plain sentence; this Mac and the other computers still work", () => {
    drawWhere({ ...base, canMakeSeparate: false, separateBlocked: "Needs Apple silicon." });
    const sep = document.querySelector<HTMLElement>("[data-new-run-separate]")!;
    expect(sep.dataset.disabled).toBe("");
    expect(sep.textContent).toContain("This Mac can't make one. Older Intel Macs can't, for example. Your agents can still work on this Mac or on another computer you add.");
    expect(document.querySelector("[data-new-run-separate-open]")).toBeNull();
    expect(screen.getByRole("button", { name: "Continue" }).hasAttribute("disabled")).toBe(false);
    expect(document.querySelector("[data-new-run-computer=mini]")!.getAttribute("aria-disabled")).toBeNull();
  });

  it("on a PC, says the host's reason", () => {
    const v = whereView({ ...base, os: "windows", canMakeSeparate: false, separateBlocked: "Docker Desktop isn't running." });
    expect(v.separate.sub).toBe("Docker Desktop isn't running. Your agents can still work on this PC or on another computer you add.");
  });

  it("explains the Local Network ask before the create, then follows it", () => {
    expect(whereView({ ...base, separate: { phase: "explain", name: null, error: null } }).separate).toMatchObject({
      state: "explain",
      action: "Make it",
      sub: "If macOS asks to let Cua Spaces find devices on your local network, choose Allow. Cua uses it to reach the new computer.",
    });
    expect(whereView({ ...base, separate: { phase: "making", name: "Linux 1", error: null } }).separate.sub).toBe("Making Linux 1 now. You can continue; it keeps going.");
    expect(whereView({ ...base, separate: { phase: "failed", name: "Linux 1", error: "Not enough memory." } }).separate).toMatchObject({ sub: "Couldn't make it. Not enough memory.", action: "Try again" });
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

  it("with only this Mac picked, says this Mac", () => {
    expect(readyView({ os: "macos", agent: "codex", computers: ["this"] })).toMatchObject({
      title: "Codex can now use this Mac",
      command: 'codex "Make a separate Linux computer on this Mac and run this repo\'s tests in it"',
    });
  });
});

describe("screen 6, next steps", () => {
  it("offers to invite a teammate, add another computer and let agents use apps on this Mac", () => {
    const onRow = vi.fn();
    render(<NextScreen view={nextView({ os: "macos" })} onRow={onRow} onDone={noop} />);
    expect(screen.getByRole("heading").textContent).toBe("Now bring the rest in");
    const rows = [...document.querySelectorAll<HTMLElement>("[data-new-run-next-row]")].map((r) => r.textContent);
    expect(rows[0]).toContain("Invite a teammate");
    expect(rows[1]).toContain("Add another computer");
    expect(rows[2]).toContain("Let agents use apps on this Mac");
    fireEvent.click(document.querySelector("[data-new-run-next-row=apps]")!);
    expect(onRow).toHaveBeenCalledWith("apps");
  });

  it("has no apps row away from a Mac", () => {
    expect(nextView({ os: "windows" }).rows.map((r) => r.id)).toEqual(["invite", "add-computer"]);
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
  resetPrepare();
  setFlagOverride("spaces_ask_first_home", true);
  setFlagOverride("spaces_new_onboarding", true);
});
afterEach(() => {
  setFlagOverride("spaces_ask_first_home", null);
  setFlagOverride("spaces_new_onboarding", null);
  resetPrepare();
});

const heading = () => document.querySelector("[data-onboarding-heading]")?.textContent ?? "";
const click = (sel: string) => fireEvent.click(document.querySelector<HTMLElement>(sel)!);
const onlyCodex = (rows: AgentSetupRow[]) => rows.map((r) => ({ ...r, installed: r.agent === "codex" }));
type Calls = { op: string; args: unknown }[];

describe.skipIf(!wasmBuilt)("the new first run on its route", () => {
  it("signed out: Skip for now, this Mac only, the agent found here, Ready, next steps, then Home; nothing is made meanwhile", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    const { router } = await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
      answer: (op, out) => (op === "agents.setup" || op === "agents.configure" ? onlyCodex(out as AgentSetupRow[]) : out),
    });
    await waitFor(() => expect(heading()).toBe("Bring all your computers under one account"));
    expect(document.querySelector("[data-float-keys]")).not.toBeNull();
    click("[data-onboarding-later]");
    await waitFor(() => expect(heading()).toBe("Pick where your agents work"));
    expect(document.querySelector("[data-new-run-this-status]")!.textContent).toBe("not added yet");
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Your agents will use Codex"), { timeout: 15_000 });
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Codex can now use this Mac"), { timeout: 15_000 });
    expect(calls.some((c) => c.op === "agents.connect" || c.op === "agents.configure")).toBe(true);
    click("[data-onboarding-primary]");
    await waitFor(() => expect(heading()).toBe("Now bring the rest in"));
    // No Space was made while the first run showed.
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
    click("[data-onboarding-primary]");
    await waitFor(() => expect(calls.find((c) => c.op === "session.completeOnboarding")).toBeTruthy());
    await waitFor(() => expect(router.state.location.pathname).toBe("/home"));
  });

  it("signed in: Add this computer opens the sign-in, then this Mac was added just now and the account's computers are listed", { timeout: 60_000 }, async () => {
    const calls: Calls = [];
    await mountApp("/onboarding", {
      failsCreates: false,
      listsGhost: false,
      demo: { signedIn: false, onboarded: false, noSpaces: true },
      intercept: (op, args) => (calls.push({ op, args }), undefined),
    });
    await waitFor(() => expect(heading()).toBe("Bring all your computers under one account"));
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
    await waitFor(() => expect(heading()).toBe("Bring all your computers under one account"));
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
});
