// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { approveOpen, approveView, devicesView, enrollInitial, enrollView, reduceEnroll, type DevicesInput } from "../model/devices";
import { createFakeDevicesBridge } from "../native/devices";
import { ApproveSheet, DevicesBanner, DevicesSection, EnrollSheet, RevokeSheet, useDevices } from "./Devices";

const NOW = 1_800_000_000;

const snapshot: DevicesInput = {
  devices: [
    { id: "dev_p", name: "Work laptop", state: "pending", platform: "windows" },
    {
      id: "dev_m",
      name: "MacBook Pro",
      state: "enrolled",
      current: true,
      enrolledUntil: NOW + 30 * 86_400,
      lastSeen: NOW - 120,
      platform: "macos",
    },
    { id: "dev_o", name: "Old laptop", state: "expired", enrolledUntil: NOW - 100, platform: "linux" },
  ],
  audit: [
    { ts: NOW - 300, kind: "machine_access", device: "dev_m", machine: "m1" },
    { ts: NOW - 200, kind: "shared_access", machine: "m1", subject: "bob@example.com" },
  ],
  localDeviceId: "dev_m",
  machineNames: { m1: "studio-mac" },
};

describe("devices model", () => {
  it("is the core's page, enroll and approve sheets", () => {
    const v = devicesView(snapshot, NOW);
    expect(v.enrolled).toBe(true);
    expect(v.thisDevice).toMatchObject({ kind: "enrolled", title: "Enrolled until", at: NOW + 30 * 86_400 });
    expect(v.rows.map((r) => r.title)).toEqual(["MacBook Pro (this device)", "Old laptop", "Work laptop"]);
    expect(v.recent.map((r) => r.text)).toEqual(["bob@example.com opened studio-mac", "MacBook Pro opened studio-mac"]);
    expect(v.approvals.map((a) => a.deviceId)).toEqual(["dev_p", "dev_o"]);
    let s = enrollInitial();
    s = reduceEnroll(s, { type: "choose", method: "approve" });
    s = reduceEnroll(s, { type: "registered", enrolled: false, code: "K7QX-M2RP" });
    expect(enrollView(s).code).toBe("K7QX-M2RP");
    expect(approveView(approveOpen(v.approvals[0]!)).needsCode).toBe(true);
  });
});

describe("Settings → Devices", () => {
  it("shows this device, the account's devices and recent access", () => {
    const v = devicesView(snapshot, NOW);
    const onRevoke = vi.fn();
    const onDeny = vi.fn();
    render(
      <DevicesSection
        onConfirmMachine={() => {}}
        view={v}
        error={null}
        signedIn
        nowMs={NOW * 1000}
        onEnroll={() => {}}
        onApprove={() => {}}
        onDeny={onDeny}
        onRename={() => {}}
        onRevoke={onRevoke}
      />,
    );
    expect(screen.getByTestId("this-device-status").textContent).toMatch(/^Enrolled until /);
    const devices = screen.getByRole("region", { name: "Your Devices" });
    expect(within(devices).getByText("MacBook Pro (this device)")).toBeInTheDocument();
    expect(within(devices).getByText(/macOS · Enrolled · re-verify in 30 days · Last seen/)).toBeInTheDocument();
    expect(within(devices).getAllByRole("button", { name: "Approve…" })).toHaveLength(2);
    const denyButtons = within(devices).getAllByRole("button", { name: "Deny" });
    expect(denyButtons).toHaveLength(2);
    // Rows sort this device first, then alphabetically: "Old laptop" (expired) before "Work laptop" (pending).
    fireEvent.click(denyButtons[1]!);
    expect(onDeny).toHaveBeenCalledWith(expect.objectContaining({ deviceId: "dev_p", expired: false }));
    fireEvent.click(within(devices).getAllByRole("button", { name: "Revoke…" })[0]!);
    expect(onRevoke).toHaveBeenCalledWith(expect.objectContaining({ id: "dev_m" }));
    const recent = screen.getByRole("region", { name: "Recent Access" });
    expect(within(recent).getByText("bob@example.com opened studio-mac")).toBeInTheDocument();
    expect(within(recent).getByLabelText("Not one of your devices")).toBeInTheDocument();
  });

  it("asks to enroll with the grace date and says so while signed out", () => {
    const v = devicesView({ devices: [], audit: [], enforceAfter: NOW + 5 * 86_400 }, NOW);
    const onEnroll = vi.fn();
    const { rerender } = render(
      <>
        <DevicesBanner view={v} onEnroll={onEnroll} />
        <DevicesSection
        onConfirmMachine={() => {}} view={v} error={null} signedIn nowMs={NOW * 1000} onEnroll={onEnroll} onApprove={() => {}} onDeny={() => {}} onRename={() => {}} onRevoke={() => {}} />
      </>,
    );
    expect(screen.getByRole("status", { name: "Device enrollment" }).textContent).toContain("within 5 days");
    expect(screen.getByTestId("this-device-status").textContent).toMatch(/grace ends/);
    fireEvent.click(screen.getAllByRole("button", { name: "Enroll…" })[0]!);
    expect(onEnroll).toHaveBeenCalled();
    rerender(
      <DevicesSection
        onConfirmMachine={() => {}} view={null} error={null} signedIn={false} nowMs={0} onEnroll={() => {}} onApprove={() => {}} onDeny={() => {}} onRename={() => {}} onRevoke={() => {}} />,
    );
    expect(screen.getByText(/Sign in to Cua/)).toBeInTheDocument();
  });

  it("confirms Revoke with the core's words", async () => {
    const v = devicesView(snapshot, NOW);
    const row = v.rows.find((r) => r.id === "dev_p")!;
    const onRevoke = vi.fn(async () => {});
    const onClose = vi.fn();
    render(<RevokeSheet row={row} onRevoke={onRevoke} onClose={onClose} />);
    expect(screen.getByText("Revoke “Work laptop”?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(onRevoke).toHaveBeenCalled();
  });
});

describe("enroll sheet", () => {
  it("approve from another device shows the code and waits for the approval", async () => {
    vi.useFakeTimers();
    try {
      const bridge = createFakeDevicesBridge({ approvedAfterChecks: 2 });
      const onEnrolled = vi.fn();
      render(<EnrollSheet bridge={bridge} signIn={async () => {}} onClose={() => {}} onEnrolled={onEnrolled} pollMs={10} />);
      fireEvent.click(screen.getByRole("button", { name: /Approve from another device/ }));
      await act(async () => {});
      expect(screen.getByLabelText("One-time code").textContent).toBe("K7QX-M2RP");
      expect(screen.getByRole("status").textContent).toBe("Waiting for approval…");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(50);
      });
      expect(onEnrolled).toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
      expect(bridge.calls.filter((c) => c === "check")).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("sign in again enrolls the first device; a failed sign-in can go back", async () => {
    const bridge = createFakeDevicesBridge({ firstDevice: true });
    const onEnrolled = vi.fn();
    const signIn = vi.fn(async () => {});
    const { unmount } = render(<EnrollSheet bridge={bridge} signIn={signIn} onClose={() => {}} onEnrolled={onEnrolled} />);
    fireEvent.click(screen.getByRole("button", { name: /Sign in again/ }));
    await waitFor(() => expect(onEnrolled).toHaveBeenCalled());
    expect(signIn).toHaveBeenCalled();
    expect(bridge.calls).toEqual(["enroll"]);
    unmount();
    render(
      <EnrollSheet
        bridge={bridge}
        signIn={async () => {
          throw new Error("sign-in was cancelled");
        }}
        onClose={() => {}}
        onEnrolled={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Sign in again/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("sign-in was cancelled");
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("button", { name: /Approve from another device/ })).toBeInTheDocument();
  });
});

describe("approval sheet", () => {
  const prompt = devicesView(snapshot, NOW).approvals[0]!;

  it("needs the whole code, and a refused presence shows why", async () => {
    const bridge = createFakeDevicesBridge({ snapshot, presenceFails: "authentication was cancelled" });
    const onDone = vi.fn();
    render(<ApproveSheet bridge={bridge} prompt={prompt} onClose={() => {}} onDone={onDone} />);
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(approve).toBeDisabled();
    fireEvent.change(screen.getByPlaceholderText("XXXX-XXXX"), { target: { value: "k7qx m2rp" } });
    expect(screen.getByPlaceholderText("XXXX-XXXX")).toHaveValue("K7QX-M2RP");
    expect(approve).toBeEnabled();
    fireEvent.click(approve);
    expect(await screen.findByRole("alert")).toHaveTextContent("authentication was cancelled");
    expect(bridge.calls).toContain("approve:K7QX-M2RP:");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("offers approving the sole matching device by id once the code expires", async () => {
    const base = createFakeDevicesBridge({ snapshot });
    let attempt = 0;
    const bridge: typeof base = {
      ...base,
      approve: async (request) => {
        attempt += 1;
        if (attempt === 1) {
          throw new Error(
            "not found: relay: no device is waiting with the code Z9WY-4TPN (codes expire after 10 minutes); " +
              "approve by id instead: `cua devices approve dev_p` (ids in `cua devices ls`)",
          );
        }
        return base.approve(request);
      },
    };
    const onDone = vi.fn();
    render(
      <ApproveSheet bridge={bridge} prompt={prompt} devices={snapshot.devices} onClose={() => {}} onDone={onDone} />,
    );
    fireEvent.change(screen.getByPlaceholderText("XXXX-XXXX"), { target: { value: "z9wy4tpn" } });
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The code expired.");
    expect(screen.getByText(/by ID instead/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Approve by ID" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(base.calls).toContain("approve::dev_p");
  });

  it("approves after presence, or denies in one click (revoking the device)", async () => {
    const bridge = createFakeDevicesBridge({ snapshot });
    const onDone = vi.fn();
    const { unmount } = render(<ApproveSheet bridge={bridge} prompt={prompt} onClose={() => {}} onDone={onDone} />);
    fireEvent.change(screen.getByPlaceholderText("XXXX-XXXX"), { target: { value: "K7QXM2RP" } });
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    unmount();
    render(<ApproveSheet bridge={bridge} prompt={prompt} onClose={() => {}} onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(2));
    expect(bridge.calls).toContain("revoke:dev_p");
  });

  it("re-verification needs no code, and Not Now only dismisses", async () => {
    const expired = devicesView(snapshot, NOW).approvals[1]!;
    const bridge = createFakeDevicesBridge({ snapshot, needsPassphrase: true });
    const onClose = vi.fn();
    render(<ApproveSheet bridge={bridge} prompt={expired} onClose={onClose} onDone={() => {}} />);
    expect(screen.queryByPlaceholderText("XXXX-XXXX")).toBeNull();
    // No OS prompt on this system: the Keyvault passphrase is asked for.
    const passphrase = await screen.findByLabelText("Keyvault passphrase");
    expect(screen.getByRole("button", { name: "Approve" })).toBeDisabled();
    fireEvent.change(passphrase, { target: { value: "hunter2" } });
    expect(screen.getByRole("button", { name: "Approve" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Not Now" }));
    expect(onClose).toHaveBeenCalled();
    expect(bridge.calls.some((c) => c.startsWith("revoke"))).toBe(false);
  });
});

describe("approval watch", () => {
  function Probe({ bridge }: { bridge: ReturnType<typeof createFakeDevicesBridge> }) {
    const d = useDevices(bridge, true, () => NOW * 1000);
    return <p data-testid="approving">{d.approving?.deviceId ?? "none"}</p>;
  }

  it("notifies once per new device and opens its sheet", async () => {
    const bridge = createFakeDevicesBridge({ snapshot });
    render(<Probe bridge={bridge} />);
    await waitFor(() => expect(screen.getByTestId("approving").textContent).toBe("dev_p"));
    const notes = bridge.calls.filter((c) => c.startsWith("notify:"));
    expect(notes).toEqual([
      "notify:Approve “Work laptop”?:A device signed in to your Cua account and asks to reach your machines.",
      "notify:Re-verify “Old laptop”?:It needs one approval to keep reaching your machines.",
    ]);
  });

  it("stays quiet signed out", async () => {
    const bridge = createFakeDevicesBridge({ snapshot });
    function Quiet() {
      const d = useDevices(bridge, false, () => NOW * 1000);
      return <p>{d.view ? "view" : "none"}</p>;
    }
    render(<Quiet />);
    await act(async () => {});
    expect(bridge.calls).toEqual([]);
  });
});
