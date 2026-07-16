import { useEffect, useState } from "react"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Header from "@cloudscape-design/components/header"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table from "@cloudscape-design/components/table"
import { api, type KubernetesEvent } from "../api/cyclops"
import { useFlash } from "../components/FlashContext"

const REFRESH_MS = 15_000

function eventTime(e: KubernetesEvent): string {
  return (
    e.lastTimestamp ??
    e.eventTime ??
    e.firstTimestamp ??
    e.metadata.creationTimestamp ??
    ""
  )
}

function fmtAge(ts: string): string {
  if (!ts) return "—"
  const ms = Date.now() - new Date(ts).getTime()
  if (Number.isNaN(ms)) return "—"
  const s = Math.max(0, Math.round(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.round(m / 60)
  if (h < 48) return `${h}h`
  return `${Math.round(h / 24)}d`
}

export function OperatorEvents() {
  const flash = useFlash()
  const [events, setEvents] = useState<KubernetesEvent[]>([])
  const [loading, setLoading] = useState(true)

  const load = async () => {
    setLoading(true)
    try {
      const list = await api.listOperatorEvents()
      const sorted = [...list.items].sort((a, b) =>
        eventTime(b).localeCompare(eventTime(a)),
      )
      setEvents(sorted)
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to list events",
        content: String((e as Error).message),
      })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
    const id = window.setInterval(load, REFRESH_MS)
    return () => window.clearInterval(id)
  }, [])

  return (
    <Table
      header={
        <Header
          variant="h1"
          counter={loading ? undefined : `(${events.length})`}
          description="Kubernetes events emitted by the pool-operator on OSGymWorkspacePool resources."
          actions={
            <Button iconName="refresh" onClick={load} disabled={loading} />
          }
        >
          Operator events
        </Header>
      }
      items={events}
      loading={loading}
      loadingText="Loading events"
      trackBy={e => e.metadata.uid ?? e.metadata.name}
      empty={
        <Box textAlign="center" padding="m" color="text-body-secondary">
          No events
        </Box>
      }
      columnDefinitions={[
        {
          id: "age",
          header: "Age",
          cell: e => fmtAge(eventTime(e)),
          width: 90,
        },
        {
          id: "type",
          header: "Type",
          cell: e =>
            e.type === "Warning" ? (
              <StatusIndicator type="warning">Warning</StatusIndicator>
            ) : (
              <StatusIndicator type="success">Normal</StatusIndicator>
            ),
          width: 130,
        },
        {
          id: "reason",
          header: "Reason",
          cell: e => <code>{e.reason ?? "—"}</code>,
          width: 220,
        },
        {
          id: "object",
          header: "Object",
          cell: e => (
            <code title={`${e.involvedObject.kind}/${e.involvedObject.name}`}>
              {e.involvedObject.name}
            </code>
          ),
          width: 200,
        },
        {
          id: "message",
          header: "Message",
          cell: e => e.message ?? "",
        },
        {
          id: "count",
          header: "Count",
          cell: e => e.count ?? 1,
          width: 80,
        },
      ]}
    />
  )
}
