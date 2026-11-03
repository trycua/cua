/**
 * Drag payloads, read the same way as the Rust core
 * (`cua_teleport::ux::drop`; both run `testdata/drops.json`).
 *
 * Paths and `file://` URIs sort into apps (`.app`, `.desktop`, `.lnk`),
 * files and folders, and URLs. In a plain browser a Finder drop carries no
 * paths, only file names; {@link classifyBrowserDrop} then matches an
 * `X.app` name against the catalog.
 */

import type { CatalogEntry } from "./model.js"

export const MAX_DROP_ITEMS = 256

export type DropKind = "empty" | "app" | "files" | "url"

export interface DropPayload {
  kind: DropKind
  apps: string[]
  files: string[]
  urls: string[]
  ignored: string[]
}

export function isAppPath(path: string): boolean {
  const l = path.toLowerCase()
  return l.endsWith(".app") || l.endsWith(".desktop") || l.endsWith(".lnk")
}

function isAbsolute(p: string): boolean {
  return p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\")
}

function trimSeparators(p: string): string {
  const t = p.replace(/[/\\]+$/, "")
  return t === "" ? p : t
}

function percentDecode(s: string): string | null {
  const bytes: number[] = []
  for (let i = 0; i < s.length; ) {
    if (s[i] === "%") {
      const hex = s.slice(i + 1, i + 3)
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return null
      bytes.push(parseInt(hex, 16))
      i += 3
    } else {
      const code = s.codePointAt(i)!
      const ch = String.fromCodePoint(code)
      for (const b of new TextEncoder().encode(ch)) bytes.push(b)
      i += ch.length
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes))
  } catch {
    return null
  }
}

/** `file:///a%20b/` -> `/a b/`; `null` for another host. */
export function fileUriToPath(uri: string): string | null {
  let rest = uri.slice("file://".length)
  if (rest.startsWith("localhost")) rest = rest.slice("localhost".length)
  if (!rest.startsWith("/")) return null
  const decoded = percentDecode(rest)
  if (decoded == null) return null
  if (/^\/[A-Za-z]:/.test(decoded) && decoded.length > 3) return decoded.slice(1)
  return decoded
}

/** Parses paths, `file://` URIs, URLs or `text/uri-list` blobs. */
export function parseDrop(items: readonly string[]): DropPayload {
  const out: DropPayload = { kind: "empty", apps: [], files: [], urls: [], ignored: [] }
  const seen = new Set<string>()
  const lines = items
    .flatMap((i) => i.split(/\r?\n/))
    .map((l) => l.trim())
    .filter((l) => l !== "" && !l.startsWith("#"))
    .slice(0, MAX_DROP_ITEMS)
  for (const line of lines) {
    if (seen.has(line)) continue
    seen.add(line)
    const lower = line.toLowerCase()
    if (lower.startsWith("http://") || lower.startsWith("https://")) {
      out.urls.push(line)
      continue
    }
    let path = line
    if (lower.startsWith("file://")) {
      const p = fileUriToPath(line)
      if (p == null) {
        out.ignored.push(line)
        continue
      }
      path = p
    }
    if (!isAbsolute(path)) {
      out.ignored.push(line)
      continue
    }
    const t = trimSeparators(path)
    if (isAppPath(t)) out.apps.push(t)
    else out.files.push(t)
  }
  out.kind = out.apps.length ? "app" : out.files.length ? "files" : out.urls.length ? "url" : "empty"
  return out
}

/** What a browser drop carries (a `DataTransfer`, flattened). */
export interface BrowserDrop {
  /** `text/uri-list`, when present. */
  uriList?: string
  /** `text/plain`, when present. */
  text?: string
  /** Dropped file names (no paths in a browser). */
  fileNames?: string[]
}

/** Reads a DOM `DataTransfer` into a {@link BrowserDrop}. */
export function readDataTransfer(dt: {
  getData(type: string): string
  files?: ArrayLike<{ name: string }>
}): BrowserDrop {
  const names: string[] = []
  if (dt.files) for (let i = 0; i < dt.files.length; i++) names.push(dt.files[i]!.name)
  return { uriList: dt.getData("text/uri-list") || undefined, text: dt.getData("text/plain") || undefined, fileNames: names } as BrowserDrop
}

export type BrowserDropResult =
  | { kind: "app"; entry: CatalogEntry | null; name: string; path?: string }
  | { kind: "files"; names: string[]; paths: string[] }
  | { kind: "url"; urls: string[] }
  | { kind: "empty" }

/** The catalog row an app file name or bundle path names (`Blender.app`). */
export function entryForAppName(catalog: readonly CatalogEntry[], nameOrPath: string): CatalogEntry | null {
  const base = trimSeparators(nameOrPath).split(/[/\\]/).pop() ?? nameOrPath
  const name = base.replace(/\.(app|desktop|lnk)$/i, "")
  const lower = name.toLowerCase()
  return (
    catalog.find((e) => e.hostPath != null && trimSeparators(e.hostPath) === trimSeparators(nameOrPath)) ??
    catalog.find((e) => e.name === name) ??
    catalog.find((e) => e.hostAppId?.toLowerCase() === lower || e.id === lower) ??
    null
  )
}

/**
 * Classifies a browser drop: URIs first (a Dock or Finder drag in Chromium
 * carries `file://` URIs), then file names (`X.app` is an app).
 */
export function classifyBrowserDrop(drop: BrowserDrop, catalog: readonly CatalogEntry[]): BrowserDropResult {
  const parsed = parseDrop([drop.uriList ?? "", drop.text ?? ""].filter(Boolean))
  if (parsed.kind === "app") {
    const path = parsed.apps[0]!
    return { kind: "app", entry: entryForAppName(catalog, path), name: path, path }
  }
  const appName = (drop.fileNames ?? []).find(isAppPath)
  if (appName) return { kind: "app", entry: entryForAppName(catalog, appName), name: appName }
  if (parsed.kind === "files" || (drop.fileNames ?? []).length > 0) {
    return { kind: "files", names: drop.fileNames ?? [], paths: parsed.files }
  }
  if (parsed.kind === "url") return { kind: "url", urls: parsed.urls }
  return { kind: "empty" }
}
