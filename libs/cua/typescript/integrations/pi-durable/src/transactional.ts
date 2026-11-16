// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The workspace image behind transactional mode.
 *
 * The log holds a content-addressed image of the Space's workspace (a path
 * table plus the bytes of each distinct file) and every wrapped call commits
 * the diff it made to that image in the same transaction as its outcome. The
 * image is therefore always the workspace as of the last committed call, and
 * whatever state a crash leaves the Space in, `reconcile` makes the Space
 * equal to the log again: write what is missing or different, delete what the
 * log does not have. No per-call checkpoint of the Space is needed.
 *
 * The workspace is scanned with a few POSIX tools through cua-spacesd
 * (`find`, `stat`, `sha256sum`, `xargs`; GNU or BSD flavors) and files are
 * read and written with its file transfer. What it tracks: regular files
 * (content and mode) and symbolic links (target). What it does not: empty
 * directories, ownership, timestamps, extended attributes, special files,
 * and file names containing a backslash or a newline (a workspace with one is
 * refused, not silently dropped).
 */

import { createHash } from "node:crypto"
import { type SpacesdLike, quote, sh } from "./spacesd.js"

/** What transactional mode may hold. */
export interface TransactionalLimits {
  /** Most tracked files (regular files and links). Default 1000. */
  maxFiles: number
  /** Most total bytes of regular files. Default 16 MiB. */
  maxBytes: number
  /** Workspace-relative paths (files or directories) left out of the image. Default none. */
  exclude: readonly string[]
}

export const DEFAULT_TRANSACTIONAL_LIMITS: TransactionalLimits = {
  maxFiles: 1000,
  maxBytes: 16 * 1024 * 1024,
  exclude: [],
}

/** One tracked path: kind (`f` file, `l` link), mode, size, SHA-256 of the content (or of the link target) and the link target. */
export type ImageFile = { k: "f" | "l"; m: number; s: number; h: string; t?: string }

/** Path (relative to the workspace) to entry. */
export type Manifest = Record<string, ImageFile>

/** The `cua.space.image` document. */
export type ImageState = {
  /** The conversation and Space this image describes. */
  owner?: number
  space?: string
  /** The life of the Space the workspace was last known to equal the image in. */
  incarnation?: string
  /** Set when the workspace outgrew the limits under explicit transactional mode: the image no longer describes it. */
  stale?: boolean
  files?: Record<string, ImageFile>
  /** Content of regular files by SHA-256, base64. */
  blobs?: Record<string, string>
}

/** The Space cannot do transactional mode (missing tools, or a file name the scan cannot represent). */
export class TransactionalUnsupported extends Error {
  override readonly name = "TransactionalUnsupported"
}

/** The workspace is larger than the limits. */
export class TransactionalLimitExceeded extends Error {
  override readonly name = "TransactionalLimitExceeded"
  readonly reason: "files" | "bytes"
  constructor(reason: "files" | "bytes", message: string) {
    super(message)
    this.reason = reason
  }
}

function sha256(data: string | Uint8Array): string {
  return createHash("sha256").update(data).digest("hex")
}

function pruneExpression(exclude: readonly string[]): string {
  if (exclude.length === 0) return ""
  const parts = exclude.map((path) => {
    const clean = path.replace(/^\.?\/+/, "").replace(/\/+$/, "")
    return `-path ${quote(`./${clean}`)} -o -path ${quote(`./${clean}/*`)}`
  })
  return `\\( ${parts.join(" -o ")} \\) -prune -o `
}

/** `find` over files and links of the workspace, as shell text, with excluded paths pruned. */
function finder(exclude: readonly string[], types: "fl" | "f" | "l"): string {
  const test = types === "fl" ? "\\( -type f -o -type l \\)" : types === "f" ? "-type f" : "-type l"
  return `find . ${pruneExpression(exclude)}${test} -print`
}

function normalize(path: string): string {
  return path.replace(/^\.\//, "")
}

/**
 * The manifest of `root`: a bounded stat pass first (so a huge tree is
 * refused before anything is hashed), then one hashing pass.
 * `cap` multiplies the limits (reconciling a Space a runaway command filled
 * must still see what to delete).
 */
export async function scan(
  spacesd: SpacesdLike,
  root: string,
  limits: TransactionalLimits,
  cap = 1,
): Promise<Manifest> {
  const maxFiles = limits.maxFiles * cap
  const maxBytes = limits.maxBytes * cap
  const list = finder(limits.exclude, "fl")
  const header = [
    `cd ${quote(root)} || exit 3`,
    "command -v sha256sum >/dev/null 2>&1 || exit 12",
    "command -v xargs >/dev/null 2>&1 || exit 12",
    "if stat -c %a . >/dev/null 2>&1; then STAT='stat -c %F|%a|%s|%n'; elif stat -f %Lp . >/dev/null 2>&1; then STAT='stat -f %HT|%Lp|%z|%N'; else exit 12; fi",
    // Names the line-based scan cannot represent.
    "bs=$(printf '\\134')",
    "nl=$(printf '\\nx'); nl=${nl%x}",
    `bad=$(${list} | head -n ${maxFiles + 1} | grep -F -e "$bs" | head -n 1)`,
    `[ -z "$bad" ] || exit 11`,
    `bad=$(find . ${pruneExpression(limits.exclude)}\\( -type f -o -type l \\) -path "*$nl*" -print | head -n 1)`,
    `[ -z "$bad" ] || exit 11`,
  ]
  const stat = await sh(
    spacesd,
    [...header, `${list} | head -n ${maxFiles + 1} | tr '\\n' '\\0' | xargs -0 -r $STAT`].join("\n"),
  )
  failFor(stat.code, root, stat.stderr || stat.error)
  const entries = new Map<string, ImageFile>()
  let bytes = 0
  for (const line of stat.stdout.split("\n")) {
    if (line === "") continue
    const first = line.indexOf("|")
    const second = line.indexOf("|", first + 1)
    const third = line.indexOf("|", second + 1)
    if (first < 0 || second < 0 || third < 0) throw new Error(`workspace scan: unexpected line ${JSON.stringify(line)}`)
    const kind = line[0]!.toLowerCase() === "r" ? "f" : line[0]!.toLowerCase() === "s" ? "l" : undefined
    if (kind === undefined) continue
    const path = normalize(line.slice(third + 1))
    const size = Number(line.slice(second + 1, third))
    entries.set(path, { k: kind, m: kind === "f" ? Number.parseInt(line.slice(first + 1, second), 8) : 0, s: size, h: "" })
    if (kind === "f") bytes += size
  }
  if (entries.size > maxFiles) {
    throw new TransactionalLimitExceeded("files", `workspace has more than ${maxFiles} files`)
  }
  if (bytes > maxBytes) {
    throw new TransactionalLimitExceeded("bytes", `workspace holds ${bytes} bytes, over the limit of ${maxBytes}`)
  }
  if (entries.size === 0) return {}
  const hashing = await sh(
    spacesd,
    [
      `cd ${quote(root)} || exit 3`,
      `${finder(limits.exclude, "f")} | tr '\\n' '\\0' | xargs -0 -r sha256sum`,
      `${finder(limits.exclude, "l")} | while IFS= read -r l; do printf 'L|%s|%s\\n' "$l" "$(readlink "$l")"; done`,
    ].join("\n"),
  )
  failFor(hashing.code, root, hashing.stderr || hashing.error)
  for (const line of hashing.stdout.split("\n")) {
    if (line === "") continue
    if (line.startsWith("L|")) {
      const cut = line.indexOf("|", 2)
      const path = normalize(line.slice(2, cut))
      const target = line.slice(cut + 1)
      const entry = entries.get(path)
      if (entry) {
        entry.h = sha256(target)
        entry.t = target
      }
      continue
    }
    const match = /^\\?([0-9a-f]{64}) [ *](.*)$/.exec(line)
    if (!match) throw new Error(`workspace scan: unexpected hash line ${JSON.stringify(line)}`)
    const entry = entries.get(normalize(match[2]!))
    if (entry) entry.h = match[1]!
  }
  const manifest: Manifest = {}
  for (const [path, entry] of entries) {
    // A file that vanished between the passes is simply not there.
    if (entry.h === "") continue
    manifest[path] = entry
  }
  return manifest
}

function failFor(code: number | undefined, root: string, detail: string | undefined): void {
  if (code === 0) return
  if (code === 11) throw new TransactionalUnsupported("the workspace has a file name containing a backslash or a newline")
  if (code === 12) throw new TransactionalUnsupported("the Space lacks find, stat, xargs or sha256sum")
  if (code === 3) throw new Error(`workspace ${root} does not exist in the Space`)
  throw new Error(`workspace scan failed (${code ?? "no exit status"}): ${detail ?? ""}`)
}

/** Whether two entries are the same content, mode and kind. */
export function same(a: ImageFile | undefined, b: ImageFile | undefined): boolean {
  return a !== undefined && b !== undefined && a.k === b.k && a.h === b.h && (a.k === "l" || a.m === b.m)
}

/** What changes `from` into `to`. */
export function diff(
  from: Record<string, ImageFile> | undefined,
  to: Manifest,
): { upsert: string[]; remove: string[] } {
  const upsert: string[] = []
  const remove: string[] = []
  for (const [path, entry] of Object.entries(to)) if (!same(from?.[path], entry)) upsert.push(path)
  for (const path of Object.keys(from ?? {})) if (!(path in to)) remove.push(path)
  upsert.sort()
  remove.sort()
  return { upsert, remove }
}

/** Reads the content of `paths` (regular files) from the Space, verified against `manifest`. Base64 by SHA-256. */
export async function capture(
  spacesd: SpacesdLike,
  root: string,
  manifest: Manifest,
  paths: readonly string[],
  have: ReadonlySet<string>,
): Promise<Map<string, string>> {
  const blobs = new Map<string, string>()
  for (const path of paths) {
    const entry = manifest[path]
    if (!entry || entry.k !== "f" || have.has(entry.h) || blobs.has(entry.h)) continue
    const bytes = Buffer.from(await spacesd.download(`${root}/${path}`))
    if (sha256(bytes) !== entry.h) {
      throw new Error(`${path} changed while the workspace was being captured`)
    }
    blobs.set(entry.h, bytes.toString("base64"))
  }
  return blobs
}

/** Applies a diff to the image document (a draft inside a commit). */
export function applyDiff(
  image: ImageState,
  manifest: Manifest,
  changes: { upsert: readonly string[]; remove: readonly string[] },
  blobs: ReadonlyMap<string, string>,
): void {
  if (image.files === undefined) image.files = {}
  if (image.blobs === undefined) image.blobs = {}
  for (const path of changes.remove) delete image.files[path]
  for (const path of changes.upsert) image.files[path] = { ...manifest[path]! }
  for (const [hash, data] of blobs) image.blobs[hash] = data
  const live = new Set<string>()
  for (const entry of Object.values(image.files)) if (entry.k === "f") live.add(entry.h)
  for (const hash of Object.keys(image.blobs)) if (!live.has(hash)) delete image.blobs[hash]
}

async function runBatches(
  spacesd: SpacesdLike,
  root: string,
  lines: readonly string[],
  what: string,
  size = 80,
): Promise<void> {
  for (let i = 0; i < lines.length; i += size) {
    const result = await sh(spacesd, [`cd ${quote(root)} || exit 3`, ...lines.slice(i, i + size)].join("\n"))
    if (result.code !== 0) throw new Error(`${what} failed in the Space: ${result.stderr || result.error || result.code}`)
  }
}

/** What `reconcile` changed. */
export interface Reconciled {
  written: number
  deleted: number
}

/**
 * Makes the workspace in the Space equal the image: files and links that are
 * missing or differ are written, anything tracked-kind that the image does not
 * have is deleted. Idempotent, so a crash halfway is repaired by running it
 * again. Needs every blob of the image (they are all in the log).
 */
export async function reconcile(
  spacesd: SpacesdLike,
  root: string,
  image: Readonly<ImageState>,
  limits: TransactionalLimits,
): Promise<Reconciled> {
  const wanted = image.files ?? {}
  let actual: Manifest
  try {
    actual = await scan(spacesd, root, limits, 20)
  } catch (error) {
    if (!(error instanceof TransactionalLimitExceeded)) throw error
    // Far more than the workspace may hold: start from empty.
    const cleared = await sh(spacesd, `cd ${quote(root)} || exit 3\nfind . -mindepth 1 -delete 2>/dev/null; true`)
    if (cleared.code !== 0) throw new Error(`clearing the workspace failed: ${cleared.stderr || cleared.error}`)
    actual = {}
  }
  const extras = Object.keys(actual).filter((path) => !(path in wanted))
  const missing = Object.keys(wanted).filter((path) => !same(actual[path], wanted[path]))
  await runBatches(
    spacesd,
    root,
    extras.map((path) => `rm -f -- ${quote(path)}`),
    "deleting files",
  )
  // Whatever stands where a file or link goes (a directory, a link) goes first.
  await runBatches(
    spacesd,
    root,
    missing.map((path) => `if [ -d ${quote(path)} ] && [ ! -L ${quote(path)} ]; then rm -rf -- ${quote(path)}; elif [ -L ${quote(path)} ]; then rm -f -- ${quote(path)}; fi`),
    "clearing paths",
  )
  const modes: string[] = []
  const links: string[] = []
  for (const path of missing) {
    const entry = wanted[path]!
    if (entry.k === "l") {
      links.push(`mkdir -p -- ${quote(dirOf(path))} && ln -sfn -- ${quote(entry.t ?? "")} ${quote(path)}`)
      continue
    }
    const data = image.blobs?.[entry.h]
    if (data === undefined) throw new Error(`the log has no content for ${path}`)
    const bytes = Buffer.from(data, "base64")
    const copy = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length) as ArrayBuffer
    await spacesd.upload(`${root}/${path}`, copy, {
      createNew: false,
      append: false,
      permissions: entry.m,
      createParents: true,
    })
    modes.push(`chmod ${entry.m.toString(8).padStart(3, "0")} ${quote(`./${path}`)}`)
  }
  await runBatches(spacesd, root, modes, "setting modes")
  await runBatches(spacesd, root, links, "linking")
  return { written: missing.length, deleted: extras.length }
}

function dirOf(path: string): string {
  const cut = path.lastIndexOf("/")
  return cut < 0 ? "." : path.slice(0, cut)
}

/**
 * How many processes other than Cua's own are running with a working
 * directory inside `root`: a server or watcher a bash call left behind.
 * Transactional mode cannot undo process state, so such a Space is better
 * served by checkpoints. Linux only (reads `/proc`); elsewhere 0.
 */
export async function backgroundProcesses(spacesd: SpacesdLike, root: string): Promise<number> {
  const result = await sh(
    spacesd,
    [
      "cd / || exit 0",
      "[ -d /proc/self ] || { echo 0; exit 0; }",
      "n=0",
      "for p in /proc/[0-9]*; do",
      '  pid=${p#/proc/}',
      '  [ "$pid" = "$$" ] && continue',
      '  [ "$pid" = "$PPID" ] && continue',
      '  c=$(readlink "$p/cwd" 2>/dev/null) || continue',
      `  case "$c" in ${quote(root)}|${quote(root)}/*) ;; *) continue ;; esac`,
      '  if tr "\\0" " " < "$p/cmdline" 2>/dev/null | grep -q "cua-pi-env"; then continue; fi',
      "  n=$((n+1))",
      "done",
      "echo $n",
    ].join("\n"),
  )
  return result.code === 0 ? Number(result.stdout.trim()) || 0 : 0
}
