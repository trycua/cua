// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { HostStatus } from "@/bridge/contracts/host";

import { otherComputerDesktops, type NewKind } from "./ask-first";
import { homeDefaultWhere, hostStatusWhenOnline, thisMacReadiness, thisMacSetupSteps } from "./this-mac-target";

const host = (over: Partial<HostStatus> = {}): HostStatus => ({
  configured: true,
  mode: "relay",
  machineId: "m-here",
  sharing: true,
  shareDesktop: true,
  service: { installed: true, running: true, kind: "launchd" },
  clients: [],
  permissions: [],
  ...over,
});
const kinds: NewKind[] = [{ os: "linux", note: "Free. Ready in 2 min.", blocked: null }];

describe("Home's box starts on this computer", () => {
  it("once it's in the account, with no Space made first", () => {
    expect(homeDefaultWhere(thisMacReadiness(true, host()), kinds)).toEqual({ kind: "this-mac" });
    // Still being read, or not set up yet: still this computer, and the chip says what's missing.
    expect(homeDefaultWhere(thisMacReadiness(true, undefined), kinds)).toEqual({ kind: "this-mac" });
    expect(homeDefaultWhere(thisMacReadiness(true, host({ configured: false })), kinds)).toEqual({ kind: "this-mac" });
  });

  it("signed out, starts on a new computer as before", () => {
    expect(thisMacReadiness(false, host())).toBe("signed-out");
    expect(homeDefaultWhere("signed-out", kinds)).toEqual({ kind: "new", os: "linux" });
  });

  it("is ready only when it's set up and shares its desktop", () => {
    expect(thisMacReadiness(true, host())).toBe("ready");
    expect(thisMacReadiness(true, undefined)).toBe("checking");
    expect(thisMacReadiness(true, null)).toBe("needs-setup");
    expect(thisMacReadiness(true, host({ configured: false }))).toBe("needs-setup");
    expect(thisMacReadiness(true, host({ shareDesktop: false }))).toBe("needs-setup");
    expect(thisMacReadiness(true, host({ sharing: false }))).toBe("needs-setup");
  });
});

describe("Set up on the chip", () => {
  it("adds this computer and shares its desktop, or only does the part that's missing", () => {
    expect(thisMacSetupSteps(host({ configured: false }))).toEqual(["add", "share-desktop"]);
    expect(thisMacSetupSteps(null)).toEqual(["add", "share-desktop"]);
    expect(thisMacSetupSteps(host({ shareDesktop: false }))).toEqual(["share-desktop"]);
    expect(thisMacSetupSteps(host({ sharing: false }))).toEqual(["resume-sharing"]);
    expect(thisMacSetupSteps(host())).toEqual([]);
  });
});

describe("the switcher's other computers", () => {
  it("lists your other computers' desktops that are on, never this computer's own", () => {
    const row = (id: string, over: object = {}) => ({ id, name: id, os: "macos", status: "running", provider: "relay", ...over }) as never;
    const list = otherComputerDesktops(
      [row("this-mac", { status: "local" }), row("relay:m-here"), row("relay:mini"), row("relay:space-abc"), row("relay:off", { status: "suspended" })],
      "m-here",
    );
    expect(list.map((s) => s.id)).toEqual(["relay:mini"]);
  });
});

describe("a job on this computer right after Set up (CUA-3974)", () => {
  /** A fake clock: `sleep` moves it, so nothing waits for real. */
  const clock = () => {
    let t = 0;
    return { now: () => t, sleep: async (ms: number) => void (t += ms) };
  };

  it("waits while its sharing service restarts, then starts", async () => {
    const reads = [host({ online: false }), host({ online: false }), host({ online: true })];
    let n = 0;
    const c = clock();
    const status = await hostStatusWhenOnline(async () => reads[Math.min(n++, reads.length - 1)]!, { ...c, everyMs: 1_000 });
    expect(status?.online).toBe(true);
    expect(n).toBe(3);
  });

  it("starts at once when it's online, not set up, or unknown", async () => {
    for (const s of [host({ online: true }), host({ online: null }), host({ configured: false, online: false }), null]) {
      let n = 0;
      await hostStatusWhenOnline(async () => (n++, s), clock());
      expect(n).toBe(1);
    }
  });

  it("gives up after the timeout and lets the start say why", async () => {
    let n = 0;
    const c = clock();
    const status = await hostStatusWhenOnline(async () => (n++, host({ online: false })), { ...c, timeoutMs: 5_000, everyMs: 1_000 });
    expect(status?.online).toBe(false);
    expect(n).toBe(6);
  });
});
