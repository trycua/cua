/**
 * Minimal type shim for @novnc/novnc's ESM RFB entrypoint (the package ships no
 * .d.ts for the deep `core/rfb.js` import). Declares only the surface DesktopPane
 * uses. See https://github.com/novnc/noVNC for the full API.
 */
declare module "@novnc/novnc" {
  export default class RFB extends EventTarget {
    constructor(
      target: HTMLElement,
      url: string,
      options?: {
        credentials?: { username?: string; password?: string; target?: string }
        shared?: boolean
        repeaterID?: string
        wsProtocols?: string[]
      },
    )
    viewOnly: boolean
    scaleViewport: boolean
    resizeSession: boolean
    background: string
    disconnect(): void
  }
}
