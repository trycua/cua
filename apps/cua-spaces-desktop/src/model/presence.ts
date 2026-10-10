// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The owner check before approving a device (the SwiftUI app's
// `LivePresence`): the system's own prompt through the app core
// (`appConfirmPresence`: Touch ID, an Apple Watch or the login password on
// macOS, Windows Hello on Windows, polkit on Linux). Where Windows or Linux
// has no usable prompt (Hello not set up, no polkit agent or action), the
// app asks in a dialog of its own that says so; a Mac without Touch ID or a
// password, and a prompt the person cancelled or failed, refuse.
import { sdkErrorKind, words } from "./errors";

/** The prompt's name on each system, for the fallback's words. */
export const PROMPT_NAME: Partial<Record<NodeJS.Platform, string>> = { win32: "Windows Hello", linux: "your password or fingerprint (polkit)" };

/** The fallback dialog's words. */
export function fallbackWords(platform: NodeJS.Platform, reason: string, why: string): { message: string; detail: string } {
  const prompt = PROMPT_NAME[platform] ?? "the system's prompt";
  return {
    message: "Approve this device?",
    detail:
      `Cua Spaces can't confirm it is you with ${prompt}: ${why}. ` +
      `It is asking to ${reason}. Approve only a device you are setting up yourself.`,
  };
}

export async function confirmOwner(
  platform: NodeJS.Platform,
  reason: string,
  /** The system's prompt (the app core's `appConfirmPresence`). */
  check: (reason: string) => Promise<void>,
  /** The app's own dialog: true when the person approved. */
  ask: (words: { message: string; detail: string }) => Promise<boolean>,
): Promise<void> {
  try {
    await check(reason);
    return;
  } catch (error) {
    if (platform === "darwin" || sdkErrorKind(error) !== "Unsupported") throw new Error(words(error));
    if (!(await ask(fallbackWords(platform, reason, words(error))))) throw new Error("Approval was cancelled");
  }
}
