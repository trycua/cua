// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { entryFromCore, type Plan, type TeleportHost } from "@trycua/cua/teleport";

import type { OpenWindow, RemoteWindow } from "../model/teleport";
import type { TeleportBridge } from "../native/teleport";
import { createFallbackTeleportAppsBridge, FIXTURE_CATALOG, type TeleportAppsBridge } from "../native/teleportApps";
import { TeleportPicker } from "./TeleportPicker";

const localWindows: OpenWindow[] = [
  {
    windowId: 11,
    appId: "vscode",
    appName: "Visual Studio Code",
    windowTitle: "Docs",
    supported: true,
    capability: "install_only",
    entry: JSON.parse(FIXTURE_CATALOG[1]!.json),
    icon: null,
  },
  { windowId: 21, appId: "figma", appName: "Figma", windowTitle: "Board", supported: false, icon: null },
];

const remoteWindows: RemoteWindow[] = [
  { id: "w-1", appName: "Firefox", title: "Cua — Docs", visible: true, appId: "firefox", targetEpoch: 1 },
  { id: "w-2", appName: "Terminal", title: "bash", visible: true, appId: "terminal", targetEpoch: 0 },
];

function fakeBridge(overrides: Partial<TeleportBridge> = {}): TeleportBridge {
  return {
    isNative: true,
    manifest: async () => ({
      provider_id: "google-chrome",
      app_display_name: "Google Chrome",
      scope: "full_profile",
      items: [],
      total_est_bytes: 0,
      notes: [],
    }),
    push: async () => ({ ok: true, provider_id: "google-chrome", launched: true, pid: 1 }),
    listOpenWindows: async () => localWindows,
    captureThumbnail: async () => null,
    appIcon: async () => null,
    spaceAppIcon: async () => null,
    spacePrimaryDisplay: async () => null,
    spaceAppIcons: async (_s: string, requests: unknown[]) => requests.map(() => null),
    listRemoteWindows: async () => remoteWindows,
    listSpaceAgents: async () => [],
    remoteWindowThumbnail: async () => null,
    streamRemoteWindow: vi.fn(async () => {}),
    openPicker: async () => {},
    pickerConfig: async () => ({ spaceId: "cloud:aurora", spaceName: "Aurora", app: null }),
    closePicker: async () => {},
    ...overrides,
  };
}

function renderPicker(bridge: TeleportBridge, apps: TeleportAppsBridge = createFallbackTeleportAppsBridge()) {
  return render(<TeleportPicker bridge={bridge} apps={apps} />);
}

/** Opens a non-default tab and waits for its first card. */
async function openTab(name: RegExp, card: RegExp) {
  fireEvent.click(await screen.findByRole("tab", { name }));
  return screen.findByRole("option", { name: card });
}

describe("TeleportPicker window tabs", () => {
  it("lists this Mac's open windows as the core's tiles, with Teleport to <Space>", async () => {
    renderPicker(fakeBridge());
    const docs = await openTab(/Open windows/, /Docs/);
    // The same tile as the Apps tab (and the SwiftUI picker).
    expect(docs).toHaveClass("ta-tile");
    expect(docs).toHaveAccessibleDescription("Visual Studio Code \u00b7 Docs");
    const go = screen.getByRole("button", { name: "Teleport to Aurora" });
    // Live only with a tile selected (the core's primary).
    expect(go).toBeDisabled();
    fireEvent.click(docs);
    expect(go).toBeEnabled();
  });

  it("teleports a picked window's app through the Apps flow, preselected", async () => {
    renderPicker(fakeBridge());
    const docs = await openTab(/Open windows/, /Docs/);
    fireEvent.click(docs);
    fireEvent.click(screen.getByRole("button", { name: /Teleport to Aurora/ }));
    // The Apps tab opens on the window's app, at the move choice.
    expect(await screen.findByRole("heading", { name: "Visual Studio Code" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Just the app/ })).toBeChecked();
    expect(screen.getByRole("tab", { name: "Apps" })).toHaveAttribute("aria-selected", "true");
  });

  it("switches to From <Space>: the Space's windows as tiles, with Stream to This Mac", async () => {
    const bridge = fakeBridge();
    renderPicker(bridge);
    await openTab(/Open windows/, /Docs/);

    fireEvent.click(screen.getByRole("tab", { name: /From Aurora/ }));

    // One line per tile: the window's title; the app rides in the tooltip.
    const firefox = await screen.findByRole("option", { name: /Cua — Docs/ });
    expect(firefox).toHaveClass("ta-tile");
    expect(firefox).toHaveAccessibleDescription("Firefox \u00b7 Cua — Docs");
    expect(screen.getByRole("button", { name: "Stream to This Mac" })).toBeInTheDocument();
    // No local windows shown on this tab.
    expect(screen.queryByRole("option", { name: /Board/ })).toBeNull();
  });

  it("streams a picked remote window via the bridge and closes the picker", async () => {
    const streamRemoteWindow = vi.fn(async () => {});
    const closePicker = vi.fn(async () => {});
    const bridge = fakeBridge({ streamRemoteWindow, closePicker });
    renderPicker(bridge);
    const terminal = await openTab(/From Aurora/, /bash/);

    fireEvent.click(terminal); // select it
    fireEvent.click(screen.getByRole("button", { name: "Stream to This Mac" }));

    await waitFor(() =>
      expect(streamRemoteWindow).toHaveBeenCalledWith("cloud:aurora", "Aurora", "w-2", "Terminal", "bash", false),
    );
    await waitFor(() => expect(closePicker).toHaveBeenCalled());
  });

  it("loads each tile's icon by its app's bundle, once per app", async () => {
    const iconUrl = "data:image/png;base64,ICON";
    const hostIcon = vi.fn(async (path: string) => (path === "/Applications/Visual Studio Code.app" ? iconUrl : null));
    const apps: TeleportAppsBridge = { ...createFallbackTeleportAppsBridge(), hostIcon };
    const windows: OpenWindow[] = [
      { ...localWindows[0]!, bundlePath: "/Applications/Visual Studio Code.app" },
      { ...localWindows[0]!, windowId: 12, windowTitle: "Notes", bundlePath: "/Applications/Visual Studio Code.app" },
      { ...localWindows[1]!, bundlePath: "/Applications/Figma.app" },
    ];
    renderPicker(fakeBridge({ listOpenWindows: async () => windows }), apps);
    await openTab(/Open windows/, /Docs/);
    await waitFor(() => expect(screen.getAllByTestId("hp-card-badge-icon")).toHaveLength(2));
    expect(screen.getAllByTestId("hp-card-badge-icon")[0]).toHaveAttribute("src", iconUrl);
    expect(hostIcon.mock.calls.filter(([p]) => p === "/Applications/Visual Studio Code.app")).toHaveLength(1);
    expect(hostIcon).toHaveBeenCalledWith("/Applications/Figma.app");
  });

  it("loads the Space's icons in one call and swaps in a window's frame", async () => {
    const thumbUrl = "data:image/png;base64,REMOTE";
    let releaseThumb: (url: string) => void = () => {};
    const pending = new Promise<string>((resolve) => {
      releaseThumb = resolve;
    });
    const remoteWindowThumbnail = vi.fn((_s: string, id: string) => (id === "w-1" ? pending : Promise.resolve(null)));
    const spaceAppIcons = vi.fn(async (_s: string, requests: unknown[]) => requests.map(() => null));
    renderPicker(fakeBridge({ remoteWindowThumbnail, spaceAppIcons }));

    const card = await openTab(/From Aurora/, /Cua — Docs/);
    expect(card.querySelector("img.hp-card-img")).toBeNull();
    await waitFor(() => expect(remoteWindowThumbnail).toHaveBeenCalledWith("cloud:aurora", "w-1", 1));
    await waitFor(() => expect(spaceAppIcons).toHaveBeenCalledTimes(1));
    expect(spaceAppIcons.mock.calls[0]![1]).toEqual([
      { appName: "Firefox", appId: "firefox", pid: 0 },
      { appName: "Terminal", appId: "terminal", pid: 0 },
    ]);
    releaseThumb(thumbUrl);
    await waitFor(() => expect(card.querySelector("img.hp-card-img")).toHaveAttribute("src", thumbUrl));
  });

  it("moves the selection with the arrow keys and opens it with Return", async () => {
    const streamRemoteWindow = vi.fn(async () => {});
    renderPicker(fakeBridge({ streamRemoteWindow }));
    const first = await openTab(/From Aurora/, /Cua — Docs/);
    const pick = first.closest(".hp-pick") as HTMLElement;
    fireEvent.keyDown(pick, { key: "ArrowRight" });
    await waitFor(() => expect(first).toHaveAttribute("aria-selected", "true"));
    fireEvent.keyDown(pick, { key: "ArrowRight" });
    await waitFor(() => expect(screen.getByRole("option", { name: /bash/ })).toHaveAttribute("aria-selected", "true"));
    fireEvent.keyDown(pick, { key: "Enter" });
    await waitFor(() => expect(streamRemoteWindow).toHaveBeenCalledWith("cloud:aurora", "Aurora", "w-2", "Terminal", "bash", false));
  });
});

/** A fixture host that records plans and runs; nothing leaves the test. */
function recordingHost(): TeleportHost & { runs: unknown[] } {
  const runs: unknown[] = [];
  const secretPlan = (entry: ReturnType<typeof entryFromCore>): Plan => ({
    app: entry,
    spaceId: "cloud:aurora",
    moves: "app_with_state",
    relayUnsealed: false,
    steps: [{ kind: "state", summary: "Import 1 firefox item(s)" }],
    consent: [
      { kind: "secret", key: "cookies.sqlite", label: "Cookies", detail: "311 cookies, cookies.sqlite", bytes: 486400, sensitive: true },
    ],
    sensitive: true,
    totalBytes: 486400,
    warnings: [],
    json: "{}",
  });
  return {
    runs,
    catalog: async () => FIXTURE_CATALOG,
    plan: async (entry) => secretPlan(entry),
    run: async (plan, consent, onEvent) => {
      runs.push({ app: plan.app.id, consent });
      onEvent({ step: 0, steps: 1, kind: "state", phase: "started", detail: "", doneBytes: 0, totalBytes: 0 });
      return { appId: plan.app.id, installed: [], sent: [], imported: ["cookies.sqlite"], skipped: [], launched: true };
    },
  };
}

describe("TeleportPicker: Teleport an app", () => {
  it("is a grid of the core's tiles: a tile previews its app's frontmost window; arrows move", async () => {
    const captureThumbnail = vi.fn(async (id: number) => (id === 11 ? "data:image/png;base64,VlND" : null));
    renderPicker(fakeBridge({ captureThumbnail }));
    const code = await screen.findByRole("option", { name: /Visual Studio Code/ });
    await waitFor(() => expect(code.querySelector("img.hp-card-img")).not.toBeNull());
    expect(code.querySelector("img.hp-card-img")).toHaveAttribute("src", "data:image/png;base64,VlND");
    expect(captureThumbnail).toHaveBeenCalledWith(11);
    // Keyboard: the first choosable tile, then the next one to the right.
    const grid = code.closest(".ta-pick") as HTMLElement;
    const selected = () => screen.getAllByRole("option").find((o) => o.getAttribute("aria-selected") === "true");
    const before = selected()?.textContent;
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    await waitFor(() => expect(selected()?.textContent).not.toBe(before));
  });


  it("lists every app with its capability, and disables unsupported ones with the reason", async () => {
    renderPicker(fakeBridge());
    expect(await screen.findByRole("tab", { name: "Apps" })).toHaveAttribute("aria-selected", "true");
    const ff = await screen.findByRole("option", { name: /Firefox/ });
    // One line per app: the capability rides in the accessible description.
    expect(ff).toHaveAccessibleDescription("App and signed-in state");
    expect(screen.getByRole("option", { name: /Visual Studio Code/ })).toHaveAccessibleDescription("App, empty or with files");
    const safari = screen.getByRole("option", { name: /Safari/ });
    expect(safari).toBeDisabled();
    expect(safari).toHaveAttribute("title", expect.stringMatching(/no Linux build/));
    expect(safari).toHaveTextContent("Safari");
    // Search narrows the list.
    fireEvent.change(screen.getByRole("searchbox", { name: "Search apps" }), { target: { value: "visual" } });
    await waitFor(() => expect(screen.queryByRole("option", { name: /Firefox/ })).toBeNull());
  });

  it("shows every secret on the consent screen and runs only after it is acknowledged", async () => {
    const host = recordingHost();
    const apps: TeleportAppsBridge = { ...createFallbackTeleportAppsBridge(), host: () => host };
    renderPicker(fakeBridge(), apps);
    fireEvent.doubleClick(await screen.findByRole("option", { name: /Firefox/ }));
    fireEvent.click(await screen.findByRole("radio", { name: /signed-in state/ }));
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(await screen.findByText(/Teleport Firefox to Aurora\?/)).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "What moves" })).toHaveTextContent("Cookies");
    const go = screen.getByRole("button", { name: "Teleport" });
    expect(go).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Send the secrets above" }));
    fireEvent.click(go);
    expect(await screen.findByText(/Firefox is in Aurora/)).toBeInTheDocument();
    expect(host.runs).toEqual([
      {
        app: "firefox",
        consent: {
          approved: true,
          acknowledgeSensitive: true,
          saveToKeyvault: false,
          acknowledgeRelayPlaintext: false,
        },
      },
    ]);
  });

  it("opens straight on a preselected app from a drop, with its files", async () => {
    const bridge = fakeBridge({
      pickerConfig: async () => ({
        spaceId: "cloud:aurora",
        spaceName: "Aurora",
        app: null,
        entry: JSON.parse(FIXTURE_CATALOG[1]!.json),
        files: ["/tmp/fixture-project"],
      }),
    });
    renderPicker(bridge);
    expect(await screen.findByRole("heading", { name: "Visual Studio Code" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /with files or folders/ })).toBeChecked();
    expect(screen.getByText("/tmp/fixture-project")).toBeInTheDocument();
  });
});
