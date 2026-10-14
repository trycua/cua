// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createFakeHostBridge } from "../native/host";
import { onboardingCopy } from "../model/onboarding";
import { createFakeInstallerBridge } from "../native/installer";
import type { TelemetryBridge } from "../native/telemetry";
import type { TelemetrySignal } from "../model/telemetry";
// The webview build has no Node types; tests run in Node.
// @ts-expect-error node:fs is untyped here
import { readFileSync } from "node:fs";
import { fakeDrive, MOUNT_OFF, MOUNTED, NEEDS_APPROVAL } from "../test/fakeDrive";
import { DRIVE_APPROVAL_POLL_MS, DRIVE_PROMPT_POLL_MS, InstallerFlow, type InstallerAuth, summarizeAgent } from "./InstallerFlow";

// The Cua Volume page shows with its experiment on (Settings, Experiments);
// "has no Volume page without the Cua Volume experiment" turns it off.
beforeEach(() => {
  window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ cuaVolume: true }));
});

function fakeAuth() {
  let signedIn: ((identity?: string) => void) | undefined;
  let failed: ((reason: string) => void) | undefined;
  const auth: InstallerAuth & { signIn: (who?: string) => void; fail: (why: string) => void } = {
    beginSignIn: vi.fn(async () => ({ userCode: "WXYZ-1234", verificationUri: "https://cua.ai/device" })),
    onSignedIn: async (handler) => {
      signedIn = handler;
      return () => {
        if (signedIn === handler) signedIn = undefined;
      };
    },
    onSignInFailed: async (handler) => {
      failed = handler;
      return () => {
        if (failed === handler) failed = undefined;
      };
    },
    signIn: (who) => signedIn?.(who),
    fail: (why) => failed?.(why),
  };
  return auth;
}

/** A telemetry bridge that keeps what the flow records. */
function fakeTelemetry() {
  const log: string[] = [];
  const signals: TelemetrySignal[] = [];
  const view = { enabled: true, source: "default", sourceKind: "default", noticeShown: true, noticeText: "", docsUrl: "" };
  const bridge: TelemetryBridge = {
    isNative: true,
    status: async () => view,
    setEnabled: async () => view,
    acknowledgeNotice: async () => {
      log.push("notice");
      return view;
    },
    welcomeLeft: async (on) => {
      log.push(`welcome-left:${on}`);
      view.enabled = on;
      return view;
    },
    recordFeature: () => {},
    recordStep: () => {},
    recordStream: () => {},
    recordSignals: (s) => {
      if (s.length) log.push("signals");
      signals.push(...s);
    },
  };
  const pages = () =>
    signals.flatMap((s) => (s.type === "onboarding-page" ? [`${s.page} ${s.action} ${s.choice}`] : []));
  const steps = () => signals.flatMap((s) => (s.type === "step" ? [s.step] : []));
  return { bridge, log, signals, pages, steps };
}

const click = (name: string | RegExp) => fireEvent.click(screen.getByRole("button", { name }));

/** The page's bottom bar: [start corner], [end corner] button labels. */
function corners() {
  const footer = document.querySelector(".ob-footer");
  const labels = (sel: string) =>
    Array.from(footer?.querySelectorAll(`${sel} button`) ?? []).map((b) => b.textContent);
  return { start: labels(".ob-footer-start"), end: labels(".ob-footer-end") };
}

describe("InstallerFlow", () => {
  it("puts Back bottom-left and Skip then the primary bottom-right (Setup Assistant)", async () => {
    const auth = fakeAuth();
    const { unmount } = render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        auth={auth}
        initialStep="signin"
        onDone={vi.fn()}
      />,
    );
    expect(corners()).toEqual({ start: ["Back"], end: ["Skip", "Sign in"] });
    unmount();

    const r2 = render(
      <InstallerFlow installer={createFakeInstallerBridge()} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />,
    );
    await screen.findByRole("group", { name: "Detected agents" });
    expect(corners()).toEqual({ start: ["Back"], end: ["Skip", "Set up"] });
    r2.unmount();

    const r3 = render(
      <InstallerFlow installer={createFakeInstallerBridge()} host={createFakeHostBridge()} initialStep="presentation" onDone={vi.fn()} />,
    );
    expect(corners()).toEqual({ start: ["Back"], end: ["Continue"] });
    r3.unmount();

    // Welcome keeps its centred Get started; no corner bar.
    render(<InstallerFlow installer={createFakeInstallerBridge()} host={createFakeHostBridge()} onDone={vi.fn()} />);
    expect(document.querySelector(".ob-footer")).toBeNull();
  });

  it("walks welcome → sign in → agents → where it shows up → Cua Volume → mode → done, installing cua silently", async () => {
    const installer = createFakeInstallerBridge();
    const host = createFakeHostBridge();
    const auth = fakeAuth();
    const onDone = vi.fn();
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF });
    const telemetry = fakeTelemetry();
    localStorage.removeItem("cua.settings.menuBar");
    render(
      <InstallerFlow
        installer={installer}
        host={host}
        auth={auth}
        drive={drive.bridge}
        os="macos"
        telemetry={telemetry.bridge}
        onDone={onDone}
      />,
    );

    expect(screen.getByText("Welcome to Cua Spaces")).toBeInTheDocument();
    // The telemetry notice is on Welcome: it is recorded as shown before
    // anything is recorded.
    expect(screen.getByTestId("telemetry-notice")).toHaveTextContent("Change it here or in Settings, Privacy.");
    expect(screen.getByRole("link", { name: "What is collected" })).toHaveAttribute(
      "href",
      "https://cua.ai/docs/cua-sdk/concepts/telemetry",
    );
    // Nothing is recorded while Welcome shows; the switch is on.
    expect(screen.getByRole("switch", { name: "Share anonymous usage data" })).toBeChecked();
    expect(telemetry.signals).toEqual([]);
    // First launch installs the bundled cua (PATH line added): no page.
    await waitFor(() => expect(installer.calls).toEqual(["cliPlan", "installCli:path"]));
    click("Get started");
    // Leaving Welcome settles the switch first, then the run is recorded.
    await waitFor(() => expect(telemetry.log.slice(0, 2)).toEqual(["welcome-left:true", "signals"]));

    // Sign in (device flow opens the browser). No Command line page.
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Command line" })).toBeNull();
    click("Sign in");
    expect(await screen.findByTestId("signin-code")).toHaveTextContent("WXYZ-1234");
    act(() => auth.signIn("ada@example.com"));
    expect(await screen.findByText("Signed in as ada@example.com.")).toBeInTheDocument();
    click("Continue");

    // Agents: installed ones are listed and ticked, missing ones marked.
    const agents = await screen.findByRole("group", { name: "Detected agents" });
    expect(within(agents).getByLabelText(/Claude Code/)).toBeChecked();
    expect(within(agents).getByLabelText(/Codex/)).toBeChecked();
    expect(screen.getByText("cua skills")).toBeInTheDocument();
    fireEvent.click(within(agents).getByLabelText(/Codex/));
    click("Set up");
    expect(await screen.findByText("Your AI agents are set up")).toBeInTheDocument();
    expect(installer.setupRequests).toEqual([{ agents: ["claude-code"], skills: true, mcp: true, driver: false }]);
    expect(screen.getByRole("list", { name: "Setup results" })).toHaveTextContent("Claude Code: done");
    expect(screen.getByTitle("cua MCP server configured, 3 skills installed")).toBeInTheDocument();
    click("Continue");

    // Where it shows up: two animated cards, the notch by default; the pick
    // is the same setting as Settings.
    expect(await screen.findByRole("heading", { name: "Where should Cua Spaces show up?" })).toBeInTheDocument();
    const notch = screen.getByRole("radio", { name: /Notch and menu bar/ });
    const menuOnly = screen.getByRole("radio", { name: /Menu bar only/ });
    expect(notch).toHaveAttribute("aria-checked", "true");
    expect(within(menuOnly).getByRole("img")).toHaveAccessibleName("The menu bar menu alone");
    fireEvent.click(menuOnly);
    expect(menuOnly).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem("cua.settings.menuBar")).toBe("true");
    click("Continue");

    // Cua Volume: off by default; Continue with the box clear mounts nothing.
    expect(await screen.findByRole("heading", { name: "Cua Volume" })).toBeInTheDocument();
    expect(screen.getByLabelText("Add Cua Volume to Finder")).not.toBeChecked();
    click("Continue");

    // Mode choice (§8.9), then done.
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Access other machines"));
    expect(await screen.findByText("You're all set")).toBeInTheDocument();
    expect(screen.getByText("/Users/ada/.local/bin/cua")).toBeInTheDocument();
    expect(screen.getByText("ada@example.com")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("Menu bar only")).toBeInTheDocument();
    expect(drive.tools()).toEqual(["volume_mount_status", "volume_storage"]);
    // The example prompts scroll under both columns: the core's list, once
    // for assistive technology (the loop's second copy is hidden).
    const ticker = screen.getByRole("list", { name: "Example prompts" });
    const rows = within(ticker).getAllByRole("listitem");
    expect(rows).toHaveLength(7);
    expect(rows[0]).toHaveTextContent("\u201Cqa my app on windows, macos and linux\u201D");
    expect(ticker.querySelectorAll(".ob-ticker-again[aria-hidden='true']")).toHaveLength(7);
    expect(within(ticker).queryByRole("button")).toBeNull();
    click("Start using Cua Spaces");
    expect(onDone).toHaveBeenCalledWith("client");
    expect(host.calls).toEqual(["onboarding:client"]);
    localStorage.removeItem("cua.settings.menuBar");
    // Every page shown and left, with its answer as a fixed word (the
    // app core's `telemetry::onboarding`, the same in the SwiftUI app).
    expect(telemetry.pages()).toEqual([
      "welcome shown none",
      "welcome completed none",
      "signin shown none",
      "signin completed signed_in",
      "agents shown none",
      "agents completed agents_set_up",
      "presentation shown none",
      "presentation completed menu_bar_only",
      "volume shown none",
      "volume completed this_mac",
      "this_machine shown none",
      "this_machine completed access_others",
      "done shown none",
      "done completed none",
    ]);
    expect(telemetry.steps()).toEqual(["onboarding_shown", "signed_in", "onboarding_completed"]);
    expect(JSON.stringify(telemetry.signals)).not.toContain("ada");
  });

  it("the usage-data switch off on Welcome: nothing is recorded for the whole run", async () => {
    const telemetry = fakeTelemetry();
    const auth = fakeAuth();
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF });
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        auth={auth}
        drive={drive.bridge}
        os="macos"
        telemetry={telemetry.bridge}
        onDone={vi.fn()}
      />,
    );
    const toggle = screen.getByRole("switch", { name: "Share anonymous usage data" });
    await waitFor(() => expect(toggle).toBeChecked());
    fireEvent.click(toggle);
    expect(toggle).not.toBeChecked();
    click("Get started");
    await waitFor(() => expect(telemetry.log).toContain("welcome-left:false"));
    await screen.findByRole("heading", { name: "Sign in" });
    click("Skip");
    await screen.findByRole("heading", { name: "AI agents" });
    click("Skip");
    await screen.findByRole("heading", { name: "Where should Cua Spaces show up?" });
    click("Continue");
    await screen.findByRole("heading", { name: "Cua Volume" });
    click("Continue");
    fireEvent.click(await screen.findByText("Access other machines"));
    click(await screen.findByRole("button", { name: "Start using Cua Spaces" }).then((b) => b.textContent!));
    expect(telemetry.signals).toEqual([]);
    expect(telemetry.log).not.toContain("signals");
  });

  it("first launch: a current cua installs nothing; a build without the sidecar installs nothing", async () => {
    const ready = createFakeInstallerBridge({ plan: { installed: true, upToDate: true, onPath: true } });
    const { unmount } = render(<InstallerFlow installer={ready} host={createFakeHostBridge()} onDone={vi.fn()} />);
    await waitFor(() => expect(ready.calls).toEqual(["cliPlan"]));
    unmount();
    const none = createFakeInstallerBridge({ plan: { source: null, method: null } });
    render(<InstallerFlow installer={none} host={createFakeHostBridge()} onDone={vi.fn()} />);
    await waitFor(() => expect(none.calls).toEqual(["cliPlan"]));
  });

  it("sign in: already signed in continues; failures show and can be skipped", async () => {
    const auth = fakeAuth();
    const { unmount } = render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        auth={auth}
        identity="grace@example.com"
        initialStep="signin"
        onDone={vi.fn()}
      />,
    );
    expect(screen.getByText("Signed in as grace@example.com.")).toBeInTheDocument();
    unmount();

    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        auth={auth}
        initialStep="signin"
        onDone={vi.fn()}
      />,
    );
    click("Sign in");
    await screen.findByTestId("signin-code");
    act(() => auth.fail("access_denied"));
    expect(await screen.findByRole("alert")).toHaveTextContent("access_denied");
    click("Skip");
    expect(await screen.findByRole("heading", { name: "AI agents" })).toBeInTheDocument();
  });

  it("agents: MCP only, errors, retry and nothing detected", async () => {
    const installer = createFakeInstallerBridge();
    const { unmount } = render(
      <InstallerFlow installer={installer} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />,
    );
    await screen.findByRole("group", { name: "Detected agents" });
    fireEvent.click(screen.getByLabelText("cua skills"));
    click("Set up");
    await screen.findByText("Your AI agents are set up");
    expect(installer.setupRequests[0]).toEqual({ agents: ["claude-code", "codex"], skills: false, mcp: true, driver: false });
    unmount();

    const none = createFakeInstallerBridge();
    const r2 = render(
      <InstallerFlow installer={none} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />,
    );
    await screen.findByRole("group", { name: "Detected agents" });
    fireEvent.click(screen.getByLabelText("cua skills"));
    fireEvent.click(screen.getByLabelText(/cua MCP server/));
    expect(screen.getByRole("button", { name: "Set up" })).toBeDisabled();
    r2.unmount();

    const broken = createFakeInstallerBridge({ failDetect: "agent setup needs the cua CLI" });
    const r3 = render(
      <InstallerFlow installer={broken} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("needs the cua CLI");
    click("Try again");
    await waitFor(() => expect(broken.calls.filter((c) => c === "detectAgents")).toHaveLength(2));
    r3.unmount();

    const empty = createFakeInstallerBridge({ agents: [] });
    render(<InstallerFlow installer={empty} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />);
    expect(await screen.findByText("No agents found.")).toBeInTheDocument();
    click("Skip");
    expect(await screen.findByText("Where should Cua Spaces show up?")).toBeInTheDocument();
  });

  it("agents: the background computer-use card is one line, off by default, and adds cua-driver", async () => {
    const copy = onboardingCopy();
    const installer = createFakeInstallerBridge();
    render(<InstallerFlow installer={installer} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />);
    await screen.findByRole("group", { name: "Detected agents" });
    // The core's words: the miniature's label and the one checkbox line.
    const card = document.querySelector<HTMLElement>(".installer-driver")!;
    expect(within(card).getByRole("img")).toHaveAccessibleName(copy.agentsDriverImage);
    expect(card.querySelector('.obp-picture[data-preview="driver"]')).not.toBeNull();
    const box = within(card).getByLabelText(copy.agentsDriver);
    expect(box).not.toBeChecked();
    expect(card.querySelectorAll("input")).toHaveLength(1);
    expect(card.querySelector("p, h2, h3")).toBeNull();
    // cua-driver alone is enough to set up.
    fireEvent.click(screen.getByLabelText("cua skills"));
    fireEvent.click(screen.getByLabelText(/cua MCP server/));
    expect(screen.getByRole("button", { name: "Set up" })).toBeDisabled();
    fireEvent.click(box);
    expect(box).toBeChecked();
    expect(screen.getByRole("button", { name: "Set up" })).toBeEnabled();
    fireEvent.click(screen.getByLabelText("cua skills"));
    fireEvent.click(screen.getByLabelText(/cua MCP server/));
    click("Set up");
    await screen.findByText("Your AI agents are set up");
    expect(installer.setupRequests).toEqual([
      { agents: ["claude-code", "codex"], skills: true, mcp: true, driver: true },
    ]);
    // The per-agent summaries include the cua-driver step (the core's words).
    expect(screen.getByRole("list", { name: "Setup results" })).toHaveTextContent("Claude Code: done");
    expect(screen.getAllByTitle(/cua-driver configured/)).toHaveLength(2);
  });

  it("agents: shows per-agent failures from the setup report", async () => {
    const installer = createFakeInstallerBridge({ failSetup: "config is not valid JSON" });
    render(<InstallerFlow installer={installer} host={createFakeHostBridge()} initialStep="agents" onDone={vi.fn()} />);
    await screen.findByRole("group", { name: "Detected agents" });
    click("Set up");
    expect(await screen.findByRole("alert")).toHaveTextContent("config is not valid JSON");
    expect(screen.getByRole("button", { name: "Set up" })).toBeEnabled();
  });

  it("host mode from the installer preselects unattended access", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onDone = vi.fn();
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={host}
        installerMode="host"
        identity="ada@example.com"
        initialStep="mode"
        onDone={onDone}
      />,
    );
    click("Set up for access");
    expect(await screen.findByText("You're all set")).toBeInTheDocument();
    expect(screen.getByText("Set up for unattended access")).toBeInTheDocument();
    expect(screen.queryByText("Grant in System Settings")).toBeNull();
    click("Start using Cua Spaces");
    expect(onDone).toHaveBeenCalledWith("host");
    expect(host.calls).toEqual(["setup:relay:https://relay.cua.ai", "onboarding:host"]);
  });
});

describe("InstallerFlow Cua Volume page", () => {
  const box = () => screen.getByRole<HTMLInputElement>("checkbox");

  it("is one checkbox line under the miniature, off by default, and mounts on Continue", async () => {
    let status: object = MOUNT_OFF;
    const drive = fakeDrive({ volume_mount_status: () => status, volume_mount: () => (status = MOUNTED) });
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={drive.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    expect(screen.getByRole("heading", { name: "Cua Volume" })).toBeInTheDocument();
    await waitFor(() => expect(box()).toBeEnabled());
    const card = document.querySelector<HTMLElement>(".installer-drive")!;
    expect(within(card).getByRole("img")).toHaveAccessibleName(
      "Files from a Space arriving in Cua Volume in Finder",
    );
    expect(card.querySelector('.obp-picture[data-preview="drive"]')).not.toBeNull();
    expect(card.querySelectorAll("input")).toHaveLength(1);
    expect(box()).not.toBeChecked();
    expect(within(card).getByText("Add Cua Volume to Finder")).toBeInTheDocument();
    fireEvent.click(box());
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.calls.map((c) => [c.tool, c.args])).toEqual([
      ["volume_mount_status", {}],
      ["volume_storage", {}],
      ["volume_mount", {}],
    ]);
  });

  it("waits for the extension's approval: opens System Settings and asks again every 2 s", async () => {
    let status: object = MOUNT_OFF;
    const drive = fakeDrive({ volume_mount_status: () => status, volume_mount: () => (status = NEEDS_APPROVAL) });
    const host = createFakeHostBridge();
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={host}
        drive={drive.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    await waitFor(() => expect(box()).toBeEnabled());
    fireEvent.click(box());
    click("Continue");
    expect(await screen.findByText("Turn on Cua Volume in File System Extensions.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Cua Volume" })).toBeInTheDocument();
    click("Open System Settings");
    expect(host.calls).toContain("settings:x-apple.systempreferences:com.apple.LoginItems-Settings.extension");
    expect(DRIVE_APPROVAL_POLL_MS).toBe(2000);
    const asked = () => drive.tools().filter((t) => t === "volume_mount_status").length;
    const before = asked();
    status = MOUNTED;
    await waitFor(() => expect(asked()).toBeGreaterThan(before), { timeout: DRIVE_APPROVAL_POLL_MS + 1500 });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Open System Settings" })).toBeNull());
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.tools().filter((t) => t === "volume_mount")).toHaveLength(1);
  });

  it("with no daemon answer the box is disabled, says so, and Continue moves on", async () => {
    const drive = fakeDrive({});
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={drive.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    expect(await screen.findByText("Not available on this Mac yet")).toBeInTheDocument();
    expect(box()).toBeDisabled();
    expect(box()).not.toBeChecked();
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.tools()).toEqual(["volume_mount_status", "volume_storage"]);
    // No storage answer: no choice to make.
    expect(screen.queryByRole("radiogroup")).toBeNull();
  });

  it("shows why mounting failed and stays; unticking an earlier mount unmounts", async () => {
    const failing = fakeDrive({ volume_mount_status: MOUNT_OFF, volume_mount: new Error("fskit: extension missing") });
    const { unmount } = render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={failing.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    await waitFor(() => expect(box()).toBeEnabled());
    fireEvent.click(box());
    click("Continue");
    expect(await screen.findByRole("alert")).toHaveTextContent("fskit: extension missing");
    expect(screen.getByRole("heading", { name: "Cua Volume" })).toBeInTheDocument();
    unmount();

    let status: object = MOUNTED;
    const mounted = fakeDrive({
      volume_mount_status: () => status,
      volume_unmount: () => (status = MOUNT_OFF),
    });
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={mounted.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    await waitFor(() => expect(box()).toBeChecked());
    fireEvent.click(box());
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(mounted.tools()).toEqual(["volume_mount_status", "volume_storage", "volume_unmount"]);
  });

  it("says Mount Cua Volume on Linux and has no page on Windows", async () => {
    const linux = fakeDrive({ volume_mount_status: { enabled: false, state: "off", method: "fuse", volume_name: "Cua Volume" } });
    const { unmount } = render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={linux.bridge}
        os="linux"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
    expect(await screen.findByLabelText("Mount Cua Volume")).toBeEnabled();
    unmount();

    const windows = fakeDrive({});
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={windows.bridge}
        os="windows"
        initialStep="presentation"
        onDone={vi.fn()}
      />,
    );
    await waitFor(() => expect(document.querySelector('li[aria-label="Cua Volume"]')).toBeNull());
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
  });

  it("has no Volume page without the Cua Volume experiment, and mounts nothing", async () => {
    window.localStorage.removeItem("cua.settings.experiments");
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF, volume_mount: MOUNTED });
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={drive.bridge}
        os="macos"
        initialStep="presentation"
        onDone={vi.fn()}
      />,
    );
    await waitFor(() => expect(document.querySelector('li[aria-label="Cua Volume"]')).toBeNull());
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.tools()).not.toContain("volume_mount");
  });
});

describe("InstallerFlow Cua Volume storage choice", () => {
  const FS = { backend: "fs", fs_path: "/Users/maya/.cua/volume/data", s3: null, has_keys: false };
  const OK = { ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied: true };
  const flow = (drive: ReturnType<typeof fakeDrive>) =>
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        drive={drive.bridge}
        os="macos"
        initialStep="drive"
        onDone={vi.fn()}
      />,
    );
  const choices = () => screen.findByRole("radiogroup", { name: "Where your files live" });

  it("this Mac: the default; Continue saves nothing and mounts when ticked", async () => {
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF, volume_storage: FS, volume_mount: MOUNTED });
    flow(drive);
    const group = await choices();
    expect(within(group).getAllByRole("radio").map((r) => [r.textContent, r.getAttribute("aria-checked")])).toEqual([
      ["This Mac", "true"],
      ["Your S3 bucket", "false"],
      ["Set up later", "false"],
    ]);
    expect(within(group).queryByText(/cloud/i)).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    await waitFor(() => expect(screen.getByRole("checkbox")).toBeEnabled());
    fireEvent.click(screen.getByRole("checkbox"));
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.calls.map((c) => [c.tool, c.args])).toEqual([
      ["volume_mount_status", {}],
      ["volume_storage", {}],
      ["volume_mount", {}],
    ]);
  });

  it("your bucket: fields inline, Test connection, then Continue saves with the keys and moves on", async () => {
    let tested = false;
    const drive = fakeDrive({
      volume_mount_status: MOUNT_OFF,
      volume_storage: FS,
      volume_storage_set: (args) => {
        if (args.dry_run) {
          tested = true;
          return { ...OK, applied: false };
        }
        return OK;
      },
    });
    flow(drive);
    fireEvent.click(within(await choices()).getByRole("radio", { name: "Your S3 bucket" }));
    click("Enter details manually");
    const endpoint = screen.getByRole("textbox", { name: "Endpoint" });
    expect(endpoint).toHaveAttribute("placeholder", "AWS, R2 or MinIO URL");
    // Continue waits for a bucket and keys.
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    fireEvent.change(endpoint, { target: { value: "http://127.0.0.1:9000" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Bucket" }), { target: { value: "cua-volume" } });
    fireEvent.click(within(screen.getByRole("radiogroup", { name: "Path-style URLs" })).getByRole("radio", { name: "On" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Access key ID" }), { target: { value: "maya-drive" } });
    const secret = screen.getByLabelText("Secret access key");
    expect(secret).toHaveAttribute("type", "password");
    fireEvent.change(secret, { target: { value: "fixture-secret" } });
    click("Test connection");
    expect(await screen.findByText("Connected, versioning on")).toBeInTheDocument();
    expect(tested).toBe(true);
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    const sets = drive.calls.filter((c) => c.tool === "volume_storage_set").map((c) => c.args);
    const bucket = { endpoint: "http://127.0.0.1:9000", region: "us-east-1", bucket: "cua-volume", root: "", path_style: true };
    const keys = { access_key_id: "maya-drive", secret_access_key: "fixture-secret" };
    expect(sets).toEqual([
      { backend: "s3", s3: bucket, ...keys, dry_run: true },
      { backend: "s3", s3: bucket, ...keys, dry_run: false },
    ]);
    // The keys went nowhere else.
    for (const c of drive.calls.filter((c) => c.tool !== "volume_storage_set")) {
      expect(JSON.stringify(c.args)).not.toContain("fixture-secret");
    }
    for (let i = 0; i < localStorage.length; i++) {
      expect(localStorage.getItem(localStorage.key(i)!)).not.toContain("fixture-secret");
    }
  });

  it("your bucket: a failed save shows why and stays on the page", async () => {
    const drive = fakeDrive({
      volume_mount_status: MOUNT_OFF,
      volume_storage: FS,
      volume_storage_set: new Error("bucket maya-files does not exist"),
    });
    flow(drive);
    fireEvent.click(within(await choices()).getByRole("radio", { name: "Your S3 bucket" }));
    click("Enter details manually");
    fireEvent.change(screen.getByRole("textbox", { name: "Bucket" }), { target: { value: "maya-files" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Access key ID" }), { target: { value: "k" } });
    fireEvent.change(screen.getByLabelText("Secret access key"), { target: { value: "s" } });
    click("Test connection");
    expect(await screen.findByRole("alert")).toHaveTextContent("bucket maya-files does not exist");
    click("Continue");
    await waitFor(() => expect(drive.tools().filter((t) => t === "volume_storage_set")).toHaveLength(2));
    await waitFor(() => expect(screen.getByRole("button", { name: "Test connection" })).toBeEnabled());
    expect(screen.getByRole("alert")).toHaveTextContent("bucket maya-files does not exist");
    expect(screen.getByRole("heading", { name: "Cua Volume" })).toBeInTheDocument();
    // Continue waits for a change after a failed check.
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "Bucket" }), { target: { value: "cua-volume" } });
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
  });

  it("your bucket: the agent prompt, Copy, and the bucket the agent sets up is adopted", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const S3 = {
      backend: "s3",
      fs_path: FS.fs_path,
      s3: { endpoint: "https://acct.r2.cloudflarestorage.com", region: "auto", bucket: "maya-volume", root: "", path_style: false },
      has_keys: true,
    };
    let storage: object = FS;
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF, volume_storage: () => storage, volume_storage_set: OK });
    flow(drive);
    // The miniature shows until the bucket's rows take its place.
    const picture = () => document.querySelector('.installer-drive .obp-picture[data-preview="drive"]');
    expect(picture()).not.toBeNull();
    fireEvent.click(within(await choices()).getByRole("radio", { name: "Your S3 bucket" }));
    expect(picture()).toBeNull();
    const prompt = screen.getByRole<HTMLTextAreaElement>("textbox", { name: "Ask your agent to set it up:" });
    expect(prompt).toHaveAttribute("readonly");
    expect(screen.queryByRole("textbox", { name: "Bucket" })).toBeNull();
    click("Copy");
    expect(writeText).toHaveBeenCalledWith(prompt.value);
    // The link toggles to the fields and back.
    click("Enter details manually");
    expect(screen.getByRole("textbox", { name: "Bucket" })).toBeInTheDocument();
    click("Use a prompt instead");
    expect(screen.getByRole("textbox", { name: "Ask your agent to set it up:" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    // Back to This Mac: the miniature returns.
    fireEvent.click(within(await choices()).getByRole("radio", { name: "This Mac" }));
    expect(picture()).not.toBeNull();
    fireEvent.click(within(await choices()).getByRole("radio", { name: "Your S3 bucket" }));

    // The user's agent saves the bucket; the 2 s poll finds it and the core adopts it.
    storage = S3;
    expect(await screen.findByText("Connected, versioning on", undefined, { timeout: DRIVE_PROMPT_POLL_MS + 1500 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
    const sets = drive.calls.filter((c) => c.tool === "volume_storage_set").map((c) => c.args);
    expect(sets).toEqual([{ backend: "s3", s3: S3.s3, access_key_id: null, secret_access_key: null, dry_run: false }]);
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.tools().filter((t) => t === "volume_storage_set")).toHaveLength(1);
  });

  it("shows where the files are stored and where the volume is, shortened in the middle", async () => {
    const drive = fakeDrive({ volume_mount_status: MOUNTED, volume_storage: FS });
    flow(drive);
    const stored = await screen.findByLabelText("Stored in ~/.cua/volume/data");
    expect(stored).toHaveAttribute("title", "/Users/maya/.cua/volume/data");
    expect(stored.querySelector(".mid-text-tail")!.textContent).toBe("/data");
    expect(stored.closest("p")).toHaveClass("st-note");
    const mounted = screen.getByLabelText(/^In Finder at /);
    expect(mounted).toHaveAttribute("title", "/Volumes/Cua Volume");
  });

  it("later: one muted line, Continue saves nothing", async () => {
    const drive = fakeDrive({ volume_mount_status: MOUNT_OFF, volume_storage: FS });
    flow(drive);
    fireEvent.click(within(await choices()).getByRole("radio", { name: "Set up later" }));
    expect(screen.getByText("Settings, Storage, any time.")).toHaveClass("st-note");
    expect(screen.queryByRole("textbox")).toBeNull();
    click("Continue");
    expect(await screen.findByText("How will you use this machine?")).toBeInTheDocument();
    expect(drive.tools()).toEqual(["volume_mount_status", "volume_storage"]);
  });
});

describe("InstallerFlow on macOS", () => {
  it("lists the permission panes to grant on Done, opening them only on click", async () => {
    const host = createFakeHostBridge({ platform: "macos" });
    const onDone = vi.fn();
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={host}
        installerMode="host"
        initialStep="mode"
        onDone={onDone}
      />,
    );
    click("Set up for access");
    expect(await screen.findByText("Grant in System Settings")).toBeInTheDocument();
    expect(screen.getByText("Screen Recording")).toBeInTheDocument();
    expect(screen.getByText("Accessibility")).toBeInTheDocument();
    expect(host.calls.some((c) => c.startsWith("settings:"))).toBe(false);
    fireEvent.click(screen.getAllByRole("button", { name: "Open Settings" })[0]!);
    expect(host.calls).toContain(
      "settings:x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    );
    click("Start using Cua Spaces");
    expect(onDone).toHaveBeenCalledWith("host");
  });
});

describe("summarizeAgent", () => {
  it("counts skills, MCP and failures for one agent", () => {
    const report = {
      outcomes: [
        { agents: ["a"], target: "mcp", item: "cua", path: "/p", change: "unchanged", detail: "" },
        { agents: ["a"], target: "skill", item: "s1", path: "/s1", change: "created", detail: "" },
        { agents: ["a", "b"], target: "skill", item: "s2", path: "/s2", change: "failed", detail: "denied" },
        { agents: ["b"], target: "mcp", item: "cua", path: "/q", change: "skipped", detail: "managed by user" },
      ],
    };
    expect(summarizeAgent(report, "a", "Agent A")).toEqual({
      text: "cua MCP server configured, 1 skill installed",
      failed: ["s2: denied"],
      line: "Agent A: failed",
    });
    expect(summarizeAgent(report, "b").text).toBe("failed");
    expect(summarizeAgent(report, "c", "C")).toEqual({ text: "nothing to change", failed: [], line: "C: done" });
  });
});

declare const process: { cwd(): string };
const cwd = () => process.cwd();

describe("onboarding helper text", () => {
  it("the telemetry notice and the card notes wrap instead of being cut short", async () => {
    const css = (readFileSync(`${cwd()}/src/styles/desktop.css`, "utf8") as string).replace(/\/\*[\s\S]*?\*\//g, "");
    const rule = (selector: string) => {
      const blocks = css
        .split("}")
        .filter((b: string) => b.split("{")[0]!.split(",").some((sel: string) => sel.trim() === selector));
      return blocks.map((b: string) => b.split("{")[1] ?? "").join("\n");
    };
    // The agent prompt reads in the notes' font, not monospace.
    expect(rule(".st-prompt-text")).toContain("var(--dw-font)");
    expect(rule(".st-prompt-text")).not.toContain("monospace");
    for (const sel of [
      ".dw .onboarding-telemetry-notice",
      ".dw .ob-page-card .st-note",
      ".dw .ob-page-card .create-cloud-error",
      ".dw .ob-page .host-setup-lede",
    ]) {
      expect(rule(sel), sel).toContain("white-space: normal");
      expect(rule(sel), sel).not.toContain("nowrap");
      expect(rule(sel), sel).not.toContain("text-overflow: ellipsis");
    }
    render(<InstallerFlow installer={createFakeInstallerBridge()} host={createFakeHostBridge()} onDone={vi.fn()} />);
    const notice = screen.getByTestId("telemetry-notice");
    // The whole sentence, with the link inline after it.
    expect(notice.textContent!.length).toBeGreaterThan(40);
    expect(notice.lastElementChild!.tagName).toBe("A");
    expect(notice).not.toHaveAttribute("title");
  });
});

describe("InstallerFlow launch at login", () => {
  it("ticks Launch at login on Done and registers when the first run finishes", async () => {
    const { fakeLoginItemBridge } = await import("../native/loginItem");
    const { readLaunchChoice } = await import("../model/loginItem");
    window.localStorage.clear();
    const loginItem = fakeLoginItemBridge("notRegistered");
    const onDone = vi.fn();
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        initialStep="done"
        loginItem={loginItem}
        onDone={onDone}
      />,
    );
    const box = screen.getByRole("checkbox", { name: "Launch at login" });
    expect(box).toBeChecked();
    click("Start using Cua Spaces");
    await waitFor(() => expect(onDone).toHaveBeenCalledWith("client"));
    expect(loginItem.calls).toEqual([true]);
    expect(readLaunchChoice()).toBe(true);
  });

  it("respects an unticked box", async () => {
    const { fakeLoginItemBridge } = await import("../native/loginItem");
    const { readLaunchChoice } = await import("../model/loginItem");
    window.localStorage.clear();
    const loginItem = fakeLoginItemBridge("notRegistered");
    const onDone = vi.fn();
    render(
      <InstallerFlow
        installer={createFakeInstallerBridge()}
        host={createFakeHostBridge()}
        initialStep="done"
        loginItem={loginItem}
        onDone={onDone}
      />,
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "Launch at login" }));
    expect(screen.getByRole("checkbox", { name: "Launch at login" })).not.toBeChecked();
    click("Start using Cua Spaces");
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(loginItem.calls).toEqual([false]);
    expect(readLaunchChoice()).toBe(false);
  });
});
