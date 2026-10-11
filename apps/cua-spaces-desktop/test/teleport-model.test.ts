// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Teleport's model pieces that need no app core (src/model/teleport.ts): the
// picker's sources over the SDK's handles (and what they answer where the
// SDK has no windows), the consent, the data URLs and the bounded cache.
import { describe, expect, it } from "vitest";
import { MAX_ENTRIES, TeleportCache, dataURL, iconRequest, liveSources, runReport, teleportConsent } from "../src/model/teleport";
import { FakeSpace, FakeTeleport, JPEG, PNG, SVG, asSpace, asTeleport, entry, plan } from "./vault-fixtures";

describe("the picker's sources", () => {
  it("lists this machine's windows as the picker's records", async () => {
    const sources = liveSources(asTeleport(new FakeTeleport()), null);
    expect(await sources.openWindows()).toEqual([{ windowId: 41, appId: "", appName: "Slack", windowTitle: "general", supported: true, bundlePath: "/Applications/Slack.app" }]);
  });

  it("has no windows where the SDK has none: Windows, Linux, or no handle", async () => {
    const t = new FakeTeleport();
    t.windows = new Error("CuaError.Unsupported: window listing is macOS only");
    expect(await liveSources(asTeleport(t), null).openWindows()).toEqual([]);
    expect(await liveSources(null, null).openWindows()).toEqual([]);
    expect(await liveSources(null, null).remoteWindows()).toEqual([]);
    expect(await liveSources(null, null).hostIcon("/x")).toBeNull();
    expect(await liveSources(null, null).hostThumbnail(1)).toBeNull();
    expect(await liveSources(null, null).guestThumbnail("w", 1n)).toBeNull();
  });

  it("lists the Space's windows with sizes and pids only when they are reported", async () => {
    const [a, b] = await liveSources(null, asSpace(new FakeSpace())).remoteWindows();
    expect(a).toEqual({ id: "w-1", appName: "Firefox", title: "Mozilla Firefox", visible: true, appId: "org.mozilla.firefox", targetEpoch: 7n, widthPx: 1280, heightPx: 720, pid: 321 });
    expect(b).toMatchObject({ id: "w-2", visible: false, widthPx: undefined, heightPx: undefined, pid: undefined });
  });

  it("answers no preview where the SDK cannot capture one", async () => {
    const t = new FakeTeleport();
    const bad = asTeleport({ ...t, captureWindowThumbnail: () => { throw new Error("CuaError.Unsupported: no Screen Recording permission"); } } as unknown as FakeTeleport);
    expect(await liveSources(bad, null).hostThumbnail(41)).toBeNull();
    expect(await liveSources(asTeleport(t), null).hostThumbnail(41)).toEqual(PNG);
    expect(await liveSources(null, asSpace(new FakeSpace())).guestThumbnail("w-1", 7n)).toEqual(JPEG);
    expect(await liveSources(null, asSpace(new FakeSpace())).guestThumbnail("w-2", 7n)).toBeNull();
  });
});

describe("data URLs", () => {
  it("names the type by the bytes: PNG, JPEG, SVG", () => {
    expect(dataURL(PNG)).toBe(`data:image/png;base64,${Buffer.from(PNG).toString("base64")}`);
    expect(dataURL(JPEG)).toMatch(/^data:image\/jpeg;base64,/);
    expect(dataURL(SVG)).toMatch(/^data:image\/svg\+xml;base64,/);
    expect(dataURL(new Uint8Array([1, 2, 3]))).toMatch(/^data:application\/octet-stream;base64,/);
  });

  it("has none for nothing", () => {
    expect(dataURL(null)).toBeNull();
    expect(dataURL(undefined)).toBeNull();
    expect(dataURL(new Uint8Array())).toBeNull();
  });

  it("reads a view into a larger buffer", () => {
    const big = new Uint8Array([0, 0, 0x89, 0x50, 0x4e, 0x47, 9, 0]);
    expect(dataURL(big.subarray(2, 7))).toBe(`data:image/png;base64,${Buffer.from([0x89, 0x50, 0x4e, 0x47, 9]).toString("base64")}`);
  });
});

describe("the consent", () => {
  it("carries every field, Save to Keyvault included, and defaults the rest off", () => {
    expect(
      teleportConsent({
        approved: true,
        acknowledgeSensitive: true,
        saveToKeyvault: true,
        acknowledgeRelayPlaintext: true,
        cookieDomains: ["github.com"],
        exclude: ["Default/Bookmarks"],
        fromVault: ["a", "b"],
        includePasswords: true,
      }),
    ).toEqual({
      approved: true,
      acknowledgeSensitive: true,
      saveToKeyvault: true,
      acknowledgeRelayPlaintext: true,
      cookieDomains: ["github.com"],
      exclude: ["Default/Bookmarks"],
      fromVault: ["a", "b"],
      includePasswords: true,
    });
    const off = teleportConsent({ approved: true, acknowledgeSensitive: true });
    expect(off.saveToKeyvault).toBe(false);
    expect(off.exclude).toEqual([]);
    expect(off.cookieDomains).toBeUndefined();
    expect(off.fromVault).toBeUndefined();
  });

  it("takes only true as yes, and only lists of strings as lists", () => {
    const c = teleportConsent({ approved: "yes", saveToKeyvault: 1, cookieDomains: ["a", 2], exclude: "x", fromVault: null });
    expect(c).toEqual({ approved: false, acknowledgeSensitive: false, saveToKeyvault: false, acknowledgeRelayPlaintext: false, cookieDomains: undefined, exclude: [], fromVault: undefined, includePasswords: false });
  });

  it("reports a run as the page reads it", () => {
    expect(runReport({ appId: "slack", installed: ["slack"], sent: ["a"], imported: ["b"], skipped: ["c"], launched: true })).toEqual({ appId: "slack", installed: ["slack"], sent: ["a"], imported: ["b"], skipped: ["c"], launched: true });
  });

  it("asks for a Space app's icon with what the page names, and nothing negative", () => {
    expect(iconRequest({ kind: "guest", appName: "Firefox", appId: "org.mozilla.firefox", pid: 12.9 })).toEqual({ appName: "Firefox", appId: "org.mozilla.firefox", pid: 12 });
    expect(iconRequest({ pid: -4 })).toEqual({ appName: "", appId: "", pid: 0 });
  });
});

describe("the cache between steps", () => {
  it("keeps the latest catalog, the apps picked since, and a bounded number of them", () => {
    const c = new TeleportCache();
    c.setCatalog([entry("a", "A"), entry("b", "B")]);
    c.addEntry(entry("c", "C"));
    expect(c.entryCount).toBe(3);
    c.setCatalog([entry("d", "D")]);
    expect(c.entryCount).toBe(1);
    for (let i = 0; i < MAX_ENTRIES + 5; i++) c.addEntry(entry(`x${i}`, `X${i}`));
    expect(c.entryCount).toBe(MAX_ENTRIES);
    expect(c.entry("x0")).toBeUndefined();
    expect(c.entry(`x${MAX_ENTRIES + 4}`)).toBeDefined();
  });

  it("keeps one plan per Space and drops it only when it is the one that ran", () => {
    const c = new TeleportCache();
    const a = { json: "A", plan: plan(entry("a", "A"), "A"), providerId: "a" };
    const b = { json: "B", plan: plan(entry("b", "B"), "B"), providerId: "b" };
    c.keepPlan("s1", a);
    c.keepPlan("s2", b);
    c.keepPlan("s1", b);
    expect(c.planCount).toBe(2);
    c.dropPlan("s1", "A");
    expect(c.plan("s1")?.json).toBe("B");
    c.dropPlan("s1", "B");
    expect(c.plan("s1")).toBeUndefined();
    expect(c.planCount).toBe(1);
  });
});
