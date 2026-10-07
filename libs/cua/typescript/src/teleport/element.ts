/**
 * Framework-agnostic web components over the headless picker:
 *
 * - `<cua-teleport-picker>`: "Teleport an app…" (search, icons, recents,
 *   capability levels, the move choice, the consent screen listing every
 *   path and secret, progress). Set `.host` (a {@link TeleportHost}) and the
 *   `space-name` attribute; optional `.preselect` jumps to one app. Fires
 *   `cua-teleport-close` and `cua-teleport-done` (`detail`: the report).
 * - `<cua-drop-zone>`: the one dashed zone for files, apps and windows
 *   (see `dropZone.ts`).
 * - `<cua-teleport-drop>`: the drop zone shown while a window or app is
 *   dragged: the dragged window's preview, the app and its capability, the
 *   target Space. Set `.state` (a {@link WindowDropState}) and `space-name`.
 *
 * Call {@link defineTeleportElements} once in a browser or webview; nothing
 * is registered on import, so Node can import this module.
 *
 * Theming: CSS custom properties `--cua-teleport-bg`, `-fg`, `-muted`,
 * `-accent`, `-border`, `-danger`, `-radius`; light and dark defaults follow
 * `prefers-color-scheme`.
 */

import { TeleportPickerController } from "./controller.js"
import {
  CAPABILITY_LABEL,
  type CatalogEntry,
  MOVE_LABEL,
  type TeleportHost,
  canConfirm,
  canPlan,
  formatBytes,
  progress,
  sections,
} from "./model.js"
import type { WindowDropState } from "./windowDrag.js"
import { defineDropZoneElement } from "./dropZone.js"

const esc = (s: unknown): string =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!)

export const TELEPORT_STYLES = `
:host { --bg: var(--cua-teleport-bg, #ffffff); --fg: var(--cua-teleport-fg, #16181d);
  --muted: var(--cua-teleport-muted, #6b7280); --accent: var(--cua-teleport-accent, #2563eb);
  --border: var(--cua-teleport-border, #e5e7eb); --danger: var(--cua-teleport-danger, #b91c1c);
  --radius: var(--cua-teleport-radius, 10px); --row: color-mix(in srgb, var(--fg) 5%, var(--bg));
  display: block; color: var(--fg); background: var(--bg); font: 13px/1.4 system-ui, -apple-system, sans-serif; }
@media (prefers-color-scheme: dark) { :host { --bg: var(--cua-teleport-bg, #1c1d21); --fg: var(--cua-teleport-fg, #eceef2);
  --muted: var(--cua-teleport-muted, #9aa0aa); --accent: var(--cua-teleport-accent, #5b8def);
  --border: var(--cua-teleport-border, #2e3036); --danger: var(--cua-teleport-danger, #f87171); } }
.panel { display: flex; flex-direction: column; gap: 12px; padding: 16px; min-height: 100%; box-sizing: border-box; }
h1 { font-size: 15px; margin: 0; font-weight: 600; }
.hint { color: var(--muted); margin: 0; }
input[type=search] { width: 100%; box-sizing: border-box; padding: 8px 10px; border-radius: 8px; border: 1px solid var(--border);
  background: var(--bg); color: var(--fg); font: inherit; }
.list { display: flex; flex-direction: column; gap: 2px; overflow: auto; flex: 1; min-height: 120px; }
.section { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: .04em; margin: 8px 4px 2px; }
.row { display: grid; grid-template-columns: 28px 1fr auto; gap: 10px; align-items: center; padding: 6px 8px;
  border-radius: 8px; border: 1px solid transparent; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.row[aria-selected=true] { background: var(--row); border-color: var(--accent); }
.row:disabled { cursor: default; opacity: .55; }
.icon { width: 28px; height: 28px; border-radius: 7px; object-fit: contain; }
.mono { width: 28px; height: 28px; border-radius: 7px; display: grid; place-items: center; background: var(--row); font-weight: 600; }
.name { font-weight: 500; } .sub { color: var(--muted); font-size: 12px; }
.badge { font-size: 11px; padding: 2px 8px; border-radius: 999px; border: 1px solid var(--border); color: var(--muted); white-space: nowrap; }
.badge.full { color: var(--accent); border-color: var(--accent); }
.choices { display: flex; flex-direction: column; gap: 6px; }
.choice { display: flex; gap: 8px; align-items: flex-start; padding: 8px; border: 1px solid var(--border); border-radius: 8px; }
.files { display: flex; flex-direction: column; gap: 4px; }
.file { display: flex; justify-content: space-between; gap: 8px; padding: 4px 8px; background: var(--row); border-radius: 6px; font-family: ui-monospace, monospace; font-size: 12px; }
.items { display: flex; flex-direction: column; gap: 4px; }
.item { display: grid; grid-template-columns: auto 1fr auto; gap: 8px; align-items: baseline; padding: 6px 8px; border-radius: 6px; background: var(--row); }
.item .k { font-size: 11px; color: var(--muted); text-transform: uppercase; }
.item.secret .k { color: var(--danger); }
.detail { color: var(--muted); font-size: 12px; word-break: break-all; }
.warn { color: var(--muted); font-size: 12px; margin: 0; }
.error { color: var(--danger); }
.bar { height: 6px; border-radius: 3px; background: var(--row); overflow: hidden; }
.bar > span { display: block; height: 100%; background: var(--accent); }
footer { display: flex; justify-content: flex-end; gap: 8px; margin-top: auto; }
button.btn { padding: 6px 14px; border-radius: 8px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); font: inherit; cursor: pointer; }
a.btn { padding: 6px 14px; border-radius: 8px; border: 1px solid var(--border); font: inherit; text-decoration: none; }
a.primary, button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
button.btn:disabled { opacity: .5; cursor: default; }
.drop { display: grid; grid-template-columns: 96px 1fr; gap: 12px; align-items: center; padding: 12px; border-radius: var(--radius);
  border: 2px dashed var(--border); }
.drop.over { border-color: var(--accent); }
.drop .thumb { width: 96px; height: 64px; border-radius: 6px; object-fit: cover; background: var(--row); }
`

function monogram(name: string): string {
  return `<span class="mono" aria-hidden="true">${esc(name.charAt(0).toUpperCase() || "?")}</span>`
}

function iconHtml(e: CatalogEntry, icons: Map<string, string | null>): string {
  const url = icons.get(e.id)
  return url ? `<img class="icon" src="${esc(url)}" alt="">` : monogram(e.name)
}

export function renderPicker(c: TeleportPickerController, icons: Map<string, string | null>): string {
  const s = c.state
  const space = esc(s.spaceName)
  switch (s.step) {
    case "loading":
      return `<div class="panel"><h1>Teleport an app to ${space}</h1><p class="hint">Looking for apps on this machine…</p></div>`
    case "pick": {
      const rows = sections(s)
        .map(
          (sec) =>
            `<div class="section">${esc(sec.title)}</div>` +
            sec.entries
              .map((e) => {
                const disabled = e.capability === "unsupported"
                return `<button class="row" role="option" data-id="${esc(e.id)}" aria-selected="${e.id === s.selectedId}" ${disabled ? "disabled" : ""}
                  title="${esc(disabled ? (e.reason ?? "") : CAPABILITY_LABEL[e.capability])}">
                  ${iconHtml(e, icons)}<span><span class="name">${esc(e.name)}</span>${disabled && e.reason ? `<br><span class="sub">${esc(e.reason)}</span>` : ""}</span>
                  <span class="badge ${esc(e.capability.replace("_", "-"))}">${esc(CAPABILITY_LABEL[e.capability])}</span></button>`
              })
              .join(""),
        )
        .join("")
      return `<div class="panel"><h1>Teleport an app to ${space}</h1>
        <input type="search" placeholder="Search apps" aria-label="Search apps" value="${esc(s.query)}">
        <div class="list" role="listbox" aria-label="Apps">${rows || `<p class="hint">No apps match.</p>`}</div>
        <footer><button class="btn" data-act="close">Cancel</button>
        <button class="btn primary" data-act="choose" ${s.selectedId ? "" : "disabled"}>Continue</button></footer></div>`
    }
    case "options": {
      const e = s.entry!
      const choices = e.moves
        .map(
          (m) => `<label class="choice"><input type="radio" name="move" value="${m}" ${m === s.move ? "checked" : ""}>
            <span>${esc(MOVE_LABEL[m])}</span></label>`,
        )
        .join("")
      const files =
        s.move === "app_with_files"
          ? `<div class="files">${s.files
              .map((f) => `<div class="file"><span>${esc(f)}</span><button class="btn" data-remove="${esc(f)}">Remove</button></div>`)
              .join("")}${c.host.chooseFiles ? `<button class="btn" data-act="files">Add files or folders…</button>` : `<p class="hint">Drop files or folders here.</p>`}</div>`
          : ""
      return `<div class="panel"><h1>${iconHtml(e, icons)} ${esc(e.name)}</h1>
        <p class="hint">What should move to ${space}?</p><div class="choices">${choices}</div>${files}
        ${e.reason ? `<p class="warn">${esc(e.reason)}</p>` : ""}
        <footer><button class="btn" data-act="back">Back</button>
        <button class="btn primary" data-act="plan" ${canPlan(s) ? "" : "disabled"}>Review</button></footer></div>`
    }
    case "planning":
      return `<div class="panel"><h1>${esc(s.entry?.name)}</h1><p class="hint">Working out what will move…</p></div>`
    case "consent": {
      const p = s.plan!
      const items = p.consent
        .map(
          (i) => `<div class="item ${i.sensitive ? "secret" : ""}"><span class="k">${esc(i.kind)}</span>
            <span><span class="name">${esc(i.label)}</span><br><span class="detail">${esc(i.detail)}</span></span>
            <span class="sub">${i.bytes ? esc(formatBytes(i.bytes)) : ""}</span></div>`,
        )
        .join("")
      return `<div class="panel"><h1>Teleport ${esc(p.app.name)} to ${space}?</h1>
        <p class="hint">${esc(p.steps.map((x) => x.summary).join(" · "))}</p>
        <div class="items" role="list">${items}</div>
        <p class="hint">${p.totalBytes ? `${esc(formatBytes(p.totalBytes))} leaves this machine.` : "No files or app data leave this machine."}</p>
        ${p.warnings.map((w) => `<p class="warn">${esc(w)}</p>`).join("")}
        ${p.sensitive ? `<label class="choice"><input type="checkbox" data-act="ack" ${s.acknowledged ? "checked" : ""}><span>I understand the secrets above leave this machine.</span></label>` : ""}
        <footer><button class="btn" data-act="back">Back</button>
        <button class="btn primary" data-act="confirm" ${canConfirm(s) ? "" : "disabled"}>Teleport</button></footer></div>`
    }
    case "running": {
      const last = s.events[s.events.length - 1]
      return `<div class="panel"><h1>Teleporting ${esc(s.plan?.app.name)} to ${space}…</h1>
        <div class="bar" role="progressbar" aria-valuenow="${Math.round(progress(s) * 100)}"><span style="width:${Math.round(progress(s) * 100)}%"></span></div>
        <p class="hint">${esc(last ? `${last.kind}: ${last.detail}` : "Starting…")}</p></div>`
    }
    case "done": {
      const r = s.report!
      return `<div class="panel"><h1>${esc(s.plan?.app.name)} is in ${space}</h1>
        <p class="hint">${[
          r.installed.length ? `Installed ${esc(r.installed.join(", "))}.` : "",
          r.sent.length ? `Sent ${r.sent.length} item(s).` : "",
          r.imported.length ? `Imported ${r.imported.length} item(s).` : "",
          r.launched ? "Opened." : "",
        ].join(" ")}</p><footer><button class="btn primary" data-act="close">Done</button></footer></div>`
    }
    case "error":
      if (s.installPrompt) {
        const p = s.installPrompt
        return `<div class="panel install-cua"><h1>${esc(p.title)}</h1>
        <p class="hint">${esc(p.message)}</p>
        <footer><button class="btn" data-act="back">Back</button><a class="btn primary" data-act="install-cua" href="${esc(p.url)}" target="_blank" rel="noopener noreferrer">${esc(p.actionLabel)}</a></footer></div>`
      }
      return `<div class="panel"><h1>Could not teleport${s.entry ? ` ${esc(s.entry.name)}` : ""}</h1>
        <p class="error" role="alert">${esc(s.error)}</p>
        <footer><button class="btn" data-act="close">Close</button><button class="btn primary" data-act="back">Back</button></footer></div>`
  }
}

export function renderDropZone(state: WindowDropState, spaceName: string): string {
  if (!state.active || !state.app) return ""
  const thumb = state.thumbnail
    ? `<img class="thumb" src="${esc(state.thumbnail)}" alt="">`
    : `<span class="thumb" aria-hidden="true"></span>`
  const title = state.window?.title ? ` · ${esc(state.window.title)}` : ""
  return `<div class="drop ${state.overId ? "over" : ""}">${thumb}<div><div class="name">${esc(state.app.name)}${title}</div>
    <div class="sub">${esc(CAPABILITY_LABEL[state.app.capability])}</div>
    <div class="hint">${state.overId ? `Release to teleport to ${esc(spaceName)}` : `Drop on a Space to teleport`}</div></div></div>`
}

/** Registers `<cua-teleport-picker>`, `<cua-teleport-drop>` and `<cua-drop-zone>` (idempotent). */
export function defineTeleportElements(registry: CustomElementRegistry = customElements): void {
  defineDropZoneElement(registry)
  if (!registry.get("cua-teleport-picker")) {
    class Picker extends HTMLElement {
      #host: TeleportHost | null = null
      #c: TeleportPickerController | null = null
      #icons = new Map<string, string | null>()
      #unsub: (() => void) | null = null
      preselect: { entry: CatalogEntry; files?: string[] } | null = null

      set host(h: TeleportHost | null) {
        this.#host = h
        if (this.isConnected) this.#start()
      }
      get host(): TeleportHost | null {
        return this.#host
      }
      get controller(): TeleportPickerController | null {
        return this.#c
      }

      #doneSent = false

      connectedCallback() {
        if (!this.shadowRoot) {
          const root = this.attachShadow({ mode: "open" })
          root.addEventListener("click", this.#onClick)
          root.addEventListener("input", this.#onInput)
          root.addEventListener("change", this.#onChange)
          root.addEventListener("dblclick", this.#onDbl)
        }
        if (this.#host) this.#start()
      }
      disconnectedCallback() {
        this.#unsub?.()
        this.#unsub = null
      }

      #start() {
        if (!this.#host || !this.shadowRoot) return
        this.#unsub?.()
        const opts: ConstructorParameters<typeof TeleportPickerController>[1] = {
          spaceName: this.getAttribute("space-name") ?? "this Space",
        }
        if (this.preselect) opts.preselect = this.preselect
        this.#c = new TeleportPickerController(this.#host, opts)
        this.#unsub = this.#c.subscribe(() => this.#render())
        this.#render()
        this.#doneSent = false
        void this.#c.load()
      }

      #render() {
        const c = this.#c
        if (!c || !this.shadowRoot) return
        const focusSearch = this.shadowRoot.activeElement?.matches("input[type=search]")
        this.shadowRoot.innerHTML = `<style>${TELEPORT_STYLES}</style>${renderPicker(c, this.#icons)}`
        if (focusSearch || c.state.step === "pick") {
          const input = this.shadowRoot.querySelector<HTMLInputElement>("input[type=search]")
          if (input && focusSearch) {
            input.focus()
            input.setSelectionRange(input.value.length, input.value.length)
          }
        }
        const host = this.#host
        if (host?.icon && c.state.entries) {
          for (const e of c.state.entries) {
            if (this.#icons.has(e.id)) continue
            this.#icons.set(e.id, null)
            void host.icon(e).then(
              (url) => {
                if (!url) return
                this.#icons.set(e.id, url)
                this.#render()
              },
              () => {},
            )
          }
        }
        if (c.state.step === "done" && !this.#doneSent) {
          this.#doneSent = true
          this.dispatchEvent(new CustomEvent("cua-teleport-done", { detail: c.state.report, bubbles: true, composed: true }))
        }
      }

      #onClick = (ev: Event) => {
        const c = this.#c
        const t = ev.target as HTMLElement
        if (!c) return
        const row = t.closest<HTMLElement>("[data-id]")
        if (row) c.dispatch({ type: "select", id: row.dataset.id! })
        const remove = t.closest<HTMLElement>("[data-remove]")
        if (remove) c.dispatch({ type: "remove-file", path: remove.dataset.remove! })
        switch (t.closest<HTMLElement>("[data-act]")?.dataset.act) {
          case "close":
            this.dispatchEvent(new CustomEvent("cua-teleport-close", { bubbles: true, composed: true }))
            break
          case "choose":
            c.choose()
            break
          case "back":
            c.dispatch({ type: "back" })
            break
          case "files":
            void c.chooseFiles()
            break
          case "plan":
            void c.plan()
            break
          case "confirm":
            void c.confirm()
            break
        }
      }
      #onDbl = (ev: Event) => {
        const row = (ev.target as HTMLElement).closest<HTMLElement>("[data-id]")
        if (row && this.#c) this.#c.choose(row.dataset.id!)
      }
      #onInput = (ev: Event) => {
        const t = ev.target as HTMLInputElement
        if (t.type === "search") this.#c?.dispatch({ type: "query", query: t.value })
      }
      #onChange = (ev: Event) => {
        const t = ev.target as HTMLInputElement
        if (t.name === "move") this.#c?.dispatch({ type: "move", move: t.value as never })
        if (t.dataset.act === "ack") this.#c?.dispatch({ type: "acknowledge", value: t.checked })
      }
    }
    registry.define("cua-teleport-picker", Picker)
  }
  if (!registry.get("cua-teleport-drop")) {
    class Drop extends HTMLElement {
      #state: WindowDropState | null = null
      set state(s: WindowDropState | null) {
        this.#state = s
        this.#render()
      }
      get state(): WindowDropState | null {
        return this.#state
      }
      connectedCallback() {
        if (!this.shadowRoot) this.attachShadow({ mode: "open" })
        this.#render()
      }
      #render() {
        if (!this.shadowRoot) return
        const html = this.#state ? renderDropZone(this.#state, this.getAttribute("space-name") ?? "") : ""
        this.shadowRoot.innerHTML = html ? `<style>${TELEPORT_STYLES}</style>${html}` : ""
      }
    }
    registry.define("cua-teleport-drop", Drop)
  }
}
