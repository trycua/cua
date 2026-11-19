/**
 * The New Space wizard's plan, and the one SDK call it maps to:
 * `spaces.create({ on, image, kind, runtime, name, ... })`.
 *
 * | Where | `on` | `kind` / `runtime` |
 * |---|---|---|
 * | This machine | `local` | container / `auto` (gVisor when available), vm / `qemu`, vm / `lume`; plus `cpus`, `memoryMb` |
 * | Cua Cloud | `cloud` | container / `gvisor`, vm / `kubevirt` |
 *
 * `kind` comes from the entry's `variant` and `runtime` from its `local` or
 * `cloud` engine in the shared list (libs/images/sandbox-images.json), so
 * the page cannot ask for a combination the image does not support.
 */
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

export interface ImageEntry {
  ref: string
  group: string
  os: string
  name: string
  variant: string
  summary: string
  spacesd: boolean
  local: string | null
  cloud: string | null
  published: boolean
}

export interface SpacePlan {
  image: string
  target: "cloud" | "local"
  name: string
  cpus?: number | null
  memory_mb?: number | null
}

/** The `spaces.create` options a plan maps to. */
export interface CreateCall {
  on: "local" | "cloud"
  image: string
  kind: "container" | "vm"
  runtime: "auto" | "qemu" | "lume" | "gvisor" | "kubevirt"
  name: string
  cpus?: number
  memoryMb?: number
  wait: true
  spacesd: boolean
}

/** The shared list's path, from dist/core or src/core. */
export const IMAGE_LIST_PATH = fileURLToPath(new URL("../../../../libs/images/sandbox-images.json", import.meta.url))

/** The `published: true` entries, in file order. */
export function publishedImages(path = IMAGE_LIST_PATH): ImageEntry[] {
  const list = JSON.parse(readFileSync(path, "utf8")) as { images: ImageEntry[] }
  return list.images.filter((i) => i.published)
}

export function isDnsLabel(name: string): boolean {
  return /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(name)
}

/** The local engine an entry names (`container` is `auto`: gVisor when available). */
const LOCAL_RUNTIME: Record<string, CreateCall["runtime"]> = { container: "auto", qemu: "qemu", lume: "lume" }

function clamp(n: number | null | undefined, fallback: number, min: number, max: number): number {
  const v = typeof n === "number" && Number.isFinite(n) ? Math.round(n) : fallback
  return Math.min(max, Math.max(min, v))
}

/** Maps a plan onto one SDK call; throws with a person-readable reason. */
export function planCall(plan: SpacePlan, images: ImageEntry[] = publishedImages()): CreateCall {
  const entry = images.find((i) => i.ref === plan.image && i.published)
  if (!entry) throw new Error(`${plan.image} is not a published image`)
  if (!isDnsLabel(plan.name)) throw new Error(`\`${plan.name}\` is not a DNS label (a-z, 0-9 and -, at most 63)`)
  const kind = entry.variant === "vm" ? "vm" : "container"
  if (plan.target === "local") {
    const runtime = entry.local ? LOCAL_RUNTIME[entry.local] : undefined
    if (!runtime) throw new Error(`${entry.name} does not run on this machine`)
    return {
      on: "local",
      image: entry.ref,
      kind,
      runtime,
      name: plan.name,
      cpus: clamp(plan.cpus, 2, 1, 64),
      memoryMb: clamp(plan.memory_mb, 4096, 1024, 262_144),
      wait: true,
      spacesd: entry.spacesd,
    }
  }
  if (plan.target === "cloud") {
    if (entry.cloud !== "gvisor" && entry.cloud !== "kubevirt") throw new Error(`${entry.name} does not run in Cua Cloud`)
    return { on: "cloud", image: entry.ref, kind, runtime: entry.cloud, name: plan.name, wait: true, spacesd: entry.spacesd }
  }
  throw new Error(`unknown target ${String(plan.target)}`)
}
