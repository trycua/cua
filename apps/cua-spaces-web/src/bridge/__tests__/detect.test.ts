// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { createAdapter } from "../adapters";
import { detectMode, type HostWindow } from "../detect";

const invoke = async () => null;
const tauri: HostWindow = { __TAURI__: { core: { invoke } } };
const tauriInternals: HostWindow = { __TAURI_INTERNALS__: { invoke } };
const electron: HostWindow = { cuaDesktop: { invoke } };
const webkit: HostWindow = { webkit: { messageHandlers: { cua: { postMessage: () => undefined } } } };

describe("detectMode", () => {
  it("is demo with no window or no host", () => {
    expect(detectMode(undefined)).toBe("demo");
    expect(detectMode({})).toBe("demo");
  });

  it("finds each host", () => {
    expect(detectMode(tauri)).toBe("tauri");
    expect(detectMode(tauriInternals)).toBe("tauri");
    expect(detectMode(electron)).toBe("electron");
    expect(detectMode(webkit)).toBe("webkit");
  });

  it("checks Tauri, then Electron, then WebKit", () => {
    expect(detectMode({ ...webkit, ...electron, ...tauri })).toBe("tauri");
    expect(detectMode({ ...webkit, ...electron })).toBe("electron");
  });

  it("ignores a WebKit handler that isn't ours", () => {
    expect(detectMode({ webkit: { messageHandlers: { other: { postMessage: () => undefined } } } })).toBe("demo");
    expect(detectMode({ cuaDesktop: {} as never })).toBe("demo");
  });

  it("?bridge=demo forces demo inside a host", () => {
    expect(detectMode({ ...tauri, location: { search: "?bridge=demo" } })).toBe("demo");
  });

  it("creates the adapter for the mode", () => {
    expect(createAdapter("tauri", tauri).mode).toBe("tauri");
    expect(createAdapter("electron", electron).mode).toBe("electron");
    expect(createAdapter("webkit", webkit).mode).toBe("webkit");
    expect(createAdapter("demo", {}).mode).toBe("demo");
    expect(createAdapter(detectMode(electron), electron).mode).toBe("electron");
  });
});
