// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { SpaceRow } from "../../model/spaces";
import { createFakeHostBridge } from "../../native/host";
import { createFakeInstallerBridge } from "../../native/installer";
import type { CreateProgress, SpaceCreateConfig } from "../../native/fleet";
import type { LocalStatus } from "../../native/local";
import { fakeFleetBridge } from "../../test/fakeFleet";
import { MainWindow } from "./MainWindow";
import { createFakeCloudBridge, type CloudBridge } from '../../native/cloud';

const ROWS: SpaceRow[] = [
  { id: "cloud:aurora", name: "aurora", provider: "cloud", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true },
  { id: "local:cua-e2e-vm", name: "cua-e2e-vm", provider: "local", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true },
];

const LOCAL: LocalStatus = { available: true, backends: ["container", "qemu"], containerImage: "", macosImage: null, error: null };

function setup(
  overrides: Parameters<typeof fakeFleetBridge>[0] = {},
  onboarded = true,
  cloud: CloudBridge = createFakeCloudBridge(),
) {
  const createSpace = vi.fn(async (config?: SpaceCreateConfig) => (config?.on === "cloud" ? ROWS[0]! : ROWS[1]!));
  const setDefaultLocation = vi.fn(async (on: "local" | "cloud") => ({ value: on, source: "config" as const, path: "/tmp/cua/config.toml" }));
  const fleet = fakeFleetBridge({
    isNative: true,
    listSpaces: async () => ROWS,
    localStatus: async () => LOCAL,
    status: async () => ({ configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" }),
    screenshot: async () => "data:image/png;base64,AAAA",
    createSpace,
    setDefaultLocation,
    ...overrides,
  });
  const host = createFakeHostBridge({ onboarding: { completed: onboarded, mode: onboarded ? "client" : null } });
  const installer = createFakeInstallerBridge({ plan: { installed: true, upToDate: true, onPath: true } });
  render(<MainWindow fleet={fleet} host={host} installer={installer} cloud={cloud} now={() => 1_000} />);
  return { fleet, host, installer, createSpace, setDefaultLocation };
}

describe("MainWindow", () => {
  it("first run shows the welcome flow in the window, then the Spaces", async () => {
    const { host } = setup({}, false);
    fireEvent.click(await screen.findByRole("button", { name: "Get started" }));
    // Already signed in (the fleet status names the account): just continue.
    await screen.findByText("Signed in as ada@example.com.");
    fireEvent.click(await screen.findByRole("button", { name: "Continue" }));
    await screen.findByRole("heading", { name: "AI agents" });
    fireEvent.click(await screen.findByRole("button", { name: "Skip" }));
    await screen.findByRole("heading", { name: "Where should Cua Spaces show up?" });
    fireEvent.click(await screen.findByRole("button", { name: "Continue" }));
    // No Cua Volume page without its experiment (Settings, Experiments).
    expect(screen.queryByRole("heading", { name: "Cua Volume" })).toBeNull();
    fireEvent.click(await screen.findByText("Access other machines"));
    fireEvent.click(await screen.findByRole("button", { name: "Start using Cua Spaces" }));
    expect(await screen.findByRole("listbox", { name: "Cua Cloud" })).toBeInTheDocument();
    expect(host.state.onboarding).toMatchObject({ completed: true, mode: "client" });
  });

  it("lists Spaces by where they run, with This machine first", async () => {
    setup();
    const cloud = await screen.findByRole("listbox", { name: "Cua Cloud" });
    expect(within(cloud).getByRole("option", { name: /Aurora/ })).toBeInTheDocument();
    expect(within(screen.getByRole("listbox", { name: "This Mac" })).getByRole("option", { name: /Cua E2e Vm/ })).toBeInTheDocument();
    expect(within(screen.getByRole("listbox", { name: "This machine" })).getByRole("option")).toBeInTheDocument();
    // Each row leads with its OS mark (the notch tiles' icon); This machine is this Mac.
    const aurora = within(cloud).getByRole("option", { name: /Aurora/ });
    expect(aurora.querySelector("[data-os-icon]")).not.toBeNull();
    const host = within(screen.getByRole("listbox", { name: "This machine" })).getByRole("option");
    expect(host.querySelector('[data-os-icon="os-macos"]')).not.toBeNull();
  });

  it("shows the selected Space's detail and live preview", async () => {
    setup();
    fireEvent.click(await screen.findByRole("option", { name: /Aurora/ }));
    expect(await screen.findByRole("heading", { level: 1, name: "Aurora" })).toBeInTheDocument();
    expect(await screen.findByAltText("Aurora desktop")).toHaveAttribute("src", "data:image/png;base64,AAAA");
    // The facts are the core's: no Location row (the sidebar section says where it runs).
    expect(screen.getByLabelText("Details")).toHaveTextContent("cloud:aurora");
    expect(screen.getByLabelText("Details")).not.toHaveTextContent("Cua Cloud");
  });

  it("New Space (Cmd+N) runs the wizard and creates a local VM from the shared image list", async () => {
    const { createSpace } = setup();
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.keyDown(window, { key: "n", metaKey: true });
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Image" }), {
      target: { value: "ghcr.io/trycua/linux:24.04-disk" },
    });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "local" } });
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await waitFor(() =>
      expect(createSpace).toHaveBeenCalledWith(
        {
        image: "ghcr.io/trycua/linux:24.04-disk",
        on: "local",
        kind: "vm",
        runtime: "auto",
        name: undefined,
        cpus: 2,
        memoryMb: 4096,
        spacesd: true,
      },
        expect.stringMatching(/^pending:/),
      ),
    );
    expect(screen.queryByRole("dialog", { name: "New Space" })).toBeNull();
  });

  it("runs on this Mac, offers no cloud without the Your cloud experiment, never Cua Cloud", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    expect(within(within(dialog).getByRole("combobox", { name: "Run on" })).getAllByRole("option").map((o) => o.textContent)).toEqual(["This Mac"]);
    expect(within(dialog).queryByRole("button", { name: /Connect a cloud/ })).toBeNull();
    expect(within(dialog).queryByText(/Cua Cloud/)).toBeNull();
  });

  it("connects a cloud from the wizard, tests it, then creates there", async () => {
    // With the Your cloud experiment (Settings, Experiments).
    window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ yourCloud: true }));
    const cloud = createFakeCloudBridge();
    const { createSpace } = setup({}, true, cloud);
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.click(within(dialog).getByRole("button", { name: /Connect a cloud/ }));
    const sheet = await screen.findByRole("dialog", { name: "Connect a cloud" });
    expect(await within(sheet).findByRole("radio", { name: /AWS/ })).toBeChecked();
    fireEvent.click(within(sheet).getByRole("button", { name: "Test" }));
    expect(await within(sheet).findByText(/Nothing was created/)).toBeInTheDocument();
    expect(cloud.calls).toContain("test:aws");
    fireEvent.click(within(sheet).getByRole("checkbox", { name: "Make default" }));
    fireEvent.click(within(sheet).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Connect a cloud" })).toBeNull());
    expect(cloud.calls).toContain("connect:aws:true");
    await waitFor(() =>
      expect(within(within(dialog).getByRole("combobox", { name: "Run on" })).getAllByRole("option").map((o) => o.getAttribute("value"))).toContain("aws"),
    );
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "aws" } });
    window.localStorage.removeItem("cua.settings.experiments");
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await waitFor(() =>
      expect(createSpace).toHaveBeenCalledWith(expect.objectContaining({ on: "aws" }), expect.stringMatching(/^pending:/)),
    );
  });
  it("Run on starts on this Mac even with a cloud default", async () => {
    const { createSpace } = setup({
      defaultLocation: async () => ({ value: "cloud", source: "config", path: "/tmp/cua/config.toml" }),
    });
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    await waitFor(() =>
      expect(within(dialog).getByRole("combobox", { name: "Run on" })).toHaveValue("local"),
    );
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await waitFor(() =>
      expect(createSpace).toHaveBeenCalledWith(expect.objectContaining({ on: "local" }), expect.stringMatching(/^pending:/)),
    );
  });
  it("the advanced runtime choice offers only what the image can run where it runs", async () => {
    const { createSpace } = setup();
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "local" } });
    fireEvent.click(within(dialog).getByRole("button", { name: /Advanced/ }));
    const runtime = () => within(dialog).getByRole("combobox", { name: "Runtime" });
    const options = () => within(runtime()).getAllByRole("option").map((o) => o.getAttribute("value"));
    expect(options()).toEqual(["auto", "gvisor", "runc"]);
    // The VM kind switches to the image's -disk variant: QEMU here.
    fireEvent.click(within(dialog).getByRole("button", { name: "Virtual machine" }));
    expect(within(dialog).getByRole("combobox", { name: "Image" })).toHaveValue("ghcr.io/trycua/linux:24.04-disk");
    expect(options()).toEqual(["auto", "qemu"]);
    // Back to a container, pinned to runc.
    fireEvent.click(within(dialog).getByRole("button", { name: "Container" }));
    fireEvent.change(runtime(), { target: { value: "runc" } });
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await waitFor(() =>
      expect(createSpace).toHaveBeenCalledWith(
        expect.objectContaining({ image: "ghcr.io/trycua/linux:24.04", on: "local", kind: "container", runtime: "runc" }),
        expect.stringMatching(/^pending:/),
      ),
    );
  });

  it("macOS images never offer the cloud or a non-Lume engine", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Image" }), {
      target: { value: "ghcr.io/trycua/macos:26" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /Advanced/ }));
    const runtime = within(dialog).getByRole("combobox", { name: "Runtime" });
    expect(within(runtime).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["auto", "lume"]);
    expect(within(dialog).getByRole("button", { name: "Container" })).toBeDisabled();
  });

  it("shows a new Space at once, selected, with its progress", async () => {
    let finish: (row: SpaceRow) => void = () => {};
    let pendingId = "";
    const progress: ((p: CreateProgress) => void)[] = [];
    setup({
      createSpace: (_config, id) => {
        pendingId = id ?? "";
        return new Promise<SpaceRow>((resolve) => (finish = resolve));
      },
      onCreateProgress: async (handler) => {
        progress.push(handler);
        return () => {};
      },
    });
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "local" } });
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    // Before the create resolves: the row, selected, at 2%, and its detail.
    const row = await screen.findByRole("option", { name: /New Space/ });
    expect(row).toHaveAttribute("aria-selected", "true");
    expect(row).toHaveTextContent("1%");
    expect(await screen.findByRole("heading", { level: 1, name: "New Space" })).toBeInTheDocument();
    expect(pendingId).toMatch(/^pending:/);
    // The SDK's pull moves it.
    act(() => progress.forEach((h) => h({ pendingId, phase: "pulling", fraction: 0.5, detail: "" })));
    await waitFor(() => expect(screen.getByRole("option", { name: /New Space/ })).toHaveTextContent("42%"));
    // Ready: it gives way to the registry's row.
    act(() => finish(ROWS[1]!));
    await waitFor(() => expect(screen.queryByRole("option", { name: /New Space/ })).toBeNull());
  });

  /** Starts a local create from the wizard; resolves with its pending id and
   * the progress handlers. */
  async function startPendingCreate(overrides: Parameters<typeof fakeFleetBridge>[0]) {
    let pendingId = "";
    let settle: { resolve: (row: SpaceRow) => void; reject: (e: Error) => void } | null = null;
    const progress: ((p: CreateProgress) => void)[] = [];
    const ctx = setup({
      createSpace: (_config, id) => {
        pendingId = id ?? "";
        return new Promise<SpaceRow>((resolve, reject) => (settle = { resolve, reject }));
      },
      onCreateProgress: async (handler) => {
        progress.push(handler);
        return () => {};
      },
      ...overrides,
    });
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "local" } });
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await screen.findByRole("option", { name: /New Space/ });
    const emit = (p: Omit<CreateProgress, "pendingId" | "detail">) =>
      act(() => progress.forEach((h) => h({ pendingId, detail: "", ...p })));
    return { ...ctx, pendingId: () => pendingId, emit, settle: () => settle! };
  }

  it("shows a download's bytes, rate and time left under the bar", async () => {
    const { emit } = await startPendingCreate({});
    emit({
      phase: "pulling",
      fraction: 0.1757,
      bytesDone: 4_200_000_000,
      bytesTotal: 23_900_000_000,
      bytesPerSecond: 85_000_000,
    });
    expect(await screen.findByText("3.9 of 22.3 GB \u00b7 81 MB/s \u00b7 about 4 min")).toBeInTheDocument();
    // Past the download the line goes.
    emit({ phase: "booting", fraction: null });
    await waitFor(() => expect(screen.queryByText(/of 22\.3 GB/)).toBeNull());
  });

  it("Cancel stops a create: Cancelling, then the row goes without a failure", async () => {
    let done: () => void = () => {};
    const cancelCreate = vi.fn(
      () => new Promise<{ id: string; state: "cancelled"; message: string }>((resolve) => {
        done = () => resolve({ id: "local:space-1", state: "cancelled", message: "Cancelled" });
      }),
    );
    const { pendingId, settle } = await startPendingCreate({ cancelCreate });
    const cancel = await screen.findByRole("button", { name: "Cancel" });
    expect(cancel).toBeEnabled();
    fireEvent.click(cancel);
    expect(cancelCreate).toHaveBeenCalledWith(pendingId());
    // The button keeps its label, disabled; the preview says Cancelling.
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled());
    expect(screen.getByText("Cancelling\u2026")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /New Space/ })).toBeInTheDocument();
    // The SDK cleans up; the create ends with its Cancelled error.
    act(() => settle().reject(new Error("cancelled: Cancelled local:space-1; removed its VM or container.")));
    await act(async () => done());
    await waitFor(() => expect(screen.queryByRole("option", { name: /New Space/ })).toBeNull());
    expect(screen.queryByText(/Failed/)).toBeNull();
    expect(screen.queryByRole("option", { name: /cancelled/i })).toBeNull();
  });

  it("a cancelled create's error never shows as a failure, even from another client", async () => {
    const { settle } = await startPendingCreate({});
    act(() => settle().reject(new Error("cancelled: Cancelled local:space-1.")));
    await waitFor(() => expect(screen.queryByRole("option", { name: /New Space/ })).toBeNull());
  });

  it("a cancel that fails says so on the row", async () => {
    const cancelCreate = vi.fn(async () => Promise.reject(new Error("timed out: the cua daemon did not answer")));
    await startPendingCreate({ cancelCreate });
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("option", { name: /the cua daemon did not answer/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("New Space offers the GPU the shell reports and creates with it", async () => {
    const gpuSupport = vi.fn(async () => [
      {
        runtime: "lume",
        id: "paravirtual",
        label: "GPU acceleration",
        experimental: true,
        supported: true,
        learnMore: "https://cua.ai/docs/lume/guides/gpu-passthrough",
      },
    ]);
    const { createSpace } = setup({
      gpuSupport,
      localStatus: async () => ({ ...LOCAL, backends: ["container", "lume"] }),
    });
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Image" }), {
      target: { value: "ghcr.io/trycua/macos:26" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(await within(dialog).findByRole("checkbox", { name: "GPU acceleration (Experimental)" }));
    for (let i = 0; i < 2; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    await waitFor(() =>
      expect(createSpace).toHaveBeenCalledWith(
        expect.objectContaining({ runtime: "auto", gpu: "paravirtual" }),
        expect.stringMatching(/^pending:/),
      ),
    );
  });

  it("shows a failed create on its row until it is removed", async () => {
    setup({ createSpace: async () => Promise.reject(new Error("docker is not running")) });
    fireEvent.click(await screen.findByRole("button", { name: "New Space" }));
    const dialog = await screen.findByRole("dialog", { name: "New Space" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Run on" }), { target: { value: "local" } });
    for (let i = 0; i < 3; i++) fireEvent.click(within(dialog).getByRole("button", { name: "Continue" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Space" }));
    const row = await screen.findByRole("option", { name: /docker is not running/ });
    expect(row).toHaveTextContent("New Space");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("the toolbar is the core's, in order, and This machine has its own page", async () => {
    setup();
    fireEvent.click(await screen.findByRole("option", { name: /Aurora/ }));
    await screen.findByRole("heading", { level: 1, name: "Aurora" });
    const labels = [...document.querySelectorAll(".dw-toolbar-actions button")].map(
      (b) => b.getAttribute("aria-label") ?? b.textContent,
    );
    // No Share without the Sharing experiment (Settings, Experiments).
    expect(labels).toEqual(["Teleport an app", "Picture in picture", "Delete Space", "Open"]);
    expect(screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual([
      "Stream",
      "Agents",
      "Teleport",
    ]);
    fireEvent.click(within(screen.getByRole("listbox", { name: "This machine" })).getByRole("option"));
    fireEvent.click(await screen.findByRole("button", { name: "Set up for access" }));
    expect(await screen.findByText("Set up this machine")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Set up for access" }));
    expect(await screen.findByRole("button", { name: "Stop sharing" })).toBeInTheDocument();
    await waitFor(() =>
      expect(within(screen.getByRole("listbox", { name: "This machine" })).getByRole("option")).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
  });

  it("the sidebar foot carries the account and Settings", async () => {
    setup();
    await screen.findByRole("option", { name: /Aurora/ });
    await waitFor(() => expect(document.querySelector(".dw-account")).toHaveTextContent("ada@example.com"));
    expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument();
  });

  it("deleting a Space asks first", async () => {
    const deleteSpace = vi.fn(async () => "Deleted cloud:aurora");
    setup({ deleteSpace });
    fireEvent.click(await screen.findByRole("option", { name: /Aurora/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete Space" }));
    expect(deleteSpace).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog", { name: "Delete Aurora?" })).toHaveTextContent("Its sandbox is deleted.");
    fireEvent.click(screen.getAllByRole("button", { name: "Delete Space" }).at(-1)!);
    await waitFor(() => expect(deleteSpace).toHaveBeenCalledWith("cloud:aurora"));
  });

  it("a Space in your cloud shows where it runs and offers Delete Permanently or Remove from List", async () => {
    const cloudRows: SpaceRow[] = [
      ...ROWS,
      { id: "relay:cloud-0000000000000a01", name: "research-box", provider: "relay", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true, cloud: "aws", cloudPlace: "AWS · us-west-2", cloudDelete: "here" },
      { id: "relay:cloud-0000000000000e01", name: "their-box", provider: "relay", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true, cloud: "gcp", cloudPlace: "Google Cloud · us-central1", cloudDelete: "elsewhere" },
    ];
    const deleteSpace = vi.fn(async () => "Deleted relay:cloud-0000000000000a01");
    const removeSpace = vi.fn(async () => {});
    setup({ listSpaces: async () => cloudRows, deleteSpace, removeSpace });
    const mine = await screen.findByRole("option", { name: /Research Box/ });
    expect(mine).toHaveTextContent("AWS · us-west-2");
    fireEvent.click(mine);
    expect(await screen.findByText("Location")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Delete Space" }));
    const ask = screen.getByRole("alertdialog", { name: "Delete Research Box?" });
    expect(ask).toHaveTextContent("everything Cua created for it in AWS · us-west-2");
    fireEvent.click(within(ask).getByRole("button", { name: "Delete Permanently" }));
    await waitFor(() => expect(deleteSpace).toHaveBeenCalledWith("relay:cloud-0000000000000a01"));
    expect(removeSpace).not.toHaveBeenCalled();

    // Another device created it: only Remove from List.
    fireEvent.click(await screen.findByRole("option", { name: /Their Box/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete Space" }));
    const theirs = screen.getByRole("alertdialog", { name: "Delete Their Box?" });
    const permanent = within(theirs).getByRole("button", { name: "Delete Permanently" });
    expect(permanent).toBeDisabled();
    expect(permanent).toHaveAttribute("title", "Created on another device: delete it there, or remove it from this list.");
    fireEvent.click(within(theirs).getByRole("button", { name: "Remove from List" }));
    await waitFor(() => expect(removeSpace).toHaveBeenCalledWith("relay:cloud-0000000000000e01"));
    expect(deleteSpace).toHaveBeenCalledTimes(1);
  });

  it("a confirmed delete shows Deleting at once and cannot be sent twice", async () => {
    let finish: (v: string) => void = () => {};
    const deleteSpace = vi.fn(() => new Promise<string>((resolve) => (finish = resolve)));
    setup({ deleteSpace });
    fireEvent.click(await screen.findByRole("option", { name: /Aurora/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Delete Space" }));
    fireEvent.click(screen.getAllByRole("button", { name: "Delete Space" }).at(-1)!);
    await waitFor(() => expect(deleteSpace).toHaveBeenCalledTimes(1));
    // Before the SDK returns: the row and the detail say Deleting.
    expect(await screen.findByRole("option", { name: /Deleting…/ })).toBeInTheDocument();
    const again = screen.getByRole("button", { name: "Delete Space" });
    expect(again).toBeDisabled();
    fireEvent.click(again);
    expect(deleteSpace).toHaveBeenCalledTimes(1);
    finish("Deleted cloud:aurora");
    await waitFor(() => expect(screen.queryByRole("option", { name: /Aurora/ })).not.toBeInTheDocument());
  });

  it("the power button next to Delete suspends, waits, and shows a failure inline", async () => {
    let fail: (e: Error) => void = () => {};
    const setSpacePower = vi.fn(
      () => new Promise<never>((_, reject) => (fail = reject)),
    );
    const box: SpaceRow = {
      id: "local:box", name: "box", provider: "local", spacesdVersion: "0.4.0", features: [],
      os: "linux", reachable: true, power: "suspend", powerState: "running",
    };
    setup({ setSpacePower, listSpaces: async () => [...ROWS, box] });
    fireEvent.click(await screen.findByRole("option", { name: /Box/ }));
    // The toolbar's power button sits right before Delete; the row has one too.
    const toolbar = document.querySelector(".dw-toolbar-actions")!;
    const labels = [...toolbar.querySelectorAll("button")].map((b) => b.getAttribute("aria-label") ?? b.textContent);
    expect(labels).toEqual(["Teleport an app", "Picture in picture", "Suspend", "Delete Space", "Open"]);
    expect(screen.getAllByRole("button", { name: "Suspend" })).toHaveLength(2);
    // Aurora (cloud) cannot be turned off: no power button on its row.
    expect(document.querySelectorAll(".dw-row-power")).toHaveLength(1);

    fireEvent.click(screen.getAllByRole("button", { name: "Suspend" })[0]!);
    await waitFor(() => expect(setSpacePower).toHaveBeenCalledWith("local:box", false));
    // While it runs: Suspending on the row, the buttons wait.
    expect(await screen.findByRole("option", { name: /Box/ })).toHaveTextContent("Box");
    const busy = screen.getAllByRole("button", { name: "Suspending…" });
    expect(busy).toHaveLength(2);
    for (const b of busy) expect(b).toBeDisabled();
    fireEvent.click(busy[0]!);
    expect(setSpacePower).toHaveBeenCalledTimes(1);

    fail(new Error("Docker is not running"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not turn it off: Docker is not running",
    );
    expect(screen.getAllByRole("button", { name: "Suspend" })[0]).toBeEnabled();
  });

  it("Settings is a page of this window (Cmd+,)", async () => {
    setup();
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    expect(await screen.findByRole("heading", { level: 1, name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "Spaces tab in the notch" })).toBeInTheDocument();
  });

  it("Settings has no default-location choice and opens the Teams waitlist", async () => {
    const openExternal = vi.fn(async () => {});
    setup({ openExternal });
    await screen.findByRole("option", { name: /Aurora/ });
    fireEvent.keyDown(window, { key: ",", metaKey: true });
    await screen.findByRole("radiogroup", { name: "Spaces tab in the notch" });
    expect(screen.queryByRole("radiogroup", { name: "New Spaces run on" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Join the waitlist" }));
    expect(openExternal).toHaveBeenCalledWith("https://cua.ai/teams");
  });
});

describe("MainWindow launch at login", () => {
  async function spareHost() {
    const host = createFakeHostBridge({ onboarding: { completed: true, mode: "host" } });
    await host.setup({ mode: "relay", profile: "spare" } as Parameters<typeof host.setup>[0]);
    return host;
  }

  it("turns it on at launch for an install that never chose and provides Spaces", async () => {
    const { fakeLoginItemBridge } = await import("../../native/loginItem");
    const { readLaunchChoice } = await import("../../model/loginItem");
    window.localStorage.clear();
    const loginItem = fakeLoginItemBridge("notRegistered");
    const host = await spareHost();
    const fleet = fakeFleetBridge({ isNative: true, listSpaces: async () => ROWS, localStatus: async () => LOCAL });
    render(<MainWindow fleet={fleet} host={host} installer={createFakeInstallerBridge()} loginItem={loginItem} now={() => 1_000} />);
    await waitFor(() => expect(loginItem.calls).toEqual([true]));
    expect(readLaunchChoice()).toBe(true);
  });

  it("leaves a choice alone", async () => {
    const { fakeLoginItemBridge } = await import("../../native/loginItem");
    const { writeLaunchChoice } = await import("../../model/loginItem");
    window.localStorage.clear();
    writeLaunchChoice(false);
    const loginItem = fakeLoginItemBridge("notRegistered");
    const statusRead = vi.spyOn(loginItem, "status");
    const host = await spareHost();
    const fleet = fakeFleetBridge({ isNative: true, listSpaces: async () => ROWS, localStatus: async () => LOCAL });
    render(<MainWindow fleet={fleet} host={host} installer={createFakeInstallerBridge()} loginItem={loginItem} now={() => 1_000} />);
    await waitFor(() => expect(statusRead).toHaveBeenCalled());
    await act(async () => {});
    expect(loginItem.calls).toEqual([]);
  });
});
