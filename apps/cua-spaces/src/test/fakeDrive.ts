// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DriveBridge } from "../native/drive";

/** A tool's answer: a value, a function of the args, or an Error to throw. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type FakeAnswer = ((args: Record<string, any>) => unknown) | object | string | number | boolean | null;

/**
 * An in-memory daemon for the Cua Volume tools: answers from `answers`
 * (a missing tool rejects, as a daemon without the tool would), and records
 * every call, reveal and settings pane opened.
 */
export function fakeDrive(answers: Record<string, FakeAnswer> = {}, home: string | null = "/Users/maya") {
  const calls: { tool: string; args: Record<string, unknown> }[] = [];
  const revealed: string[] = [];
  const opened: string[] = [];
  const bridge: DriveBridge = {
    call: async (tool, args = {}) => {
      calls.push({ tool, args });
      if (!(tool in answers)) throw new Error(`unknown tool ${tool}`);
      const a = answers[tool];
      const value = typeof a === "function" ? (a as (x: Record<string, unknown>) => unknown)(args) : a;
      if (value instanceof Error) throw value;
      return value;
    },
    reveal: async (path) => {
      revealed.push(path);
    },
    openSettings: async (url) => {
      opened.push(url);
    },
    home: async () => home,
  };
  const tools = () => calls.map((c) => c.tool);
  return { bridge, calls, tools, revealed, opened, answers };
}

export const MOUNT_OFF = { enabled: false, state: "off", method: "fskit", path: null, volume_name: "Cua Volume" };
export const MOUNTED = {
  enabled: true,
  state: "mounted",
  method: "fskit",
  path: "/Volumes/Cua Volume",
  volume_name: "Cua Volume",
};
export const NEEDS_APPROVAL = {
  enabled: true,
  state: "needs_approval",
  method: "fskit",
  path: null,
  volume_name: "Cua Volume",
  detail: "Turn on Cua Volume in File System Extensions.",
  settings_url: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension",
};
