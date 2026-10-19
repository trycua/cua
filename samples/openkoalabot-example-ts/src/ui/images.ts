// The sandbox images the New Space wizard offers: the `published: true`
// entries of the shared list (libs/images/sandbox-images.json), in file
// order, grouped by the list's group labels. The page imports the file
// (web/main.ts) and the server reads it (src/core/plan.ts): one source
// for the docs and every app picker. Call `useImageList` once at startup.

export type Os = "linux" | "windows" | "macos"

export interface ImageEntry {
  ref: string
  group: string
  os: Os
  name: string
  variant: string
  summary: string
  spacesd: boolean
  /** Local runtime (`container`, `qemu`, `lume`); null: cloud only. */
  local: string | null
  /** Cloud runtime (`gvisor`, `kubevirt`); null: local only. */
  cloud: string | null
  /** `slim`, `full` (the default) or `xcode`: canonical images only. */
  tier?: "slim" | "full" | "xcode"
  published: boolean
}

export interface ImageGroup {
  id: string
  label: string
  images: ImageEntry[]
}

export interface ImageList {
  groups: Array<{ id: string; label: string }>
  images: ImageEntry[]
}

let list: ImageList = { groups: [], images: [] }
let all: ImageEntry[] = []

/** Sets the list the helpers below read. */
export function useImageList(value: unknown): void {
  list = value as ImageList
  all = list.images
}

/** Every published entry, in file order. */
export function publishedImages(): ImageEntry[] {
  return all.filter((i) => i.published)
}

/** Published entries as `<optgroup>`s: consecutive entries of one group
 *  share a group, so the options read in file order; labels come from the
 *  list's `groups`. */
export function imageGroups(): ImageGroup[] {
  const label = new Map(list.groups.map((g) => [g.id, g.label]))
  const out: ImageGroup[] = []
  for (const image of publishedImages()) {
    const last = out[out.length - 1]
    if (last && last.id === image.group) last.images.push(image)
    else out.push({ id: image.group, label: label.get(image.group) ?? image.group, images: [image] })
  }
  return out
}

export function imageByRef(ref: string): ImageEntry | undefined {
  return publishedImages().find((i) => i.ref === ref)
}
