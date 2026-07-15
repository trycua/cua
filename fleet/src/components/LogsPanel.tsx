import { useEffect, useRef, useState } from "react"
import AnsiToHtml from "ansi-to-html"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import { api } from "../api/cyclops"

const MAX_LOG_LINES = 2000

// SGR-only ANSI → HTML converter shared by every line append. ansi-to-html
// HTML-escapes its input first (escapeXML: true), so `<script>` in a log
// line lands as `&lt;script&gt;`.
const ANSI = new AnsiToHtml({
  fg: "#d1d5db",
  bg: "#0f1419",
  newline: false,
  escapeXML: true,
})

export function LogsPanel({
  namespace,
  podName,
  container,
  title,
  waitingText,
}: {
  namespace?: string
  podName?: string
  container: string
  title: string
  waitingText?: string
}) {
  const [lines, setLines] = useState<string[]>([])
  const [paused, setPaused] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const preRef = useRef<HTMLPreElement>(null)
  const stickToBottomRef = useRef(true)

  useEffect(() => {
    setLines([])
  }, [namespace, podName, container])

  useEffect(() => {
    if (!namespace || !podName || paused) return
    setError(null)
    const url = api.podLogsUrl(namespace, podName, container)
    const ctrl = new AbortController()
    ;(async () => {
      try {
        const { getToken } = await import("../auth/keycloak")
        const token = await getToken()
        const headers: Record<string, string> = {}
        if (token) headers.Authorization = `Bearer ${token}`
        const res = await fetch(url, { signal: ctrl.signal, headers })
        if (!res.ok || !res.body) {
          setError(`Log stream failed: ${res.status}`)
          return
        }
        const reader = res.body.getReader()
        const decoder = new TextDecoder()
        let buffer = ""
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })
          const parts = buffer.split("\n")
          buffer = parts.pop() ?? ""
          if (parts.length > 0) {
            const htmlLines = parts.map(l => ANSI.toHtml(l))
            setLines(prev => {
              const next = [...prev, ...htmlLines]
              return next.length > MAX_LOG_LINES
                ? next.slice(next.length - MAX_LOG_LINES)
                : next
            })
          }
        }
      } catch {
        if (!ctrl.signal.aborted) {
          setError("Log stream interrupted. Click Resume to retry.")
        }
      }
    })()
    return () => ctrl.abort()
  }, [namespace, podName, container, paused])

  useEffect(() => {
    const el = preRef.current
    if (!el || !stickToBottomRef.current) return
    el.scrollTop = el.scrollHeight
  }, [lines])

  const onScroll = () => {
    const el = preRef.current
    if (!el) return
    const distanceFromBottom = el.scrollHeight - (el.scrollTop + el.clientHeight)
    stickToBottomRef.current = distanceFromBottom < 40
  }

  return (
    <Container
      header={
        <Header
          variant="h2"
          counter={`(${lines.length})`}
          actions={
            <SpaceBetween direction="horizontal" size="xs">
              <Button
                onClick={() => setPaused(p => !p)}
                iconName={paused ? "caret-right-filled" : "close"}
              >
                {paused ? "Resume" : "Pause"}
              </Button>
              <Button onClick={() => setLines([])}>Clear</Button>
            </SpaceBetween>
          }
        >
          {title}
        </Header>
      }
    >
      {error && (
        <Box color="text-status-warning" margin={{ bottom: "xs" }}>
          <StatusIndicator type="warning">{error}</StatusIndicator>
        </Box>
      )}
      {lines.length === 0 ? (
        <Box color="text-body-secondary" textAlign="center" padding="m">
          {!namespace || !podName
            ? (waitingText ?? "Waiting for pod…")
            : paused
              ? "Paused."
              : "Connecting…"}
        </Box>
      ) : (
        <pre
          ref={preRef}
          onScroll={onScroll}
          // ansi-to-html HTML-escapes input first (escapeXML: true), so
          // dangerouslySetInnerHTML here is safe — log payload can't
          // inject markup.
          dangerouslySetInnerHTML={{ __html: lines.join("\n") }}
          style={{
            maxHeight: 480,
            overflow: "auto",
            margin: 0,
            padding: "0.75rem",
            backgroundColor: "#0f1419",
            color: "#d1d5db",
            fontSize: 12,
            fontFamily:
              "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
            borderRadius: 4,
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
          }}
        />
      )}
    </Container>
  )
}
