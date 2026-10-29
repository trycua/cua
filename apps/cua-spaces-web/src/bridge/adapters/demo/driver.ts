// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Cua Driver (`ops/driver.ts`): the driver this Mac runs
 * for coding agents, with 0.38.1 ready to install, and the account's other
 * computers. Its permissions are This machine's (`state.host`), as the
 * embedded driver's are Cua Spaces' own grants, so allowing one in the
 * first run allows both.
 */

import { HostError } from "../../adapter";
import type { DriverApp, DriverConfig, DriverLiveTest, DriverMachine, DriverPermission, DriverStatus } from "../../ops/driver";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoDriverState {
  driver: {
    version: string;
    update: { version: string; note: string | null } | null;
    paused: boolean;
    lastTest: DriverLiveTest | null;
    config: DriverConfig;
    apps: DriverApp[];
    /** The account's other computers with a Cua Driver. */
    others: DriverMachine[];
  };
}

const APPS: DriverApp[] = [
  { id: "claude-code", name: "Claude Code", on: true, own: false, bringsOwn: false },
  { id: "codex", name: "Codex", on: true, own: false, bringsOwn: false },
  { id: "t3-code", name: "T3 Code", on: true, own: false, bringsOwn: true },
];

/** `get_config` as the 0.38 driver answers it, with trycua/cua#6142's cursor keys. */
const CONFIG: DriverConfig = {
  version: "0.38.0",
  platform: "macos",
  agent_cursor: { enabled: true },
  cursor: { badges: "minimal", float: true, marks: false, rotate: false, motion: { style: "signature_arc", timing: "steady", effects: {} } },
  max_image_dimension: 1366,
};

const OTHERS: DriverMachine[] = [
  { machineId: "mac-mini", name: "Mac mini", os: "macos", thisMachine: false, state: "ready", version: "0.37.2", update: "0.38.1" },
  { machineId: "linux-box", name: "Linux box", os: "linux", thisMachine: false, state: "ready", version: "0.38.1", update: null },
];

export const demoDriverState = (now: number, firstRun: boolean): DemoDriverState => ({
  driver: {
    version: "0.38.0",
    update: { version: "0.38.1", note: "Faster clicks in Chrome." },
    paused: false,
    lastTest: firstRun ? null : { passed: true, atMs: now - 2 * 60_000, failure: null, tookMs: 4_000 },
    config: structuredClone(CONFIG),
    apps: APPS.map((a) => ({ ...a })),
    others: OTHERS.map((m) => ({ ...m })),
  },
});

/** A dotted `set_config` key into the nested `get_config` object. */
function setPath(config: DriverConfig, key: string, value: unknown): void {
  const parts = key.split(".");
  let at = config as Record<string, unknown>;
  for (const p of parts.slice(0, -1)) {
    if (typeof at[p] !== "object" || at[p] === null) at[p] = {};
    at = at[p] as Record<string, unknown>;
  }
  at[parts.at(-1)!] = value === "on" ? true : value === "off" ? false : value;
}

export function demoDriverHandlers({ state, now, wait, emit, stepMs }: DemoContext): DemoHandlers<
  "driver.status" | "driver.list" | "driver.config" | "driver.setConfig" | "driver.test" | "driver.pause" | "driver.setApp" | "driver.update"
> {
  const d = () => state.driver;
  const permissions = (): DriverPermission[] =>
    (["screen-recording", "accessibility"] as const).map((id) => {
      const p = state.host.permissions.find((x) => x.id === id);
      return { id: id === "screen-recording" ? "screen" : "accessibility", granted: p ? Boolean(p.granted) : true, settingsUrl: p?.settingsUrl ?? null };
    });
  const remote = (machine: string | null | undefined) => (machine && machine !== "this-mac" ? d().others.find((m) => m.machineId === machine) : undefined);
  const status = (): DriverStatus => {
    const perms = permissions();
    const allowed = perms.every((p) => p.granted);
    return {
      machineId: "this-mac",
      machineName: "This Mac",
      thisMachine: true,
      state: !allowed ? "needs-permission" : d().paused ? "paused" : "ready",
      version: d().version,
      update: d().update ? { ...d().update! } : null,
      lastTest: d().lastTest ? { ...d().lastTest! } : null,
      lastError: null,
      permissions: perms,
      apps: d().apps.map((a) => ({ ...a })),
      activity: allowed && !d().paused && d().lastTest ? [{ app: "Claude Code", target: "Xcode" }] : [],
      canPause: true,
    };
  };
  return {
    "driver.status": ({ machine }) => {
      const other = remote(machine);
      if (!other) return status();
      return {
        machineId: other.machineId,
        machineName: other.name,
        thisMachine: false,
        state: other.state,
        version: other.version,
        update: other.update ? { version: other.update, note: null } : null,
        lastTest: { passed: true, atMs: now() - 3 * 60 * 60_000, failure: null },
        lastError: null,
        permissions: [],
        apps: null,
        activity: [],
        canPause: false,
      };
    },
    "driver.list": () => {
      const s = status();
      return [{ machineId: "this-mac", name: "This Mac", os: "macos", thisMachine: true, state: s.state, version: s.version, update: s.update?.version ?? null }, ...d().others.map((m) => ({ ...m }))];
    },
    "driver.config": () => structuredClone(d().config),
    "driver.setConfig": ({ changes }) => {
      for (const [k, v] of Object.entries(changes)) setPath(d().config, k, v);
      return structuredClone(d().config);
    },
    "driver.test": async () => {
      const started = now();
      await wait(stepMs * 3);
      const missing = permissions().find((p) => !p.granted);
      const test: DriverLiveTest = {
        passed: !missing,
        atMs: now(),
        failure: missing ? (missing.id === "screen" ? "Screen Recording is off for Cua Spaces" : "Accessibility is off for Cua Spaces") : null,
        tookMs: Math.max(now() - started, 4_000),
      };
      d().lastTest = test;
      emit({ type: "machines.changed" });
      return { ...test };
    },
    "driver.pause": ({ paused }) => {
      d().paused = paused;
      return null;
    },
    "driver.setApp": ({ app, on }) => {
      const a = d().apps.find((x) => x.id === app);
      if (!a) throw new HostError(`No coding app ${app}`, "not_found");
      a.on = on;
      return null;
    },
    "driver.update": async ({ machine }) => {
      const other = remote(machine);
      await wait(stepMs * 4);
      if (other) {
        other.version = other.update ?? other.version;
        other.update = null;
        return null;
      }
      if (!d().update) return null;
      d().version = d().update!.version;
      (d().config as Record<string, unknown>).version = d().version;
      d().update = null;
      d().lastTest = { passed: true, atMs: now(), failure: null, tookMs: 4_000 };
      return null;
    },
  };
}
