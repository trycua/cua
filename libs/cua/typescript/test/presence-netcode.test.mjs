// The presence netcode model (PresenceView) and cursor art, against the
// conformance vectors the Rust core and the Swift SDK run.
import { test } from "node:test"
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import {
  CURSOR_ART,
  CURSOR_SHAPES,
  PresenceRoster,
  PresenceView,
  cursorArt,
  cursorArtSvg,
  idleAlpha,
} from "../dist/spaces/presence.js"

const vectors = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../crates/cua-spaces/assets/presence-conformance.json", import.meta.url)), "utf8"),
)

const camel = (o) =>
  o && typeof o === "object" && !Array.isArray(o)
    ? Object.fromEntries(Object.entries(o).map(([k, v]) => [k.replace(/_([a-z])/g, (_, c) => c.toUpperCase()), camel(v)]))
    : o

/** The Rust serde form of an event -> the SDK's PresenceEvent record. */
function event(e) {
  const c = camel(e)
  if (e.kind === "shape_changed") return { kind: c.kind, participantId: c.participantId, shape: c.shape, shapeSource: c.source }
  return c
}

for (const kase of vectors.cases) {
  test(`conformance: ${kase.name}`, () => {
    const v = new PresenceView(kase.me, kase.delay_ms)
    kase.steps.forEach((step, i) => {
      if (step.event) {
        v.apply(event(step.event), step.at)
        return
      }
      const pointer = step.pointer ? { x: step.pointer[0], y: step.pointer[1] } : undefined
      const got = v.drawables(step.at, pointer)
      assert.equal(got.length, step.expect.length, `${kase.name}#${i}: ${JSON.stringify(got)}`)
      step.expect.forEach((w, j) => {
        const g = got[j]
        assert.equal(g.participantId, w.participant_id)
        assert.ok(Math.abs(g.x - w.x) < vectors.tolerance, `${kase.name}#${i} x ${g.x} != ${w.x}`)
        assert.ok(Math.abs(g.y - w.y) < vectors.tolerance, `${kase.name}#${i} y`)
        assert.ok(Math.abs(g.alpha - w.alpha) < vectors.tolerance, `${kase.name}#${i} alpha ${g.alpha}`)
        assert.equal(g.shape, w.shape)
        assert.equal(g.isMe, w.is_me)
      })
    })
  })
}

test("the idle fade curve", () => {
  assert.equal(idleAlpha(5000), 1)
  assert.ok(Math.abs(idleAlpha(5150) - 0.5) < 1e-9)
  assert.equal(idleAlpha(5300), 0)
})

test("every shape has art, unknown names draw the arrow", () => {
  assert.equal(CURSOR_SHAPES.length, 14)
  for (const s of CURSOR_SHAPES) {
    const a = cursorArt(s)
    assert.match(a.d, /^M[MLCZ \-.0-9]+$/)
    assert.ok(a.hotspot[0] >= 0 && a.hotspot[0] <= CURSOR_ART.canvas)
  }
  assert.equal(cursorArt("nope"), CURSOR_ART.shapes.arrow)
  const svg = cursorArtSvg("text", "#e6194b", 24)
  assert.match(svg, /fill="#e6194b"/)
  assert.match(svg, /width="24"/)
  assert.match(cursorArtSvg("text", "javascript:alert(1)"), /fill="#3b82f6"/, "colors are validated")
})

test("the roster follows shapes and drops participants a heartbeat omits", () => {
  const P = (id) => ({ participantId: id, principalId: id, displayName: id, color: "#123456", kind: "human" })
  const r = PresenceRoster.from(P("me"), [{ participant: P("a") }, { participant: P("b") }])
  r.apply({ kind: "cursor_moved", participantId: "a", cursor: { displayId: "", x: 0.1, y: 0.2, visible: true, pressed: false, shape: "text", shapeSource: "hit_test", atMs: 0, receivedMs: 0 } })
  assert.equal(r.get("a").cursor.shape, "text")
  r.apply({ kind: "shape_changed", participantId: "a", shape: "pointer", shapeSource: "probe" })
  assert.equal(r.get("a").cursor.shape, "pointer")
  assert.ok(r.apply({ kind: "heartbeat", participantIds: ["me", "a"] }))
  assert.deepEqual(r.entries.map((e) => e.participantId), ["me", "a"])
})
