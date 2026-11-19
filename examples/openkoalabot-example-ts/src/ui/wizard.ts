// The New Space wizard's state machine, DOM-free: four steps (System,
// Resources, Options, Summary), what each image supports, and the plan the
// Create button sends to the server (`POST /api/spaces/create`, see
// src/core/plan.ts for the SDK call it maps to).
import { imageByRef, publishedImages, type ImageEntry, type Os } from "./images.js"

export const STEPS = ["System", "Resources", "Options", "Summary"] as const
export type Target = "cloud" | "local"

export interface WizardState {
  step: number
  os: Os
  image: string
  target: Target
  cpus: number
  memoryMb: number
  name: string
  openWhenReady: boolean
}

/** The plan `POST /api/spaces/create` takes. */
export interface SpacePlan {
  image: string
  target: Target
  name: string
  cpus: number | null
  memory_mb: number | null
}

export const OS_LABEL: Record<Os, string> = { linux: "Linux", windows: "Windows", macos: "macOS" }
export const OSES: Os[] = ["linux", "windows", "macos"]
export const CPU_RANGE = { min: 1, max: 16 }
export const MEMORY_RANGE = { min: 2048, max: 32768, step: 1024 }

export const RUNTIME_LABEL: Record<string, string> = {
  container: "Container",
  qemu: "QEMU virtual machine",
  lume: "Lume (Apple Virtualization)",
  gvisor: "gVisor sandbox",
  kubevirt: "KubeVirt virtual machine",
}

export function supports(image: ImageEntry, target: Target): boolean {
  return target === "cloud" ? image.cloud !== null : image.local !== null
}

export function imagesFor(os: Os): ImageEntry[] {
  return publishedImages().filter((i) => i.os === os)
}

export function osAvailable(os: Os): boolean {
  return imagesFor(os).length > 0
}

function suffix(): string {
  return Math.random().toString(16).slice(2, 6).padEnd(4, "0")
}

/** A default DNS-label name for an image. */
export function defaultName(image: ImageEntry | undefined, tail = suffix()): string {
  const base = (image?.os ?? "space").replace(/[^a-z0-9-]/g, "")
  return `${base}-${tail}`
}

/** The target to keep (or switch to) when the image changes. */
export function targetFor(image: ImageEntry, preferred: Target): Target {
  if (supports(image, preferred)) return preferred
  return preferred === "cloud" ? "local" : "cloud"
}

export function initialState(preferCloud: boolean): WizardState {
  const first = publishedImages()[0]
  const target = first ? targetFor(first, preferCloud ? "cloud" : "local") : "local"
  return {
    step: 0,
    os: first?.os ?? "linux",
    image: first?.ref ?? "",
    target,
    cpus: 4,
    memoryMb: 8192,
    name: defaultName(first),
    openWhenReady: true,
  }
}

/** Picks an OS tile: the first image for it, and a target it supports. */
export function pickOs(s: WizardState, os: Os): WizardState {
  const image = imagesFor(os)[0]
  if (!image) return s
  return { ...s, os, image: image.ref, target: targetFor(image, s.target), name: renamed(s, image) }
}

/** Picks an image from the dropdown (which may change the OS tile). */
export function pickImage(s: WizardState, ref: string): WizardState {
  const image = imageByRef(ref)
  if (!image) return s
  return { ...s, os: image.os, image: ref, target: targetFor(image, s.target), name: renamed(s, image) }
}

/** Keeps a name the user typed; refreshes a generated one. */
function renamed(s: WizardState, image: ImageEntry): string {
  const generated = /^(linux|windows|macos|space)-[0-9a-f]{4}$/.test(s.name)
  return generated ? `${image.os}-${s.name.slice(-4)}` : s.name
}

export function pickTarget(s: WizardState, target: Target): WizardState {
  const image = imageByRef(s.image)
  if (!image || !supports(image, target)) return s
  return { ...s, target }
}

export function isDnsLabel(name: string): boolean {
  return /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(name)
}

/** Why the current step cannot continue, or "" when it can. */
export function blocker(s: WizardState): string {
  const image = imageByRef(s.image)
  if (!image) return "Pick an image."
  if (!supports(image, s.target)) return `${image.name} does not run ${s.target === "cloud" ? "in Cua Cloud" : "on this machine"}.`
  if (s.step >= 2 && !isDnsLabel(s.name)) return "Use lowercase letters, digits and dashes (at most 63)."
  return ""
}

export function next(s: WizardState): WizardState {
  return blocker(s) ? s : { ...s, step: Math.min(s.step + 1, STEPS.length - 1) }
}

export function back(s: WizardState): WizardState {
  return { ...s, step: Math.max(s.step - 1, 0) }
}

/** What Create sends. Resources only apply on this machine. */
export function toPlan(s: WizardState): SpacePlan {
  const local = s.target === "local"
  return {
    image: s.image,
    target: s.target,
    name: s.name,
    cpus: local ? s.cpus : null,
    memory_mb: local ? s.memoryMb : null,
  }
}

export function formatMemory(mb: number): string {
  return `${Math.round(mb / 1024)} GB`
}

/** The summary rows for the last step. */
export function summaryRows(s: WizardState): Array<[string, string]> {
  const image = imageByRef(s.image)
  if (!image) return []
  const runtime = s.target === "cloud" ? image.cloud : image.local
  const rows: Array<[string, string]> = [
    ["System", `${OS_LABEL[image.os]}, ${image.name}`],
    ["Image", image.ref],
    ["Runs", s.target === "cloud" ? "Cua Cloud (metered)" : "This machine"],
    ["Runtime", RUNTIME_LABEL[runtime ?? ""] ?? runtime ?? ""],
  ]
  if (s.target === "local") rows.push(["Resources", `${s.cpus} CPU cores, ${formatMemory(s.memoryMb)} memory`])
  rows.push(["Name", s.name], ["When ready", s.openWhenReady ? "Open the desktop" : "Stay in the thread"])
  rows.push(["Agent service", image.spacesd ? "cua-spacesd" : "Not included (legacy guest agent)"])
  return rows
}
