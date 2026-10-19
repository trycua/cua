/**
 * `@trycua/cua/teleport`: the shared "Teleport an app…" and drag-and-drop
 * UX, headless and framework-agnostic. Browser, webview and Node safe (no
 * native library): the Spaces app, the OpenKoalaBots examples and any
 * client render the same flow over a {@link TeleportHost}.
 *
 * ```ts
 * import { TeleportPickerController, defineTeleportElements } from "@trycua/cua/teleport"
 *
 * // Any framework: the controller is the state machine.
 * const c = new TeleportPickerController(host, { spaceName: "dev" })
 * c.subscribe(() => render(c.state))
 * await c.load()
 *
 * // Or the web component.
 * defineTeleportElements()
 * const el = document.createElement("cua-teleport-picker")
 * el.setAttribute("space-name", "dev")
 * el.host = host
 * ```
 *
 * A host implements {@link TeleportHost}: the Cua Spaces apps back it with
 * their own teleport (host-side app teleport ships with Cua Spaces,
 * source-available), a test with a fake.
 */

export * from "./model.js"
export * from "./install.js"
export * from "./drop.js"
export * from "./windowDrag.js"
export * from "./dropZone.js"
export { TeleportPickerController, type ControllerOptions } from "./controller.js"
export { defineTeleportElements, renderDropZone, renderPicker, TELEPORT_STYLES } from "./element.js"
