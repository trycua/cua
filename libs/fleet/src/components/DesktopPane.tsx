/**
 * DesktopPane — live macOS sandbox desktop via noVNC.
 *
 * Renders the macOS sandbox's screen (RFB) in the browser. The sandbox exposes
 * its desktop as RFB-over-WebSocket (a noVNC RFB server bound to the
 * sandbox's routable IP, bridged by websockify — see docs/macos-novnc-plane.md).
 * That WebSocket is reached through the SAME cyclops-cs `/api/svc` proxy the rest
 * of the app uses; the proxy already supports the WebSocket upgrade (handlers.go
 * `statusCapture.Hijack` — "noVNC's /websockify"). So the connect URL is just:
 *
 *     wss://<host>/api/svc/<namespace>/<sandbox>-<vncService>/websockify
 *
 * No new gateway route, no direct pod exposure — the sandbox's routable IP stays
 * private to the node; the browser only ever talks to run.cua.ai.
 */
import { useEffect, useRef, useState } from "react"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
// noVNC RFB client (browser-side VNC over WebSocket).
import RFB from "@novnc/novnc"

type Props = {
  /** Pool namespace. */
  namespace: string
  /** Bound sandbox name (claim.status.sandbox). */
  sandboxName: string
  /** The VNC service name on the pool (default "vnc"); the K8s service is
   *  `<sandboxName>-<vncService>` and the websockify path is under it. */
  vncService?: string
}

type Phase = "idle" | "connecting" | "connected" | "disconnected" | "error"

/** Build the wss:// URL to the sandbox's websockify endpoint via /api/svc. */
function websockifyUrl(namespace: string, sandboxName: string, vncService: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:"
  const svc = `${sandboxName}-${vncService}`
  return `${proto}//${window.location.host}/api/svc/${namespace}/${svc}/websockify`
}

export function DesktopPane({ namespace, sandboxName, vncService = "vnc" }: Props) {
  const screenRef = useRef<HTMLDivElement>(null)
  const rfbRef = useRef<RFB | null>(null)
  const [phase, setPhase] = useState<Phase>("idle")
  const [nonce, setNonce] = useState(0) // bump to force a reconnect

  useEffect(() => {
    if (!screenRef.current) return
    const url = websockifyUrl(namespace, sandboxName, vncService)
    setPhase("connecting")

    let rfb: RFB
    try {
      // RFB security type "None" on the sandbox side (loopback-bound server,
      // reached only via the authenticated proxy) — no VNC password here.
      rfb = new RFB(screenRef.current, url, {})
    } catch {
      setPhase("error")
      return
    }
    rfb.viewOnly = false
    rfb.scaleViewport = true
    rfb.resizeSession = false
    rfb.background = "#1b1b1b"

    const onConnect = () => setPhase("connected")
    const onDisconnect = () => setPhase("disconnected")
    const onSecurityFailure = () => setPhase("error")
    rfb.addEventListener("connect", onConnect)
    rfb.addEventListener("disconnect", onDisconnect)
    rfb.addEventListener("securityfailure", onSecurityFailure)
    rfbRef.current = rfb

    return () => {
      rfb.removeEventListener("connect", onConnect)
      rfb.removeEventListener("disconnect", onDisconnect)
      rfb.removeEventListener("securityfailure", onSecurityFailure)
      try { rfb.disconnect() } catch { /* already gone */ }
      rfbRef.current = null
    }
  }, [namespace, sandboxName, vncService, nonce])

  const status = {
    idle: <StatusIndicator type="pending">Idle</StatusIndicator>,
    connecting: <StatusIndicator type="loading">Connecting…</StatusIndicator>,
    connected: <StatusIndicator type="success">Connected</StatusIndicator>,
    disconnected: <StatusIndicator type="stopped">Disconnected</StatusIndicator>,
    error: <StatusIndicator type="error">Connection failed</StatusIndicator>,
  }[phase]

  return (
    <SpaceBetween size="s">
      <Box>
        {status}
        <Box float="right">
          <Button iconName="refresh" onClick={() => setNonce(n => n + 1)}>
            Reconnect
          </Button>
        </Box>
      </Box>
      <div
        ref={screenRef}
        style={{
          width: "100%",
          height: 640,
          background: "#1b1b1b",
          borderRadius: 4,
          overflow: "hidden",
        }}
      />
      {phase === "disconnected" && (
        <Box color="text-status-inactive">
          The sandbox closed the desktop stream (session ended or reset). Use
          Reconnect once it is Ready again.
        </Box>
      )}
      {phase === "error" && (
        <Box color="text-status-inactive">
          Could not reach the desktop. Confirm the pool exposes a{" "}
          <code>{vncService}</code> service and the sandbox is Ready.
        </Box>
      )}
    </SpaceBetween>
  )
}
