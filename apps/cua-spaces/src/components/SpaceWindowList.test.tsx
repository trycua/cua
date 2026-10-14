// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { RemoteWindow } from "../model/teleport";
import type { TeleportBridge } from "../native/teleport";
import { SpaceWindowList } from "./SpaceWindowList";

const SPACE = { id: "local:aurora", os: "linux" as const, osName: "Ubuntu 24.04" };

const LONG_TITLE = "Quarterly planning notes for the Spaces launch, draft 7 (unsaved changes) - gedit";

const remoteWindows: RemoteWindow[] = [
  { id: "w-1", appName: "Firefox", title: "Cua Docs", visible: true, appId: "firefox", targetEpoch: 1, pid: 812 },
  { id: "w-2", appName: "Firefox", title: "Release notes", visible: true, appId: "firefox", targetEpoch: 1, pid: 812 },
  { id: "w-3", appName: "xterm", title: "", visible: true, appId: "xterm", targetEpoch: 0, pid: 904 },
  { id: "w-4", appName: "gedit", title: LONG_TITLE, visible: true, appId: "org.gnome.gedit", targetEpoch: 2 },
  // The driver's own capture target: the guest's screen, not an app window.
  {
    id: "w-screen",
    appName: "Cua Driver Local",
    title: "",
    visible: true,
    appId: "cua-driver-local",
    targetEpoch: 1,
    widthPx: 1024,
    heightPx: 768,
  },
];

function fakeTeleport(overrides: Partial<TeleportBridge> = {}): TeleportBridge {
  return {
    isNative: true,
    manifest: async () => {
      throw new Error("unused");
    },
    push: async () => {
      throw new Error("unused");
    },
    listOpenWindows: async () => [],
    captureThumbnail: async () => null,
    appIcon: async () => null,
    spaceAppIcon: async () => null,
    spacePrimaryDisplay: async () => ({ widthPx: 1280, heightPx: 800 }),
    spaceAppIcons: async (_s: string, requests: unknown[]) => requests.map(() => null),
    listRemoteWindows: async () => remoteWindows,
    listSpaceAgents: async () => [],
    remoteWindowThumbnail: async () => null,
    streamRemoteWindow: vi.fn(async () => {}),
    openPicker: async () => {},
    pickerConfig: async () => {
      throw new Error("unused");
    },
    closePicker: async () => {},
    ...overrides,
  };
}

function renderList(teleport: TeleportBridge = fakeTeleport()) {
  const onPipDesktop = vi.fn();
  const onPipWindow = vi.fn();
  render(<SpaceWindowList space={SPACE} teleport={teleport} onPipDesktop={onPipDesktop} onPipWindow={onPipWindow} />);
  return { onPipDesktop, onPipWindow };
}

const rowOf = (label: string) => screen.getByText(label).closest("li") as HTMLElement;

describe("SpaceWindowList (the Stream section)", () => {
  it("draws the core's rows: Desktop with the display's resolution, then each window by title", async () => {
    renderList();
    await screen.findByText("Cua Docs");
    expect(await screen.findByText("Desktop (1280×800)")).toBeInTheDocument();
    const labels = Array.from(document.querySelectorAll(".ssr-label")).map((e) => e.textContent);
    expect(labels).toEqual(["Desktop (1280×800)", "Cua Docs", "Release notes", "xterm", LONG_TITLE]);
    // The screen target is the Desktop row, not a window row.
    expect(screen.queryByText("Cua Driver Local")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("shows plain Desktop until the display list answers", async () => {
    renderList(fakeTeleport({ spacePrimaryDisplay: async () => null }));
    await screen.findByText("Cua Docs");
    expect(screen.getByText("Desktop")).toBeInTheDocument();
  });

  it("puts the full label in the tooltip", async () => {
    renderList();
    const label = await screen.findByText(LONG_TITLE);
    expect(label).toHaveAttribute("title", LONG_TITLE);
    expect(label).toHaveClass("ssr-label");
  });

  it("asks the SDK for every app's icon in one call, and shows none when it has none", async () => {
    const spaceAppIcons = vi.fn(async (_s: string, requests: { appName: string }[]) =>
      requests.map((r) => (r.appName === "Firefox" ? "data:image/png;base64,ZmY=" : null)),
    );
    renderList(fakeTeleport({ spaceAppIcons }));
    await screen.findByText("Cua Docs");
    await waitFor(() => expect(within(rowOf("Cua Docs")).queryByTestId("ssr-app-icon")).not.toBeNull());
    expect(spaceAppIcons).toHaveBeenCalledTimes(1);
    expect(spaceAppIcons).toHaveBeenCalledWith(SPACE.id, [
      { appName: "Firefox", appId: "firefox", pid: 812 },
      { appName: "xterm", appId: "xterm", pid: 904 },
      { appName: "gedit", appId: "org.gnome.gedit", pid: 0 },
    ]);
    expect(within(rowOf("xterm")).queryByTestId("ssr-app-icon")).toBeNull();
    expect(within(rowOf("xterm")).queryByRole("img")).toBeNull();
    // The Desktop row carries the Space's OS mark.
    expect(rowOf("Desktop (1280×800)").querySelector('[data-os-icon="os-ubuntu"]')).not.toBeNull();
  });

  it("gives every row a Picture in picture icon button", async () => {
    renderList();
    await screen.findByText("Cua Docs");
    const buttons = screen.getAllByRole("button", { name: "Picture in picture" });
    expect(buttons).toHaveLength(5);
    expect(buttons[0]).toHaveAttribute("title", "Picture in picture");
    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
  });

  it("the Desktop row's button pins the desktop", async () => {
    const { onPipDesktop, onPipWindow } = renderList();
    await screen.findByText("Cua Docs");
    fireEvent.click(within(rowOf("Desktop (1280×800)")).getByRole("button", { name: "Picture in picture" }));
    expect(onPipDesktop).toHaveBeenCalledTimes(1);
    expect(onPipWindow).not.toHaveBeenCalled();
  });

  it("a window row's button streams that window (shift for a replica)", async () => {
    const { onPipWindow } = renderList();
    await screen.findByText("Cua Docs");
    const button = within(rowOf("Cua Docs")).getByRole("button", { name: "Picture in picture" });
    fireEvent.click(button);
    expect(onPipWindow).toHaveBeenLastCalledWith(expect.objectContaining({ id: "w-1" }), { replica: false });
    fireEvent.click(button, { shiftKey: true });
    expect(onPipWindow).toHaveBeenLastCalledWith(expect.objectContaining({ id: "w-1" }), { replica: true });
  });

  it("says when the window list could not be read", async () => {
    renderList(
      fakeTeleport({
        listRemoteWindows: async () => {
          throw new Error("down");
        },
      }),
    );
    expect(await screen.findByRole("status")).toHaveTextContent("No windows: the Space’s window host is not up.");
    expect(screen.getByText("Desktop (1280×800)")).toBeInTheDocument();
  });
  it("tracks open picture-in-picture panels: pip.exit and Close while open", async () => {
    let panels: string[] = ["w-2"];
    let changed: () => void = () => {};
    const closeStreamWindow = vi.fn(async (_s: string, id: string) => {
      panels = panels.filter((p) => p !== id);
    });
    const teleport = fakeTeleport({
      streamPanels: async () => panels,
      closeStreamWindow,
      onStreamPanelsChanged: async (listener) => {
        changed = listener;
        return () => {};
      },
    });
    const onPipDesktop = vi.fn();
    const onClosePipDesktop = vi.fn();
    const onPipWindow = vi.fn();
    render(
      <SpaceWindowList
        space={SPACE}
        teleport={teleport}
        onPipDesktop={onPipDesktop}
        onClosePipDesktop={onClosePipDesktop}
        onPipWindow={onPipWindow}
      />,
    );
    await screen.findByText("Release notes");
    // The panel the shell already had open shows its exit button.
    const notes = within(rowOf("Release notes")).getByRole("button");
    await waitFor(() => expect(notes).toHaveAccessibleName("Close picture in picture"));
    expect(notes).toHaveAttribute("aria-pressed", "true");
    // Opening the Desktop's: the row flips at once.
    const desktop = within(rowOf("Desktop (1280×800)")).getByRole("button");
    fireEvent.click(desktop);
    expect(onPipDesktop).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(desktop).toHaveAccessibleName("Close picture in picture"));
    // A second click closes it rather than opening another.
    fireEvent.click(desktop);
    expect(onClosePipDesktop).toHaveBeenCalledTimes(1);
    expect(onPipDesktop).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(desktop).toHaveAccessibleName("Picture in picture"));
    // Closing a window's stream goes through the shell.
    fireEvent.click(notes);
    expect(closeStreamWindow).toHaveBeenCalledWith("local:aurora", "w-2");
    expect(onPipWindow).not.toHaveBeenCalled();
    await waitFor(() => expect(notes).toHaveAccessibleName("Picture in picture"));
    // A panel closed from its own title bar: the shell says so, rows re-read.
    panels = ["w-1"];
    changed();
    const docs = within(rowOf("Cua Docs")).getByRole("button");
    await waitFor(() => expect(docs).toHaveAccessibleName("Close picture in picture"));
  });
});
