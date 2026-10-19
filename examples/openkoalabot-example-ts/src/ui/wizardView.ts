// The New Space wizard as a modal sheet (System, Resources, Options,
// Summary). Create posts the plan to the server, which makes one real SDK
// call: `spaces.create` on this machine or in Cua Cloud.
import { append, h, icon } from "./dom.js"
import { imageByRef, imageGroups } from "./images.js"
import {
  CPU_RANGE,
  MEMORY_RANGE,
  OSES,
  OS_LABEL,
  RUNTIME_LABEL,
  STEPS,
  back,
  blocker,
  formatMemory,
  initialState,
  isDnsLabel,
  next,
  osAvailable,
  pickImage,
  pickOs,
  pickTarget,
  summaryRows,
  supports,
  toPlan,
  type SpacePlan,
  type WizardState,
} from "./wizard.js"

/** The image dropdown: exactly the published entries of the shared list,
 *  one `<optgroup>` per group, in file order. */
export function renderImageSelect(value: string, onChange: (ref: string) => void): HTMLSelectElement {
  const select = h(
    "select",
    { class: "select", "aria-label": "Image", onchange: (e) => onChange((e.target as HTMLSelectElement).value) },
    ...imageGroups().map((g) => h("optgroup", { label: g.label }, ...g.images.map((i) => h("option", { value: i.ref }, `${i.name} (${i.ref})`)))),
  )
  select.value = value
  return select
}

export interface WizardProps {
  cloud: boolean
  initial?: Partial<WizardState>
  onCancel: () => void
  onCreate: (plan: SpacePlan, openWhenReady: boolean) => Promise<boolean>
  onAddByAddress: () => void
}

function note(kind: string, iconName: string, text: string): HTMLElement {
  return h("div", { class: `note ${kind}` }, icon(iconName), h("span", {}, text))
}

/** Mounts the wizard; returns the scrim (remove it to close). */
export function renderWizard(p: WizardProps): HTMLElement {
  let s: WizardState = { ...initialState(p.cloud), ...p.initial }
  let busy = false
  const scrim = h("div", { class: "scrim", role: "dialog", "aria-modal": "true", "aria-label": "New Space" })
  const set = (next: WizardState) => {
    s = next
    draw()
  }

  function body(): Node[] {
    const image = imageByRef(s.image)
    if (!image) return []
    if (s.step === 0)
      return [
        h(
          "div",
          { class: "field" },
          h("span", {}, "Operating system"),
          h(
            "div",
            { class: "tiles" },
            ...OSES.map((os) => h("button", { class: `tile ${s.os === os ? "on" : ""}`, disabled: !osAvailable(os), onclick: () => set(pickOs(s, os)) }, icon(os, 26), OS_LABEL[os])),
          ),
        ),
        h("label", { class: "field" }, h("span", {}, "Image"), renderImageSelect(s.image, (ref) => set(pickImage(s, ref))), h("span", { class: "image-summary" }, image.summary)),
        h(
          "div",
          { class: "field" },
          h("span", {}, "Where it runs"),
          h(
            "div",
            { class: "seg" },
            h(
              "button",
              { class: `tile ${s.target === "cloud" ? "on" : ""}`, disabled: !supports(image, "cloud"), onclick: () => set(pickTarget(s, "cloud")) },
              icon("cloud"),
              "Cua Cloud",
              h("small", {}, image.cloud ? RUNTIME_LABEL[image.cloud] : "Not available for this image"),
            ),
            h(
              "button",
              { class: `tile ${s.target === "local" ? "on" : ""}`, disabled: !supports(image, "local"), onclick: () => set(pickTarget(s, "local")) },
              icon("laptop"),
              "This machine",
              h("small", {}, image.local ? RUNTIME_LABEL[image.local] : "Not available for this image"),
            ),
          ),
        ),
      ]
    if (s.step === 1 && s.target === "local") {
      // Sliders update in place (a redraw would drop the drag).
      const slider = (label: string, fmt: (v: number) => string, attrs: Record<string, number>, on: (v: number) => void) => {
        const out = h("output", {}, fmt(attrs.value))
        return h(
          "div",
          { class: "slider" },
          h("div", { class: "top" }, h("span", {}, label), out),
          h("input", {
            type: "range",
            ...attrs,
            oninput: (e) => {
              const v = Number((e.target as HTMLInputElement).value)
              out.textContent = fmt(v)
              on(v)
            },
          }),
        )
      }
      return [
        slider("CPU cores", String, { min: CPU_RANGE.min, max: CPU_RANGE.max, value: s.cpus }, (v) => (s = { ...s, cpus: v })),
        slider("Memory", formatMemory, { min: MEMORY_RANGE.min, max: MEMORY_RANGE.max, step: MEMORY_RANGE.step, value: s.memoryMb }, (v) => (s = { ...s, memoryMb: v })),
        note("", "info", `Runtime: ${RUNTIME_LABEL[image.local ?? ""]}. The SDK sets it up on this machine itself; the first start downloads the image.`),
      ]
    }
    if (s.step === 1)
      return [
        note(
          "",
          "cloud",
          image.cloud === "gvisor"
            ? "This image runs in a gVisor sandbox: a container with its own kernel boundary. It starts in seconds and uses little memory."
            : "This image runs as a KubeVirt virtual machine: a full VM with its own kernel. It takes longer to start and suits desktops that need one.",
        ),
        note("", "info", "A Cua Cloud Space is metered until you delete it."),
        p.cloud ? null : note("warn", "alert", "Cua Cloud is not signed in. Run `cua auth login` (or set CUA_CLIENT_ID and CUA_CLIENT_SECRET) and restart the server."),
      ].filter((n): n is HTMLElement => n !== null)
    if (s.step === 2) {
      const name = h("input", {
        class: `input ${s.name && !isDnsLabel(s.name) ? "invalid" : ""}`,
        value: s.name,
        spellcheck: "false",
        "aria-label": "Name",
        onchange: (e) => set({ ...s, name: (e.target as HTMLInputElement).value.toLowerCase() }),
        oninput: (e) => {
          s = { ...s, name: (e.target as HTMLInputElement).value.toLowerCase() }
          drawFooter()
        },
      })
      return [
        h("label", { class: "field" }, h("span", {}, "Name"), name, h("span", { class: "hint" }, "A DNS label: lowercase letters, digits and dashes.")),
        h(
          "label",
          { class: "check" },
          h("input", { type: "checkbox", checked: s.openWhenReady, onchange: (e) => set({ ...s, openWhenReady: (e.target as HTMLInputElement).checked }) }),
          h("span", {}, "Open the desktop when ready", h("div", { class: "hint" }, "Shows the Space in the Computer panel as soon as it answers.")),
        ),
        image.spacesd ? null : note("warn", "alert", `${image.name} does not run cua-spacesd yet: Bots, file drop, streaming and teleport need it. The Space still starts and shows up in the list.`),
      ].filter((n): n is HTMLElement => n !== null)
    }
    return [h("dl", { class: "summary" }, ...summaryRows(s).flatMap(([k, v]) => [h("dt", {}, k), h("dd", {}, v)]))]
  }

  const foot = h("div", { class: "sheet-foot" })
  const errorLine = h("div", { class: "error-text" })
  function drawFooter() {
    const why = blocker(s)
    errorLine.textContent = why
    const last = s.step === STEPS.length - 1
    append((foot.replaceChildren(), foot), [
      h("button", { class: "btn btn-ghost", onclick: p.onAddByAddress }, "Add by address…"),
      h("div", { class: "spacer" }),
      h("button", { class: "btn", onclick: p.onCancel }, "Cancel"),
      s.step > 0 ? h("button", { class: "btn", onclick: () => set(back(s)) }, "Back") : null,
      last
        ? h(
            "button",
            {
              class: "btn btn-primary",
              "data-action": "create",
              disabled: !!why || busy,
              onclick: async () => {
                busy = true
                drawFooter()
                const ok = await p.onCreate(toPlan(s), s.openWhenReady)
                busy = false
                if (ok) p.onCancel()
                else drawFooter()
              },
            },
            busy ? "Creating…" : "Create",
          )
        : h("button", { class: "btn btn-primary", "data-action": "continue", disabled: !!why, onclick: () => set(next(s)) }, "Continue"),
    ])
  }

  function draw() {
    scrim.replaceChildren(
      h(
        "div",
        { class: "sheet" },
        h("div", { class: "sheet-head" }, h("h2", {}, "New Space"), h("p", {}, "A computer for your Bots: pick a system, where it runs, and a name.")),
        h(
          "div",
          { class: "steps", "aria-label": "Steps" },
          ...STEPS.flatMap((label, i) => [
            i > 0 ? h("div", { class: "step-line" }) : null,
            h("div", { class: `step ${i === s.step ? "on" : i < s.step ? "done" : ""}` }, h("span", { class: "n" }, i + 1), label),
          ]).filter((n) => n !== null) as HTMLElement[],
        ),
        h("div", { class: "sheet-body" }, ...body(), errorLine),
        foot,
      ),
    )
    drawFooter()
  }
  draw()
  return scrim
}
