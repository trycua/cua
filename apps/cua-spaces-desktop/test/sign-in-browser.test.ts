// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Signing in on a machine without a browser (a Linux box): the browser
// probe, and the device flow the account falls back to, whose page opens on
// any device with the code the page shows (as `cua auth login --remote`).
import { describe, expect, it } from "vitest";
import { hasBrowser, type BrowserProbe } from "../src/browser";
import { LiveAccount } from "../src/model/services";
import type { AuthLike } from "../src/native/generated/index";

const probe = (over: Partial<BrowserProbe>): BrowserProbe => ({
  platform: "linux",
  env: { HOME: "/home/ada" },
  defaultBrowser: () => "",
  exists: () => false,
  ...over,
});

describe("a browser on this machine", () => {
  it("is always there on a Mac and Windows", () => {
    expect(hasBrowser(probe({ platform: "darwin" }))).toBe(true);
    expect(hasBrowser(probe({ platform: "win32" }))).toBe(true);
  });

  it("on Linux is $BROWSER, or a default browser that is installed", () => {
    expect(hasBrowser(probe({}))).toBe(false);
    expect(hasBrowser(probe({ env: { BROWSER: "w3m" } }))).toBe(true);
    // Named, but its desktop entry is gone (a snap never installed).
    expect(hasBrowser(probe({ defaultBrowser: () => "firefox_firefox.desktop" }))).toBe(false);
    const installed = new Set(["/usr/share/applications/firefox.desktop"]);
    expect(hasBrowser(probe({ defaultBrowser: () => "firefox.desktop", exists: (f) => installed.has(f) }))).toBe(true);
  });
});

/** A login attempt as the SDK starts it. */
const attempt = (method: "browser" | "device", url: string, userCode?: string) => ({
  method: () => method,
  url: () => url,
  userCode: () => userCode,
  note: () => undefined,
  wait: async () => ({ email: "ada@example.com" }),
});

function fakeAuth() {
  const flows: (string | undefined)[] = [];
  const auth = {
    beginLogin: async (flow: string | undefined) => {
      flows.push(flow);
      return flow === "device" ? attempt("device", "https://auth.cua.ai/device", "WDJB-MJHT") : attempt("browser", "https://auth.cua.ai/authorize?x=1");
    },
  } as unknown as AuthLike;
  return { auth, flows };
}

describe("the account's sign-in", () => {
  it("finishes in this machine's browser when it opens", async () => {
    const { auth, flows } = fakeAuth();
    const opened: string[] = [];
    const a = await new LiveAccount(auth, async (url) => (opened.push(url), true)).beginSignIn();
    expect(flows).toEqual([undefined]);
    expect(opened).toEqual(["https://auth.cua.ai/authorize?x=1"]);
    expect([a.url, a.userCode]).toEqual(["https://auth.cua.ai/authorize?x=1", null]);
  });

  it("uses the device flow without a browser: a page for any device and its code", async () => {
    const { auth, flows } = fakeAuth();
    const a = await new LiveAccount(auth, async () => false).beginSignIn();
    expect(flows).toEqual([undefined, "device"]);
    expect([a.url, a.userCode]).toEqual(["https://auth.cua.ai/device", "WDJB-MJHT"]);
    expect(await a.wait()).toBe("ada@example.com");
  });
});
