// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Typed wrappers over the shell's commands (src-tauri/app/src/lib.rs).
import { invoke } from "@tauri-apps/api/core"
import type { Decision, Line, TeleportManifest } from "./logic"
import type { SpacePlan } from "./spaceWizard"
import type { Routine, RoutineFiring, RoutineSchedule } from "@trycua/cua/spaces/routines"

export interface SpaceInfo {
  id: string
  name: string
  provider: string
  spacesd_version: string
  features: string[]
}

export interface ThreadView {
  run_id: string | null
  status: string | null
  accepts_message: boolean
  transcript: Line[]
  /** The agent's last message on one line (the SDK's transcript preview). */
  preview?: string | null
}

export interface RosterEntry {
  run_id: string
  agent: string | null
  status: string
  reason: string
  accepts_message: boolean
  summary: string | null
}

export interface Avatar {
  participant_id: string
  /** The id it joined with; a Bot's is the Bot's id. */
  principal_id: string
  display_name: string
  color: string
  agent: boolean
  cursor: [number, number] | null
  me: boolean
  /** The guest's cursor shape at `cursor` (the shared art's names). */
  shape?: string
  /** Cursor opacity: fades out after 5 s without movement. */
  alpha?: number
}

/** A row of the Computer panel's window list (`list_windows`). */
export interface WindowRow {
  window_id: string
  app: string
  title: string
  width: number
  height: number
  /** The app's icon as a `data:` URL, when the Space has one. */
  icon?: string
}

export interface StreamTicket {
  ws_url: string
  media_session_id: string
  codec: string
  frame_size: [number, number]
}

/** A saved routine plus its schedule label (`routines_list`). */
export interface RoutineRow extends Routine {
  label: string
}

export interface RoutinesView {
  routines: RoutineRow[]
  log: Array<{ routineID: string; title: string; at: string; firing: RoutineFiring }>
}

export type GroupSpeakerView = { kind: "human" } | { kind: "system" } | { kind: "bot"; botID: string }

/** A group chat as the core serializes it (`cua_spaces::groups::GroupChat`). */
export interface GroupView {
  id: string
  title: string
  memberIDs: string[]
  createdAt: string
  messages: Array<{ id: string; speaker: GroupSpeakerView; text: string; at: string; undelivered: boolean; reaction?: string }>
  /** `2 of 6 bots`. */
  label: string
  /** Members producing output right now. */
  working: string[]
}

export const api = {
  /** Saves one key of the page's state in the app's data directory. */
  uiStateSet: (key: string, value: unknown) => invoke<void>("ui_state_set", { key, value }),
  listSpaces: () => invoke<SpaceInfo[]>("list_spaces"),
  addSpace: (url: string, token: string, name: string) => invoke<SpaceInfo>("add_space", { url, token, name }),
  createCloudSpace: (image: string) => invoke<SpaceInfo>("create_cloud_space", { image }),
  /** The New Space wizard: one `Spaces::create` call, on this machine or in Cua Cloud. */
  createSpace: (plan: SpacePlan) => invoke<SpaceInfo>("create_space", { plan }),
  cloudConfigured: () => invoke<boolean>("cloud_configured"),
  walkthrough: () => invoke<Walkthrough | null>("walkthrough"),
  deleteSpace: (id: string) => invoke<string>("delete_space", { id }),
  /** The desktop, or one window of the Space. */
  openStream: (space: string, windowId?: string) =>
    invoke<StreamTicket>("open_stream", { space, maxFps: 15, maxDimension: 1280, windowId: windowId ?? null }),
  listWindows: (space: string) => invoke<WindowRow[]>("list_windows", { space }),
  /** A small always-on-top window (see ./Pip). */
  openPip: (label: string, route: string, title: string, width: number, height: number) =>
    invoke<void>("open_pip", { label, route, title, width, height }),
  closePip: (label: string) => invoke<void>("close_pip", { label }),
  windowDragsAllowed: () => invoke<boolean>("window_drags_allowed"),
  botSend: (space: string, bot: string, agent: string, text: string, name?: string) =>
    invoke<ThreadView>("bot_send", { space, bot, agent, text, name: name ?? null }),
  /** The Bot list, so routines and group chats can name and hire them. */
  botsSync: (bots: Array<{ id: string; name: string; agent: string; space: string }>) => invoke<void>("bots_sync", { bots }),
  routines: () => invoke<RoutinesView>("routines_list"),
  routineCreate: (bot: string, title: string, prompt: string, schedule: RoutineSchedule) =>
    invoke<RoutinesView>("routine_create", { bot, title, prompt, schedule }),
  routineEnable: (id: string, enabled: boolean) => invoke<RoutinesView>("routine_set_enabled", { id, enabled }),
  routineDelete: (id: string) => invoke<RoutinesView>("routine_delete", { id }),
  routineRun: (id: string) => invoke<RoutinesView>("routine_run", { id }),
  groups: () => invoke<GroupView[]>("groups_list"),
  groupCreate: (title: string, members: string[]) => invoke<GroupView[]>("group_create", { title, members }),
  groupSend: (chat: string, text: string) => invoke<GroupView[]>("group_send", { chat, text }),
  groupCollect: (chat: string) => invoke<GroupView[]>("group_collect", { chat }),
  botPoll: (bot: string) => invoke<ThreadView>("bot_poll", { bot }),
  roster: (space: string) => invoke<RosterEntry[]>("roster", { space }),
  sendFile: (space: string, path: string) =>
    invoke<{ guest_path: string; bytes: number; sha256: string; verified: boolean }>("send_file", { space, path }),
  sendFileBytes: (space: string, name: string, bytes: Uint8Array) =>
    invoke<{ guest_path: string; bytes: number; sha256: string; verified: boolean }>("send_file_bytes", { space, name, bytes: Array.from(bytes) }),
  teleportManifest: (app: string) => invoke<TeleportManifest>("teleport_manifest", { app }),
  teleportApp: (space: string, app: string, decision: Decision) =>
    invoke<{ imported: string[]; bundle_bytes: number }>("teleport_app", { space, app, decision }),
  presenceJoin: (space: string, name: string) => invoke<Avatar[]>("presence_join", { space, name }),
  presencePump: () => invoke<Avatar[]>("presence_pump"),
  presenceCursor: (x: number, y: number) => invoke<void>("presence_cursor", { x, y }),
  presenceLeave: () => invoke<void>("presence_leave"),
}

/** `OPENKOALABOTS_WALKTHROUGH`: a scripted demo run through the real commands. */
export interface Walkthrough {
  theme?: "light" | "dark"
  spaces?: Array<{ url: string; token?: string; name?: string }>
  bots?: Array<{ name: string; agent?: string; say?: string[] }>
  select?: string
  sheet?: "wizard" | "address" | "bot" | "teleport"
  wizard?: Partial<import("./spaceWizard").WizardState>
  computer?: boolean
  /** Pop out `desktop` and/or the first window whose app or title matches (comma-separated). */
  pip?: string
  /** A group chat of the named Bots, sent one message, then opened. */
  group?: { title: string; bots: string[]; say?: string }
  /** Open the thread's activity groups that have a step containing this text. */
  expand?: string
  /** A routine for a named Bot, run now, then its panel opened. */
  routine?: { bot: string; title: string; prompt: string; minutes?: number; run?: boolean }
}
