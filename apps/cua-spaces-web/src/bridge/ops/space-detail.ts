// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What a Space's detail reads from the host, beyond its registry row: its
 * memory and storage use (the facts), its windows and primary display (the
 * Stream section), and its picture-in-picture panels. Each is something
 * the native apps already read from the SDK; nothing here is new.
 *
 * | Operation | Tauri | SwiftUI | Electron |
 * |---|---|---|---|
 * | `spaces.usage {spaceId}` | `space_usage` | `backend.usage(id:)` (`Space.usage`) | the same SDK call |
 * | `spaces.windows {spaceId}` | none: its viewer lists windows over its own stream | `StreamRowsModel`'s `availableWindows` and `primaryDisplay` | `StreamRows` (`Space.windows`, `displays`) |
 * | `stream.pip {spaceId, command}` | none: its PiP is a viewer window | the stream panel set (`StreamSource.pipKey`) | floating windows showing `/pip` (`src/pip.ts`) |
 * | `spaces.thumbnail {spaceId, maxAgeMs?}` | none | `SpaceThumbnails.refresh` (the notch's and the cover's image) | `SpaceThumbnails`, ported |
 * | `spaces.chooseFiles {}` | none | "Send file…"'s `NSOpenPanel` (`SpaceDetailView.chooseFiles`) | Electron's open dialog |
 * | `spaces.droppedFiles {names}` | none | the drag pasteboard's file URLs (`TeleportDropZone`'s drop) | the drop's paths, read by the preload |
 * | `spaces.sendFiles {spaceId, paths}` | none | `backend.sendFiles` (`Space.send_file`) | the same SDK call |
 *
 * Types mirror `cua-spaces-app-core/src/spaces/sidebar.rs` and `stream.rs`.
 */

import type { OpCoverage } from "../coverage";
import type { MachineAccessNotice } from "../contracts/devices";
import type { SpaceOs } from "../contracts/spaces";

/* ---- Contracts (app core `spaces::sidebar`, `spaces::stream`) ----------------- */

/** Memory and storage use now (`sidebar::SpaceUsage`). */
export interface SpaceUsage {
  memoryUsed: number;
  memoryTotal: number;
  /** `memoryTotal` is the guest's own limit. */
  memoryLimited: boolean;
  diskUsed: number;
  diskTotal: number;
  /** `diskTotal` is the guest's own disk. */
  diskLimited: boolean;
}

/** One window in a Space (`model::RemoteWindow`), shared with Teleport's picker. */
import type { RemoteWindow } from "../contracts/teleport";
export type { RemoteWindow };

/** The Space's primary display (`stream::StreamDisplay`). */
export interface StreamDisplay {
  widthPx: number;
  heightPx: number;
}

/** A Space's latest thumbnail (`Space.thumbnail`, as `SpaceThumbnails`
 * keeps it): an image `data:` URL and when it was captured. */
export interface SpaceThumbnail {
  url: string;
  capturedAtMs: number;
}

/** A file a drop sent, verified in the Space (`transfer::SentFileInfo`). */
export interface SentFileInfo {
  name: string;
  /** Where it landed in the Space. */
  dest: string;
  bytes: number;
}

/** What `spaces.windows` answers: the window list and the primary display. */
export interface SpaceWindows {
  windows: RemoteWindow[];
  display: StreamDisplay | null;
}

/** What the Stream section is built from (`stream::StreamSectionInput`). */
export interface StreamSectionInput {
  /** Null while the list loads. */
  windows: RemoteWindow[] | null;
  failed?: boolean;
  display?: StreamDisplay | null;
  os: SpaceOs;
  osName?: string | null;
  /** Rows whose picture in picture is open. */
  open?: string[];
  query?: string;
}

export type StreamRowIcon = { kind: "os"; id: string } | { kind: "app"; appName: string; appId: string; pid: number };

export interface StreamRowAction {
  id: "pip";
  symbol: string;
  help: string;
  active: boolean;
}

/** One row of the Stream section (`stream::StreamRow`). */
export interface StreamRow {
  id: string;
  kind: "desktop" | "window";
  label: string;
  help: string;
  resolution?: string | null;
  icon: StreamRowIcon;
  actions: StreamRowAction[];
}

/** The Stream section (`stream::StreamSection`). */
export interface StreamSection {
  rows: StreamRow[];
  statusText?: string | null;
}

/** A row's picture-in-picture button press (`stream::PipCommand`). */
export interface PipCommand {
  type: "open" | "close";
  row: string;
}

/** A change to the open panels (`stream::PipEvent`). */
export type PipEvent = { type: "opened"; row: string } | { type: "closed"; row: string } | { type: "synced"; rows: string[] };

/** One fact (`sidebar::Fact`). */
export interface SpaceFact {
  label: string;
  value: string;
  /** A copy button for `text`; shows `doneHelp` for `confirmMs` after a copy. */
  copy?: { text: string; symbol: string; help: string; doneSymbol: string; doneHelp: string; confirmMs: number } | null;
  /** The tooltip: the full value (and an image's digest). */
  help?: string | null;
  /** A warning after the value (a local Space emulating another architecture). */
  warning?: { symbol: string; help: string } | null;
}

export interface DetailAction {
  id: string;
  label: string;
  symbol?: string | null;
  help: string;
  enabled: boolean;
  destructive: boolean;
  primary: boolean;
  busy?: boolean;
}

/** What Delete asks (`sidebar::DeleteConfirm`). */
export interface DeleteConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  confirmEnabled: boolean;
  disabledReason?: string | null;
  removeLabel?: string | null;
  cancelLabel: string;
}

/** A Space's detail (`sidebar::SpaceDetail`, the parts the web draws). */
export interface SpaceDetailView {
  id: string;
  title: string;
  /** Status, Image, Identifier, System, Kind, Architecture, Memory, Storage (each when known). */
  facts: SpaceFact[];
  isHost: boolean;
  showSections: boolean;
  /** Streaming is possible. */
  canStream: boolean;
  /** Over the preview until a frame arrives ("Loading the desktop…", "Not reachable"). */
  previewText: string;
  deleteLabel: string;
  removeOnly: boolean;
  actions: DetailAction[];
  confirm: DeleteConfirm;
  /** The sections under the facts, in order (`Stream`, `Agents`, `Teleport`). */
  sections: string[];
  powerError?: string | null;
  /** Signed in, but this device is not enrolled: a machine reached through
   * the relay shows its Connect greyed out under this line and its one
   * action (`sidebar::detail_for`). */
  access?: MachineAccessNotice | null;
  /** One of your machines that does not share its desktop: the line in
   * place of its desktop, Stream, Agents and Teleport. */
  desktopNote?: string | null;
  /** With `desktopNote`, when the machine provides Spaces: New Space on it
   * (the detail lists its Spaces; no button beside the note). */
  newSpace?: { label: string; on: string } | null;
}

/** Where the shell's stream stands (`cover::StreamPhase`). */
export type StreamPhase = "nosession" | "idle" | "connecting" | "reconnecting" | "streaming" | "suspended" | "failed";

/** What the preview's cover is built from (`cover::DesktopCoverInput`). */
export interface DesktopCoverInput {
  canStream: boolean;
  previewText: string;
  /** Settings: "Connect to the desktop automatically". */
  autoConnect: boolean;
  /** Connect (or Try again) was pressed for this Space. */
  connectRequested: boolean;
  stream: StreamPhase;
  access?: MachineAccessNotice | null;
}

/** What the preview shows over (or instead of) the live desktop
 * (`cover::DesktopCover`): the Space's thumbnail blurred and dimmed, with
 * "Connecting…", Connect, or a line and Try again. */
export interface DesktopCover {
  kind: "stream" | "connecting" | "connect" | "status";
  text?: string | null;
  button?: string | null;
  buttonHelp?: string | null;
  /** The shell opens the stream now. */
  openStream: boolean;
  /** The shell starts the stream again (Try again was pressed). */
  retry: boolean;
  /** Connect shows greyed out (this device is not enrolled). */
  buttonDisabled?: boolean;
  /** The one action above a greyed-out Connect ("Enroll This Mac…"). */
  action?: string | null;
}

/** The words of a Space's sections (`sidebar::DetailCopy`). */
export interface DetailCopy {
  streamLoading: string;
  streamEmpty: string;
  streamFailed: string;
  streamNoMatch: string;
  agentsLoading: string;
  agentsEmpty: string;
  /** The runs could not be read (not the same as none). */
  agentsFailed: string;
  agentsNoMatch: string;
  /** The drop well's caption ("Drop a file or window"). */
  dropCaption: string;
  /** "Send file…" */
  sendFile: string;
  /** "Teleport an app…" */
  teleportApp: string;
}

/* ---- Operations ------------------------------------------------------------------ */

export interface SpaceDetailOps {
  /** Memory and storage use now (the SDK's `Space.usage`); null when the Space can't say. */
  "spaces.usage": { args: { spaceId: string }; result: SpaceUsage | null };
  /** The Space's windows and primary display. */
  "spaces.windows": { args: { spaceId: string }; result: SpaceWindows };
  /** Opens or closes a row's picture-in-picture panel; answers the rows whose panel is open. */
  "stream.pip": { args: { spaceId: string; command: PipCommand }; result: string[] };
  /** The Space's latest thumbnail, after asking for one no older than
   * `maxAgeMs` (the host's background interval when absent); null while
   * there is none. */
  "spaces.thumbnail": { args: { spaceId: string; maxAgeMs?: number }; result: SpaceThumbnail | null };
  /** "Send file…": the host's own file picker; the local paths chosen
   * (none when cancelled). Only these, or dropped ones, can be sent. */
  "spaces.chooseFiles": { args: Record<string, never>; result: string[] };
  /** Files dropped on a Space's drop well: the page sees only their names,
   * the host answers their local paths (from the drag itself). */
  "spaces.droppedFiles": { args: { names: string[] }; result: string[] };
  /** Sends picked or dropped files into the Space's Downloads. */
  "spaces.sendFiles": { args: { spaceId: string; paths: string[] }; result: SentFileInfo[] };
}


/* ---- Hosts ------------------------------------------------------------------------- */

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Args<K extends keyof SpaceDetailOps> = SpaceDetailOps[K]["args"];

/** Ops the Tauri shell has no command for (they reject as unsupported). */
export const TAURI_SPACE_DETAIL_UNSUPPORTED = [
  "spaces.windows",
  "stream.pip",
  "spaces.thumbnail",
  "spaces.chooseFiles",
  "spaces.droppedFiles",
  "spaces.sendFiles",
] as const;

export function tauriSpaceDetailOps(invoke: Invoke) {
  return {
    "spaces.usage": ({ spaceId }: Args<"spaces.usage">) => invoke<SpaceUsage | null>("space_usage", { spaceId }),
  };
}

const VIEWER = "the Tauri app's viewer reads windows and opens picture in picture over its own stream";
const DROP_WELL = "the Tauri app's viewer has its own drop zone";

export const SPACE_DETAIL_COVERAGE = {
  "spaces.usage": {
    webkit: { methods: ["spaces.usage"] },
    tauri: ["space_usage"],
  },
  "spaces.windows": {
    webkit: { methods: ["spaces.windows"] },
    tauri: [],
    unsupported: { tauri: VIEWER },
  },
  "stream.pip": {
    webkit: { methods: ["stream.pip"] },
    tauri: [],
    unsupported: { tauri: VIEWER },
  },
  "spaces.thumbnail": {
    webkit: { methods: ["spaces.thumbnail"] },
    tauri: [],
    unsupported: { tauri: "the Tauri app draws no Space thumbnails" },
  },
  "spaces.chooseFiles": {
    webkit: { methods: ["spaces.chooseFiles"] },
    tauri: [],
    unsupported: { tauri: DROP_WELL },
  },
  "spaces.droppedFiles": {
    webkit: { methods: ["spaces.droppedFiles"] },
    tauri: [],
    unsupported: { tauri: DROP_WELL },
  },
  "spaces.sendFiles": {
    webkit: { methods: ["spaces.sendFiles"] },
    tauri: [],
    unsupported: { tauri: DROP_WELL },
  },
} as const satisfies Record<keyof SpaceDetailOps, OpCoverage>;

/** Arguments that take each W5 operation down its usual path in the demo host (coverage tests). */
export const DETAIL_ARGS = {
  "telemetry.track": { signals: [{ type: "step" as const, step: "app_launched", ok: true }] },
  "spaces.usage": { spaceId: "local:design-review" },
  "spaces.windows": { spaceId: "local:design-review" },
  "stream.pip": { spaceId: "local:design-review", command: { type: "open" as const, row: "desktop" } },
  "spaces.thumbnail": { spaceId: "local:design-review" },
  "spaces.chooseFiles": {},
  "spaces.droppedFiles": { names: ["notes.txt"] },
  "spaces.sendFiles": { spaceId: "local:design-review", paths: ["/Users/demo/notes.txt"] },
  "host.setUp": { request: { mode: "relay" } },
  "host.action": { action: "stop-sharing" as const },
  "host.openSettings": { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" },
};
