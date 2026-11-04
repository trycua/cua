// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// macOS: whether the keychain's password prompt is on screen (the SwiftUI
// app's SecurityAgentWindow): a SecurityAgent window wider than 100 pt (it
// keeps a tiny offscreen one when the prompt is gone). Window owners and
// bounds need no Screen Recording permission. Electron has no window list,
// so `osascript` reads CoreGraphics' (JavaScript for Automation, which
// needs no Automation permission either); the launch asks only while it
// waits for the keychain, and reads the last answer (`promptShowing`).
import { execFile } from "node:child_process";

export const SECURITY_AGENT_SCRIPT = [
  "ObjC.import('CoreGraphics');",
  // kCGWindowListOptionOnScreenOnly, kCGNullWindowID.
  "var list = ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1, 0));",
  "var showing = false;",
  "for (var i = 0; i < list.count; i++) {",
  "  var w = list.objectAtIndex(i);",
  "  var owner = w.objectForKey('kCGWindowOwnerName');",
  "  if (!owner || owner.js !== 'SecurityAgent') continue;",
  "  var b = w.objectForKey('kCGWindowBounds');",
  "  if (b && b.objectForKey('Width').js > 100) showing = true;",
  "}",
  "showing ? 'yes' : 'no'",
].join("\n");

/** Reads the window list once: true or false, null when it could not. */
export type PromptRead = () => Promise<boolean | null>;

export const readPrompt: PromptRead = () =>
  new Promise((resolve) => {
    execFile("/usr/bin/osascript", ["-l", "JavaScript", "-e", SECURITY_AGENT_SCRIPT], { timeout: 3000 }, (error, stdout) => {
      const out = String(stdout ?? "").trim();
      resolve(error ? null : out === "yes" ? true : out === "no" ? false : null);
    });
  });

/** Reads it every `everyMs` while `watching()`; `current` is the last answer (null: unknown). */
export class SecurityAgentWatcher {
  current: boolean | null = null;
  private timer: NodeJS.Timeout | null = null;
  private reading = false;

  constructor(
    private readonly read: PromptRead = readPrompt,
    private readonly everyMs = 2000,
  ) {}

  /** Starts or stops following it. */
  follow(watching: boolean): void {
    if (watching && !this.timer) {
      const tick = async () => {
        if (this.reading) return;
        this.reading = true;
        try {
          this.current = await this.read();
        } finally {
          this.reading = false;
        }
      };
      void tick();
      this.timer = setInterval(() => void tick(), this.everyMs);
      this.timer.unref?.();
    } else if (!watching && this.timer) {
      clearInterval(this.timer);
      this.timer = null;
      this.current = null;
    }
  }
}
