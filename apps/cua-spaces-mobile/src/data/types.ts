// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** What the screens read. Both backends (the live relay client and the labelled sample data) produce these. */

import type { AgentMember, ThreadPage, ThreadEntry, ThreadView, LiveAgent } from "./thread/protocol";
import type { AgentTone } from "../core/views";
import type { TaskRequest } from "../core/phone";
import type { ChangeSet, FileDiffData, FilePreviewData, TaskOptions, TaskResult, ToolGroup } from "./thread/phone-protocol";

export type { ChangeSet, FileDiffData, FilePreviewData, TaskOptions, TaskResult, ToolGroup };

export type { AgentMember, ThreadPage, ThreadEntry, ThreadView, LiveAgent };

/** A machine the account can reach. State is on the row; there is no separate machines page. */
export interface MachineInfo {
  id: string;
  name: string;
  /** `mac`, `pc`, `linux` or `cloud`: picks the glyph. */
  kind: "mac" | "pc" | "linux" | "cloud";
  online: boolean;
  /** When it was last seen, for an offline machine (Unix ms). */
  lastSeenMs: number | null;
}

/** App Space: one app on a machine made into a Space. Whole machine: the machine itself. */
export type SpaceScope = "app" | "machine";

/** The scene a sample thumbnail draws; live thumbnails come from the stream. */
export type Scene = "code" | "browser" | "notes" | "terminal" | "design" | "desktop" | "blank";

export interface SpaceMemberRow {
  /** Handle or account id. */
  id: string;
  name: string;
  kind: "person" | "agent";
  harness: string | null;
  status: string;
  activity: string | null;
  role: string | null;
  here: boolean;
}

export interface SpaceSummary {
  /** The Space id the daemon calls it (`relay:<machine id>`). */
  id: string;
  name: string;
  machineId: string;
  machineName: string;
  machineOnline: boolean;
  scope: SpaceScope;
  /** The OS label under the name ("macOS VM", "Linux"). */
  kindLabel: string;
  /** One status for the row, in the core's tone words. */
  tone: AgentTone;
  /** What the status says ("Needs you", "Running", "Asleep"). */
  statusWord: string;
  /** What the status adds ("Waiting for you in checkout-api", "Running tests"). */
  statusLine: string;
  members: SpaceMemberRow[];
  lastMessage: { text: string; author: string; tsMs: number } | null;
  unread: number;
  /** A sample thumbnail scene (live thumbnails replace it when the stream is up). */
  scene: Scene;
  /** `owner`, `editor` or `viewer`. */
  role: "owner" | "editor" | "viewer";
  /** The reader can put this Space to sleep from here. */
  canSleep: boolean;
  /** The reader can share it from here (owners). */
  canShare: boolean;
}

export interface SpaceList {
  spaces: SpaceSummary[];
  /** The computers a call can go to (the Spaces' computers); the whole pool, phones included, is `computers`. */
  machines: MachineInfo[];
  /** The pool's computers and phones, each with its state: a computer is never a row of `spaces`. */
  computers?: import("../core/views").ComputerRow[];
  /** The core's words around the lists: while reading, and when there is nothing to show. */
  words?: { reachingLine: string | null; empty: { title: string; body: string } | null };
  /** Some machines did not answer yet. */
  incomplete: boolean;
  reaching: number;
  /** Agent rows for the conversation list (the core's order and words). */
  agents: import("../core/views").AgentListRow[];
}

/** The Space or computer runs Cua Spaces from before phones: the relay does not send a phone's calls to it. */
export class NeedsUpdateError extends Error {
  constructor(readonly what: "Space" | "computer") {
    super(
      what === "Space"
        ? "This Space needs an update. Update Cua Spaces on the computer it runs on, then try again."
        : "This computer needs an update. Update Cua Spaces on it, then try again.",
    );
    this.name = "NeedsUpdateError";
  }
}

/** The relay's words for a computer that predates phones (`PHONE_OLD_COMPUTER`). */
export const OLD_COMPUTER_REFUSAL = /needs the latest Cua Spaces to be used from a phone/i;

/** The computer refused a touch: the share lets this account look but not touch, or the desktop is not shared. */
export class ControlNotAllowedError extends Error {
  constructor() {
    super("You can only look at this desktop. Its owner can let you control it in the share.");
    this.name = "ControlNotAllowedError";
  }
}

export interface StreamTicket {
  /** `https` viewer page (the daemon's HTML5 viewer, ticket in the fragment). */
  viewerUrl: string;
  /** When the ticket stops working (ms since the epoch): the page is loaded again with a new one before. It carries no account credential: the ticket in the fragment is all the page has. */
  expiresAtMs: number;
  /** Pixel size of the desktop, when known (for touch mapping). */
  width: number | null;
  height: number | null;
}

/** A touch becomes one of these; points are normalized (0 to 1 across the desktop), which the Space's computer service takes as is. */
export type InputAction =
  | { type: "click"; x: number; y: number; button?: "left" | "right" | "double" }
  | { type: "scroll"; x: number; y: number; dx: number; dy: number }
  | { type: "type"; text: string }
  | { type: "key"; key: string; modifiers?: string[] }
  | { type: "drag"; from: { x: number; y: number }; to: { x: number; y: number } };

/** What a backend can do today; screens hide or explain what it cannot. */
export interface Capabilities {
  /** Sample data, shown with a label on every screen. */
  sample: boolean;
  stream: "viewer" | "none";
  input: boolean;
  sleep: boolean;
  share: boolean;
  /** Start a task: make a Space or message an agent from the phone. */
  startTask: boolean;
  /** Review what an agent changed (file list, diff, preview). */
  diff: boolean;
  /** Allow once, allow for this Space, decline. */
  scopes: boolean;
  /** Steer or queue a message while an agent runs. */
  steer: boolean;
  /** Push to a closed app (the relay can register this phone). */
  push: boolean;
  /** Why a missing capability is missing, in plain words. */
  why: { stream?: string; input?: string; sleep?: string; share?: string; startTask?: string; diff?: string; push?: string };
}

/** Push registration for this phone (APNs token to the relay). */
export interface PushTransport {
  /** Whether the relay can send push at all (`GET /v1/push`: a provider is set up). Throws where the relay has no push. */
  status(): Promise<{ configured: boolean }>;
  register(body: Record<string, unknown>): Promise<void>;
  unregister(): Promise<void>;
}

export interface PostOptions {
  replyTo?: number | null;
  view?: ThreadView | null;
  /** The outbox's id for it: the idempotency key sent to the daemon. */
  clientId?: string | null;
  /** How it reaches an agent that is running: `queue` waits behind the turn, `steer` interrupts it first. */
  mode?: "send" | "queue" | "steer";
  /** The agent a steer interrupts (its handle). */
  agent?: string | null;
}

export interface AnswerOptions {
  scope?: "once" | "space";
  clientId?: string | null;
}

export interface Backend {
  readonly kind: "live" | "demo";
  /** Sample builds only: a switch that makes the phone pretend it has no connection, to see the offline states. */
  readonly sample?: { readonly offline: boolean; setOffline(on: boolean): void };
  readonly capabilities: Capabilities;
  listSpaces(): Promise<SpaceList>;
  readThread(
    spaceId: string,
    opts: { after?: number | null; before?: number | null; limit?: number; followMs?: number; markRead?: boolean; liveAfter?: string | null },
  ): Promise<ThreadPage>;
  members(spaceId: string): Promise<AgentMember[]>;
  post(spaceId: string, text: string, opts?: PostOptions): Promise<ThreadEntry | null>;
  answer(spaceId: string, approval: number, approve: boolean, opts?: AnswerOptions): Promise<ThreadEntry | null>;
  streamTicket(spaceId: string): Promise<StreamTicket | null>;
  sendInput(spaceId: string, action: InputAction): Promise<void>;
  /** Pause the agents while a person drives (taking control) and resume them. */
  takeControl(spaceId: string, on: boolean): Promise<void>;
  /** Interrupts what an agent is doing now (the stop button). */
  stopAgent(spaceId: string, handle: string): Promise<void>;
  /**
   * The tool calls agents made, as the daemon's rows (`thread_tool_rows`): the sample backend answers in the same
   * wire shape and both go through `toolRowsFromTool`. Absent where the daemon does not send them yet.
   */
  toolRows?(spaceId: string, opts?: { cursor?: number | null; followMs?: number }): Promise<{ groups: ToolGroup[]; cursor: number; caughtUp: boolean }>;
  /** What can start a task now: machines (online, room) and agents (connected). */
  taskOptions?(): Promise<TaskOptions>;
  /** Starts a task: the machine's daemon makes the Space (or finds the agent's home) and posts the job. Idempotent on `client_id`. */
  startTask?(req: TaskRequest): Promise<TaskResult>;
  /** How a task that was still being made is coming (`task_status`): the Space when it exists, or why it failed. */
  taskStatus?(clientId: string): Promise<TaskResult>;
  /** The files an agent changed in a Space, one file's diff, and a file to preview. */
  changes?(spaceId: string, opts?: { repo?: string | null }): Promise<ChangeSet>;
  fileDiff?(spaceId: string, path: string, opts?: { repo?: string | null }): Promise<FileDiffData>;
  filePreview?(spaceId: string, path: string, opts?: { repo?: string | null; rev?: "head" | "base" | "working" }): Promise<FilePreviewData>;
  /** Registers this phone's push token with the relay; absent where there is no push path. */
  push?: PushTransport;
  /** Removes every phone of the account (this one too) from it, at once; absent where there is no account. Needs a recent sign-in. */
  removeAllPhones?(): Promise<number>;
  sleep(spaceId: string): Promise<void>;
  share(spaceId: string, email: string, role: "editor" | "viewer"): Promise<void>;
}
