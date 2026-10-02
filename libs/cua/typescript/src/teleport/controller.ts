/**
 * {@link TeleportPickerController}: the headless state machine bound to a
 * {@link TeleportHost}. Frameworks subscribe to it (React:
 * `useSyncExternalStore(c.subscribe, () => c.state)`; the web component
 * re-renders on each change).
 */

import {
  type CatalogEntry,
  type PickerEvent,
  type PickerState,
  type TeleportHost,
  canConfirm,
  canPlan,
  initialState,
  planSensitive,
  reduce,
} from "./model.js"

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export interface ControllerOptions {
  spaceName: string
  /** Jump straight to the options for this app (a drop or window drag). */
  preselect?: { entry: CatalogEntry; files?: string[] }
}

export class TeleportPickerController {
  #state: PickerState
  #listeners = new Set<() => void>()
  #generation = 0
  readonly host: TeleportHost

  constructor(host: TeleportHost, options: ControllerOptions) {
    this.host = host
    this.#state = initialState(options.spaceName)
    if (options.preselect) {
      this.#state = reduce(this.#state, {
        type: "preselect",
        entry: options.preselect.entry,
        files: options.preselect.files ?? [],
      })
    }
  }

  get state(): PickerState {
    return this.#state
  }

  /** Subscribe to changes; returns the unsubscribe. */
  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener)
    return () => this.#listeners.delete(listener)
  }

  dispatch = (event: PickerEvent): void => {
    const next = reduce(this.#state, event)
    if (next === this.#state) return
    this.#state = next
    for (const l of [...this.#listeners]) l()
  }

  /** Loads (or reloads) the catalog. */
  async load(): Promise<void> {
    const gen = ++this.#generation
    try {
      const entries = await this.host.catalog()
      if (gen === this.#generation) this.dispatch({ type: "loaded", entries })
    } catch (e) {
      if (gen === this.#generation) this.dispatch({ type: "failed", message: message(e), cause: e })
    }
  }

  /** Opens the options for the selected (or given) app. */
  choose(id?: string): void {
    this.dispatch(id === undefined ? { type: "choose" } : { type: "choose", id })
  }

  /** Opens the native chooser and adds what the user picked. */
  async chooseFiles(): Promise<void> {
    if (!this.host.chooseFiles) return
    const files = await this.host.chooseFiles()
    if (files.length) this.dispatch({ type: "files", files })
  }

  /** Builds the plan (sizes, installs, consent items). */
  async plan(): Promise<void> {
    const s = this.#state
    if (s.step !== "options" || !canPlan(s) || !s.entry || !s.move) return
    this.dispatch({ type: "plan" })
    try {
      const groups = planSensitive(s)
      const plan = await this.host.plan(s.entry, {
        moves: s.move,
        files: s.files,
        ...(groups.length ? { sensitiveGroups: groups } : {}),
      })
      this.dispatch({ type: "planned", plan })
    } catch (e) {
      this.dispatch({ type: "failed", message: message(e), cause: e })
    }
  }

  /** Runs the approved plan with the user's consent. */
  async confirm(): Promise<void> {
    const s = this.#state
    if (s.step !== "consent" || !canConfirm(s) || !s.plan) return
    const plan = s.plan
    this.dispatch({ type: "confirm" })
    try {
      const report = await this.host.run(
        plan,
        {
          approved: true,
          acknowledgeSensitive: s.acknowledged,
          saveToKeyvault: s.saveToKeyvault,
          acknowledgeRelayPlaintext: s.acknowledgedRelayPlaintext,
        },
        (event) => this.dispatch({ type: "progress", event }),
      )
      this.dispatch({ type: "finished", report })
    } catch (e) {
      this.dispatch({ type: "failed", message: message(e), cause: e })
    }
  }
}
