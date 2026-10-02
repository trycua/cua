// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "../App";
import { FIXTURE_NOW } from "../model/fixtures";
import {
  clientLabel,
  hostFormInitial,
  hostFormView,
  hostSummary,
  reduceHostForm,
  thisMachineSpace,
  withThisMachine,
} from "../model/host";
import { notchActivity } from "../model/notch";
import { rowToSpace } from "../model/spaces";
import { createFallbackBridge } from "../native/bridge";
import { createFakeHostBridge, unconfiguredStatus, type HostStatus } from "../native/host";
import { fakeFleetBridge } from "../test/fakeFleet";
import { ThisMachinePanel } from "./ThisMachinePanel";

const sharing: HostStatus = {
  configured: true,
  mode: "relay",
  relayUrl: "https://relay.cua.ai",
  machineId: "0123abcd4567",
  name: "Studio",
  sharing: true,
  service: { installed: true, running: true, kind: "launchd" },
  online: true,
  clients: [{ id: "u2", email: "grace@example.com", streams: 3 }],
  permissions: [],
};

describe("host model", () => {
  it("summarises the host state", () => {
    expect(hostSummary(null)).toBe("Set up for access");
    expect(hostSummary(unconfiguredStatus())).toBe("Set up for access");
    expect(hostSummary(sharing)).toBe("Sharing · 1 connected");
    expect(hostSummary({ ...sharing, clients: [] })).toBe("Sharing · Relay");
    expect(hostSummary({ ...sharing, sharing: false })).toBe("Not sharing");
    expect(hostSummary({ ...sharing, service: { ...sharing.service, running: false } })).toBe("Host service stopped");
    expect(hostSummary({ ...sharing, mode: "direct", clients: [], online: false })).toBe("Direct · offline");
  });

  it("builds the This machine entry, first in the roster, and the setup form", () => {
    const space = thisMachineSpace(null, 5, "linux");
    expect(space).toMatchObject({ id: "this-mac", name: "This machine", status: "suspended", detail: "Set up for access" });
    expect(withThisMachine([], sharing, 5, "macos").map((s) => s.detail)).toEqual(["Sharing · 1 connected"]);
    expect(clientLabel({ id: "u", email: "e@x" })).toBe("e@x");
    let form = hostFormInitial();
    form = reduceHostForm(form, { type: "set-allow", allow: " a@x.com, b@y.com  acct-3" });
    expect(hostFormView(form).request?.allow).toEqual(["a@x.com", "b@y.com", "acct-3"]);
    form = reduceHostForm(form, { type: "toggle-advanced" });
    form = reduceHostForm(form, { type: "set-direct", on: true });
    form = reduceHostForm(form, { type: "set-listen", listen: "1.2.3.4:70000" });
    expect(hostFormView(form).canSubmit).toBe(false);
    form = reduceHostForm(form, { type: "set-listen", listen: "[::]:3211" });
    expect(hostFormView(form).request).toMatchObject({ mode: "direct", direct: "[::]:3211" });
  });

  it("lights the notch while someone is connected to this machine", () => {
    const roster = withThisMachine([], sharing, 5, "macos");
    expect(notchActivity(roster, true)).toMatchObject({
      kind: "remote-access",
      label: "Someone is connected to this machine",
    });
    expect(notchActivity(withThisMachine([], { ...sharing, clients: [] }, 5, "macos"), false)).toBeNull();
  });

  it("maps relay rows (space://relay/<id>) as My machines", () => {
    const space = rowToSpace(
      {
        id: "space://relay/0123abcd4567",
        name: "studio",
        provider: "relay",
        spacesdVersion: "0.1.0",
        features: ["desktop_stream"],
        reachable: true,
        os: "macos",
      },
      FIXTURE_NOW,
    );
    expect(space.provider).toBe("relay");
    expect(space.detail).toBe("My machines · via relay");
    expect(space.fleetId).toBe("relay");
  });
});

describe("ThisMachinePanel", () => {
  it("unconfigured: offers Set up for access and runs the host flow", async () => {
    const host = createFakeHostBridge({ platform: "linux" });
    const onStatus = vi.fn();
    render(<ThisMachinePanel host={host} onStatus={onStatus} />);
    fireEvent.click(await screen.findByRole("button", { name: "Set up for access" }));
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    expect(await screen.findByRole("button", { name: "Stop sharing" })).toBeInTheDocument();
    expect(host.calls).toEqual(["setup:relay:https://relay.cua.ai"]);
    expect(onStatus).toHaveBeenLastCalledWith(expect.objectContaining({ configured: true, sharing: true }));
  });

  it("configured: shows presence and the Stop sharing kill switch, then resume and remove", async () => {
    const host = createFakeHostBridge({ status: sharing, onboarding: { completed: true } });
    render(<ThisMachinePanel host={host} />);
    expect(await screen.findByText("Sharing · 1 connected")).toBeInTheDocument();
    expect(screen.getByText("grace@example.com · 3 streams")).toBeInTheDocument();
    expect(screen.getByText("Connected now")).toBeInTheDocument();
    expect(screen.getByText("Relay https://relay.cua.ai")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop sharing" }));
    expect(await screen.findByText("Not sharing")).toBeInTheDocument();
    expect(screen.getByText("Nobody")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Resume sharing" }));
    expect(await screen.findByRole("button", { name: "Stop sharing" })).toBeInTheDocument();
    // Removing asks first; Cancel removes nothing.
    fireEvent.click(screen.getByRole("button", { name: "Remove host setup" }));
    const ask = await screen.findByRole("alertdialog", { name: "Remove host setup?" });
    fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(host.calls).toEqual(["stop", "start"]);
    fireEvent.click(screen.getByRole("button", { name: "Remove host setup" }));
    fireEvent.click(
      within(await screen.findByRole("alertdialog", { name: "Remove host setup?" })).getByRole("button", {
        name: "Remove",
      }),
    );
    expect(await screen.findByRole("button", { name: "Set up for access" })).toBeInTheDocument();
    expect(host.calls).toEqual(["stop", "start", "remove"]);
  });

  it("lists who accessed this machine and warns when the access log was altered", async () => {
    const now = Date.now();
    const host = createFakeHostBridge({
      status: {
        ...sharing,
        recentAccess: [
          { atMs: now - 5_000, via: "relay", who: "Ada (acct-1)", what: "FilesystemService" },
          { atMs: now - 7_200_000, via: "token", who: "token", what: "ProcessService" },
        ],
        accessLogError: "line 2: altered",
      },
      onboarding: { completed: true },
    });
    render(<ThisMachinePanel host={host} />);
    const recent = await screen.findByRole("generic", { name: "Recent access" });
    expect(within(recent).getByText(/Ada \(acct-1\) · Files/)).toBeInTheDocument();
    expect(within(recent).getByText(/Access token · Terminal and processes/)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("The access log was changed outside Cua");
  });
});

describe("ThisMachinePanel: a spare machine", () => {
  it("sets up a spare machine: desktop private, Spaces on, limits shown", async () => {
    const host = createFakeHostBridge({ platform: "macos" });
    render(<ThisMachinePanel host={host} />);
    fireEvent.click(await screen.findByRole("button", { name: "Set up for access" }));
    fireEvent.click(screen.getByRole("radio", { name: "A spare machine for Spaces" }));
    expect(screen.getByText(/its desktop stays private/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    const desktop = await screen.findByRole("switch", { name: /Share this desktop/ });
    expect(desktop).not.toBeChecked();
    expect(screen.getByRole("switch", { name: /Provide Spaces/ })).toBeChecked();
    // The only setting on cannot be turned off.
    expect(screen.getByRole("switch", { name: /Provide Spaces/ })).toBeDisabled();
    expect(screen.getByText(/2 macOS VMs \(Apple’s license allows two per Mac\)/)).toBeInTheDocument();
    expect(screen.getByText("None yet")).toBeInTheDocument();
    // No desktop, so no screen permissions to grant.
    expect(screen.queryByText("Screen Recording")).toBeNull();
    fireEvent.click(desktop);
    await waitFor(() => expect(screen.getByRole("switch", { name: /Share this desktop/ })).toBeChecked());
    expect(host.calls.find((c) => c.startsWith("configure:"))).toMatch(/"shareDesktop":true/);
  });

  it("lists the Spaces it provides and every remote create or refusal", async () => {
    const now = Date.now();
    const host = createFakeHostBridge({
      status: {
        ...sharing,
        shareDesktop: false,
        provideSpaces: true,
        maxSpaces: 4,
        maxMacosVms: 2,
        providedSpaces: [
          {
            relayMachine: "space-1",
            localSpace: "local:mac-1",
            name: "mac-1",
            image: "ghcr.io/trycua/macos:26",
            os: "macos",
            kind: "vm",
            createdBy: "Ada",
            createdAtMs: now - 60_000,
          },
        ],
        spacesAudit: [
          { atMs: now - 10_000, action: "refused", who: "Bob", space: "space-2", detail: "already runs 2 macOS VMs" },
          { atMs: now - 60_000, action: "create", who: "Ada", space: "space-1", detail: "" },
        ],
        spacesAuditError: "line 3: altered",
      },
      onboarding: { completed: true },
    });
    render(<ThisMachinePanel host={host} />);
    const provided = await screen.findByRole("generic", { name: "Spaces for your devices" });
    expect(within(provided).getByText(/mac-1 · macOS · Ada/)).toBeInTheDocument();
    const activity = screen.getByRole("generic", { name: "Spaces activity" });
    expect(within(activity).getByText(/Refused space-2 · Bob · already runs 2 macOS VMs/)).toBeInTheDocument();
    expect(within(activity).getByText(/Created space-1 · Ada/)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Spaces activity log does not verify");
  });
});

describe("roster", () => {
  it("always lists This machine in the live roster, next to relay machines", async () => {
    const user = userEvent.setup();
    const host = createFakeHostBridge({ status: sharing, onboarding: { completed: true } });
    const fleet = fakeFleetBridge({
      isNative: true,
      listSpaces: async () => [
        {
          id: "space://relay/9999abcd0000",
          name: "office-pc",
          provider: "relay",
          spacesdVersion: "0.1.0",
          features: [],
          reachable: true,
          os: "windows",
        },
      ],
    });
    render(<App bridge={createFallbackBridge("notched")} fleet={fleet} host={host} now={() => FIXTURE_NOW} />);
    await user.click(await screen.findByRole("button", { name: /Spaces/ }));
    const listbox = await screen.findByRole("listbox");
    await waitFor(() => {
      const names = within(listbox)
        .getAllByRole("option")
        .filter((el) => !el.classList.contains("tile-new"))
        .map((el) => el.getAttribute("aria-label")?.split(",")[0]);
      expect([...names].sort()).toEqual(["Office Pc", "This machine"]);
    });
    // This machine's page opens in the main window, not inside the notch.
    await user.click(within(listbox).getByRole("option", { name: /This machine/ }));
    expect(screen.queryByRole("button", { name: "Stop sharing" })).toBeNull();
  });
});
