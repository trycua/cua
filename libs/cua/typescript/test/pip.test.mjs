// `@trycua/cua/spaces/pip`: sources, routes and labels for a shell's PiP
// window, and the browser controller (Document PiP first, video PiP as the
// fallback) against a scripted window. No browser, no Space.
import assert from "node:assert/strict"
import { test } from "node:test"

import {
  PictureInPicture,
  desktopSource,
  encodePipRoute,
  parsePipRoute,
  pipKey,
  pipLabel,
  pipSize,
  pipSupport,
  pipWindowLabel,
  windowSource,
} from "../dist/spaces/pip.js"

const win1 = windowSource({ window_id: "0x1a00003", app_name: "Firefox", title: "Mozilla Firefox" })

test("window sources come from either JSON shape", () => {
  assert.deepEqual(win1, { kind: "window", windowId: "0x1a00003", app: "Firefox", title: "Mozilla Firefox" })
  assert.deepEqual(windowSource({ windowId: "7", appName: "Thunar", title: "Downloads" }), {
    kind: "window",
    windowId: "7",
    app: "Thunar",
    title: "Downloads",
  })
})

test("keys and labels", () => {
  assert.equal(pipKey(desktopSource), "desktop")
  assert.equal(pipKey(win1), "window:0x1a00003")
  assert.equal(pipLabel(desktopSource), "Desktop")
  assert.equal(pipLabel(desktopSource, "dev"), "dev")
  assert.equal(pipLabel(win1), "Mozilla Firefox", "the app name is not repeated")
  assert.equal(pipLabel(windowSource({ windowId: "7", appName: "Thunar", title: "Downloads" })), "Thunar · Downloads")
  assert.equal(pipLabel(windowSource({ windowId: "7", appName: "xterm", title: "" })), "xterm")
})

test("routes round-trip, including awkward titles", () => {
  assert.equal(encodePipRoute(desktopSource), "pip=desktop")
  assert.deepEqual(parsePipRoute("#pip=desktop"), desktopSource)
  const odd = windowSource({ windowId: "a/b c", appName: "Tëst & co", title: "x=1#y?" })
  assert.deepEqual(parsePipRoute(`#${encodePipRoute(odd)}`), odd)
  assert.deepEqual(parsePipRoute(`?${encodePipRoute(odd)}`), odd)
  assert.equal(parsePipRoute(""), null)
  assert.equal(parsePipRoute("#token=abc"), null)
  assert.equal(parsePipRoute("#pip=window"), null, "a window route needs an id")
})

test("shell window labels are safe and distinct", () => {
  assert.equal(pipWindowLabel(desktopSource), "pip-desktop")
  assert.equal(pipWindowLabel(windowSource({ windowId: "0x1a00003" })), "pip-w-0x1a00003")
  const a = pipWindowLabel(windowSource({ windowId: "a/b" }))
  const b = pipWindowLabel(windowSource({ windowId: "a:b" }))
  assert.match(a, /^[A-Za-z0-9_-]+$/)
  assert.notEqual(a, b)
  assert.ok(pipWindowLabel(windowSource({ windowId: "x".repeat(500) })).length < 80)
})

test("sizes keep the aspect and the long edge", () => {
  assert.deepEqual(pipSize(1280, 800), { width: 480, height: 300 })
  assert.deepEqual(pipSize(600, 1200, 400), { width: 200, height: 400 })
  assert.deepEqual(pipSize(0, 0), { width: 480, height: 270 })
})

// -- the browser controller over a scripted window ---------------------------

class FakeTarget {
  #l = new Map()
  addEventListener(type, fn) {
    ;(this.#l.get(type) ?? this.#l.set(type, []).get(type)).push(fn)
  }
  fire(type) {
    const fns = this.#l.get(type) ?? []
    this.#l.delete(type)
    for (const fn of fns) fn()
  }
}

class FakeVideo extends FakeTarget {
  srcObject = null
  played = false
  pip = false
  constructor(doc) {
    super()
    this.doc = doc
  }
  async play() {
    this.played = true
  }
  async requestPictureInPicture() {
    this.doc.pictureInPictureElement = this
    this.pip = true
  }
}

function fakeDocument() {
  const doc = {
    title: "",
    children: [],
    pictureInPictureEnabled: true,
    pictureInPictureElement: null,
    createElement: (tag) => (tag === "video" ? new FakeVideo(doc) : { textContent: "" }),
    head: { append: () => {} },
    body: { append: (el) => doc.children.push(el) },
    async exitPictureInPicture() {
      const v = doc.pictureInPictureElement
      doc.pictureInPictureElement = null
      v?.fire("leavepictureinpicture")
    },
  }
  return doc
}

function fakeCanvas(width = 1280, height = 800) {
  const tracks = []
  return {
    width,
    height,
    tracks,
    captureStream(fps) {
      const t = { fps, stopped: false, stop() { this.stopped = true } }
      tracks.push(t)
      return { getTracks: () => [t] }
    },
  }
}

function documentPipWindow() {
  const requested = []
  const windows = []
  const api = {
    async requestWindow(size) {
      requested.push(size)
      // The browser closes an open PiP window when another is requested.
      for (const w of windows) if (!w.closed) w.close()
      const w = Object.assign(new FakeTarget(), {
        closed: false,
        document: fakeDocument(),
        close() {
          if (this.closed) return
          this.closed = true
          this.fire("pagehide")
        },
      })
      windows.push(w)
      return w
    },
  }
  return { host: { document: fakeDocument(), documentPictureInPicture: api }, requested, windows }
}

test("support: Document PiP, then video PiP, then none", () => {
  assert.equal(pipSupport(documentPipWindow().host), "document")
  assert.equal(pipSupport({ document: fakeDocument() }), "video")
  assert.equal(pipSupport({ document: { ...fakeDocument(), pictureInPictureEnabled: false } }), "none")
  assert.equal(pipSupport(undefined), "none")
})

test("Document PiP: a live video of the canvas, titled, and one at a time", async () => {
  const { host, requested, windows } = documentPipWindow()
  const pip = new PictureInPicture({ window: host, fps: 24 })
  const seen = []
  pip.subscribe((s) => seen.push(s && pipKey(s)))
  const canvas = fakeCanvas()
  const h = await pip.open(desktopSource, canvas, { title: "dev" })
  assert.equal(h.mode, "document")
  assert.deepEqual(requested[0], { width: 480, height: 300 })
  assert.equal(windows[0].document.title, "dev")
  assert.equal(windows[0].document.children.length, 1)
  assert.equal(windows[0].document.children[0].played, true)
  assert.equal(canvas.tracks[0].fps, 24)
  assert.ok(pip.isOpen(desktopSource))

  // A window PiP replaces the desktop one; the desktop capture stops.
  const wc = fakeCanvas(800, 600)
  await pip.open(win1, wc)
  assert.equal(windows[0].closed, true)
  assert.equal(canvas.tracks[0].stopped, true)
  assert.equal(windows[1].document.title, "Mozilla Firefox")
  assert.equal(pipKey(pip.current), "window:0x1a00003")

  // The user closes the PiP window.
  windows[1].close()
  await h.closed
  assert.equal(pip.current, null)
  assert.equal(wc.tracks[0].stopped, true)
  assert.deepEqual(seen, ["desktop", "window:0x1a00003", null])
})

test("toggle closes what is open", async () => {
  const { host, windows } = documentPipWindow()
  const pip = new PictureInPicture({ window: host })
  await pip.toggle(desktopSource, fakeCanvas())
  assert.ok(pip.isOpen(desktopSource))
  assert.equal(await pip.toggle(desktopSource, fakeCanvas()), null)
  assert.equal(windows[0].closed, true)
  assert.equal(pip.current, null)
})

test("video PiP fallback, closed by the user or by close()", async () => {
  const host = { document: fakeDocument() }
  const pip = new PictureInPicture({ window: host })
  const canvas = fakeCanvas()
  const h = await pip.open(win1, canvas)
  assert.equal(h.mode, "video")
  assert.equal(h.window, null)
  assert.ok(host.document.pictureInPictureElement.pip)
  host.document.pictureInPictureElement.fire("leavepictureinpicture")
  await h.closed
  assert.equal(pip.current, null)
  assert.equal(canvas.tracks[0].stopped, true)

  const h2 = await pip.open(desktopSource, fakeCanvas())
  pip.close()
  await h2.closed
  assert.equal(host.document.pictureInPictureElement, null)
})

test("no PiP at all is an error, not a silent no-op", async () => {
  const pip = new PictureInPicture({ window: { document: { ...fakeDocument(), pictureInPictureEnabled: false } } })
  await assert.rejects(pip.open(desktopSource, fakeCanvas()), /no picture-in-picture/)
})
