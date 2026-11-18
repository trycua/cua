/**
 * The one drop zone for a Space: a dashed well captioned "Drop a file or
 * window", with "Send file…" and "Teleport an app…" below it. Files, app
 * bundles and dragged windows all land on the same zone; the host decides what
 * each becomes (a verified file send, or the "Teleport an app…" picker). The
 * same zone as the Cua Spaces app's.
 *
 * - {@link dropZoneStatusLine}: what the zone says it did, concretely (a
 *   file is only "sent" once the Space verified it).
 * - {@link renderTeleportDropZone}: the markup, for any framework.
 * - `<cua-drop-zone>` ({@link defineDropZoneElement}): the web component.
 *   Set `.status` and, for drags the DOM does not see (a webview's native
 *   file drop, a window drag), `.over`. Fires `cua-drop` (`detail`: the
 *   `DataTransfer` of a DOM drop), `cua-send-file` and `cua-teleport-app`.
 *   Put `data-teleport-target` on it to make it a window-drag target.
 */

import { formatBytes } from "./model.js"

export const DROP_ZONE_CAPTION = "Drop a file or window"
export const DROP_ZONE_SEND_FILE = "Send file…"
export const DROP_ZONE_TELEPORT_APP = "Teleport an app…"

/** One file the Space verified. */
export interface DropZoneFile {
  name: string
  bytes: number
  /** Where it landed in the Space. */
  dest: string
}

export type DropZoneStatus =
  | { kind: "idle" }
  | { kind: "sending"; label: string }
  | { kind: "sent"; files: DropZoneFile[] }
  | { kind: "failed"; message: string }

export const idleDropZone: DropZoneStatus = Object.freeze({ kind: "idle" }) as DropZoneStatus

/** The status line under the buttons, or null when there is nothing to say. */
export function dropZoneStatusLine(status: DropZoneStatus): string | null {
  switch (status.kind) {
    case "idle":
      return null
    case "sending":
      return `Sending ${status.label}…`
    case "sent": {
      const [first] = status.files
      if (!first) return null
      if (status.files.length === 1) return `${first.name} (${formatBytes(first.bytes)}) verified in ${first.dest}`
      const total = status.files.reduce((sum, f) => sum + f.bytes, 0)
      return `${status.files.length} files (${formatBytes(total)}) verified`
    }
    case "failed":
      return status.message
  }
}

/** "Sending report.pdf…" / "Sending 3 files…". */
export function sendingLabel(names: readonly string[]): string {
  if (names.length === 1) return names[0]!.split(/[\\/]/).pop() || names[0]!
  return `${names.length} files`
}

/** Whether a client-space point is inside `element`'s box. */
export function isInsideZone(element: { getBoundingClientRect(): DOMRect } | null, point: { x: number; y: number }): boolean {
  if (!element) return false
  const b = element.getBoundingClientRect()
  if (b.width === 0 && b.height === 0) return false
  return point.x >= b.left && point.x <= b.right && point.y >= b.top && point.y <= b.bottom
}

export const DROP_ZONE_STYLES = `
:host { --fg: var(--cua-teleport-fg, #16181d); --muted: var(--cua-teleport-muted, #6b7280);
  --accent: var(--cua-teleport-accent, #2563eb); --border: var(--cua-teleport-border, #d1d5db);
  --danger: var(--cua-teleport-danger, #b91c1c); --radius: var(--cua-teleport-radius, 10px);
  display: block; color: var(--fg); font: 13px/1.35 system-ui, -apple-system, sans-serif; }
@media (prefers-color-scheme: dark) { :host { --fg: var(--cua-teleport-fg, #eceef2); --muted: var(--cua-teleport-muted, #9aa0aa);
  --accent: var(--cua-teleport-accent, #5b8def); --border: var(--cua-teleport-border, #3a3d44); --danger: var(--cua-teleport-danger, #f87171); } }
.zone { display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 14px 12px 12px; text-align: center;
  border: 1.5px dashed var(--border); border-radius: var(--radius); background: color-mix(in srgb, var(--fg) 4%, transparent); }
.zone[data-drop-target=true] { border-style: solid; border-color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, transparent); }
.zone[data-busy=true] { opacity: .72; }
.caption { margin: 0; color: var(--muted); font-size: 12.5px; }
.actions { display: flex; flex-wrap: wrap; justify-content: center; gap: 6px; margin-top: 2px; }
button { padding: 4px 12px; font: inherit; font-size: 12.5px; color: var(--fg); cursor: pointer;
  background: color-mix(in srgb, var(--fg) 8%, transparent); border: 1px solid var(--border); border-radius: 999px; }
button:hover:not(:disabled) { background: color-mix(in srgb, var(--fg) 14%, transparent); }
button:disabled { opacity: .5; cursor: default; }
.status { margin: 2px 0 0; max-width: 40ch; color: var(--muted); font-size: 11.5px; overflow-wrap: anywhere; }
.status[data-kind=failed] { color: var(--danger); }
`

const esc = (s: unknown): string =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!)

/** The zone's markup. Buttons carry `data-act="send-file"` / `data-act="teleport-app"`. */
export function renderTeleportDropZone(state: { over?: boolean; status?: DropZoneStatus; canSendFile?: boolean }): string {
  const status = state.status ?? idleDropZone
  const busy = status.kind === "sending"
  const line = dropZoneStatusLine(status)
  const off = busy ? " disabled" : ""
  return `<div class="zone" part="zone"${state.over ? ' data-drop-target="true"' : ""}${busy ? ' data-busy="true"' : ""}>
    <p class="caption">${esc(DROP_ZONE_CAPTION)}</p>
    <div class="actions">${
      state.canSendFile === false ? "" : `<button type="button" data-act="send-file"${off}>${esc(DROP_ZONE_SEND_FILE)}</button>`
    }<button type="button" data-act="teleport-app"${off}>${esc(DROP_ZONE_TELEPORT_APP)}</button></div>
    ${line ? `<p class="status" role="status" data-kind="${status.kind}">${esc(line)}</p>` : ""}
  </div>`
}

/** Registers `<cua-drop-zone>` (idempotent). Nothing is registered on import. */
export function defineDropZoneElement(registry: CustomElementRegistry = customElements): void {
  if (registry.get("cua-drop-zone")) return
  class DropZone extends HTMLElement {
    #over = false
    #domOver = false
    #status: DropZoneStatus = idleDropZone

    set over(v: boolean) {
      this.#over = !!v
      this.#render()
    }
    get over(): boolean {
      return this.#over || this.#domOver
    }
    set status(s: DropZoneStatus) {
      this.#status = s ?? idleDropZone
      this.#render()
    }
    get status(): DropZoneStatus {
      return this.#status
    }

    connectedCallback() {
      if (!this.shadowRoot) {
        const root = this.attachShadow({ mode: "open" })
        root.addEventListener("click", this.#onClick)
        this.addEventListener("dragover", this.#onDragOver)
        this.addEventListener("dragleave", this.#onDragLeave)
        this.addEventListener("drop", this.#onDrop)
      }
      this.#render()
    }

    #fire(type: string, detail?: unknown) {
      this.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, composed: true }))
    }
    #onClick = (ev: Event) => {
      const act = (ev.target as HTMLElement).closest<HTMLElement>("[data-act]")?.dataset.act
      if (act === "send-file") this.#fire("cua-send-file")
      if (act === "teleport-app") this.#fire("cua-teleport-app")
    }
    #onDragOver = (ev: Event) => {
      ev.preventDefault()
      if (!this.#domOver) {
        this.#domOver = true
        this.#render()
      }
    }
    #onDragLeave = (ev: Event) => {
      const into = (ev as DragEvent).relatedTarget as Node | null
      if (into && (into === this || this.contains(into) || this.shadowRoot?.contains(into))) return
      this.#domOver = false
      this.#render()
    }
    #onDrop = (ev: Event) => {
      ev.preventDefault()
      this.#domOver = false
      this.#render()
      const dt = (ev as DragEvent).dataTransfer
      if (dt) this.#fire("cua-drop", dt)
    }
    #render() {
      if (!this.shadowRoot) return
      this.toggleAttribute("data-over", this.over)
      this.shadowRoot.innerHTML = `<style>${DROP_ZONE_STYLES}</style>${renderTeleportDropZone({
        over: this.over,
        status: this.#status,
        canSendFile: !this.hasAttribute("no-send-file"),
      })}`
    }
  }
  registry.define("cua-drop-zone", DropZone)
}
