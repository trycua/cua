// One muted activity group in a Bot's thread: the agent's install progress,
// tool calls, thinking and turn ends, which are not messages. No bubble, small
// secondary text, one line per step, collapsed to its summary (`5 steps`).
import { h } from "./dom.js"

export interface ActivityProps {
  summary: string
  steps: string[]
  /** Whether the group starts expanded (the page remembers the user's toggle). */
  open: boolean
  onToggle?: (open: boolean) => void
}

export function renderActivity(p: ActivityProps): HTMLElement {
  const el = h(
    "details",
    { class: "activity", open: p.open },
    h("summary", { class: "activity-summary", title: p.steps.at(-1) ?? p.summary }, p.summary),
    h("div", { class: "activity-steps", role: "list" }, ...p.steps.map((s) => h("div", { class: "activity-step", role: "listitem", title: s }, s))),
  )
  el.addEventListener("toggle", () => p.onToggle?.((el as HTMLDetailsElement).open))
  return el
}
