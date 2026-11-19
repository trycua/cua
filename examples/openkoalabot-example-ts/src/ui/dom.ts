// A tiny DOM builder for the page (no framework). Uses the global
// `document`, so the tests run it under happy-dom.

type Child = Node | string | number | null | undefined | false
type Attrs = Record<string, string | number | boolean | null | undefined | ((e: Event) => void)>

/** `h("button", { class: "btn", onclick: fn }, "Label")`. */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag)
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue
    if (typeof v === "function") el.addEventListener(k.replace(/^on/, ""), v as EventListener)
    else if (k === "value" && "value" in el) (el as unknown as { value: string }).value = String(v)
    else if (k === "checked" && "checked" in el) (el as unknown as { checked: boolean }).checked = v === true
    else if (v === true) el.setAttribute(k, "")
    else el.setAttribute(k, String(v))
  }
  append(el, children)
  return el
}

export function append(el: Element, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue
    el.append(typeof c === "string" || typeof c === "number" ? String(c) : c)
  }
}

const PATHS: Record<string, string> = {
  plus: "M12 5v14M5 12h14",
  search: "M20 20l-4-4M17.5 11a6.5 6.5 0 1 1-13 0 6.5 6.5 0 0 1 13 0",
  paperclip: "M20.5 11.5 12.4 19.6a5 5 0 0 1-7.1-7.1l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8",
  arrowUp: "M12 19V5M6 11l6-6 6 6",
  monitor: "M5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM8 20h8M12 16v4",
  stop: "M8 6h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z",
  play: "M7 5v14l11-7z",
  expand: "M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7",
  pip: "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM12 12h6v4.5h-6z",
  close: "M6 6l12 12M18 6 6 18",
  panelRight: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM15 4v16",
  upload: "M12 16V4M7 9l5-5 5 5M4 16v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2",
  terminal: "m5 8 4 4-4 4M12 16h7",
  info: "M12 11v5M12 8h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0",
  alert: "M12 4 2.8 19.5h18.4zM12 10v4M12 17h.01",
  file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5",
  cloud: "M7 18h10a4 4 0 0 0 .6-8A6 6 0 0 0 6 9.5 4.3 4.3 0 0 0 7 18z",
  laptop: "M5.5 5h13A1.5 1.5 0 0 1 20 6.5v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 14.5v-8A1.5 1.5 0 0 1 5.5 5zM2 19h20",
  teleport: "M4 12h11M11 7l5 5-5 5M20 4v16",
  link: "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
  check: "m5 12 5 5 9-10",
  clock: "M12 7v5l3 2M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0",
  linux: "M4 3h16a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM5 7l5 5-5 5M12 17h7",
  windows: "M5 4h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM14 4h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM5 13h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1zM14 13h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1z",
  macos: "M5 4h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 21h6M3 8h18",
}

/** A line icon (24px grid, 1.8 stroke), drawn for this app. */
export function icon(name: keyof typeof PATHS | string, size = 16): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg"
  const svg = document.createElementNS(ns, "svg")
  svg.setAttribute("class", "icon")
  svg.setAttribute("viewBox", "0 0 24 24")
  svg.setAttribute("width", String(size))
  svg.setAttribute("height", String(size))
  svg.setAttribute("fill", "none")
  svg.setAttribute("stroke", "currentColor")
  svg.setAttribute("stroke-width", "1.8")
  svg.setAttribute("stroke-linecap", "round")
  svg.setAttribute("stroke-linejoin", "round")
  svg.setAttribute("aria-hidden", "true")
  if (size !== 16) svg.style.cssText = `width:${size}px;height:${size}px`
  const path = document.createElementNS(ns, "path")
  path.setAttribute("d", PATHS[name] ?? "")
  svg.append(path)
  return svg
}
