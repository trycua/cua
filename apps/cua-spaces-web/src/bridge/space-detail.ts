// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's detail: its facts (`sidebar.detail`) and its Stream section
 * (`sidebar.streamSection`, `stream.pipClick`, `stream.pipReduce`), the
 * same core calls the SwiftUI app's `SpaceDetailView` and `StreamRowsModel`
 * make. `SpaceDetailStore` reads what they are built from while a detail
 * shows: memory and storage use every `USAGE_REFRESH_MS` (the core's
 * `USAGE_REFRESH_MS`), windows and the display every `WINDOWS_REFRESH_MS`
 * (`StreamRowsModel.poll`).
 */

import { useContext, useEffect, useSyncExternalStore } from "react";
import { isUnsupported, type DataAdapter } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import { sharesDesktop } from "./derive";
import type { MachineAccessNotice } from "./contracts/devices";
import type { SpaceAgentRun } from "./contracts/agents";
import type { Space } from "./contracts/spaces";
import type { Experiments } from "./contracts/volume";
import type {
  DesktopCover,
  DesktopCoverInput,
  DetailCopy,
  PipCommand,
  PipEvent,
  RemoteWindow,
  SpaceDetailView,
  SpaceUsage,
  StreamDisplay,
  StreamSection,
  StreamSectionInput,
} from "./ops/space-detail";

export type * from "./ops/space-detail";

export const USAGE_REFRESH_MS = 10_000;
export const WINDOWS_REFRESH_MS = 5_000;
/** How often a shown Space's agent runs are read (`AgentRunsModel.poll`). */
export const RUNS_REFRESH_MS = 4_000;

/* ---- The core's decisions ------------------------------------------------------ */

/** The Space's detail (`sidebar.detail`): facts, sections, preview text and
 * Delete's question, as this device sees it (`sidebar::detail_for`, the
 * SwiftUI app's `AppModel.detail`). */
export function spaceDetail(
  core: CoreClient,
  space: Space,
  usage?: SpaceUsage | null,
  hostArch?: string | null,
  /** With these, the core leaves out what Settings, Experiments hides (Share). */
  experiments?: Experiments,
  /** This device is signed in but not enrolled: a machine on the relay
   * shows why, with its connection actions off. */
  access?: MachineAccessNotice | null,
): SpaceDetailView {
  return core.tryCall<SpaceDetailView>("sidebar.detail", { space, usage, hostArch, experiments, access: access ?? undefined }) ?? fallbackDetail(space);
}

/** What the preview shows over (or instead of) the live desktop
 * (`spaces.desktopCover`, the SwiftUI app's `AppModel.cover`). */
export function desktopCover(core: CoreClient, input: DesktopCoverInput): DesktopCover {
  return core.tryCall<DesktopCover>("spaces.desktopCover", { input }) ?? fallbackCover(input);
}

/** The sections' words (`sidebar.detailCopy`). */
export function detailCopy(core: CoreClient): DetailCopy | null {
  return core.tryCall<DetailCopy>("sidebar.detailCopy", {}) ?? null;
}

/** The Stream section's rows (`sidebar.streamSection`). */
export function streamSection(core: CoreClient, input: StreamSectionInput): StreamSection {
  return core.tryCall<StreamSection>("sidebar.streamSection", { input }) ?? fallbackSection(input);
}

/** The open panels after `event` (`stream.pipReduce`). */
export function pipReduce(core: CoreClient, open: string[], event: PipEvent): string[] {
  return (
    core.tryCall<string[]>("stream.pipReduce", { open, event }) ??
    (event.type === "synced" ? event.rows : event.type === "opened" ? [...open.filter((r) => r !== event.row), event.row] : open.filter((r) => r !== event.row))
  );
}

/** What a row's picture-in-picture button does (`stream.pipClick`). */
export function pipClick(core: CoreClient, open: string[], row: string): PipCommand {
  return core.tryCall<PipCommand>("stream.pipClick", { open, row }) ?? { type: open.includes(row) ? "close" : "open", row };
}

/* Stand-ins without the core: enough to draw the page, not the product. */

function fallbackCover(input: DesktopCoverInput): DesktopCover {
  const cover = (kind: DesktopCover["kind"], text: string | null, button: string | null = null): DesktopCover => ({ kind, text, button, openStream: false, retry: false });
  if (input.access) return { ...cover("connect", input.access.text, "Connect"), buttonDisabled: true, action: input.access.actionLabel };
  if (!input.canStream) return cover("status", input.previewText);
  if (input.stream === "streaming" || input.stream === "suspended") return cover("stream", null);
  if (input.stream === "failed") return { ...cover("status", "Could not connect to the desktop", "Try again"), retry: input.connectRequested };
  if ((input.stream === "nosession" || input.stream === "idle") && !input.autoConnect && !input.connectRequested) return cover("connect", null, "Connect");
  return { ...cover("connecting", input.stream === "reconnecting" ? "Reconnecting…" : "Connecting…"), openStream: input.stream === "nosession" };
}

function fallbackDetail(space: Space): SpaceDetailView {
  const running = space.status === "running" && !space.power?.off;
  return {
    id: space.id,
    title: space.name,
    facts: [
      { label: "Status", value: space.detail || space.status },
      ...(space.image ? [{ label: "Image", value: space.image }] : []),
      { label: "Identifier", value: space.id },
      { label: "System", value: space.osPrettyName ?? space.osName ?? space.os },
    ],
    isHost: false,
    showSections: running,
    canStream: running,
    previewText: running ? "Loading the desktop…" : "Not reachable",
    deleteLabel: "Delete Space",
    removeOnly: false,
    actions: [
      { id: "teleport", label: "Teleport an app", help: "Teleport an app into this Space", enabled: running, destructive: false, primary: false },
      { id: "share", label: "Share", help: "Share this Space", enabled: running, destructive: false, primary: false },
    ],
    confirm: { title: `Delete ${space.name}?`, message: "Everything inside it, including its files, is deleted and can't be recovered.", confirmLabel: "Delete Space", confirmEnabled: true, cancelLabel: "Keep" },
    sections: running ? ["Stream"] : [],
  };
}

function fallbackSection(input: StreamSectionInput): StreamSection {
  const res = input.display ? `${input.display.widthPx}×${input.display.heightPx}` : null;
  const pip = (id: string) => {
    const on = (input.open ?? []).includes(id);
    return [{ id: "pip" as const, symbol: on ? "pip.exit" : "pip.enter", help: on ? "Close picture in picture" : "Picture in picture", active: on }];
  };
  const desktop = res ? `Desktop (${res})` : "Desktop";
  const windows = (input.windows ?? []).map((w) => {
    const label = w.title.trim() || w.appName.trim() || w.appId;
    return { id: w.id, kind: "window" as const, label, help: label, icon: { kind: "app" as const, appName: w.appName, appId: w.appId, pid: w.pid ?? 0 }, actions: pip(w.id) };
  });
  return {
    rows: [{ id: "desktop", kind: "desktop", label: desktop, help: desktop, resolution: res, icon: { kind: "os", id: `os-${input.os}` }, actions: pip("desktop") }, ...windows],
    statusText: input.windows === null ? "Looking for this Space’s windows…" : windows.length ? null : input.failed ? "No windows: the Space’s window host is not up." : "No open windows in this Space yet.",
  };
}

/* ---- What the detail reads while it shows ----------------------------------------- */

export interface DetailReadings {
  /** Memory and storage use; null until read, or when the host can't say. */
  usage: SpaceUsage | null;
  /** Null while the first list loads. */
  windows: RemoteWindow[] | null;
  display: StreamDisplay | null;
  /** The window list could not be read. */
  failed: boolean;
  /** Rows whose picture in picture is open. */
  open: string[];
  /** The Stream section's filter. */
  query: string;
  /** The host can't list a Space's windows: only the Desktop row shows. */
  windowsUnsupported?: boolean;
  /** This machine's architecture, when the parity harness sets it. */
  hostArch?: string | null;
  /** The Space's coding-agent runs; null while the first read runs. */
  runs?: SpaceAgentRun[] | null;
  /** Why the runs could not be read ("could not ask" is not "none"). */
  runsFailed?: string | null;
  /** The host can't list a Space's runs: no Agents section. */
  runsUnsupported?: boolean;
}

const NOTHING_YET: DetailReadings = { usage: null, windows: null, display: null, failed: false, open: [], query: "" };

/** What a picture-in-picture press did. */
export type PipOutcome = "done" | "open-window";

export class SpaceDetailStore {
  private readings = new Map<string, DetailReadings>();
  private watched = new Map<string, { count: number; timers: ReturnType<typeof setTimeout>[] }>();
  /** Spaces whose readings the parity harness put on screen: not polled. */
  private held = new Set<string>();
  private listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly adapter: DataAdapter,
    private readonly core: CoreClient,
  ) {}

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  get(spaceId: string): DetailReadings {
    return this.readings.get(spaceId) ?? NOTHING_YET;
  }

  private patch(spaceId: string, next: Partial<DetailReadings>): void {
    if (this.disposed) return;
    this.readings.set(spaceId, { ...this.get(spaceId), ...next });
    for (const l of [...this.listeners]) l();
  }

  /** Reads the Space's usage and windows while something watches it. */
  watch(spaceId: string): () => void {
    const w = this.watched.get(spaceId) ?? { count: 0, timers: [] };
    w.count += 1;
    this.watched.set(spaceId, w);
    if (w.count === 1 && !this.held.has(spaceId)) {
      const loop = (read: () => Promise<void>, every: number) => {
        const tick = async () => {
          await read();
          if (this.watched.get(spaceId)?.count && !this.disposed) w.timers.push(setTimeout(tick, every));
        };
        void tick();
      };
      loop(() => this.readUsage(spaceId), USAGE_REFRESH_MS);
      loop(() => this.readWindows(spaceId), WINDOWS_REFRESH_MS);
      loop(() => this.readRuns(spaceId), RUNS_REFRESH_MS);
    }
    return () => {
      w.count -= 1;
      if (w.count === 0) {
        w.timers.forEach(clearTimeout);
        w.timers = [];
        this.watched.delete(spaceId);
      }
    };
  }

  private async readUsage(spaceId: string): Promise<void> {
    try {
      const usage = await this.adapter.call("spaces.usage", { spaceId });
      if (!this.held.has(spaceId)) this.patch(spaceId, { usage });
    } catch {
      // No usage: Memory and Storage stay out of the facts.
    }
  }

  /** As `AgentRunsModel.refresh`: a failed read is its own state. */
  private async readRuns(spaceId: string): Promise<void> {
    try {
      const runs = await this.adapter.call("agents.runs", { spaceId });
      if (!this.held.has(spaceId)) this.patch(spaceId, { runs, runsFailed: null });
    } catch (e) {
      if (this.held.has(spaceId)) return;
      if (isUnsupported(e)) this.patch(spaceId, { runs: [], runsUnsupported: true });
      else this.patch(spaceId, { runs: null, runsFailed: e instanceof Error ? e.message : String(e) });
    }
  }

  private async readWindows(spaceId: string): Promise<void> {
    try {
      const { windows, display } = await this.adapter.call("spaces.windows", { spaceId });
      if (!this.held.has(spaceId)) this.patch(spaceId, { windows, display: display ?? this.get(spaceId).display, failed: false });
    } catch (e) {
      if (this.held.has(spaceId)) return;
      if (isUnsupported(e)) this.patch(spaceId, { windows: [], windowsUnsupported: true });
      // As `StreamRowsModel.refresh`: a failed read keeps what was listed.
      else this.patch(spaceId, { windows: this.get(spaceId).windows ?? [], failed: true });
    }
  }

  /**
   * A row's picture-in-picture button: the core says open or close, the
   * host does it and lists the panels open now. A host with no panels
   * (`unsupported`) answers `open-window`: the caller opens the Space's
   * own window instead.
   */
  async pip(spaceId: string, row: string): Promise<PipOutcome> {
    const command = pipClick(this.core, this.get(spaceId).open, row);
    try {
      const rows = await this.adapter.call("stream.pip", { spaceId, command });
      this.patch(spaceId, { open: pipReduce(this.core, this.get(spaceId).open, { type: "synced", rows }) });
      return "done";
    } catch (e) {
      if (isUnsupported(e)) return "open-window";
      throw e;
    }
  }

  /** Filters the Stream section's windows. */
  setQuery(spaceId: string, query: string): void {
    this.patch(spaceId, { query });
  }

  /** Parity harness: these readings, held (no polling overwrites them). */
  show(spaceId: string, readings: Partial<DetailReadings>): void {
    this.held.add(spaceId);
    this.patch(spaceId, { ...NOTHING_YET, ...readings });
  }

  dispose(): void {
    this.disposed = true;
    for (const w of this.watched.values()) w.timers.forEach(clearTimeout);
    this.watched.clear();
    this.listeners.clear();
  }
}

/* ---- Hooks ------------------------------------------------------------------------- */

/** The preview's cover for `input`, from the core (a stand-in without it). */
export function useDesktopCover(input: DesktopCoverInput): DesktopCover {
  const ctx = useContext(BridgeContext);
  return ctx ? desktopCover(ctx.core, input) : fallbackCover(input);
}

const noopSubscribe = () => () => {};

export interface SpaceDetailHook {
  /** The core's detail for `space`; null while there is no Space. */
  detail: SpaceDetailView | null;
  /** The Stream section; null while there is no Space. */
  stream: StreamSection | null;
  /** The host can't list windows: the section has its Desktop row only, and no status line. */
  windowsUnsupported: boolean;
  /** The sections' words (loading, empty, failed). */
  copy: DetailCopy | null;
  /** A row's picture-in-picture button. */
  pip(row: string): Promise<PipOutcome>;
  /** The Stream section's filter. */
  query: string;
  setQuery(query: string): void;
  /** The Agents section: null while the first read runs; `failed` when
   * the runs could not be read; `unsupported` where the host can't list them. */
  agents: { runs: AgentRunLine[] | null; failed: string | null; unsupported: boolean };
}

/** One row of a Space's Agents section, in the core's words
 * (`AgentRunsModel.line` and `.status`). */
export interface AgentRunLine {
  runId: string;
  /** The agent, then what it was asked: "Claude Code · Fix the tests". */
  line: string;
  /** "Running", "Idle", ... */
  status: string;
  /** Why it is in that status (the tooltip). */
  reason: string;
}

const STATUS_WORD: Record<SpaceAgentRun["status"], string> = { running: "Running", idle: "Idle", failed: "Failed", crashed: "Crashed", unknown: "Unknown" };

/** The Agents section's rows (`agents.name`, `agents.subtitle`, `agents.statusLabel`). */
export function agentRunLines(core: CoreClient, runs: SpaceAgentRun[]): AgentRunLine[] {
  return runs.map((run) => {
    const name = core.tryCall<string>("agents.name", { agent: run.agent }) ?? run.agent;
    const subtitle = core.tryCall<string>("agents.subtitle", { run }) ?? run.summary;
    const status = core.tryCall<string>("agents.statusLabel", { status: run.status }) ?? STATUS_WORD[run.status];
    return { runId: run.runId, line: `${name} \u00b7 ${subtitle}`, status, reason: run.reason };
  });
}

/** What the detail is drawn for, beyond the Space: this device's view of it. */
export interface DetailFor {
  /** Settings, Experiments (Share shows only with Sharing on); undefined
   * where the host has none (every action shows). */
  experiments?: Experiments;
  /** This machine's `accessNotice` (`useMachines`). */
  access?: MachineAccessNotice | null;
}

/**
 * A Space's facts and Stream section, from the core, with what they are
 * built from read while the component is mounted. `hostArch` is this
 * machine's architecture (`useMachines`), for the emulation warning.
 */
export function useSpaceDetail(space: Space | undefined, hostArch: string | null, seen: DetailFor = {}): SpaceDetailHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const details = ctx.store?.details;
  const id = space?.id;
  // A machine that does not share its desktop refuses its usage and its
  // windows: it is not asked (the core's detail says why in its note).
  // Nor is a machine on the relay while this device is not enrolled (the
  // relay refuses it; the detail says why).
  const noDesktop = space ? sharesDesktop(ctx.core, space) === false || Boolean(seen.access && space.id.startsWith("relay:")) : false;
  useEffect(() => {
    if (!details || !id || id.startsWith("pending:") || noDesktop) return;
    return details.watch(id);
  }, [details, id, noDesktop]);
  const readings = useSyncExternalStore(
    details ? details.subscribe : noopSubscribe,
    () => (details && id ? details.get(id) : NOTHING_YET),
    () => NOTHING_YET,
  );
  const core = ctx.core;
  if (!space)
    return {
      detail: null,
      stream: null,
      windowsUnsupported: false,
      copy: null,
      pip: async () => "done",
      query: "",
      setQuery: () => {},
      agents: { runs: null, failed: null, unsupported: false },
    };
  return {
    detail: spaceDetail(core, space, readings.usage, readings.hostArch !== undefined ? readings.hostArch : hostArch, seen.experiments, seen.access),
    stream: streamSection(core, {
      windows: readings.windows,
      failed: readings.failed,
      display: readings.display,
      os: space.os,
      osName: space.osName ?? null,
      open: readings.open,
      query: readings.query,
    }),
    query: readings.query,
    setQuery: (q) => (details && space ? details.setQuery(space.id, q) : undefined),
    windowsUnsupported: Boolean(readings.windowsUnsupported),
    copy: detailCopy(core),
    pip: (row) => (details ? details.pip(space.id, row) : Promise.resolve("open-window")),
    agents: {
      runs: readings.runs ? agentRunLines(core, readings.runs) : null,
      failed: readings.runsFailed ?? null,
      unsupported: Boolean(readings.runsUnsupported),
    },
  };
}
