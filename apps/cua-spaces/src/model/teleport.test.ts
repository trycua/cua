// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  appsFromWindows,
  filterRemoteWindows,
  filterWindows,
  groupWindowsByApp,
  teleportButtonEnabled,
  pickerPrimary,
  type OpenWindow,
  type RemoteWindow,
} from "./teleport";

const openWindows: OpenWindow[] = [
  { windowId: 11, appId: "google-chrome", appName: "Google Chrome", windowTitle: "In Call", supported: true, icon: null },
  { windowId: 12, appId: "google-chrome", appName: "Google Chrome", windowTitle: "Docs", supported: true, icon: null },
  { windowId: 21, appId: "figma", appName: "Figma", windowTitle: "ScreenshareBarView.swift", supported: false, icon: null },
  { windowId: 31, appId: "finder", appName: "Finder", windowTitle: "", supported: false, icon: null },
];

describe("teleport picker filtering and supported-gating", () => {
  it("filters windows by app name or window title, case-insensitively", () => {
    expect(filterWindows(openWindows, "").map((w) => w.windowId)).toEqual([11, 12, 21, 31]);
    // Matches the app name.
    expect(filterWindows(openWindows, "chrome").map((w) => w.windowId)).toEqual([11, 12]);
    // Matches a window title, ignoring case.
    expect(filterWindows(openWindows, "screenshare").map((w) => w.windowId)).toEqual([21]);
    expect(filterWindows(openWindows, "in call").map((w) => w.windowId)).toEqual([11]);
    // Whitespace-only is treated as blank.
    expect(filterWindows(openWindows, "   ").length).toBe(4);
    // No match.
    expect(filterWindows(openWindows, "safari")).toEqual([]);
  });

  it("derives the app-filter row as distinct apps in first-seen order", () => {
    const apps = appsFromWindows(openWindows);
    expect(apps.map((a) => a.appId)).toEqual(["google-chrome", "figma", "finder"]);
    expect(apps.find((a) => a.appId === "google-chrome")?.supported).toBe(true);
    expect(apps.find((a) => a.appId === "figma")?.supported).toBe(false);
    // The app row narrows with the query.
    expect(appsFromWindows(filterWindows(openWindows, "chrome")).map((a) => a.appId)).toEqual([
      "google-chrome",
    ]);
  });

  it("enables the Teleport button only for a supported selection", () => {
    expect(teleportButtonEnabled(null)).toBe(false);
    // Supported app (Chrome) -> enabled.
    expect(teleportButtonEnabled(openWindows[0]!)).toBe(true);
    // Unsupported app (Figma / Finder) -> disabled even though a window is picked.
    expect(teleportButtonEnabled(openWindows[2]!)).toBe(false);
    expect(teleportButtonEnabled(openWindows[3]!)).toBe(false);
  });
});

const remoteWindows: RemoteWindow[] = [
  { id: "w-1", appName: "Firefox", title: "Cua — Docs", visible: true, appId: "firefox", targetEpoch: 1 },
  { id: "w-2", appName: "Terminal", title: "bash", visible: true, appId: "terminal", targetEpoch: 0 },
];

describe("two-way picker: tab-driven button label + remote filtering", () => {
  it("labels the primary Teleport for a supported local window on the {Space} tab", () => {
    expect(pickerPrimary("space", { supported: true })).toEqual({
      label: "Teleport",
      action: "teleport",
      enabled: true,
    });
  });

  it("labels it Stream (coming-soon, still clickable) for an unsupported local window", () => {
    // Unsupported app on the Space tab: the button flips to "Stream" but its
    // click only surfaces the honest notice (no local window host yet).
    expect(pickerPrimary("space", { supported: false })).toEqual({
      label: "Stream",
      action: "stream-local-soon",
      enabled: true,
    });
  });

  it("disables Teleport when nothing is selected on the {Space} tab", () => {
    expect(pickerPrimary("space", null)).toEqual({
      label: "Teleport",
      action: "teleport",
      enabled: false,
    });
  });

  it("labels the primary Stream on the This-Mac tab, enabled only with a selection", () => {
    expect(pickerPrimary("thisMac", { supported: true })).toEqual({
      label: "Stream",
      action: "stream-remote",
      enabled: true,
    });
    // Remote windows have no notion of a provider; a selection alone enables it.
    expect(pickerPrimary("thisMac", { supported: false })).toEqual({
      label: "Stream",
      action: "stream-remote",
      enabled: true,
    });
    expect(pickerPrimary("thisMac", null)).toEqual({
      label: "Stream",
      action: "stream-remote",
      enabled: false,
    });
  });

  it("filters the Space's remote windows by app or title, case-insensitively", () => {
    expect(filterRemoteWindows(remoteWindows, "").map((w) => w.id)).toEqual(["w-1", "w-2"]);
    expect(filterRemoteWindows(remoteWindows, "fire").map((w) => w.id)).toEqual(["w-1"]);
    expect(filterRemoteWindows(remoteWindows, "BASH").map((w) => w.id)).toEqual(["w-2"]);
    expect(filterRemoteWindows(remoteWindows, "   ").length).toBe(2);
    expect(filterRemoteWindows(remoteWindows, "safari")).toEqual([]);
  });
});

describe("groupWindowsByApp", () => {
  const win = (id: string, appId: string, appName: string): RemoteWindow => ({
    id,
    appId,
    appName,
    title: `${appName} ${id}`,
    visible: true,
    targetEpoch: 0,
  });

  it("groups by app, first-seen order, keeping each app's window order", () => {
    const groups = groupWindowsByApp([
      win("1", "firefox", "Firefox"),
      win("2", "terminal", "Terminal"),
      win("3", "firefox", "Firefox"),
    ]);
    expect(groups.map((g) => g.appId)).toEqual(["firefox", "terminal"]);
    expect(groups[0]!.windows.map((w) => w.id)).toEqual(["1", "3"]);
    expect(groups[1]!.windows.map((w) => w.id)).toEqual(["2"]);
  });

  it("is empty for no windows", () => {
    expect(groupWindowsByApp([])).toEqual([]);
  });
});
