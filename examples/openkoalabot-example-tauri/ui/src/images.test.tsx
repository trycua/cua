// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import { imageGroups, publishedImages } from "./images"
import { ImageSelect } from "./Wizard"

// The shared list itself, filtered here independently of the module under test.
import raw from "../../../../libs/images/sandbox-images.json"

const file = raw as {
  groups: Array<{ id: string; label: string }>
  images: Array<{ ref: string; group: string; name: string; published: boolean }>
}
const published = file.images.filter((i) => i.published)

describe("the image dropdown", () => {
  const html = renderToStaticMarkup(<ImageSelect value={published[0].ref} onChange={() => {}} />)

  it("offers exactly the published entries, in file order", () => {
    const values = [...html.matchAll(/<option value="([^"]*)"/g)].map((m) => m[1])
    expect(values).toEqual(published.map((i) => i.ref))
    expect(values.length).toBeGreaterThan(0)
    for (const unpublished of file.images.filter((i) => !i.published)) expect(values).not.toContain(unpublished.ref)
  })

  it("uses the list's group labels as optgroups", () => {
    const labels = [...html.matchAll(/<optgroup label="([^"]*)"/g)].map((m) => m[1])
    const want: string[] = []
    for (const i of published) {
      const label = file.groups.find((g) => g.id === i.group)!.label
      if (want[want.length - 1] !== label) want.push(label)
    }
    expect(labels).toEqual(want)
  })

  it("shows each entry's name", () => {
    for (const i of published) expect(html).toContain(`${i.name} (${i.ref})`)
  })

  it("the module agrees with the file", () => {
    expect(publishedImages().map((i) => i.ref)).toEqual(published.map((i) => i.ref))
    expect(imageGroups().flatMap((g) => g.images.map((i) => i.ref))).toEqual(published.map((i) => i.ref))
  })
})
