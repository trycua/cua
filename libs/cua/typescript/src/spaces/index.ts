/**
 * `@trycua/cua/spaces`: Cua Spaces for Node, on the native cua SDK.
 *
 * ```ts
 * import { embedded } from "@trycua/cua"
 * import { approve, startThread, SpaceSendFileOptions } from "@trycua/cua/spaces"
 *
 * const spaces = embedded().spaces()               // or connect().spaces() (cua daemon)
 * const info = await spaces.add("http://10.0.0.5:3211", token, "dev")
 * const space = await spaces.space(info.id)
 * console.log((await space.bash("uname -a", undefined)).stdout)
 * await space.sendFile("./report.pdf", SpaceSendFileOptions.create({}))
 * await space.teleport("firefox", undefined, approve((manifest) => ({
 *   include: undefined, acknowledgeSensitive: true,       // after asking a human
 * })))
 * const bot = await startThread(spaces, {
 *   agent: "claude-code", prompt: "summarise the open PRs",
 *   placement: { kind: "shared", space, acknowledgeNoIsolation: true },
 * })
 * for await (const e of bot.events({ maxPolls: 60 })) if (e.kind === "text") console.log(e.text)
 * ```
 *
 * Every call is the Rust `cua-spaces` implementation (embedded, or in
 * `cua daemon` through `SpaceService`); `spaces.create({ on })` makes a
 * Space locally or in the cloud. Webviews and browsers, which cannot
 * load the native binding, use `@trycua/cua/spaces/transport` (MCP over the
 * daemon's `/mcp` or a Tauri `invoke`) instead.
 */

import { type TeleportApprover, type TeleportDecision, type TeleportManifest } from "../native/index.js"

export {
  AgentActionReport,
  AgentRunStatus,
  AgentStartReport,
  PresenceCursor,
  PresenceIdentity,
  Space,
  SpaceCreateOptions,
  SpaceCreateResult,
  SpacePresence,
  SpaceSendFileOptions,
  SpaceStreamOptions,
  SpaceStreamSession,
  Spaces,
  TeleportDecision,
  spacesToolMethods,
} from "../native/index.js"
export type {
  AudioPacket,
  AudioSink,
  FrameSink,
  MediaEvent,
  MediaSessionLike,
  MediaStats,
  PresenceEvent,
  PresenceMember,
  PresenceParticipant,
  SpaceBashResult,
  SpaceHotspotStatus,
  SpaceInfo,
  SpaceLike,
  SpacePresenceLike,
  SpaceSendFileReport,
  SpaceSentFile,
  SpaceStreamSessionLike,
  SpaceStreamStats,
  SpaceStreamTicket,
  SpaceToolInfo,
  SpaceToolResult,
  SpaceTransferReport,
  SpaceWindow,
  SpaceWriteReport,
  SpacesLike,
  SpacesToolMethod,
  TeleportApprover,
  TeleportItem,
  TeleportManifest,
  TeleportReceipt,
  VideoFrame,
} from "../native/index.js"

export { SpacesError, isSpacesError, excerpt, toSpacesError, wrapErrors } from "./errors.js"
export type { SpacesErrorCode, SpacesErrorOptions } from "./errors.js"
export { TranscriptAdapter, detectApproval } from "./events.js"
export type {
  AgentStatus,
  ApprovalRequestEvent,
  ErrorEvent,
  FileEvent,
  ImageEvent,
  LinkEvent,
  StateEvent,
  TextEvent,
  ThreadEvent,
  ThreadEventKind,
  ThreadStatus,
} from "./events.js"
export {
  AGENT_IDS,
  SETTLED,
  SpaceTurnLock,
  Thread,
  adoptThread,
  lockFor,
  startThread,
  validatePlacement,
} from "./thread.js"
export type { AgentId, EventOptions, StartThreadOptions, ThreadPlacement, Turn } from "./thread.js"
export {
  CURSOR_ART,
  CURSOR_SHAPES,
  CursorSmoother,
  DATAGRAM_DELAY_MS,
  PRESENCE_FALLBACK_COLOR,
  PRESENCE_PALETTE,
  PresenceRoster,
  PresenceView,
  STREAM_DELAY_MS,
  cursorArt,
  cursorArtSvg,
  idleAlpha,
  isCursorShape,
  agentIdentity,
  presenceColor,
  presenceTextColor,
  waitForPresence,
} from "./presence.js"
export type { CursorArtShape, CursorShapeName, PresenceDrawable, PresenceEntry } from "./presence.js"
export * from "./routines.js"
export * from "./groups.js"

/**
 * A {@link TeleportApprover} from a function. The function sees exactly what
 * would leave this machine and returns the human's decision, or `undefined`
 * to cancel (the teleport then fails with `TeleportRefused`). It runs on a
 * worker thread of the SDK and must be synchronous.
 */
export function approve(
  decide: (manifest: TeleportManifest) => TeleportDecision | undefined,
): TeleportApprover {
  return { approve: decide }
}
