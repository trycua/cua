import { useCallback, useEffect, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import ColumnLayout from "@cloudscape-design/components/column-layout"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import Link from "@cloudscape-design/components/link"
import Modal from "@cloudscape-design/components/modal"
import Select from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Spinner from "@cloudscape-design/components/spinner"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table from "@cloudscape-design/components/table"
import Tabs from "@cloudscape-design/components/tabs"
import { api, claimsApi, type Claim } from "../api/cyclops"
import { derivePoolStatus, tombstonePool } from "../api/pools"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { useFlash } from "../components/FlashContext"
import { PoolStatusPill } from "../components/PoolStatus"

interface ServiceDef {
  name: string
  targetPort: number
  protocol: string
}

interface PoolData {
  name: string
  namespace: string
  replicas: number
  cpu: number
  ram: string
  ociImage: string
  services: ServiceDef[]
  probes?: { readinessProbe?: Record<string, unknown>; livenessProbe?: Record<string, unknown> }
  phase: string
  totalCount: number
  availableCount: number
  claimedCount: number
}

export function PoolDetail() {
  const { namespace = "", name = "" } = useParams()
  const navigate = useNavigate()
  const flash = useFlash()
  const { admin } = useFeatureFlags()

  const [pool, setPool] = useState<PoolData | null>(null)
  const [loading, setLoading] = useState(true)
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)

  // Orchestrator restart state (admin-only)
  const [confirmingOrchestratorRestart, setConfirmingOrchestratorRestart] = useState(false)
  const [orchestratorRestarting, setOrchestratorRestarting] = useState(false)

  const load = async () => {
    setLoading(true)
    try {
      const p = await api.getPool(namespace, name)
      setPool(p)
    } catch (e) {
      flash.push({
        type: "error",
        header: `Failed to load pool "${name}"`,
        content: String((e as Error).message),
      })
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    if (namespace && name) load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [namespace, name])

  const remove = async () => {
    setDeleting(true)
    try {
      await api.deletePool(namespace, name)
      tombstonePool(name)
      flash.push({ type: "success", header: `Deleted ${name}` })
      navigate("/pools")
    } catch (e) {
      flash.push({
        type: "error",
        header: "Delete failed",
        content: String((e as Error).message),
      })
      setDeleting(false)
    }
  }

  const restartOrchestrator = async () => {
    if (!pool) return
    setOrchestratorRestarting(true)
    try {
      await api.restartOrchestrator(`pool-${pool.name}`, pool.name)
      flash.push({
        type: "success",
        header: `Restarting orchestrator for ${pool.name}`,
        content:
          "A rollout restart has been triggered. The orchestrator pod will be replaced momentarily.",
      })
    } catch (e) {
      flash.push({
        type: "error",
        header: "Orchestrator restart failed",
        content: String((e as Error).message),
      })
    } finally {
      setOrchestratorRestarting(false)
      setConfirmingOrchestratorRestart(false)
    }
  }

  if (loading && !pool) {
    return (
      <Container header={<Header variant="h1">{name}</Header>}>
        <Box textAlign="center" padding="l">
          <Spinner /> Loading pool…
        </Box>
      </Container>
    )
  }
  if (!pool) return null

  const status = derivePoolStatus(pool)

  return (
    <SpaceBetween size="l">
      <Container
        header={
          <Header
            variant="h1"
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button iconName="refresh" onClick={load} />
                <Button
                  onClick={() =>
                    pool && navigate("/pools/new", { state: { source: pool } })
                  }
                >
                  Duplicate
                </Button>
                {admin && (
                  <Button
                    onClick={() => setConfirmingOrchestratorRestart(true)}
                    disabled={orchestratorRestarting}
                  >
                    Restart orchestrator
                  </Button>
                )}
                <Button onClick={() => setConfirmingDelete(true)}>
                  Delete
                </Button>
              </SpaceBetween>
            }
          >
            {pool.name}
          </Header>
        }
      >
        <ColumnLayout columns={admin ? 3 : 2} variant="text-grid">
          <div>
            <Box variant="awsui-key-label">Status</Box>
            <PoolStatusPill status={status} />
          </div>
          <div>
            <Box variant="awsui-key-label">Namespace</Box>
            <div>{pool.namespace}</div>
          </div>
          {admin && (
            <div>
              <Box variant="awsui-key-label">Gateway</Box>
              <div>
                <code>{window.location.origin}/api/gateway/{pool.name}</code>
              </div>
            </div>
          )}
        </ColumnLayout>
      </Container>

      <Tabs
        tabs={[
          {
            label: "Claims",
            id: "claims",
            content: <ClaimsTable pool={pool} />,
          },
          {
            label: "Configuration",
            id: "configuration",
            content: <ConfigurationTab pool={pool} onServicesUpdated={load} />,
          },
        ]}
      />

      <Modal
        visible={confirmingDelete}
        onDismiss={() => setConfirmingDelete(false)}
        header={`Delete ${name}?`}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button onClick={() => setConfirmingDelete(false)}>
                Cancel
              </Button>
              <Button variant="primary" onClick={remove} loading={deleting}>
                Delete
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        This deletes the pool and tears down all VMs and the orchestrator
        in <b>{pool.namespace}</b>.
      </Modal>

      {admin && (
        <Modal
          visible={confirmingOrchestratorRestart}
          onDismiss={() => { if (!orchestratorRestarting) setConfirmingOrchestratorRestart(false) }}
          header={`Restart orchestrator for ${name}?`}
          footer={
            <Box float="right">
              <SpaceBetween direction="horizontal" size="xs">
                <Button
                  onClick={() => setConfirmingOrchestratorRestart(false)}
                  disabled={orchestratorRestarting}
                >
                  Cancel
                </Button>
                <Button
                  variant="primary"
                  onClick={restartOrchestrator}
                  loading={orchestratorRestarting}
                >
                  Restart orchestrator
                </Button>
              </SpaceBetween>
            </Box>
          }
        >
          This will trigger a rolling restart of the{" "}
          <b>{name}-orchestrator</b> Deployment in{" "}
          <b>{pool.namespace}</b>. Equivalent to{" "}
          <code>kubectl rollout restart deployment/{name}-orchestrator</code>.
          In-flight requests will be drained before the old pod is terminated.
        </Modal>
      )}
    </SpaceBetween>
  )
}

function probePort(probe?: Record<string, unknown>): string {
  if (!probe) return ""
  const tcp = probe.tcpSocket as { port?: number } | undefined
  if (tcp?.port) return `TCP :${tcp.port}`
  const http = probe.httpGet as { port?: number; path?: string } | undefined
  if (http?.port) return `HTTP :${http.port}${http.path ?? ""}`
  return JSON.stringify(probe)
}

// -- Configuration tab ----------------------------------------------------------

function ConfigurationTab({
  pool,
  onServicesUpdated,
}: {
  pool: PoolData
  onServicesUpdated: () => void
}) {
  return (
    <SpaceBetween size="l">
      <Container header={<Header variant="h2">Configuration</Header>}>
        <ColumnLayout columns={2} variant="text-grid">
          <div>
            <Box variant="awsui-key-label">vCPU cores</Box>
            <div>{pool.cpu}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">RAM</Box>
            <div>{pool.ram}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">OCI image</Box>
            <div><code>{pool.ociImage}</code></div>
          </div>
          <div>
            <Box variant="awsui-key-label">Replicas</Box>
            <div>{pool.replicas}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Available / Total</Box>
            <div>{pool.availableCount} / {pool.totalCount}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Claimed</Box>
            <div>{pool.claimedCount}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Readiness probe</Box>
            <div>{probePort(pool.probes?.readinessProbe) || "None"}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Liveness probe</Box>
            <div>{probePort(pool.probes?.livenessProbe) || "None"}</div>
          </div>
        </ColumnLayout>
      </Container>
      <ServicesEditor pool={pool} onSaved={onServicesUpdated} />
    </SpaceBetween>
  )
}

// -- Services editor ------------------------------------------------------------

interface ServiceRow {
  id: string
  name: string
  targetPort: string
  protocol: string
}

const PROTOCOL_OPTIONS = [
  { label: "TCP", value: "TCP" },
  { label: "UDP", value: "UDP" },
]

let _svcId = 0
function svcId(): string {
  return String(++_svcId)
}

function ServicesEditor({
  pool,
  onSaved,
}: {
  pool: PoolData
  onSaved: () => void
}) {
  const flash = useFlash()
  const [rows, setRows] = useState<ServiceRow[]>(() =>
    pool.services.map(s => ({
      id: svcId(),
      name: s.name,
      targetPort: String(s.targetPort),
      protocol: s.protocol,
    })),
  )
  const [saving, setSaving] = useState(false)

  // Reset rows when pool data changes (e.g. after reload)
  useEffect(() => {
    setRows(
      pool.services.map(s => ({
        id: svcId(),
        name: s.name,
        targetPort: String(s.targetPort),
        protocol: s.protocol,
      })),
    )
  }, [pool.services])

  const addRow = () =>
    setRows(prev => [...prev, { id: svcId(), name: "", targetPort: "", protocol: "TCP" }])

  const updateRow = (id: string, field: keyof ServiceRow, value: string) =>
    setRows(prev => prev.map(r => (r.id === id ? { ...r, [field]: value } : r)))

  const removeRow = (id: string) =>
    setRows(prev => prev.filter(r => r.id !== id))

  const save = async () => {
    const filled = rows.filter(r => r.name && r.targetPort)
    // Validate port range
    for (const r of filled) {
      const p = parseInt(r.targetPort, 10)
      if (isNaN(p) || p < 1 || p > 65535) {
        flash.push({
          type: "error",
          header: "Invalid port",
          content: `"${r.targetPort}" is not a valid port (1-65535).`,
        })
        return
      }
    }
    // Validate no duplicate names
    const seen = new Set<string>()
    for (const r of filled) {
      if (seen.has(r.name)) {
        flash.push({
          type: "error",
          header: "Duplicate service name",
          content: `"${r.name}" is used more than once.`,
        })
        return
      }
      seen.add(r.name)
    }

    setSaving(true)
    try {
      const services = filled.map(r => ({
        name: r.name,
        targetPort: parseInt(r.targetPort, 10),
        protocol: r.protocol || "TCP",
      }))
      await api.updatePoolServices(pool.namespace, pool.name, services)
      flash.push({ type: "success", header: "Services updated" })
      onSaved()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to update services",
        content: String((e as Error).message),
      })
    } finally {
      setSaving(false)
    }
  }

  return (
    <Container
      header={
        <Header
          variant="h2"
          actions={
            <SpaceBetween direction="horizontal" size="xs">
              <Button iconName="add-plus" onClick={addRow}>
                Add service
              </Button>
              <Button variant="primary" loading={saving} onClick={save}>
                Save
              </Button>
            </SpaceBetween>
          }
        >
          Services
        </Header>
      }
    >
      {rows.length === 0 ? (
        <Box color="text-status-inactive">
          No services defined. Add one to expose extra ports on each sandbox.
        </Box>
      ) : (
        <SpaceBetween size="s">
          {rows.map(row => (
            <ColumnLayout key={row.id} columns={4}>
              <Input
                value={row.name}
                onChange={({ detail }) => updateRow(row.id, "name", detail.value)}
                placeholder="Service name"
              />
              <Input
                type="number"
                value={row.targetPort}
                onChange={({ detail }) => updateRow(row.id, "targetPort", detail.value)}
                placeholder="Target port"
              />
              <Select
                selectedOption={
                  PROTOCOL_OPTIONS.find(o => o.value === row.protocol) ?? PROTOCOL_OPTIONS[0]
                }
                onChange={({ detail }) =>
                  updateRow(row.id, "protocol", detail.selectedOption.value ?? "TCP")
                }
                options={PROTOCOL_OPTIONS}
              />
              <Button iconName="remove" variant="icon" onClick={() => removeRow(row.id)} />
            </ColumnLayout>
          ))}
        </SpaceBetween>
      )}
    </Container>
  )
}

// ── Claims table ──────────────────────────────────────────────────────────

function claimStatusType(phase: string): "success" | "pending" | "error" | "info" {
  switch (phase) {
    case "Bound":
      return "success"
    case "Pending":
      return "pending"
    case "Failed":
      return "error"
    default:
      return "info"
  }
}

function age(iso: string): string {
  if (!iso) return "-"
  const ms = Date.now() - new Date(iso).getTime()
  if (ms < 0) return "just now"
  const secs = Math.floor(ms / 1000)
  if (secs < 60) return `${secs}s`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `${hrs}h`
  return `${Math.floor(hrs / 24)}d`
}

function ClaimsTable({ pool }: { pool: PoolData }) {
  const navigate = useNavigate()
  const flash = useFlash()
  const [claims, setClaims] = useState<Claim[]>([])
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [creating, setCreating] = useState(false)
  const [confirmRelease, setConfirmRelease] = useState<string | null>(null)
  const [releasing, setReleasing] = useState(false)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Pool name = namespace name (1:1 mapping).
  const claimNamespace = pool.namespace
  // The compat shim names the template <pool-name>-template.
  const templateRef = `${pool.name}-template`

  const loadClaims = useCallback(async () => {
    try {
      const list = await claimsApi.list(claimNamespace)
      setClaims(list)
    } catch {
      // Silently ignore polling errors — the table will show stale data
      // until the next successful poll.
    } finally {
      setLoading(false)
    }
  }, [claimNamespace])

  // Initial load + 5-second auto-refresh.
  useEffect(() => {
    loadClaims()
    timerRef.current = setInterval(loadClaims, 5000)
    return () => {
      if (timerRef.current) clearInterval(timerRef.current)
    }
  }, [loadClaims])

  const createClaim = async () => {
    setCreating(true)
    try {
      const id = `claim-${Math.random().toString(36).slice(2, 10)}`
      await claimsApi.create(claimNamespace, id, templateRef)
      flash.push({ type: "success", header: `Created claim ${id}` })
      setShowCreate(false)
      await loadClaims()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to create claim",
        content: String((e as Error).message),
      })
    } finally {
      setCreating(false)
    }
  }

  const releaseClaim = async (claimName: string) => {
    setReleasing(true)
    try {
      await claimsApi.remove(claimNamespace, claimName)
      flash.push({ type: "success", header: `Released claim ${claimName}` })
      setConfirmRelease(null)
      await loadClaims()
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to release claim",
        content: String((e as Error).message),
      })
    } finally {
      setReleasing(false)
    }
  }

  return (
    <>
      <Table
        loading={loading}
        items={claims}
        header={
          <Header
            variant="h2"
            counter={`(${claims.length})`}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button iconName="refresh" onClick={loadClaims} />
                <Button variant="primary" onClick={() => setShowCreate(true)}>
                  Create claim
                </Button>
              </SpaceBetween>
            }
          >
            Claims
          </Header>
        }
        columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: (c: Claim) => (
              <Link
                href={`#/pools/${pool.namespace}/${pool.name}/claims/${c.name}`}
                onFollow={e => {
                  e.preventDefault()
                  navigate(`/pools/${pool.namespace}/${pool.name}/claims/${c.name}`)
                }}
              >
                {c.name}
              </Link>
            ),
            sortingField: "name",
          },
          {
            id: "status",
            header: "Status",
            cell: (c: Claim) => (
              <StatusIndicator type={claimStatusType(c.phase)}>
                {c.phase}
              </StatusIndicator>
            ),
            sortingField: "phase",
          },
          {
            id: "sandbox",
            header: "VM Name",
            cell: (c: Claim) => c.sandboxName ?? "-",
          },
          {
            id: "age",
            header: "Age",
            cell: (c: Claim) => age(c.createdAt),
          },
          {
            id: "actions",
            header: "Actions",
            cell: (c: Claim) => (
              <Button
                variant="inline-link"
                onClick={() => setConfirmRelease(c.name)}
              >
                Release
              </Button>
            ),
          },
        ]}
        empty={
          <Box textAlign="center" color="text-status-inactive" padding="l">
            No claims. Create one to get a VM from this pool.
          </Box>
        }
      />

      {/* Create claim modal */}
      <Modal
        visible={showCreate}
        onDismiss={() => { if (!creating) setShowCreate(false) }}
        header="Create claim"
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button
                onClick={() => setShowCreate(false)}
                disabled={creating}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                onClick={createClaim}
                loading={creating}
              >
                Create
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        <SpaceBetween size="m">
          <div>
            <Box variant="awsui-key-label">Namespace</Box>
            <div>{claimNamespace}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Name</Box>
            <div>Auto-generated (claim-&lt;random&gt;)</div>
          </div>
        </SpaceBetween>
      </Modal>

      {/* Release confirmation modal */}
      <Modal
        visible={confirmRelease !== null}
        onDismiss={() => { if (!releasing) setConfirmRelease(null) }}
        header={`Release claim ${confirmRelease}?`}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button
                onClick={() => setConfirmRelease(null)}
                disabled={releasing}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                onClick={() => confirmRelease && releaseClaim(confirmRelease)}
                loading={releasing}
              >
                Release
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        This will delete the claim and return its sandbox VM back to the
        warm pool (default Retain policy). The VM will be restarted with a
        clean state.
      </Modal>
    </>
  )
}
