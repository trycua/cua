/**
 * Native runtime plumbing: an embedded cua SDK whose every home is a
 * directory we chose, and the two desktop-stream paths (the SDK's native
 * session, and the web UI's wire parser over a ticketed `/media` socket).
 *
 * Host safety: `spacesHome`, `stateDir` and `teleportHome` are always passed,
 * so nothing reads or writes `~/.cua` or a real app profile, and
 * `agentCredentialsHome` stays unset so no agent login is ever copied.
 */
import { embedded, type CuaLike } from "@trycua/cua"
import {
  SpaceStreamOptions,
  type FrameSink,
  type MediaEvent,
  type MediaStats,
  type SpaceLike,
  type VideoFrame,
} from "@trycua/cua/spaces"
import { mkdirSync, readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { WebSocket as WsClient } from "ws"
import { join } from "node:path"
import type { MediaSocketState } from "./wire.js"
import { attachMedia, closeMedia, openDesktopMedia } from "./webmedia.js"
import { sleep } from "./app.js"

export interface RuntimeDirs {
  root: string
  teleportHome: string
  /** Read Cua Cloud credentials from the environment. */
  cloudFromEnv?: boolean
}

/** An embedded runtime rooted at `dirs.root`. */
export function openRuntime(dirs: RuntimeDirs): CuaLike {
  mkdirSync(dirs.root, { recursive: true })
  return embedded({
    stateDir: join(dirs.root, "sandboxes"),
    spacesHome: join(dirs.root, "cua"),
    teleportHome: dirs.teleportHome,
    fleetFromEnv: dirs.cloudFromEnv ?? false,
  })
}

/** Counts frames; keeps no pixels. */
export class FrameCounter implements FrameSink {
  frames = 0
  keyframes = 0
  firstIsKey: boolean | null = null
  width = 0
  height = 0
  codec = ""
  closed = 0

  onFrame(frame: VideoFrame): void {
    if (this.firstIsKey === null) this.firstIsKey = frame.keyframe
    this.frames += 1
    if (frame.keyframe) this.keyframes += 1
    this.width = frame.width
    this.height = frame.height
    this.codec = frame.codec
  }

  onEvent(event: MediaEvent): void {
    if (event.kind === "closed") this.closed += 1
  }
}

async function waitCount(read: () => number, n: number, pollMs: number, maxPolls: number, what: string): Promise<void> {
  for (let i = 0; i < maxPolls; i++) {
    if (read() >= n) return
    await sleep(pollMs)
  }
  throw new Error(`only ${read()} ${what} arrived (wanted ${n})`)
}

export interface NativeStreamResult {
  frames: number
  keyframes: number
  firstIsKey: boolean | null
  width: number
  height: number
  codec: string
  stats: MediaStats
  keyframeRequests: number
}

/** Desktop stream through the SDK (a ticket, then the media socket's
 * encoded frames): first frame is a keyframe, and a keyframe request
 * produces another frame. */
export async function nativeDesktopStream(
  space: SpaceLike,
  opts: { maxFps: number; maxDimension: number; pollMs: number; maxPolls: number },
): Promise<NativeStreamResult> {
  const counter = new FrameCounter()
  // #region docs:ts-stream
  const ticket = await space.openStream(
    SpaceStreamOptions.create({ maxFps: opts.maxFps, maxDimension: opts.maxDimension }),
  )
  const session = await space.attachStream(ticket, counter, undefined)
  // #endregion docs:ts-stream
  let stats: MediaStats
  let keyframeRequests = 0
  try {
    await waitCount(() => counter.frames, 1, opts.pollMs, opts.maxPolls, "frames")
    await session.requestKeyframe()
    keyframeRequests += 1
    await waitCount(() => counter.frames, 2, opts.pollMs, opts.maxPolls, "frames after a keyframe request")
  } finally {
    stats = session.stats()
    await session.close()
    await space.closeStream(ticket.mediaSessionId)
  }
  return {
    frames: counter.frames,
    keyframes: counter.keyframes,
    firstIsKey: counter.firstIsKey,
    width: counter.width,
    height: counter.height,
    codec: counter.codec,
    stats,
    keyframeRequests,
  }
}

/** The web UI's stream path, in Node. */
export interface WebProbeResult {
  via: "browser-wasm" | "sdk-ticket"
  ticketVia: "subprotocol" | "query"
  client: string
  state: MediaSocketState
}

/** Loads `@trycua/cua/browser` (the wasm/gRPC-Web build) in Node. */
export async function loadBrowserSdk(): Promise<typeof import("@trycua/cua/browser")> {
  const sdk = await import("@trycua/cua/browser")
  const wasm = new URL("./wasm-bindgen/index_bg.wasm", import.meta.resolve("@trycua/cua/browser"))
  await sdk.initialize(readFileSync(fileURLToPath(wasm)))
  return sdk
}

/**
 * Runs the web UI's streaming code in Node: with a direct URL and token, the
 * wasm `SpacesdClient` opens the session over gRPC-Web exactly as the page does;
 * otherwise the SDK mints the ticket (`openStream`, what the local server
 * hands the page). Either way the `/media` socket is attached with the
 * ticket subprotocol through Node's WHATWG `WebSocket`, and the same
 * `MediaSocketState` checks the handshake and the first packet.
 */
export async function webClientProbe(
  space: SpaceLike,
  opts: { maxFps: number; maxDimension: number; timeoutMs: number; url?: string; token?: string },
): Promise<WebProbeResult> {
  const whatwg = (globalThis as { WebSocket?: unknown }).WebSocket
  // Node's WHATWG WebSocket first (the browser's semantics); the `ws` client
  // as a fallback, since undici may reuse the gRPC-Web keep-alive connection
  // for the upgrade, which a server that routes `/media` per connection
  // (cua-test-fixtures) does not follow.
  const clients: Array<[string, unknown]> = whatwg ? [["whatwg", whatwg], ["ws", WsClient]] : [["ws", WsClient]]
  let lastError: unknown
  for (const [client, ctor] of clients) {
    try {
      return { ...(await probeOnce(space, opts, ctor)), client }
    } catch (e) {
      lastError = e
    }
  }
  throw lastError
}

async function probeOnce(
  space: SpaceLike,
  opts: { maxFps: number; maxDimension: number; timeoutMs: number; url?: string; token?: string },
  ctor: unknown,
): Promise<Omit<WebProbeResult, "client">> {
  if (opts.url && opts.token !== undefined) {
    const sdk = await loadBrowserSdk()
    const env = await sdk.Cua.embedded(sdk.CuaConfig.create({})).spacesd(opts.url, opts.token)
    const media = await openDesktopMedia(env, opts.url, { maxFps: opts.maxFps, maxDimension: opts.maxDimension })
    try {
      const { ws, state, ticketVia } = await attachMedia(ctor as never, media, opts.timeoutMs)
      ws.close(1000)
      return { via: "browser-wasm", ticketVia, state }
    } finally {
      await closeMedia(env, media.mediaSessionId).catch(() => {})
    }
  }
  // #region docs:ts-ticket
  const ticket = await space.openStream(
    SpaceStreamOptions.create({ maxFps: opts.maxFps, maxDimension: opts.maxDimension, codecs: ["h264"] }),
  )
  // #endregion docs:ts-ticket
  try {
    if (ticket.needsHeaders) {
      throw new Error("this Space's media socket needs gateway headers, which a browser WebSocket cannot send")
    }
    const media = { mediaSessionId: ticket.mediaSessionId, ticket: ticket.ticket, wsUrl: ticket.wsUrl, codec: ticket.codec, width: ticket.width, height: ticket.height }
    const { ws, state, ticketVia } = await attachMedia(ctor as never, media, opts.timeoutMs)
    ws.close(1000)
    return { via: "sdk-ticket", ticketVia, state }
  } finally {
    await space.closeStream(ticket.mediaSessionId).catch(() => {})
  }
}
