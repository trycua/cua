// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Cua Driver inside Cua Spaces, for the page, the first run and the top
 * bar (`ops/driver.ts`). One store per computer reads `driver.status`
 * while something shows it: at once, every few seconds, and when the app
 * comes to the front. A host without the operations, or the
 * `spaces_embedded_driver` flag off, reads as "not here", and nothing about
 * the driver is drawn.
 */

import { useCallback, useContext, useEffect, useState, useSyncExternalStore } from "react";

import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import { useFlag } from "./flags";
import type { DriverConfig, DriverConfigValue, DriverLiveTest, DriverMachine, DriverStatus } from "./ops/driver";

export type { DriverActivity, DriverApp, DriverConfig, DriverLiveTest, DriverMachine, DriverPermission, DriverStatus, DriverUpdate } from "./ops/driver";

export const DRIVER_POLL_MS = 5_000;

interface Snapshot {
  status: DriverStatus | null;
  config: DriverConfig | null;
  /** The host has no driver operations. */
  unsupported: boolean;
  /** The first read hasn't answered. */
  loading: boolean;
  error: string | null;
}

const INITIAL: Snapshot = { status: null, config: null, unsupported: false, loading: true, error: null };
const NONE: Snapshot = { ...INITIAL, loading: false, unsupported: true };

class DriverStore {
  snap: Snapshot = INITIAL;
  private listeners = new Set<() => void>();
  private stop: (() => void) | null = null;
  private reading = false;

  constructor(
    private readonly adapter: DataAdapter,
    readonly machine: string | null,
  ) {}

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    if (this.listeners.size === 1) this.start();
    return () => {
      this.listeners.delete(l);
      if (this.listeners.size === 0) {
        this.stop?.();
        this.stop = null;
      }
    };
  };

  snapshot = () => this.snap;

  set(patch: Partial<Snapshot>) {
    this.snap = { ...this.snap, ...patch };
    for (const l of [...this.listeners]) l();
  }

  private get args() {
    return this.machine ? { machine: this.machine } : {};
  }

  async read(withConfig = false): Promise<void> {
    if (this.snap.unsupported || this.reading) return;
    this.reading = true;
    try {
      const status = await this.adapter.call("driver.status", this.args);
      const config = status && (withConfig || !this.snap.config) ? await this.adapter.call("driver.config", this.args).catch(() => this.snap.config) : this.snap.config;
      this.set({ status, config, loading: false, error: null });
    } catch (e) {
      if (isUnsupported(e)) this.set({ ...NONE });
      else this.set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    } finally {
      this.reading = false;
    }
  }

  async call<T>(run: () => Promise<T>): Promise<T> {
    const out = await run();
    await this.read();
    return out;
  }

  private start() {
    void this.read(true);
    const timer = setInterval(() => void this.read(), DRIVER_POLL_MS);
    const win = typeof window !== "undefined" ? window : null;
    const onFocus = () => void this.read(true);
    win?.addEventListener("focus", onFocus);
    const off = this.adapter.subscribe((e) => {
      if (e.type === "machines.changed") void this.read();
    });
    this.stop = () => {
      clearInterval(timer);
      win?.removeEventListener("focus", onFocus);
      off();
    };
  }

  setConfig(changes: Record<string, DriverConfigValue>) {
    return this.adapter.call("driver.setConfig", { ...this.args, changes }).then((config) => {
      this.set({ config });
      return config;
    });
  }
}

const stores = new WeakMap<DataAdapter, Map<string, DriverStore>>();

function storeFor(adapter: DataAdapter, machine: string | null): DriverStore {
  let byMachine = stores.get(adapter);
  if (!byMachine) stores.set(adapter, (byMachine = new Map()));
  const key = machine ?? "";
  let s = byMachine.get(key);
  if (!s) byMachine.set(key, (s = new DriverStore(adapter, machine)));
  return s;
}

const noop = () => () => {};
const none = () => NONE;

export interface DriverHook extends Snapshot {
  /** The page, the first-run step and the top-bar line may show. */
  available: boolean;
  refresh(): Promise<void>;
  setConfig(changes: Record<string, DriverConfigValue>): Promise<DriverConfig>;
  test(): Promise<DriverLiveTest>;
  pause(paused: boolean): Promise<null>;
  setApp(app: string, on: boolean): Promise<null>;
  update(): Promise<null>;
}

/** This Mac's driver (`machine` null), or another computer's. */
export function useDriver(machine: string | null = null): DriverHook {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const on = useFlag("spaces_embedded_driver");
  const store = adapter && on ? storeFor(adapter, machine) : null;
  const snap = useSyncExternalStore(store?.subscribe ?? noop, store?.snapshot ?? none, none);
  const args = machine ? { machine } : {};
  const need = () => {
    if (!store || !adapter) throw new Error("Cua Driver isn't available here");
    return { store, adapter };
  };
  return {
    ...snap,
    available: Boolean(store && !snap.unsupported && snap.status),
    refresh: () => store?.read(true) ?? Promise.resolve(),
    setConfig: (changes) => need().store.setConfig(changes),
    test: () => need().store.call(() => need().adapter.call("driver.test", args)),
    pause: (paused) => need().store.call(() => need().adapter.call("driver.pause", { ...args, paused })),
    setApp: (app, on) => need().store.call(() => need().adapter.call("driver.setApp", { app, on })),
    update: () => need().store.call(() => need().adapter.call("driver.update", args)),
  };
}

/** The account's computers with a Cua Driver (this one first); empty where the host can't list them. */
export function useDriverMachines(enabled: boolean): DriverMachine[] {
  const adapter = useContext(BridgeContext)?.adapter ?? null;
  const [list, setList] = useState<DriverMachine[]>([]);
  const read = useCallback(() => {
    if (!adapter || !enabled) return;
    adapter
      .call("driver.list", {})
      .then(setList)
      .catch(() => setList([]));
  }, [adapter, enabled]);
  useEffect(() => {
    read();
    const t = setInterval(read, DRIVER_POLL_MS * 6);
    return () => clearInterval(t);
  }, [read]);
  return list;
}

/* ---- The pointer settings, from `get_config` ---------------------------- */

/** One driver setting the page can draw; null when this driver doesn't have it (the row hides). */
export interface PointerSettings {
  /** `agent_cursor.enabled`. */
  show: boolean | null;
  /** `cursor.motion.style`. */
  style: string | null;
  /** `cursor.float` (trycua/cua#6142). */
  float: boolean | null;
  /** `cursor.marks`. */
  marks: boolean | null;
  /** `cursor.rotate`. */
  rotate: boolean | null;
  /** `cursor.badges`: `minimal`, `verbose` or `off`. */
  nameTag: "minimal" | "verbose" | "off" | null;
}

const at = (config: DriverConfig | null, path: string): unknown =>
  path.split(".").reduce<unknown>((o, k) => (o && typeof o === "object" ? (o as Record<string, unknown>)[k] : undefined), config ?? undefined);

function onOff(v: unknown): boolean | null {
  if (typeof v === "boolean") return v;
  if (v === "on") return true;
  if (v === "off") return false;
  return null;
}

/** What the page draws from the driver's settings. A key the driver doesn't report is null. */
export function pointerSettings(config: DriverConfig | null): PointerSettings {
  const style = at(config, "cursor.motion.style");
  const badges = at(config, "cursor.badges");
  return {
    show: onOff(at(config, "agent_cursor.enabled")),
    style: typeof style === "string" ? style : null,
    float: onOff(at(config, "cursor.float")),
    marks: onOff(at(config, "cursor.marks")),
    rotate: onOff(at(config, "cursor.rotate")),
    nameTag: badges === "minimal" || badges === "verbose" || badges === "off" ? badges : null,
  };
}

/** The `set_config` key and value for one pointer setting. */
export function pointerChange<K extends keyof PointerSettings>(key: K, value: NonNullable<PointerSettings[K]>): Record<string, DriverConfigValue> {
  switch (key) {
    case "show":
      return { "agent_cursor.enabled": value as boolean };
    case "style":
      return { "cursor.motion.style": value as string };
    case "nameTag":
      return { "cursor.badges": value as string };
    default:
      return { [`cursor.${key}`]: value ? "on" : "off" };
  }
}

/** How the pointer travels, in plain words (`cursor.motion.style`). */
export const POINTER_STYLES: { value: string; label: string }[] = [
  { value: "signature_arc", label: "Smooth arc" },
  { value: "spring_settle", label: "Springy" },
  { value: "comet_swoop", label: "Swoop" },
  { value: "magnetic", label: "Snap to targets" },
  { value: "adaptive", label: "Adaptive" },
  { value: "classic", label: "Straight glide" },
];

/** "2 minutes ago", for the last test. */
export function sinceText(atMs: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - atMs) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return m === 1 ? "a minute ago" : `${m} minutes ago`;
  const h = Math.round(m / 60);
  if (h < 24) return h === 1 ? "an hour ago" : `${h} hours ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}
