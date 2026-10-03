// The shared streaming scenario (examples/streaming/SCENARIO.md) on
// @trycua/cua for Node: connect, start the grid fixture, list targets,
// stream the desktop and the grid window (decoded H.264 -> BGRA, Opus -> PCM
// through the SDK), click a grid cell over the media plane, and print one
// SUMMARY line. With CUA_BENCH_JSONL it is a benchmark "language lane".
//
// Run: node main.ts   (Node >= 23.6 strips the types; see README.md)
import { closeSync, mkdirSync, openSync, writeFileSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import {
  embedded,
  MediaOpenOptions,
  type DecodedVideoFrame,
  type SpacesdClientLike,
  type MediaEvent,
  type MediaSessionLike,
  type PcmAudio,
  type VideoFrame,
  type AudioPacket,
} from "@trycua/cua"

import { BGRA, fnv1a64, pixelAt, readTimecode, unixNs, wavBytes } from "./pixels.ts"

const ENV_URL = process.env.CUA_ENV_URL ?? "http://127.0.0.1:33211"
const ENV_TOKEN = process.env.CUA_ENV_TOKEN
const SECONDS = Number(process.env.CUA_STREAM_SECONDS ?? "5")
const OUT_DIR = process.env.CUA_OUT_DIR ?? "./out"
const BENCH_JSONL = process.env.CUA_BENCH_JSONL
// `decoded` (default): the SDK decodes (VideoToolbox / OpenH264, libopus);
// pixels are hashed, the timecode is read, audio goes to WAV. `encoded`: raw
// access units and Opus packets (bytes/keyframes known, no pixels, no WAV).
const MODE = (process.env.CUA_NODE_MEDIA ?? "decoded") === "encoded" ? "encoded" : "decoded"

const GRID_TITLE = "CUA Fixture Grid"
const GRID_LOG = "/tmp/cua-fixtures/grid.jsonl"
const CLICK_CELL = [2, 3]
const CLICK_CONTENT = { x: 200, y: 280 }
const CELL_RGB = [72, 153, 128]

type Target = {
  kind: "display" | "window"
  id: string
  title: string
  width: number
  height: number
  x: number
  y: number
  available: boolean
  primary: boolean
}

type StreamStats = {
  frames: number
  keyframes: number | null
  bytes: number | null
  first_frame_ms: number | null
  fps: number
  audio_packets: number
  last_hash: string | null
  hash_of: "bgra" | "access_unit"
  wav: string | null
  decode_errors: number
}

type LastFrame = {
  sequence: bigint
  geometryEpoch: bigint
  width: number
  height: number
  stride: number
  data: Uint8Array
}

const log = (...a: unknown[]) => console.log(...a)

// ---------------------------------------------------------------- targets

function num(v: unknown): number {
  return typeof v === "number" ? v : typeof v === "string" ? Number(v) : 0
}

async function listTargets(env: SpacesdClientLike): Promise<Target[]> {
  const raw = JSON.parse(
    await env.callJson("StreamService/ListTargets", JSON.stringify({ includeWindows: true })),
  )
  const out: Target[] = []
  for (const t of raw.targets ?? []) {
    const available = t.available ?? false
    if (t.display) {
      const d = t.display
      const b = d.bounds ?? {}
      const n = d.nativeSize ?? d.native_size ?? {}
      out.push({
        kind: "display",
        id: d.id ?? "",
        title: d.name ?? "",
        width: num(n.width) || num(b.width),
        height: num(n.height) || num(b.height),
        x: num(b.x),
        y: num(b.y),
        available,
        primary: d.primary ?? false,
      })
    } else if (t.window) {
      const w = t.window
      const b = w.bounds ?? {}
      out.push({
        kind: "window",
        id: w.ref?.id ?? "",
        title: w.title ?? "",
        width: num(b.width),
        height: num(b.height),
        x: num(b.x),
        y: num(b.y),
        available,
        primary: false,
      })
    }
  }
  return out
}

// ---------------------------------------------------------------- bench JSONL

class Jsonl {
  fd: number | null
  constructor(path: string | undefined) {
    this.fd = path ? openSync(path, "a") : null
  }
  write(obj: Record<string, unknown>) {
    if (this.fd === null) return
    writeSync(
      this.fd,
      JSON.stringify(obj, (_, v) => (typeof v === "bigint" ? Number(v) : v)) + "\n",
    )
  }
  // unix_ns must stay exact: splice it in as a raw integer.
  line(t: string, fields: Record<string, unknown> = {}) {
    if (this.fd === null) return
    const rest = JSON.stringify(fields, (_, v) => (typeof v === "bigint" ? Number(v) : v))
    const body = rest === "{}" ? "" : "," + rest.slice(1, -1)
    writeSync(this.fd, `{"t":${JSON.stringify(t)},"unix_ns":${unixNs()}${body}}\n`)
  }
  close() {
    if (this.fd !== null) closeSync(this.fd)
    this.fd = null
  }
}

// ---------------------------------------------------------------- streaming

type StreamOptions = {
  target: Target
  audio: boolean
  seconds: number
  wavName: string | null
  jsonl: Jsonl
  // Called once while the stream is open, after the first frame.
  during?: (s: OpenStream) => Promise<void>
  requestJson?: string
}

class OpenStream {
  session!: MediaSessionLike
  last: LastFrame | null = null
  lastAu: Uint8Array | null = null
  events: MediaEvent[] = []
  frames = 0
  keyframes = 0
  bytes = 0
  audioPackets = 0
  decodeErrors = 0
  firstFrameAt: number | null = null
  openedAt = 0
  pcm: Int16Array[] = []
  pcmRate = 48000
  pcmChannels = 2
  scale = 1
  waiters: Array<(e: MediaEvent) => boolean> = []

  opts: StreamOptions
  constructor(opts: StreamOptions) {
    this.opts = opts
  }

  onEvent(e: MediaEvent) {
    if (this.events.length < 1000) this.events.push(e)
    if (e.kind === "decode_error") this.decodeErrors++
    this.waiters = this.waiters.filter((w) => !w(e))
  }

  waitEvent(pred: (e: MediaEvent) => boolean, ms: number): Promise<MediaEvent | null> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== waiter)
        resolve(null)
      }, ms)
      const waiter = (e: MediaEvent) => {
        if (!pred(e)) return false
        clearTimeout(timer)
        resolve(e)
        return true
      }
      this.waiters.push(waiter)
    })
  }

  firstFrame() {
    if (this.firstFrameAt === null) this.firstFrameAt = performance.now()
  }

  onDecoded(f: DecodedVideoFrame) {
    this.frames++
    this.firstFrame()
    const data = new Uint8Array(f.data)
    this.last = {
      sequence: f.sequence,
      geometryEpoch: f.geometryEpoch,
      width: f.width,
      height: f.height,
      stride: f.stride,
      data,
    }
    if (this.opts.jsonl.fd !== null) {
      const clientMs = performance.timeOrigin + performance.now()
      const tc = readTimecode(
        data,
        f.width,
        f.height,
        f.stride,
        f.width / (this.opts.target.width || f.width),
        BGRA,
        clientMs,
      )
      this.opts.jsonl.line("frame", {
        seq: f.sequence,
        bytes: null, // the decoded callback does not expose the access-unit size
        key: null, // nor the keyframe flag
        cap_us: f.captureTimestampUs,
        w: f.width,
        h: f.height,
        tc_ms: tc,
      })
    }
  }

  onEncoded(f: VideoFrame) {
    this.frames++
    this.firstFrame()
    if (f.keyframe) this.keyframes++
    this.bytes += f.data.byteLength
    this.lastAu = new Uint8Array(f.data)
    this.last = {
      sequence: f.sequence,
      geometryEpoch: f.geometryEpoch,
      width: f.width,
      height: f.height,
      stride: 0,
      data: new Uint8Array(),
    }
    this.opts.jsonl.line("frame", {
      seq: f.sequence,
      bytes: f.data.byteLength,
      key: f.keyframe,
      cap_us: f.captureTimestampUs,
      w: f.width,
      h: f.height,
      tc_ms: null,
    })
  }

  onPcm(a: PcmAudio) {
    this.audioPackets++
    this.pcmRate = a.sampleRate
    this.pcmChannels = a.channels
    const samples = Int16Array.from(a.samples)
    if (this.opts.wavName) this.pcm.push(samples)
    this.opts.jsonl.line("audio", {
      pts_us: a.ptsUs,
      bytes: null, // the PCM callback does not expose the Opus packet size
      samples: samples.length / Math.max(1, a.channels),
    })
  }

  onAudioPacket(p: AudioPacket) {
    this.audioPackets++
    this.opts.jsonl.line("audio", {
      pts_us: p.ptsUs,
      bytes: p.data.byteLength,
      samples: p.frameSamples,
    })
  }
}

async function stream(env: SpacesdClientLike, opts: StreamOptions): Promise<[StreamStats, OpenStream]> {
  const s = new OpenStream(opts)
  const t = opts.target
  const options = MediaOpenOptions.create({
    display: t.kind === "display" ? t.id : undefined,
    windowHandle: t.kind === "window" ? t.id : undefined,
    maxFps: 30,
    maxDimension: 0,
    audio: opts.audio,
    disableVideo: false,
    requestJson: opts.requestJson,
  })
  opts.jsonl.line("open")
  s.openedAt = performance.now()
  if (MODE === "decoded") {
    s.session = await env.openMediaDecodedWithAudio(
      options,
      { onDecodedFrame: (f) => s.onDecoded(f), onEvent: (e) => s.onEvent(e) },
      { onPcm: (a) => s.onPcm(a) },
    )
  } else {
    s.session = await env.openMediaWithAudio(
      options,
      { onFrame: (f) => s.onEncoded(f), onEvent: (e) => s.onEvent(e) },
      { onAudio: (p) => s.onAudioPacket(p) },
    )
  }
  const deadline = s.openedAt + opts.seconds * 1000
  if (opts.during) {
    // Wait (bounded) for a first frame, then run the in-stream step.
    for (let i = 0; i < 100 && s.frames === 0 && performance.now() < deadline; i++) await sleep(50)
    await opts.during(s)
  }
  const left = deadline - performance.now()
  if (left > 0) await sleep(left)
  const endAt = performance.now()
  await s.session.close()

  const first = s.firstFrameAt === null ? null : s.firstFrameAt - s.openedAt
  const span = s.firstFrameAt === null ? 0 : (endAt - s.firstFrameAt) / 1000
  let wav: string | null = null
  if (opts.wavName && MODE === "decoded") {
    mkdirSync(OUT_DIR, { recursive: true })
    wav = join(OUT_DIR, opts.wavName)
    writeFileSync(wav, wavBytes(s.pcm, s.pcmRate, s.pcmChannels))
  }
  let lastHash: string | null = null
  if (MODE === "decoded" && s.last) {
    const { data, width, height, stride } = s.last
    // Hash the packed width*4 bytes of every row.
    let packed = data.subarray(0, width * height * 4)
    if (stride !== width * 4) {
      packed = new Uint8Array(width * height * 4)
      for (let y = 0; y < height; y++)
        packed.set(data.subarray(y * stride, y * stride + width * 4), y * width * 4)
    }
    lastHash = fnv1a64(packed)
  } else if (s.lastAu) {
    lastHash = fnv1a64(s.lastAu)
  }
  const stats: StreamStats = {
    frames: s.frames,
    keyframes: MODE === "encoded" ? s.keyframes : null,
    bytes: MODE === "encoded" ? s.bytes : null,
    first_frame_ms: first === null ? null : Math.round(first * 10) / 10,
    fps: span > 0 ? Math.round((s.frames / span) * 10) / 10 : 0,
    audio_packets: s.audioPackets,
    last_hash: lastHash,
    hash_of: MODE === "decoded" ? "bgra" : "access_unit",
    wav,
    decode_errors: s.decodeErrors,
  }
  return [stats, s]
}

// ---------------------------------------------------------------- click

async function gridLogLines(env: SpacesdClientLike): Promise<string[]> {
  const out = await env.sh(`cat ${GRID_LOG} 2>/dev/null || true`, 10_000)
  return new TextDecoder().decode(out.stdout).split("\n").filter(Boolean)
}

function isCellPress(line: string): boolean {
  try {
    const v = JSON.parse(line)
    const kind = v.event ?? v.kind ?? v.type
    return kind === "button_press" && Array.isArray(v.cell) && v.cell[0] === CLICK_CELL[0] && v.cell[1] === CLICK_CELL[1]
  } catch {
    return false
  }
}

type ClickResult = {
  sent: boolean
  via: "action" | "env" | null
  logged: boolean
  pixel_ok: boolean | null
  pixel?: number[]
  action_delivered?: boolean | null
  action_error?: string
}

async function clickCell(env: SpacesdClientLike, s: OpenStream, win: Target): Promise<ClickResult> {
  const result: ClickResult = { sent: false, via: null, logged: false, pixel_ok: null }
  const before = (await gridLogLines(env)).length
  const last = s.last
  if (last) {
    const fx = (CLICK_CONTENT.x * last.width) / (win.width || last.width)
    const fy = (CLICK_CONTENT.y * last.height) / (win.height || last.height)
    const actionId = `node-click-${Date.now()}`
    s.session.sendControl(
      JSON.stringify({
        type: "action",
        payload: {
          action_id: actionId,
          session_id: s.session.sessionId(),
          tool: "click",
          arguments: { x: Math.round(fx), y: Math.round(fy) },
          basis: {
            kind: "pixel",
            geometry_epoch: Number(last.geometryEpoch),
            frame_sequence: Number(last.sequence),
          },
        },
      }),
    )
    result.sent = true
    result.via = "action"
    const ack = await s.waitEvent((e) => e.kind === "action_result" && e.json.includes(actionId), 3000)
    const payload = ack ? JSON.parse(ack.json).payload ?? {} : {}
    result.action_delivered = ack ? payload.delivered === true : null
    if (!ack || payload.delivered !== true) {
      result.action_error = ack ? JSON.stringify(payload.error ?? payload) : "no action_result within 3 s"
    }
  }
  if (!result.sent || result.action_error) {
    // Fallback: the env click API in screen coordinates.
    await env.click(win.x + CLICK_CONTENT.x, win.y + CLICK_CONTENT.y)
    result.sent = true
    result.via = "env"
  }
  for (let i = 0; i < 20 && !result.logged; i++) {
    await sleep(250)
    result.logged = (await gridLogLines(env)).slice(before).some(isCellPress)
  }
  return result
}

function checkPixel(s: OpenStream, win: Target, click: ClickResult) {
  const last = s.last
  if (MODE !== "decoded" || !last || last.stride === 0) return
  const x = (CLICK_CONTENT.x * last.width) / (win.width || last.width)
  const y = (CLICK_CONTENT.y * last.height) / (win.height || last.height)
  const px = pixelAt(last.data, last.width, last.stride, x, y, BGRA)
  click.pixel = px
  click.pixel_ok = px.every((c, i) => Math.abs(c - CELL_RGB[i]) <= 24)
}

// ---------------------------------------------------------------- main

function fmtTarget(t: Target): string {
  return `${t.kind.padEnd(7)} ${t.id.padEnd(24)} ${JSON.stringify(t.title).padEnd(28)} ${t.width}x${t.height} available=${t.available}`
}

async function main(): Promise<number> {
  if (!ENV_TOKEN) {
    console.error("CUA_ENV_TOKEN is required (see examples/streaming/SCENARIO.md)")
    return 2
  }
  if (process.env.CUA_HEADLESS === "0") {
    log("note: the Node example has no renderer; running headless (CUA_HEADLESS=0 ignored)")
  }
  // No state is written: the direct env client needs no sandbox registry.
  const cua = embedded({ fleetFromEnv: false, stateDir: join(tmpdir(), "cua-example-node") })
  const env = await cua.spacesd(ENV_URL, ENV_TOKEN)
  const jsonl = new Jsonl(BENCH_JSONL)

  if (BENCH_JSONL) {
    const spec = process.env.CUA_BENCH_TARGET ?? "display:primary"
    const seconds = Number(process.env.CUA_BENCH_SECONDS ?? SECONDS)
    const audio = (process.env.CUA_BENCH_AUDIO ?? "1") !== "0"
    const targets = await listTargets(env)
    const [kind, ...rest] = spec.split(":")
    const key = rest.join(":")
    let target: Target | undefined
    if (kind === "window") target = targets.find((t) => t.kind === "window" && t.title === key)
    else
      target =
        key === "primary"
          ? (targets.find((t) => t.kind === "display" && t.primary) ?? targets.find((t) => t.kind === "display"))
          : targets.find((t) => t.kind === "display" && t.id === key)
    if (!target) {
      console.error(`bench target ${spec} not found; targets:\n${targets.map(fmtTarget).join("\n")}`)
      jsonl.close()
      return 1
    }
    const [stats] = await stream(env, { target, audio, seconds, wavName: null, jsonl })
    const ru = process.resourceUsage()
    jsonl.line("end", { cpu_user_s: ru.userCPUTime / 1e6, cpu_sys_s: ru.systemCPUTime / 1e6 })
    jsonl.close()
    log(`BENCH ${JSON.stringify({ example: "typescript-node", mode: MODE, target: spec, ...stats })}`)
    return stats.frames > 0 ? 0 : 1
  }

  log(`health ${await env.health()}`)
  const fx = await env.sh("cua-fixtures start grid", 30_000)
  log(
    `fixture grid: exit=${fx.exit.code ?? fx.exit.signal ?? fx.exit.error} ${new TextDecoder().decode(fx.stdout).trim()}`,
  )

  // The fixture window may take a moment to map: poll ListTargets (bounded).
  let targets: Target[] = []
  let win: Target | undefined
  for (let i = 0; i < 40; i++) {
    targets = await listTargets(env)
    win = targets.find((t) => t.kind === "window" && t.title === GRID_TITLE)
    if (win) break
    await sleep(250)
  }
  for (const t of targets) log(`target ${fmtTarget(t)}`)
  const display = targets.find((t) => t.kind === "display" && t.primary) ?? targets.find((t) => t.kind === "display")
  if (!display) throw new Error("no display target")
  if (!win) throw new Error(`no window titled ${GRID_TITLE}`)

  const [desktop] = await stream(env, {
    target: display,
    audio: true,
    seconds: SECONDS,
    wavName: "desktop.wav",
    jsonl,
  })
  log(`desktop ${JSON.stringify(desktop)}`)

  let click: ClickResult = { sent: false, via: null, logged: false, pixel_ok: null }
  const [window, ws] = await stream(env, {
    target: win,
    audio: true,
    seconds: SECONDS,
    wavName: "window.wav",
    jsonl,
    // Input over the media plane needs a policy other than view-only.
    requestJson: JSON.stringify({ policy: "SESSION_POLICY_ALLOW_ACTIVATION" }),
    during: async (s) => {
      await sleep(Math.min(1000, (SECONDS * 1000) / 4))
      click = await clickCell(env, s, win!)
    },
  })
  checkPixel(ws, win, click)
  log(`window ${JSON.stringify(window)}`)
  jsonl.close()

  const summary = {
    example: "typescript-node",
    media: MODE,
    desktop,
    window,
    click,
  }
  log(`SUMMARY ${JSON.stringify(summary)}`)
  return desktop.frames > 0 && window.frames > 0 && click.logged ? 0 : 1
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error(err?.stack ?? String(err))
    process.exit(1)
  },
)
