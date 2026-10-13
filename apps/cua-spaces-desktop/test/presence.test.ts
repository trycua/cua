// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The owner check before approving a device: the system's prompt, and the
// app's own dialog (which says so) only where Windows or Linux has none.
import { describe, expect, it } from "vitest";
import { confirmOwner, fallbackWords } from "../src/model/presence";
import { sdkError } from "./host-fixtures";

const reason = "approve “Work laptop” for your Cua account";

describe("the owner check", () => {
  it("asks the system's prompt, and nothing else when it answers", async () => {
    const asked: string[] = [];
    for (const p of ["darwin", "win32", "linux"] as const) {
      await confirmOwner(p, reason, async (r) => void asked.push(r), async () => {
        throw new Error("no dialog");
      });
    }
    expect(asked).toEqual([reason, reason, reason]);
  });

  it("refuses a prompt the person cancelled or failed, without a dialog", async () => {
    for (const p of ["darwin", "win32", "linux"] as const) {
      await expect(
        confirmOwner(p, reason, async () => Promise.reject(sdkError("PermissionDenied", "Approval was cancelled")), async () => true),
      ).rejects.toThrow("Approval was cancelled");
    }
  });

  it("falls back to the app's dialog only where Windows or Linux has no prompt, and says so", async () => {
    const shown: { message: string; detail: string }[] = [];
    const none = async () => Promise.reject(sdkError("Unsupported", "Windows Hello is not set up"));
    await confirmOwner("win32", reason, none, async (w) => (shown.push(w), true));
    expect(shown[0]!.detail).toContain("can't confirm it is you with Windows Hello: Windows Hello is not set up");
    expect(shown[0]!.detail).toContain(reason);
    await expect(confirmOwner("linux", reason, none, async () => false)).rejects.toThrow("Approval was cancelled");
    // A Mac without Touch ID or a password refuses (the Swift app does too).
    await expect(confirmOwner("darwin", reason, none, async () => true)).rejects.toThrow("Windows Hello is not set up");
    expect(fallbackWords("linux", reason, "no agent").detail).toContain("polkit");
  });
});
