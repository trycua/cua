/**
 * `@trycua/cua/spaces/pip`: picture-in-picture of a Space, for browsers and
 * webviews. No native library, no Node imports.
 *
 * A PiP shows one {@link PipSource}: the whole desktop, or a single window
 * from the Space's window list (`space.windows()`, streamed with
 * `SpaceStreamOptions.windowId`). The stream itself is the caller's: whatever
 * paints a canvas (WebCodecs over the media socket) is what the PiP shows.
 *
 * - In a browser, {@link PictureInPicture} opens the Document
 *   Picture-in-Picture API when it exists (an always-on-top window holding a
 *   live `<video>` of the canvas) and falls back to video picture-in-picture.
 *   Browsers allow one PiP window at a time, so opening another source
 *   replaces the current one.
 * - In a desktop shell (Tauri and the like), open a small always-on-top
 *   window per source: {@link pipWindowLabel} names it, {@link encodePipRoute}
 *   tells its page what to stream and {@link parsePipRoute} reads that back.
 *
 * ```ts
 * import { PictureInPicture, desktopSource, windowSource } from "@trycua/cua/spaces/pip"
 *
 * const pip = new PictureInPicture()
 * button.onclick = () => pip.toggle(desktopSource, canvas, { title: "dev" })
 * row.onclick = () => pip.open(windowSource(w), windowCanvas, { title: pipLabel(windowSource(w)) })
 * ```
 */

/** What a PiP shows: the whole desktop or one window of the Space. */
export type PipSource =
  | { kind: "desktop" }
  | { kind: "window"; windowId: string; app: string; title: string }

export const desktopSource: PipSource = Object.freeze({ kind: "desktop" }) as PipSource

/** A window source from a window-list row (`SpaceWindow` or the core's JSON). */
export function windowSource(w: {
  windowId?: string
  window_id?: string
  appName?: string
  app_name?: string
  app?: string
  title?: string
}): PipSource {
  return {
    kind: "window",
    windowId: String(w.windowId ?? w.window_id ?? ""),
    app: String(w.appName ?? w.app_name ?? w.app ?? ""),
    title: String(w.title ?? ""),
  }
}

/** A stable key: `desktop`, or `window:<id>`. */
export function pipKey(source: PipSource): string {
  return source.kind === "desktop" ? "desktop" : `window:${source.windowId}`
}

/** One line for a title bar or a list row. */
export function pipLabel(source: PipSource, spaceName = ""): string {
  if (source.kind === "desktop") return spaceName || "Desktop"
  if (!source.title) return source.app || "Window"
  if (!source.app || source.title.includes(source.app)) return source.title
  return `${source.app} · ${source.title}`
}

/**
 * A window label every shell accepts (`[A-Za-z0-9_-]`, bounded): `pip-desktop`,
 * or `pip-w-` plus the window id with anything else replaced. Distinct ids
 * that collide after replacement get a short hash suffix.
 */
export function pipWindowLabel(source: PipSource): string {
  if (source.kind === "desktop") return "pip-desktop"
  const clean = source.windowId.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 48)
  const suffix = clean === source.windowId ? "" : `-${hash(source.windowId)}`
  return `pip-w-${clean}${suffix}`
}

function hash(s: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return (h >>> 0).toString(36)
}

/** The PiP page's route: `pip=desktop`, or `pip=window&id=…&app=…&title=…`. */
export function encodePipRoute(source: PipSource): string {
  const p = new URLSearchParams()
  if (source.kind === "desktop") p.set("pip", "desktop")
  else {
    p.set("pip", "window")
    p.set("id", source.windowId)
    if (source.app) p.set("app", source.app)
    if (source.title) p.set("title", source.title)
  }
  return p.toString()
}

/** Reads {@link encodePipRoute} from a hash or query (`#…`/`?…` allowed); null when absent. */
export function parsePipRoute(route: string): PipSource | null {
  const p = new URLSearchParams(route.replace(/^[#?]/, ""))
  switch (p.get("pip")) {
    case "desktop":
      return desktopSource
    case "window": {
      const id = p.get("id")
      return id ? { kind: "window", windowId: id, app: p.get("app") ?? "", title: p.get("title") ?? "" } : null
    }
    default:
      return null
  }
}

/** Fits a `width`×`height` picture into a PiP whose long edge is `longEdge`. */
export function pipSize(width: number, height: number, longEdge = 480): { width: number; height: number } {
  const w = width > 0 ? width : 16
  const h = height > 0 ? height : 9
  const scale = longEdge / Math.max(w, h)
  return { width: Math.max(1, Math.round(w * scale)), height: Math.max(1, Math.round(h * scale)) }
}

// ---------------------------------------------------------------- browser

/** `document`: Document Picture-in-Picture; `video`: video PiP; `none`: neither. */
export type PipMode = "document" | "video" | "none"

interface DocumentPipApi {
  requestWindow(options?: { width?: number; height?: number }): Promise<Window>
  window?: Window | null
}

/** The window-like object {@link PictureInPicture} needs (a real `window` in a page). */
export interface PipHostWindow {
  document: Document
  documentPictureInPicture?: DocumentPipApi
}

/** Which PiP this page can open. */
export function pipSupport(win: PipHostWindow | undefined = globalThis as unknown as PipHostWindow): PipMode {
  if (!win?.document) return "none"
  if (win.documentPictureInPicture && typeof win.documentPictureInPicture.requestWindow === "function") return "document"
  return (win.document as Document & { pictureInPictureEnabled?: boolean }).pictureInPictureEnabled ? "video" : "none"
}

export interface PipOpenOptions {
  /** The PiP's title (Document PiP only; video PiP has none). */
  title?: string
  /** Initial size; defaults to the canvas fitted by {@link pipSize}. */
  width?: number
  height?: number
}

/** An open PiP. */
export interface PipHandle {
  readonly mode: Exclude<PipMode, "none">
  readonly source: PipSource
  /** The Document PiP window, when that is the mode. */
  readonly window: Window | null
  /** Resolves once the PiP is closed, by {@link close} or by the user. */
  readonly closed: Promise<void>
  close(): void
}

export interface PictureInPictureOptions {
  /** The page's window (default: `globalThis`). */
  window?: PipHostWindow
  /** Frames per second of the canvas capture (default 30). */
  fps?: number
}

/**
 * One PiP at a time, of the desktop or of a window. The canvas keeps being
 * painted by its stream; the PiP shows a live capture of it, so nothing moves
 * out of the page and closing the PiP never stops the stream.
 */
export class PictureInPicture {
  #win: PipHostWindow | undefined
  #fps: number
  #current: PipHandle | null = null
  #listeners = new Set<(source: PipSource | null) => void>()

  constructor(options: PictureInPictureOptions = {}) {
    this.#win = options.window ?? (globalThis as unknown as PipHostWindow)
    this.#fps = options.fps ?? 30
  }

  get mode(): PipMode {
    return pipSupport(this.#win)
  }

  /** What is in the PiP now, or null. */
  get current(): PipSource | null {
    return this.#current?.source ?? null
  }

  /** Whether `source` is what the PiP shows. */
  isOpen(source: PipSource): boolean {
    return this.#current !== null && pipKey(this.#current.source) === pipKey(source)
  }

  /** Called with the new source (or null) on every open and close. */
  subscribe(fn: (source: PipSource | null) => void): () => void {
    this.#listeners.add(fn)
    return () => this.#listeners.delete(fn)
  }

  /**
   * Opens `source` from `canvas`, replacing any open PiP. Call it from the
   * click handler: browsers only open a PiP on a user gesture.
   */
  async open(source: PipSource, canvas: HTMLCanvasElement, options: PipOpenOptions = {}): Promise<PipHandle> {
    const win = this.#win
    const mode = pipSupport(win)
    if (!win || mode === "none") throw new Error("this browser has no picture-in-picture")
    const size = options.width && options.height ? { width: options.width, height: options.height } : pipSize(canvas.width, canvas.height)
    // The PiP being replaced closes quietly: listeners see the new source,
    // not a null in between.
    const previous = this.#current
    this.#current = null
    let pipWindow: Window | null = null
    try {
      // Document PiP first, before any other await: it needs the gesture.
      if (mode === "document") pipWindow = await win.documentPictureInPicture!.requestWindow(size)
    } catch (e) {
      this.#current = previous
      throw e
    }
    previous?.close()
    const stream = canvas.captureStream(this.#fps)
    const doc = pipWindow?.document ?? win.document
    const video = doc.createElement("video")
    video.muted = true
    video.autoplay = true
    video.playsInline = true
    video.srcObject = stream
    let resolveClosed = () => {}
    const closed = new Promise<void>((r) => (resolveClosed = r))
    let done = false
    const finish = () => {
      if (done) return
      done = true
      for (const t of stream.getTracks()) t.stop()
      video.srcObject = null
      if (this.#current === handle) {
        this.#current = null
        this.#emit(null)
      }
      resolveClosed()
    }
    const handle: PipHandle = {
      mode: mode,
      source,
      window: pipWindow,
      closed,
      close: () => {
        if (done) return
        if (pipWindow) pipWindow.close()
        else if ((win.document as Document & { pictureInPictureElement?: Element | null }).pictureInPictureElement === video)
          void (win.document as Document & { exitPictureInPicture?: () => Promise<void> }).exitPictureInPicture?.().catch(() => {})
        finish()
      },
    }
    if (pipWindow) {
      pipWindow.document.title = options.title ?? pipLabel(source)
      const style = pipWindow.document.createElement("style")
      style.textContent =
        "html,body{margin:0;height:100%;background:#000;overflow:hidden}video{display:block;width:100%;height:100%;object-fit:contain}"
      pipWindow.document.head.append(style)
      pipWindow.document.body.append(video)
      pipWindow.addEventListener("pagehide", finish, { once: true })
      await video.play().catch(() => {})
    } else {
      video.addEventListener("leavepictureinpicture", finish, { once: true })
      try {
        await video.play()
        await (video as HTMLVideoElement & { requestPictureInPicture(): Promise<unknown> }).requestPictureInPicture()
      } catch (e) {
        finish()
        if (previous) this.#emit(null)
        throw e
      }
    }
    this.#current = handle
    this.#emit(source)
    return handle
  }

  /** Opens `source`, or closes it when it is what the PiP shows. */
  async toggle(source: PipSource, canvas: HTMLCanvasElement, options: PipOpenOptions = {}): Promise<PipHandle | null> {
    if (this.isOpen(source)) {
      this.close()
      return null
    }
    return this.open(source, canvas, options)
  }

  close(): void {
    this.#current?.close()
  }

  #emit(source: PipSource | null): void {
    for (const fn of this.#listeners) fn(source)
  }
}
