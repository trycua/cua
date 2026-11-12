// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A failed host setup or This machine button in words (the SwiftUI app's
// HostSetupFailureTests): the kind from the raw error, a title and message
// that never carry a URL or the raw error, which stays whole as Details, and
// Sign In or Retry. On Windows and Linux the words name the machine.
import { describe, expect, it } from "vitest";
import { classify, presentHostFailure, refusalCode } from "../src/model/host-failure";
import { Failure, reply } from "../src/bridge/host";

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

  it("a throttled setup says how long to wait, in plain words", () => {
    const raw = "http: Too many attempts right now. Wait 3 seconds, then try again.";
    const f = presentHostFailure(raw);
    expect(f.kind).toBe("throttled");
    expect(f.title).toBe("Too many attempts right now");
    expect(f.message).toBe("Trying again in 3 seconds.");
    expect(f.details).toBe(raw);
    expect(f.actionLabel).toBe("Retry");
    expect(presentHostFailure("Too many attempts right now. Wait 1 second, then try again.").message).toBe("Trying again in 1 second.");
  });

  it("a raw HTTP 429 from an older host is the same, without a number and without the raw words", () => {
    const f = presentHostFailure("http: relay: HTTP 429: Too Many Requests");
    expect(f.kind).toBe("throttled");
    expect(f.message).toBe("Wait a few seconds, then try again.");
    expect(f.message).not.toMatch(/429|http|relay/i);
  });

  it.each(["http: relay: HTTP 429: computer limit for this token reached", "HTTP 429: at most 5 unused pairing codes", "You have 5 of 5 computers in use at once on Pro. Close one or upgrade to Teams."])(
    "a limit the relay explains is not a throttle: %s",
    (raw) => expect(classify(raw)).toBe("other"),
  );

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

  it("a new computer that needs proof asks for a code from another computer, in plain words", () => {
    const raw = "permission denied: relay: add computers from a computer you already added, or with a pairing code [code: new_computer_needs_proof]";
    const f = presentHostFailure(raw, "win32");
    expect(f.kind).toBe("needsProof");
    expect(f.refusalCode).toBe("new_computer_needs_proof");
    expect(f.title).toBe("Confirm it’s you");
    expect(f.message).toContain("To add this PC to your account, enter a code from a computer you already added.");
    expect(f.actionLabel).toBe("Use a code from another computer");
    expect(f.details).toBe(raw);
    for (const word of ["relay", "enrol", "MFA", "token", "pairing"]) expect(`${f.title} ${f.message}`).not.toContain(word);
    // The kind and the code reach the page.
    const e = reply("1", new Failure("failed", f.message, f));
    expect(e).toMatchObject({ ok: false, error: { kind: "needsProof", refusalCode: "new_computer_needs_proof", actionLabel: "Use a code from another computer" } });
  });

  it("reads a refusal code only from the end of the error", () => {
    expect(refusalCode("relay: no [code: phone]")).toBe("phone");
    expect(refusalCode("relay: that host is shared with you to watch only")).toBeUndefined();
    expect(refusalCode("relay: x [code: Bad-Code]")).toBeUndefined();
    expect(classify("relay: x [code: phone]")).toBe("other");
  });

  it("a port number is not a status code", () => {
    expect(classify("listen 0.0.0.0:14041 failed")).toBe("other");
  });

  it("the account's refusal is Sign In whatever its words", () => {
    expect(presentHostFailure("relay: invalid account token: ExpiredSignature", "darwin", "signedOut").actionLabel).toBe("Sign In");
  });

  it("names the machine and its system on Windows and Linux", () => {
    expect(presentHostFailure("odd", "win32").title).toBe("Couldn’t set up this PC for access");
    expect(presentHostFailure("odd", "linux").title).toBe("Couldn’t set up this machine for access");
    const svc = presentHostFailure("schtasks /Create failed: access denied", "win32");
    expect(svc.kind).toBe("service");
    expect(svc.message).toContain("Windows didn’t start");
    expect(svc.message).toContain("Run as administrator");
    expect(svc.message).not.toContain("restart this PC");
    expect(presentHostFailure("systemctl --user start failed", "linux").message).toContain("restart this machine");
    expect(presentHostFailure("relay: HTTP 401", "linux").message).toBe("Your Cua account isn’t signed in on this machine. Sign in, then try again.");
  });
});
