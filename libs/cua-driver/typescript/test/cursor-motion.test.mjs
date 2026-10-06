// Cua Cursor Motion through the generated TypeScript bindings: replays the
// golden trajectories the Rust crate and the standalone web port share
// (rust/crates/cua-cursor-motion/fixtures/golden.json) through
// planCursorMove / planCursorSpec in the staged native library.
import assert from "node:assert/strict"
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const libraryName =
  process.platform === "darwin"
    ? "libcua_driver_sdk.dylib"
    : process.platform === "win32"
      ? "cua_driver_sdk.dll"
      : "libcua_driver_sdk.so"
const nodeTriple =
  process.platform === "darwin"
    ? `darwin-${process.arch}`
    : process.platform === "win32"
      ? `win32-${process.arch}-msvc`
      : `linux-${process.arch}-${process.report.getReport().header.glibcVersionRuntime ? "gnu" : "musl"}`
const library = path.resolve(
  testDirectory,
  "../node_modules/@trycua",
  `cua-driver-${nodeTriple}`,
  libraryName,
)
if (process.env.CUA_DRIVER_REQUIRE_UNIFFI === "1" && !existsSync(library)) {
  throw new Error(`required staged UniFFI library is missing: ${library}`)
}
const skip = !existsSync(library)

const golden = JSON.parse(
  readFileSync(
    path.resolve(testDirectory, "../../rust/crates/cua-cursor-motion/fixtures/golden.json"),
    "utf8",
  ),
)
// The fixture rounds to 1e-9; the bindings run the same Rust code.
const TOL = 1e-8
const FRAME_FRACTIONS = [0.2, 0.5, 0.8, 1.0]
const EFFECTS = ["trail", "glow", "magnet", "ripple", "squish"]

const pascal = (snake) => snake.replace(/(^|_)([a-z])/g, (_, __, c) => c.toUpperCase())
const camel = (snake) => snake.replace(/_([a-z])/g, (_, c) => c.toUpperCase())
const camelKeys = (o) => Object.fromEntries(Object.entries(o).map(([k, v]) => [camel(k), v]))

function near(name, got, want) {
  assert.ok(Math.abs(got - want) <= TOL, `${name}: ${got} vs ${want}`)
}

async function load() {
  const m = await import("@trycua/cua-driver")
  const variant = (enumType, value) => {
    const { type, ...fields } = value
    const ctor = enumType[pascal(type)]
    return Object.keys(fields).length ? ctor.new(camelKeys(fields)) : ctor.new()
  }
  const effects = (o) =>
    Object.fromEntries(EFFECTS.map((k) => [k, o[k] === null ? undefined : o[k]]))
  const params = (p) => ({
    style: m.CursorMotionStyle[pascal(p.style)],
    timing: m.CursorMotionTiming[pascal(p.timing)],
    effects: effects(p.effects),
    startHandle: p.start_handle,
    endHandle: p.end_handle,
    arcSize: p.arc_size,
    arcFlow: p.arc_flow,
    spring: p.spring,
    glideDurationMs: p.glide_duration_ms,
    peakSpeed: p.peak_speed,
    minStartSpeed: p.min_start_speed,
    minEndSpeed: p.min_end_speed,
    turnRadius: p.turn_radius,
  })
  const request = (r) => ({
    fromPoint: { x: r.from[0], y: r.from[1] },
    toPoint: { x: r.to[0], y: r.to[1] },
    fromHeading: r.from_heading,
    endHeading: r.end_heading,
    target: r.target
      ? { x: r.target[0], y: r.target[1], width: r.target[2], height: r.target[3] }
      : undefined,
    seed: r.seed,
    reducedMotion: r.reduced_motion,
  })
  const spec = (s) => ({
    path: variant(m.CursorPathShape, s.path),
    ease: variant(m.CursorEase, s.ease),
    settle: variant(m.CursorSettle, s.settle),
    duration: variant(m.CursorMotionDuration, s.duration),
    heading: m.CursorHeading[pascal(s.heading)],
    effects: s.effects,
    trail: camelKeys(s.trail),
  })
  return { m, params, request, spec }
}

function checkFrame(name, frame, click, want) {
  const present = (label, value, expected, fields) => {
    assert.equal(value === undefined, expected === null, `${name}.${label} presence`)
    if (value !== undefined) fields(value).forEach((v, i) => near(`${name}.${label}[${i}]`, v, expected[i]))
  }
  present("glow", frame.glow, want.glow, (g) => [g.x, g.y, g.r, g.alpha])
  present("magnet", frame.magnet, want.magnet, (mg) => [
    mg.rect.x,
    mg.rect.y,
    mg.rect.width,
    mg.rect.height,
    mg.glow,
  ])
  present("ripple", click.ripple, want.ripple, (r) => [r.x, r.y, r.r, r.width, r.alpha])
  near(`${name}.squish`, click.squish, want.squish)
  assert.equal(frame.trail.length, want.trail.length, `${name}.trail length`)
  frame.trail.forEach((s, i) =>
    [s.ax, s.ay, s.bx, s.by, s.width, s.alpha].forEach((v, j) =>
      near(`${name}.trail[${i}][${j}]`, v, want.trail[i][j]),
    ),
  )
}

function check(m, name, traj, out) {
  assert.equal(traj.samples().length, out.samples, `${name}: sample count`)
  const d = traj.duration()
  near(`${name}.duration`, d, out.duration)
  near(`${name}.arrival`, traj.arrivalT(), out.arrival_t)
  const snap = traj.snapT()
  assert.equal(snap === undefined, out.snap_t === null, `${name}: snap`)
  if (snap !== undefined) near(`${name}.snap`, snap, out.snap_t)
  const t = traj.target()
  ;[t.x, t.y, t.width, t.height].forEach((v, i) => near(`${name}.target`, v, out.target[i]))
  assert.equal(traj.targetKnown(), out.target_known)
  const fx = traj.effects()
  assert.deepEqual(Object.fromEntries(EFFECTS.map((k) => [k, fx[k]])), out.effects)
  out.grid.forEach((w, i) => {
    const s = traj.sampleAt((d * i) / golden.grid)
    ;[s.t, s.x, s.y, s.heading].forEach((v, j) => near(`${name}.grid[${i}][${j}]`, v, w[j]))
  })
  if (out.frames) {
    near(`${name}.linger`, traj.linger(), out.linger)
    const clicks = { trail: false, glow: false, magnet: false, ripple: true, squish: true }
    out.frames.forEach((f, i) => {
      const k = FRAME_FRACTIONS[i]
      const at = d * k
      const s = traj.sampleAt(at)
      checkFrame(
        `${name}@${k}`,
        traj.effectFrame(at, true),
        m.cursorClickEffects(clicks, 0.03 + 0.4 * k, s.x, s.y),
        f.frame,
      )
    })
  }
}

test("built-in styles match the golden trajectories", { skip }, async () => {
  const { m, params, request } = await load()
  assert.ok(golden.cases.length > 100)
  for (const c of golden.cases) {
    check(m, c.name, m.planCursorMove(params(c.params), request(c.request)), c.out)
  }
})

test("custom specs match the golden trajectories", { skip }, async () => {
  const { m, request, spec } = await load()
  for (const c of golden.spec_cases) {
    check(m, c.name, m.planCursorSpec(spec(c.spec), request(c.request)), c.out)
  }
})

test("the arc styles are specs", { skip }, async () => {
  const { m, request } = await load()
  const params = { ...m.defaultCursorMotionParams(), style: m.CursorMotionStyle.CometSwoop }
  const req = request(golden.cases[0].request)
  const spec = m.cursorMotionSpecForStyle(m.CursorMotionStyle.CometSwoop, params)
  assert.ok(spec)
  assert.deepEqual(m.planCursorSpec(spec, req).samples(), m.planCursorMove(params, req).samples())
  assert.equal(m.cursorMotionSpecForStyle(m.CursorMotionStyle.Magnetic, params), undefined)
})
