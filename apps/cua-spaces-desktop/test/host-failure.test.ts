// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A failed host setup or This machine button in words (the SwiftUI app's
// HostSetupFailureTests): the kind from the raw error, a title and message
// that never carry a URL or the raw error, which stays whole as Details, and
// Sign In or Retry. On Windows and Linux the words name the computer.
import { describe, expect, it } from "vitest";
import { classify, presentHostFailure } from "../src/model/host-failure";

const MISSING_RELEASE =
  "http: download: https://github.com/trycua/cua/releases/download/cua-spacesd-v0.2.0/cua-spacesd-aarch64-apple-darwin.tar.gz: HTTP 404";

describe("host setup failures", () => {
  it("a missing release is a download failure without the URL", () => {
    const f = presentHostFailure(MISSING_RELEASE);
    expect(f.kind).toBe("download");
    expect(f.title).toBe("Couldn’t download the Cua host service");
    expect(f.message).toContain("update Cua Spaces");
    expect(f.message).not.toContain("http");
    expect(f.title).not.toContain("http");
    expect(f.details).toBe(MISSING_RELEASE);
    expect(f.actionLabel).toBe("Retry");
  });

  it.each(["cua-spacesd: no published release for this platform", "download: cua-spacesd v0.2.0 is not available", "Download failed: HTTP 404 Not Found"])(
    "release not available: %s",
    (raw) => expect(classify(raw)).toBe("download"),
  );

  it.each([
    "error sending request for url (https://relay.cua.ai/v1/hosts): operation timed out",
    "The Internet connection appears to be offline.",
    "tcp connect error: Connection refused (os error 61)",
    "dns error: failed to lookup address information",
    "download: https://github.com/x: operation timed out",
  ])("network: %s", (raw) => {
    const f = presentHostFailure(raw);
    expect(f.kind).toBe("network");
    expect(f.message).not.toContain("http");
  });

  it.each(["launchctl bootstrap gui/501: Bootstrap failed: 125: Domain does not support specified action (no Aqua session)", "no GUI session: sign in at the console"])(
    "no one at the screen: %s",
    (raw) => {
      const f = presentHostFailure(raw);
      expect(f.kind).toBe("guiSession");
      expect(f.message).toContain("Sign in at the Mac");
    },
  );

  it.each(["relay: HTTP 401 Unauthorized", "not signed in: run `cua login`", "unauthenticated"])("signed out: %s", (raw) => {
    const f = presentHostFailure(raw);
    expect(f.kind).toBe("signedOut");
    expect(f.message).toContain("Sign in");
    expect(f.actionLabel).toBe("Sign In");
  });

  it.each([
    'local runtime: service: "launchctl" "bootstrap" "gui/501" "/Users/u/Library/LaunchAgents/com.trycua.spacesd.host.plist" failed: Bootstrap failed: 5: Input/output error',
    "local runtime: service: macOS did not start the Cua host service (launchd: Bootstrap failed: 5: Input/output error); try again",
  ])("the service did not start: %s", (raw) => {
    const f = presentHostFailure(raw);
    expect(f.kind).toBe("service");
    expect(f.title).toBe("Couldn’t start the Cua host service");
    expect(f.message).not.toContain("launchctl");
    expect(f.details).toBe(raw);
  });

  it("anything else is generic", () => {
    const f = presentHostFailure("something odd failed\nmore");
    expect(f.kind).toBe("other");
    expect(f.title).toBe("Couldn’t set up this Mac for access");
    expect(f.message).toBe("Something went wrong. Try again, or open Details to see what happened.");
    expect(f.details).toBe("something odd failed\nmore");
  });

  it("a port number is not a status code", () => {
    expect(classify("listen 0.0.0.0:14041 failed")).toBe("other");
  });

  it("the account's refusal is Sign In whatever its words", () => {
    expect(presentHostFailure("relay: invalid account token: ExpiredSignature", "darwin", "signedOut").actionLabel).toBe("Sign In");
  });

  it("names the computer and its system on Windows and Linux", () => {
    expect(presentHostFailure("odd", "win32").title).toBe("Couldn’t set up this PC for access");
    expect(presentHostFailure("odd", "linux").title).toBe("Couldn’t set up this computer for access");
    const svc = presentHostFailure("schtasks /Create failed: access denied", "win32");
    expect(svc.kind).toBe("service");
    expect(svc.message).toContain("Windows didn’t start");
    expect(presentHostFailure("systemctl --user start failed", "linux").message).toContain("restart this computer");
    expect(presentHostFailure("relay: HTTP 401", "linux").message).toBe("Your Cua account isn’t signed in on this computer. Sign in, then try again.");
  });
});
