// The shared streaming scenario (examples/streaming/SCENARIO.md) in a page:
// @trycua/cua/browser (wasm, gRPC-Web) for the env calls, the media plane
// (MEDIA.md, rcdp wire v2) on a WebSocket, WebCodecs VideoDecoder -> canvas
// and AudioDecoder -> AudioContext / WAV.
//
// Interactive: fill in the form. Headless (headless.mjs, Playwright): the
// runner injects `window.__CUA_CONFIG` and `window.cuaReport(kind, data)`.
import { Cua, CuaConfig, initialize } from "/sdk/index.js"

import { RGBA, fnv1a64, readTimecode, unixNs, wavBytes } from "./pixels.js"

const GRID_TITLE = "CUA Fixture Grid"
const GRID_LOG = "/tmp/cua-fixtures/grid.jsonl"
const CLICK_CELL = [2, 3]
const CLICK_CONTENT = { x: 200, y: 280 }
const CELL_RGB = [72, 153, 128]

const $ = (id) => document.getElementById(id)
const canvas = $("video")
const ctx = canvas.getContext("2d", { willReadFrequently: true })

function report(kind, data) {
  if (typeof window.cuaReport === "function") window.cuaReport(kind, data)
  if (kind === "log" || kind === "summary") {
    const line = kind === "summary" ? `SUMMARY ${JSON.stringify(data)}` : data
    $("log").textContent += line + "\n"
    if (typeof window.cuaReport !== "function") console.log(line)
  }
}
const log = (s) => report("log", s)

// ------------------------------------------------------------ bench JSONL

let benchOn = false
function jsonl(t, fields = {}) {
  if (!benchOn) return
  const rest = JSON.stringify(fields)
  report("jsonl", `{"t":${JSON.stringify(t)},"unix_ns":${unixNs()}${rest === "{}" ? "" : "," + rest.slice(1, -1)}}`)
}

// ------------------------------------------------------------ targets

const num = (v) => (typeof v === "number" ? v : typeof v === "string" ? Number(v) : 0)

async function listTargets(env) {
  const raw = JSON.parse(await env.callJson("StreamService/ListTargets", JSON.stringify({ includeWindows: true })))
  const out = []
  for (const t of raw.targets ?? []) {
    const available = t.available ?? false
    if (t.display) {
      const d = t.display, b = d.bounds ?? {}, n = d.nativeSize ?? d.native_size ?? {}
      out.push({ kind: "display", id: d.id ?? "", title: d.name ?? "", width: num(n.width) || num(b.width),
        height: num(n.height) || num(b.height), x: num(b.x), y: num(b.y), available, primary: d.primary ?? false })
    } else if (t.window) {
      const w = t.window, b = w.bounds ?? {}
      out.push({ kind: "window", id: w.ref?.id ?? "", title: w.title ?? "", width: num(b.width), height: num(b.height),
        x: num(b.x), y: num(b.y), available, primary: false })
    }
  }
  return out
}

const fmtTarget = (t) =>
  `${t.kind.padEnd(7)} ${t.id.padEnd(24)} ${JSON.stringify(t.title).padEnd(28)} ${t.width}x${t.height} available=${t.available}`

// ------------------------------------------------------------ media plane

function findDescriptor(v, depth) {
  if (!v || typeof v !== "object") return null
  if ("sequence" in v && "codec" in v) return v
  if (depth === 0) return null
  for (const c of Object.values(v)) {
    const d = findDescriptor(c, depth - 1)
    if (d) return d
  }
  return null
}

/** `avc1.PPCCLL` from the first SPS of an Annex B access unit. */
function avcCodecString(au) {
  for (let i = 0; i + 4 < au.length; i++) {
    if (au[i] === 0 && au[i + 1] === 0 && (au[i + 2] === 1 || (au[i + 2] === 0 && au[i + 3] === 1))) {
      const at = au[i + 2] === 1 ? i + 3 : i + 4
      if ((au[at] & 0x1f) === 7 && at + 3 < au.length) {
        const hex = (b) => b.toString(16).padStart(2, "0")
        return `avc1.${hex(au[at + 1])}${hex(au[at + 2])}${hex(au[at + 3])}`
      }
    }
  }
  return null
}

class Stream {
  constructor(env, cfg, target, { audio, wavName, playAudio, policy }) {
    Object.assign(this, { env, cfg, target, audio, wavName, playAudio, policy })
    this.frames = 0 // decoded frames
    this.received = 0 // video packets
    this.keyframes = 0
    this.bytes = 0
    this.audioPackets = 0
    this.decodeErrors = 0
    this.firstFrameAt = null
    this.pending = new Map() // decoder timestamp -> packet meta
    this.lastMeta = null
    this.decoder = null
    this.codecEpoch = null
    this.needKey = true
    this.tracks = new Map() // track_id -> { decoder, epoch, rate, channels, codec }
    this.audioBytes = new Map() // pts_us -> encoded bytes
    this.pcm = []
    this.pcmRate = 48000
    this.pcmChannels = 2
    this.waiters = []
    this.playhead = 0
  }

  async open() {
    const t = this.target
    const req = {
      target: t.kind === "display" ? { displayId: t.id } : { window: { id: t.id } },
      codecs: ["MEDIA_CODEC_H264"],
      maxFps: 30,
      ...(this.audio ? { audio: { enabled: true } } : {}),
      ...(this.policy ? { policy: this.policy } : {}),
    }
    jsonl("open")
    this.openedAt = performance.now()
    const resp = JSON.parse(await this.env.callJson("StreamService/OpenMedia", JSON.stringify(req)))
    this.sessionId = resp.mediaSessionId ?? resp.media_session_id
    const ticket = resp.ticket
    // The ticket goes in the subprotocol, not the URL (MEDIA.md §2).
    const wsUrl = this.cfg.env.replace(/^http/, "ws").replace(/\/$/, "") + "/media"
    this.ws = new WebSocket(wsUrl, ["rcdp.v2", `cua.ticket.${ticket}`])
    this.ws.binaryType = "arraybuffer"
    this.ws.onmessage = (m) => (typeof m.data === "string" ? this.onControl(m.data) : this.onBinary(new Uint8Array(m.data)))
    this.closed = new Promise((ok) => (this.ws.onclose = (e) => ok(e)))
    await new Promise((ok, fail) => {
      this.ws.onopen = ok
      this.ws.onerror = () => fail(new Error(`media socket ${wsUrl} failed`))
    })
  }

  send(obj) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(obj))
  }

  waitEvent(pred, ms) {
    return new Promise((ok) => {
      const waiter = (m) => {
        if (!pred(m)) return false
        clearTimeout(timer)
        ok(m)
        return true
      }
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== waiter)
        ok(null)
      }, ms)
      this.waiters.push(waiter)
    })
  }

  onControl(text) {
    let m
    try { m = JSON.parse(text) } catch { return }
    if (m.type === "audio_config") this.configureAudio(m.payload)
    if (m.type === "error") log(`media error ${JSON.stringify(m.payload)}`)
    this.waiters = this.waiters.filter((w) => !w(m))
  }

  onBinary(b) {
    if (b.length >= 24 && b[0] === 0x52 && b[1] === 0x41 && b[2] === 0x55 && b[3] === 0x32) return this.onAudio(b)
    if (b.length < 8) return
    const v = new DataView(b.buffer, b.byteOffset, b.byteLength)
    const hl = v.getUint32(0), pl = v.getUint32(4)
    if (hl > 1 << 20 || 8 + hl + pl !== b.length) return
    let d
    try { d = findDescriptor(JSON.parse(new TextDecoder().decode(b.subarray(8, 8 + hl))), 4) } catch { return }
    if (!d) return
    this.onVideo(d, b.subarray(8 + hl))
  }

  requestKeyframe() {
    const now = performance.now()
    if (this.lastKeyReq && now - this.lastKeyReq < 1000) return
    this.lastKeyReq = now
    this.send({ type: "request_keyframe", payload: { session_id: this.sessionId } })
  }

  onVideo(d, payload) {
    this.received++
    if (d.keyframe) this.keyframes++
    this.bytes += payload.length
    const meta = {
      seq: d.sequence, bytes: payload.length, key: !!d.keyframe, cap_us: d.capture_timestamp_us,
      w: d.width_px, h: d.height_px, geometry_epoch: d.geometry_epoch,
    }
    if (d.codec !== "h264") return // bgra/png are loopback fallbacks; not handled here
    if (this.decoder && this.codecEpoch !== d.codec_epoch) {
      this.decoder.close()
      this.decoder = null
      this.needKey = true
    }
    if (this.needKey && !d.keyframe) return this.requestKeyframe()
    if (!this.decoder) {
      const codec = avcCodecString(payload)
      if (!codec) return this.requestKeyframe()
      this.decoder = new VideoDecoder({
        output: (f) => this.onDecoded(f),
        error: (e) => {
          this.decodeErrors++
          log(`decode error ${e.message}`)
          this.decoder = null
          this.needKey = true
        },
      })
      // No `description`: the bitstream is Annex B with in-band SPS/PPS.
      this.decoder.configure({ codec, optimizeForLatency: true, hardwareAcceleration: "no-preference" })
      this.codecEpoch = d.codec_epoch
    }
    this.needKey = false
    // Timestamps must be unique per chunk: use the capture time.
    const ts = Number(d.capture_timestamp_us)
    this.pending.set(ts, meta)
    if (this.pending.size > 128) this.pending.delete(this.pending.keys().next().value)
    try {
      this.decoder.decode(new EncodedVideoChunk({ type: d.keyframe ? "key" : "delta", timestamp: ts, data: payload }))
    } catch (e) {
      this.decodeErrors++
      this.needKey = true
      this.requestKeyframe()
    }
  }

  onDecoded(frame) {
    const clientMs = performance.timeOrigin + performance.now()
    const meta = this.pending.get(frame.timestamp) ?? {}
    this.pending.delete(frame.timestamp)
    this.frames++
    if (this.firstFrameAt === null) this.firstFrameAt = performance.now()
    const w = frame.displayWidth, h = frame.displayHeight
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w
      canvas.height = h
    }
    ctx.drawImage(frame, 0, 0)
    frame.close()
    this.lastMeta = meta
    if (benchOn) {
      const scale = w / (this.target.width || w)
      const stripH = Math.min(h, Math.ceil(16 * scale))
      const strip = ctx.getImageData(0, 0, w, stripH).data
      const tc = readTimecode(strip, w, stripH, w * 4, scale, RGBA, clientMs)
      jsonl("frame", { seq: meta.seq ?? null, bytes: meta.bytes ?? null, key: meta.key ?? null,
        cap_us: meta.cap_us ?? frame.timestamp, w, h, tc_ms: tc })
    }
  }

  configureAudio(p) {
    if (p.direction && p.direction !== "down") return
    const prev = this.tracks.get(p.track_id)
    prev?.decoder?.close?.()
    const track = { epoch: p.config_epoch, rate: p.sample_rate_hz, channels: p.channels, codec: p.codec, decoder: null }
    if (p.codec === "opus") {
      track.decoder = new AudioDecoder({
        output: (a) => this.onPcm(track, a),
        error: (e) => {
          this.decodeErrors++
          log(`audio decode error ${e.message}`)
        },
      })
      track.decoder.configure({ codec: "opus", sampleRate: p.sample_rate_hz, numberOfChannels: p.channels })
    }
    this.tracks.set(p.track_id, track)
  }

  onAudio(b) {
    const v = new DataView(b.buffer, b.byteOffset, b.byteLength)
    if (b[4] !== 2 || b[23] !== 0 || (b[5] & 0xfc) !== 0) return
    const trackId = v.getUint16(6)
    const track = this.tracks.get(trackId)
    if (!track || b[22] !== track.epoch) return
    const pts = Number(v.getBigUint64(12))
    const frameSamples = v.getUint16(20)
    const payload = b.subarray(24)
    this.audioPackets++
    if (track.codec === "opus" && track.decoder?.state === "configured") {
      this.audioBytes.set(pts, payload.length)
      if (this.audioBytes.size > 256) this.audioBytes.delete(this.audioBytes.keys().next().value)
      track.decoder.decode(new EncodedAudioChunk({ type: "key", timestamp: pts, data: payload }))
    } else if (track.codec === "pcm_s16le") {
      const s = new Int16Array(payload.length / 2)
      for (let i = 0; i < s.length; i++) s[i] = v.getInt16(24 + i * 2, true)
      this.pushPcm(s, track.rate, track.channels, pts, payload.length, frameSamples)
    }
  }

  onPcm(track, a) {
    const n = a.numberOfFrames, ch = a.numberOfChannels
    const planes = []
    for (let c = 0; c < ch; c++) {
      const plane = new Float32Array(n)
      a.copyTo(plane, { planeIndex: c, format: "f32-planar" })
      planes.push(plane)
    }
    const s = new Int16Array(n * ch)
    for (let i = 0; i < n; i++)
      for (let c = 0; c < ch; c++) s[i * ch + c] = Math.max(-32768, Math.min(32767, Math.round(planes[c][i] * 32767)))
    const pts = a.timestamp
    const bytes = this.audioBytes.get(pts) ?? null
    this.audioBytes.delete(pts)
    a.close()
    this.pushPcm(s, a.sampleRate ?? track.rate, ch, pts, bytes, n, planes)
  }

  pushPcm(s, rate, ch, pts, bytes, samples, planes) {
    this.pcmRate = rate
    this.pcmChannels = ch
    if (this.wavName) this.pcm.push(s)
    jsonl("audio", { pts_us: pts, bytes, samples })
    if (this.playAudio) this.play(s, rate, ch, planes)
  }

  play(s, rate, ch, planes) {
    if (!this.ac) {
      this.ac = new AudioContext({ sampleRate: rate, latencyHint: "interactive" })
      this.ac.resume().catch(() => {})
    }
    const n = s.length / ch
    const buf = this.ac.createBuffer(ch, n, rate)
    for (let c = 0; c < ch; c++) {
      const out = buf.getChannelData(c)
      if (planes) out.set(planes[c])
      else for (let i = 0; i < n; i++) out[i] = s[i * ch + c] / 32768
    }
    const src = this.ac.createBufferSource()
    src.buffer = buf
    src.connect(this.ac.destination)
    // Small jitter buffer (40 ms, MEDIA.md §12.5) without drift correction.
    const now = this.ac.currentTime
    if (this.playhead < now + 0.02 || this.playhead > now + 0.25) this.playhead = now + 0.04
    src.start(this.playhead)
    this.playhead += n / rate
  }

  async close() {
    const endAt = performance.now()
    this.ws.close(1000)
    await Promise.race([this.closed, new Promise((ok) => setTimeout(ok, 2000))])
    try { await this.env.callJson("StreamService/CloseMedia", JSON.stringify({ mediaSessionId: this.sessionId })) } catch {}
    const flush = (d) => (d && d.state === "configured" ? Promise.race([d.flush().catch(() => {}), new Promise((ok) => setTimeout(ok, 1000))]) : null)
    await Promise.all([flush(this.decoder), ...[...this.tracks.values()].map((t) => flush(t.decoder))])
    this.decoder?.state !== "closed" && this.decoder?.close()
    for (const t of this.tracks.values()) t.decoder?.state !== "closed" && t.decoder?.close()
    await this.ac?.close()
    return endAt
  }

  lastBgraHash() {
    if (!this.frames) return null
    const rgba = ctx.getImageData(0, 0, canvas.width, canvas.height).data
    const bgra = new Uint8Array(rgba.length)
    for (let i = 0; i < rgba.length; i += 4) {
      bgra[i] = rgba[i + 2]; bgra[i + 1] = rgba[i + 1]; bgra[i + 2] = rgba[i]; bgra[i + 3] = rgba[i + 3]
    }
    return fnv1a64(bgra)
  }
}

async function runStream(env, cfg, target, opts, during) {
  const s = new Stream(env, cfg, target, opts)
  await s.open()
  const deadline = s.openedAt + cfg.seconds * 1000
  if (during) {
    for (let i = 0; i < 100 && s.frames === 0 && performance.now() < deadline; i++) await sleep(50)
    await during(s)
  }
  const left = deadline - performance.now()
  if (left > 0) await sleep(left)
  const endAt = await s.close()
  const first = s.firstFrameAt === null ? null : s.firstFrameAt - s.openedAt
  const span = s.firstFrameAt === null ? 0 : (endAt - s.firstFrameAt) / 1000
  let wav = null
  if (opts.wavName) {
    const bytes = wavBytes(s.pcm, s.pcmRate, s.pcmChannels)
    wav = `${cfg.outDir}/${opts.wavName}`
    report("file", { name: opts.wavName, base64: toBase64(bytes) })
    if (typeof window.cuaReport !== "function") {
      const a = document.createElement("a")
      a.href = URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }))
      a.download = opts.wavName
      a.textContent = ` ${opts.wavName}`
      $("downloads").append(a)
    }
  }
  const stats = {
    frames: s.frames,
    received: s.received,
    keyframes: s.keyframes,
    bytes: s.bytes,
    first_frame_ms: first === null ? null : Math.round(first * 10) / 10,
    fps: span > 0 ? Math.round((s.frames / span) * 10) / 10 : 0,
    audio_packets: s.audioPackets,
    last_hash: s.lastBgraHash(),
    hash_of: "bgra (canvas readback)",
    wav,
    decode_errors: s.decodeErrors,
  }
  return [stats, s]
}

const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms))

function toBase64(bytes) {
  let s = ""
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

// ------------------------------------------------------------ click

async function gridLogLines(env) {
  const out = await env.sh(`cat ${GRID_LOG} 2>/dev/null || true`, 10000)
  return new TextDecoder().decode(out.stdout).split("\n").filter(Boolean)
}

function isCellPress(line) {
  try {
    const v = JSON.parse(line)
    const kind = v.event ?? v.kind ?? v.type
    return kind === "button_press" && Array.isArray(v.cell) && v.cell[0] === CLICK_CELL[0] && v.cell[1] === CLICK_CELL[1]
  } catch {
    return false
  }
}

function clickPoint(meta, win) {
  const w = canvas.width, h = canvas.height
  return [(CLICK_CONTENT.x * w) / (win.width || w), (CLICK_CONTENT.y * h) / (win.height || h)]
}

async function clickCell(env, s, win) {
  const result = { sent: false, via: null, logged: false, pixel_ok: null }
  const before = (await gridLogLines(env)).length
  const meta = s.lastMeta
  if (meta) {
    const [x, y] = clickPoint(meta, win)
    const actionId = `web-click-${Date.now()}`
    s.send({ type: "action", payload: { action_id: actionId, session_id: s.sessionId, tool: "click",
      arguments: { x: Math.round(x), y: Math.round(y) },
      basis: { kind: "pixel", geometry_epoch: meta.geometry_epoch, frame_sequence: meta.seq } } })
    result.sent = true
    result.via = "action"
    const ack = await s.waitEvent((m) => m.type === "action_result" && m.payload?.action_id === actionId, 3000)
    result.action_delivered = ack ? ack.payload.delivered === true : null
    if (!ack || ack.payload.delivered !== true)
      result.action_error = ack ? JSON.stringify(ack.payload.error ?? ack.payload) : "no action_result within 3 s"
  }
  if (!result.sent || result.action_error) {
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

// ------------------------------------------------------------ main

async function run(cfg) {
  await initialize()
  const cua = Cua.embedded(CuaConfig.create({ fleetFromEnv: false }))
  const env = await cua.spacesd(cfg.env, cfg.token)

  if (cfg.bench) {
    benchOn = true
    const targets = await listTargets(env)
    const [kind, ...rest] = cfg.benchTarget.split(":")
    const key = rest.join(":")
    const target = kind === "window"
      ? targets.find((t) => t.kind === "window" && t.title === key)
      : key === "primary"
        ? targets.find((t) => t.kind === "display" && t.primary) ?? targets.find((t) => t.kind === "display")
        : targets.find((t) => t.kind === "display" && t.id === key)
    if (!target) {
      log(`bench target ${cfg.benchTarget} not found; targets:\n${targets.map(fmtTarget).join("\n")}`)
      return 1
    }
    const [stats] = await runStream(env, cfg, target, { audio: cfg.benchAudio, wavName: null, playAudio: false })
    log(`BENCH ${JSON.stringify({ example: "typescript-web", target: cfg.benchTarget, ...stats })}`)
    return stats.frames > 0 ? 0 : 1
  }

  log(`health ${await env.health()}`)
  const fx = await env.sh("cua-fixtures start grid", 30000)
  log(`fixture grid: exit=${fx.exit.code ?? fx.exit.signal ?? fx.exit.error} ${new TextDecoder().decode(fx.stdout).trim()}`)
  let targets = [], win
  for (let i = 0; i < 40 && !win; i++) {
    targets = await listTargets(env)
    win = targets.find((t) => t.kind === "window" && t.title === GRID_TITLE)
    if (!win) await sleep(250)
  }
  for (const t of targets) log(`target ${fmtTarget(t)}`)
  const display = targets.find((t) => t.kind === "display" && t.primary) ?? targets.find((t) => t.kind === "display")
  if (!display) throw new Error("no display target")
  if (!win) throw new Error(`no window titled ${GRID_TITLE}`)

  const play = !cfg.headless
  const [desktop] = await runStream(env, cfg, display, { audio: true, wavName: "desktop.wav", playAudio: play })
  log(`desktop ${JSON.stringify(desktop)}`)
  let click = { sent: false, via: null, logged: false, pixel_ok: null }
  const [window_, ws] = await runStream(
    env, cfg, win,
    { audio: true, wavName: "window.wav", playAudio: play, policy: "SESSION_POLICY_ALLOW_ACTIVATION" },
    async (s) => {
      await sleep(Math.min(1000, (cfg.seconds * 1000) / 4))
      click = await clickCell(env, s, win)
    },
  )
  if (ws.frames) {
    const [x, y] = clickPoint(ws.lastMeta, win)
    const px = [...ctx.getImageData(Math.round(x), Math.round(y), 1, 1).data.subarray(0, 3)]
    click.pixel = px
    click.pixel_ok = px.every((c, i) => Math.abs(c - CELL_RGB[i]) <= 24)
  }
  log(`window ${JSON.stringify(window_)}`)
  report("summary", { example: "typescript-web", desktop, window: window_, click })
  return desktop.frames > 0 && window_.frames > 0 && click.logged ? 0 : 1
}

function start(cfg) {
  run(cfg).then(
    (code) => report("done", code),
    (e) => {
      log(`error: ${e?.stack ?? e}`)
      report("done", 1)
    },
  )
}

if (window.__CUA_CONFIG) {
  document.getElementById("form").hidden = true
  start({ seconds: 5, outDir: "./out", headless: true, bench: false, benchAudio: true, ...window.__CUA_CONFIG })
} else {
  const q = new URLSearchParams(location.search)
  if (q.get("env")) $("env").value = q.get("env")
  if (q.get("seconds")) $("seconds").value = q.get("seconds")
  $("form").addEventListener("submit", (e) => {
    e.preventDefault()
    $("run").disabled = true
    $("log").textContent = ""
    start({ env: $("env").value.trim(), token: $("token").value, seconds: Number($("seconds").value) || 5,
      outDir: "(download)", headless: false, bench: false })
  })
}
