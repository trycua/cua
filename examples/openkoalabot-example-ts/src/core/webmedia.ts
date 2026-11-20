/**
 * The web UI's media session setup over `@trycua/cua/browser`: the wasm
 * gRPC-Web `SpacesdClient` opens the media session (`StreamService/OpenMedia`,
 * proto3 JSON through `callJson`), and the ticket goes on the `/media`
 * WebSocket as a subprotocol (browsers cannot set headers). The env token
 * stays in the gRPC-Web channel; the socket only ever sees the ticket.
 *
 * DOM-free: the page and the Node scenario runner share it.
 */
import { MediaSocketState, mediaSubprotocols, mediaUrl } from "./wire.js"

/** What this module needs from `@trycua/cua/browser`'s `SpacesdClient`. */
export interface JsonRpc {
  callJson(method: string, requestJson: string): Promise<string>
}

export interface OpenedMedia {
  mediaSessionId: string
  ticket: string
  wsUrl: string
  codec: string
  width: number
  height: number
}

export interface OpenMediaOptions {
  display?: string
  maxFps?: number
  maxDimension?: number
  ticketTtlMs?: number
}

/** The proto3-JSON body of `OpenMediaRequest` for a desktop target. */
export function openMediaRequest(o: OpenMediaOptions = {}): Record<string, unknown> {
  const req: Record<string, unknown> = {
    target: { displayId: o.display ?? "primary" },
    codecs: ["MEDIA_CODEC_H264"],
    maxFps: o.maxFps ?? 30,
    maxDimension: o.maxDimension ?? 0,
  }
  if (o.ticketTtlMs) req.ticketTtl = `${(o.ticketTtlMs / 1000).toFixed(3)}s`
  return req
}

/** Parses an `OpenMediaResponse` (proto3 JSON) against the driver base URL. */
export function parseOpenMedia(baseUrl: string, json: string): OpenedMedia {
  const r = JSON.parse(json) as Record<string, unknown>
  const wsPath = String(r.wsPath ?? r.ws_path ?? "")
  const ticket = String(r.ticket ?? "")
  const id = String(r.mediaSessionId ?? r.media_session_id ?? "")
  if (!wsPath || !ticket || !id) throw new Error(`OpenMedia returned no ticket/ws_path: ${json.slice(0, 200)}`)
  const geometry = (r.geometry ?? {}) as Record<string, unknown>
  const size = (geometry.frameSize ?? geometry.frame_size ?? {}) as Record<string, unknown>
  return {
    mediaSessionId: id,
    ticket,
    wsUrl: mediaUrl(baseUrl, wsPath),
    codec: String(r.codec ?? ""),
    width: Number(size.width ?? geometry.widthPx ?? 0),
    height: Number(size.height ?? geometry.heightPx ?? 0),
  }
}

export async function openDesktopMedia(env: JsonRpc, baseUrl: string, o: OpenMediaOptions = {}): Promise<OpenedMedia> {
  const json = await env.callJson("StreamService/OpenMedia", JSON.stringify(openMediaRequest(o)))
  return parseOpenMedia(baseUrl, json)
}

export async function closeMedia(env: JsonRpc, mediaSessionId: string): Promise<void> {
  await env.callJson("StreamService/CloseMedia", JSON.stringify({ mediaSessionId }))
}

type WS = {
  binaryType: string
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: ((ev: { code: number; reason: string }) => void) | null
  onerror: ((ev: unknown) => void) | null
  close(code?: number): void
}

export interface AttachedMedia {
  ws: WS
  state: MediaSocketState
  /** How the ticket was presented: the header-free subprotocol, or the
   * `?ticket=` query `ws_path` already carries. */
  ticketVia: "subprotocol" | "query"
}

type Ctor = new (url: string, protocols?: string[]) => WS

/**
 * Attaches to `/media` and resolves after the first video packet, with the
 * socket still open (the caller keeps or closes it). Bounded by `timeoutMs`.
 *
 * The ticket goes as the `cua.ticket.<t>` subprotocol first. A WHATWG
 * WebSocket fails the handshake when the server does not echo a requested
 * subprotocol, so if the socket dies before its first message and the URL
 * already carries `?ticket=`, it retries once with the query alone.
 */
export async function attachMedia(
  WebSocketCtor: Ctor,
  media: OpenedMedia,
  timeoutMs: number,
  onPacket?: (state: MediaSocketState, data: ArrayBuffer) => void,
): Promise<AttachedMedia> {
  try {
    return await attachOnce(WebSocketCtor, media, timeoutMs, true, onPacket)
  } catch (e) {
    const early = e instanceof EarlyClose
    if (!early || !/[?&]ticket=/.test(media.wsUrl)) throw e
    return attachOnce(WebSocketCtor, media, timeoutMs, false, onPacket)
  }
}

class EarlyClose extends Error {}

function attachOnce(
  WebSocketCtor: Ctor,
  media: OpenedMedia,
  timeoutMs: number,
  subprotocol: boolean,
  onPacket?: (state: MediaSocketState, data: ArrayBuffer) => void,
): Promise<AttachedMedia> {
  const state = new MediaSocketState()
  const ws = subprotocol ? new WebSocketCtor(media.wsUrl, mediaSubprotocols(media.ticket)) : new WebSocketCtor(media.wsUrl)
  const ticketVia = subprotocol ? "subprotocol" : "query"
  ws.binaryType = "arraybuffer"
  let messages = 0
  return new Promise((resolve, reject) => {
    let settled = false
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      ws.close(1000)
      reject(new Error(`no video packet within ${timeoutMs} ms`))
    }, timeoutMs)
    ws.onmessage = (ev) => {
      messages += 1
      if (typeof ev.data === "string") {
        state.onText(ev.data)
        return
      }
      const data = ev.data as ArrayBuffer
      const d = state.onBinary(data)
      onPacket?.(state, data)
      if (d && !settled) {
        settled = true
        clearTimeout(timer)
        resolve({ ws, state, ticketVia })
      }
    }
    ws.onclose = (ev) => {
      state.onClose(ev.code, ev.reason)
      if (!settled) {
        settled = true
        clearTimeout(timer)
        const msg = `media socket closed ${ev.code} ${ev.reason}`
        reject(subprotocol && messages === 0 && ev.code === 1006 ? new EarlyClose(msg) : new Error(msg))
      }
    }
    ws.onerror = () => {}
  })
}
