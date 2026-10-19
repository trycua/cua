// The sidebar's Space section: the picker, New Space, and Delete. Delete
// is its own control (never inside the pick target) and always asks first:
// deleting a created Space deletes its sandbox; a Space added by address is
// only removed from the list.
import { h, icon } from "./dom.js"

export interface SpaceRow {
  id: string
  name: string
  provider: string
}

export interface SpaceSectionProps {
  spaces: SpaceRow[]
  selected: string | null
  onSelect: (id: string) => void
  onNewSpace: () => void
  onDelete: (id: string) => void
}

/** The location a Space runs in, as the page labels it. */
export function providerLabel(provider: string): string {
  return provider === "cloud" ? "Cua Cloud" : provider === "local" ? "This machine" : provider
}

/** A Space this app created (deleting it deletes its sandbox). */
function isCreated(provider: string): boolean {
  return provider === "cloud" || provider === "local"
}

export function deleteText(provider: string): string {
  if (provider === "cloud") return "This deletes the Cua Cloud Space and everything on it. Metering stops."
  if (provider === "local") return "This deletes the Space from this machine, with everything on it."
  return "This removes the Space from the list. The machine itself keeps running."
}

export function renderSpaceSection(p: SpaceSectionProps): HTMLElement {
  const current = p.spaces.find((s) => s.id === p.selected) ?? null
  const select = h(
    "select",
    { class: "select", "aria-label": "Space", disabled: p.spaces.length === 0, onchange: (e) => p.onSelect((e.target as HTMLSelectElement).value) },
    ...(p.spaces.length === 0 ? [h("option", { value: "" }, "No Spaces yet")] : p.spaces.map((s) => h("option", { value: s.id }, `${s.name} (${providerLabel(s.provider)})`))),
  )
  if (current) select.value = current.id
  const root = h("div", { class: "space-section" })
  const confirm = (space: SpaceRow) => {
    const sheet = h(
      "div",
      { class: "scrim", role: "dialog", "aria-modal": "true", "aria-label": "Delete Space" },
      h(
        "div",
        { class: "sheet sm" },
        h("div", { class: "sheet-head" }, h("h2", {}, `${isCreated(space.provider) ? "Delete" : "Remove"} ${space.name}?`), h("p", {}, deleteText(space.provider))),
        h(
          "div",
          { class: "sheet-foot" },
          h("span", { class: "spacer" }),
          h("button", { class: "btn", "data-action": "cancel", onclick: () => sheet.remove() }, "Cancel"),
          h(
            "button",
            {
              class: "btn btn-primary",
              "data-action": "confirm-delete",
              onclick: () => {
                sheet.remove()
                p.onDelete(space.id)
              },
            },
            isCreated(space.provider) ? "Delete" : "Remove",
          ),
        ),
      ),
    )
    root.append(sheet)
  }
  root.append(
    h("div", { class: "section-label" }, "Space"),
    h(
      "div",
      { class: "space-box" },
      select,
      h(
        "div",
        { class: "row" },
        h("button", { class: "btn", onclick: p.onNewSpace }, icon("plus"), "New Space"),
        h("button", { class: "btn btn-ghost btn-danger", "data-action": "delete", disabled: !current, title: "Delete this Space", onclick: () => current && confirm(current) }, "Delete…"),
      ),
    ),
  )
  return root
}
