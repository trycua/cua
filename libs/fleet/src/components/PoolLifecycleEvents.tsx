import { useEffect, useState } from "react"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table, { type TableProps } from "@cloudscape-design/components/table"
import { api, type KubernetesEvent } from "../api/cyclops"

// ── Shared helpers ─────────────────────────────────────────────────────────

export function eventTime(e: KubernetesEvent): string {
  return e.lastTimestamp ?? e.eventTime ?? e.firstTimestamp ?? e.metadata.creationTimestamp ?? ""
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

function AgeCell({ e }: { e: KubernetesEvent }) {
  return (
    <Box variant="small" color="text-body-secondary">
      {fmtAge(eventTime(e))}
    </Box>
  )
}

function TypeCell({ e }: { e: KubernetesEvent }) {
  return e.type === "Warning" ? (
    <StatusIndicator type="warning">Warning</StatusIndicator>
  ) : (
    <StatusIndicator type="success">Normal</StatusIndicator>
  )
}

// ── Strategy interface ─────────────────────────────────────────────────────

/**
 * EventViewStrategy determines what events are visible and how they render.
 * Each concrete strategy encapsulates one audience's view of lifecycle events.
 * Adding a new audience (e.g. "support tier") is a new class, not a new branch.
 */
interface EventViewStrategy {
  /** Filter the full sorted event list to what this audience should see. */
  filter(events: KubernetesEvent[]): KubernetesEvent[]
  /** Column definitions for the Cloudscape Table. */
  columns(): TableProps.ColumnDefinition<KubernetesEvent>[]
}

// ── Customer strategy ──────────────────────────────────────────────────────

// Mapping of K8s event reason → marketing-friendly display label.
// Used purely for display translation — filtering is now done server-side
// by OPA's visible_events rule before events reach the SPA.
const CUSTOMER_REASON_LABELS: Record<string, string> = {
  PoolCreated:       "Pool created",
  PoolUpdated:       "Pool updated",
  PoolDeleted:       "Pool deleted",
  PoolScaled:        "Replicas changed",
  PoolReady:         "Pool ready",
  PoolDegraded:      "Pool degraded",
  PoolFailed:        "Pool failed",
  VmStarted:         "VM started",
  VmStopped:         "VM stopped",
  VmFailed:          "VM failed",
  VmProvisioning:    "Provisioning VM",
  OrchestratorReady: "Orchestrator ready",
  OrchestratorDown:  "Orchestrator restarted",
}

// Optional user-facing subtitles for actionable/notable events.
const CUSTOMER_REASON_DESC: Record<string, string> = {
  PoolDegraded:     "The pool has fewer ready replicas than requested.",
  PoolFailed:       "The pool encountered an error. Check your configuration.",
  VmFailed:         "A VM replica failed. It will be replaced automatically.",
  OrchestratorDown: "The orchestrator pod was replaced.",
}

class CustomerEventView implements EventViewStrategy {
  /** No client-side filtering — server already applied visible_events via OPA. */
  filter(events: KubernetesEvent[]): KubernetesEvent[] {
    return events
  }

  columns(): TableProps.ColumnDefinition<KubernetesEvent>[] {
    return [
      {
        id: "age",
        header: "Age",
        cell: e => <AgeCell e={e} />,
        width: 80,
      },
      {
        id: "type",
        header: "Status",
        cell: e => <TypeCell e={e} />,
        width: 120,
      },
      {
        id: "event",
        header: "Event",
        isRowHeader: true,
        cell: e => {
          const label = e.reason ? (CUSTOMER_REASON_LABELS[e.reason] ?? e.reason) : "—"
          const desc = e.reason ? CUSTOMER_REASON_DESC[e.reason] : undefined
          return (
            <Box>
              <Box variant="span">{label}</Box>
              {desc && (
                <Box variant="small" color="text-body-secondary">
                  {desc}
                </Box>
              )}
            </Box>
          )
        },
      },
    ]
  }
}

// ── Admin strategy ─────────────────────────────────────────────────────────

class AdminEventView implements EventViewStrategy {
  /** Admins see every event for this pool — no filtering. */
  filter(events: KubernetesEvent[]): KubernetesEvent[] {
    return events
  }

  columns(): TableProps.ColumnDefinition<KubernetesEvent>[] {
    return [
      {
        id: "age",
        header: "Age",
        cell: e => <AgeCell e={e} />,
        width: 80,
      },
      {
        id: "type",
        header: "Type",
        cell: e => <TypeCell e={e} />,
        width: 120,
      },
      {
        id: "reason",
        header: "Reason",
        isRowHeader: true,
        cell: e => <code>{e.reason ?? "—"}</code>,
        width: 200,
      },
      {
        id: "message",
        header: "Message",
        cell: e => <Box variant="small">{e.message ?? "—"}</Box>,
      },
      {
        id: "count",
        header: "Count",
        cell: e => e.count ?? 1,
        width: 70,
      },
    ]
  }
}

// ── Strategy factory ───────────────────────────────────────────────────────

/** Returns the correct strategy for the given audience. */
function eventViewStrategyFor(admin: boolean): EventViewStrategy {
  return admin ? new AdminEventView() : new CustomerEventView()
}

// ── Component ──────────────────────────────────────────────────────────────

const REFRESH_MS = 15_000

interface PoolLifecycleEventsProps {
  /** Pool name (without the pool- prefix). Used to filter events to this pool. */
  poolName: string
  /** Whether the current user has admin access. Selects the AdminEventView strategy. */
  admin?: boolean
}

/**
 * Shows Kubernetes operator events for a specific pool.
 * The rendering strategy (columns, event filter, labels) is selected
 * by `admin` via `eventViewStrategyFor` — no if-statements in render.
 */
export function PoolLifecycleEvents({ poolName, admin = false }: PoolLifecycleEventsProps) {
  const [events, setEvents] = useState<KubernetesEvent[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const strategy = eventViewStrategyFor(admin)

  const load = async () => {
    setLoading(true)
    try {
      const list = await api.listOperatorEvents()
      const poolEvents = list.items
        .filter(e => e.involvedObject.name === poolName)
        .sort((a, b) => eventTime(b).localeCompare(eventTime(a)))
      setEvents(poolEvents)
      setError(null)
    } catch (e) {
      setError(String((e as Error).message))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
    const id = window.setInterval(load, REFRESH_MS)
    return () => window.clearInterval(id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [poolName])

  const visibleEvents = strategy.filter(events)

  return (
    <Container
      header={
        <Header
          variant="h2"
          counter={loading ? undefined : `(${visibleEvents.length})`}
          description="Recent lifecycle events for this pool."
          actions={<Button iconName="refresh" onClick={load} disabled={loading} />}
        >
          Lifecycle events
        </Header>
      }
    >
      <Table
        variant="embedded"
        items={visibleEvents}
        loading={loading}
        loadingText="Loading events"
        trackBy={e => e.metadata.uid ?? e.metadata.name + eventTime(e)}
        columnDefinitions={strategy.columns()}
        empty={
          error ? (
            <Box color="text-status-warning" textAlign="center" padding="m">
              Could not load events.
            </Box>
          ) : (
            <Box textAlign="center" color="text-body-secondary" padding="m">
              No lifecycle events yet.
            </Box>
          )
        }
      />
    </Container>
  )
}
