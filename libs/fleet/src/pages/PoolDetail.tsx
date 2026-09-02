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
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table from "@cloudscape-design/components/table"
import Tabs from "@cloudscape-design/components/tabs"
import { useFlash } from "../components/FlashContext"
import { PoolStatusPill } from "../components/PoolStatus"
import { createClaim as createSdkClaim, deleteClaim, listClaims } from "../fleet/claims"
import { listPoolInstances } from "../fleet/instances"
import type { Claim, PoolData, PoolInstance } from "../fleet/models"
import { deletePool, getPool, updatePoolServices } from "../fleet/pools"
import { tombstonePool } from "../fleet/status"
import { CuaButton } from "../components/CuaButton"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import { formatTtl } from "../fleet/ttl"

export function PoolDetail() {
  const { namespace = "", name = "" } = useParams()
  const navigate = useNavigate()
  const flash = useFlash()

  const [pool, setPool] = useState<PoolData | null>(null)
  const [loading, setLoading] = useState(true)
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)

  const load = async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const p = await getPool(namespace, name)
      setPool(p)
    } catch (e) {
      const message = String((e as Error).message)
      setLoadError(message)
      flash.push({
        type: "error",
        header: `Failed to load pool "${name}"`,
        content: message,
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
      await deletePool(namespace, name)
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

  if (loading && !pool) {
    return (
      <PageShell eyebrow="Fleet / Pool" title={name || "Pool"}>
        <PageEmpty title="Loading pool…" />
      </PageShell>
    )
  }
  if (!pool) {
    return (
      <PageShell eyebrow="Fleet / Pool" title={name || "Pool"}>
        <PageError
          title="Pool unavailable"
          action={<CuaButton onClick={load}>Try again</CuaButton>}
        >
          {loadError}
        </PageError>
      </PageShell>
    )
  }

  return (
    <PageShell
      eyebrow="Fleet / Pool"
      title={pool.name}
      description={<PoolStatusPill status={pool.status} />}
      secondaryActions={
        <SpaceBetween direction="horizontal" size="xs">
          <CuaButton
            tone="icon"
            ariaLabel="Refresh pool"
            iconName="refresh"
            onClick={load}
          />
          <CuaButton
            onClick={() =>
              navigate("/pools/new", { state: { source: pool } })
            }
          >
            Duplicate
          </CuaButton>
          <CuaButton onClick={() => setConfirmingDelete(true)}>
            Delete
          </CuaButton>
        </SpaceBetween>
      }
    >
      <SpaceBetween size="l">
      <Tabs
        tabs={[
          {
            label: "Instances",
            id: "instances",
            content: <InstancesTable pool={pool} />,
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
              <CuaButton onClick={() => setConfirmingDelete(false)}>
                Cancel
              </CuaButton>
              <CuaButton tone="danger" onClick={remove} loading={deleting}>
                Delete
              </CuaButton>
            </SpaceBetween>
          </Box>
        }
      >
        This deletes the pool and tears down all VMs and the orchestrator
        in <b>{pool.namespace}</b>.
      </Modal>

      </SpaceBetween>
    </PageShell>
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
      await updatePoolServices(pool.namespace, pool.name, services)
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
              <CuaButton iconName="add-plus" onClick={addRow}>
                Add service
              </CuaButton>
              <CuaButton tone="primary" loading={saving} onClick={save}>
                Save
              </CuaButton>
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
              <CuaButton
                tone="icon"
                ariaLabel={`Remove service ${row.name || "row"}`}
                iconName="remove"
                onClick={() => removeRow(row.id)}
              />
            </ColumnLayout>
          ))}
        </SpaceBetween>
      )}
    </Container>
  )
}

// ── Instances table ───────────────────────────────────────────────────────

function instanceStatusType(
  phase: string,
): "success" | "pending" | "error" | "info" {
  switch (phase) {
    case "Ready":
      return "success"
    case "Pending":
    case "Resetting":
      return "pending"
    case "Terminating":
      return "error"
    default:
      return "info"
  }
}

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

interface InstanceRow extends PoolInstance {
  claim?: Claim
}

function InstancesTable({ pool }: { pool: PoolData }) {
  const navigate = useNavigate()
  const flash = useFlash()
  const [instances, setInstances] = useState<PoolInstance[]>([])
  const [claims, setClaims] = useState<Claim[]>([])
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [creating, setCreating] = useState(false)
  const [confirmRelease, setConfirmRelease] = useState<string | null>(null)
  const [releasing, setReleasing] = useState(false)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Pool name = namespace name (1:1 mapping).
  const claimNamespace = pool.namespace
  const loadInstances = useCallback(async () => {
    try {
      const [instanceList, claimList] = await Promise.all([
        listPoolInstances(claimNamespace, pool.name),
        listClaims(claimNamespace),
      ])
      setInstances(instanceList)
      setClaims(claimList)
    } catch {
      // Silently ignore polling errors — the table will show stale data
      // until the next successful poll.
    } finally {
      setLoading(false)
    }
  }, [claimNamespace, pool.name])

  // Initial load + 5-second auto-refresh.
  useEffect(() => {
    loadInstances()
    timerRef.current = setInterval(loadInstances, 5000)
    return () => {
      if (timerRef.current) clearInterval(timerRef.current)
    }
  }, [loadInstances])

  const createClaim = async () => {
    setCreating(true)
    try {
      const claim = await createSdkClaim(claimNamespace, pool.name)
      flash.push({ type: "success", header: `Created claim ${claim.name}` })
      setShowCreate(false)
      await loadInstances()
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
      await deleteClaim(claimNamespace, claimName)
      flash.push({ type: "success", header: `Released claim ${claimName}` })
      setConfirmRelease(null)
      await loadInstances()
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

  const claimsByName = new Map(claims.map(claim => [claim.name, claim]))
  const claimsBySandbox = new Map(
    claims
      .filter(claim => claim.sandboxName)
      .map(claim => [claim.sandboxName as string, claim]),
  )
  const rows: InstanceRow[] = instances.map(instance => ({
    ...instance,
    claim:
      (instance.claimName && claimsByName.get(instance.claimName)) ||
      claimsBySandbox.get(instance.name),
  }))

  return (
    <>
      <Table
        loading={loading}
        items={rows}
        header={
          <Header
            variant="h2"
            counter={`(${rows.length})`}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <CuaButton
                  tone="icon"
                  ariaLabel="Refresh instances"
                  iconName="refresh"
                  onClick={loadInstances}
                />
                <CuaButton tone="primary" onClick={() => setShowCreate(true)}>
                  Create claim
                </CuaButton>
              </SpaceBetween>
            }
          >
            Instances
          </Header>
        }
        columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: (instance: InstanceRow) => (
              <Link
                href={`#/pools/${pool.namespace}/${pool.name}/instances/${instance.name}`}
                onFollow={event => {
                  event.preventDefault()
                  navigate(
                    `/pools/${pool.namespace}/${pool.name}/instances/${instance.name}`,
                  )
                }}
              >
                {instance.name}
              </Link>
            ),
            sortingField: "name",
          },
          {
            id: "instanceStatus",
            header: "Instance status",
            cell: (instance: InstanceRow) => (
              <StatusIndicator type={instanceStatusType(instance.phase)}>
                {instance.phase}
              </StatusIndicator>
            ),
            sortingField: "phase",
          },
          {
            id: "claim",
            header: "Claim",
            cell: (instance: InstanceRow) => {
              const claimName = instance.claim?.name ?? instance.claimName
              if (!claimName) return "-"
              return (
                <Link
                  href={`#/pools/${pool.namespace}/${pool.name}/claims/${claimName}`}
                  onFollow={event => {
                    event.preventDefault()
                    navigate(
                      `/pools/${pool.namespace}/${pool.name}/claims/${claimName}`,
                    )
                  }}
                >
                  {claimName}
                </Link>
              )
            },
          },
          {
            id: "claimStatus",
            header: "Claim status",
            cell: (instance: InstanceRow) => {
              const claimName = instance.claim?.name ?? instance.claimName
              const phase =
                instance.claim?.phase ?? (claimName ? "Claimed" : "Unclaimed")
              return (
                <StatusIndicator
                  type={claimName ? claimStatusType(phase) : "stopped"}
                >
                  {phase}
                </StatusIndicator>
              )
            },
          },
          {
            id: "ttl",
            header: "TTL",
            cell: (instance: InstanceRow) =>
              instance.claim
                ? formatTtl(instance.claim.ttlSecondsAfterCreated)
                : "-",
          },
          {
            id: "age",
            header: "Age",
            cell: (instance: InstanceRow) => age(instance.createdAt),
          },
          {
            id: "actions",
            header: "Actions",
            cell: (instance: InstanceRow) => {
              const claimName = instance.claim?.name ?? instance.claimName
              if (!claimName) return "-"
              return (
                <Button
                  variant="inline-link"
                  onClick={() => setConfirmRelease(claimName)}
                >
                  Release
                </Button>
              )
            },
          },
        ]}
        empty={
          <Box textAlign="center" color="text-status-inactive" padding="l">
            No instances are currently registered for this pool.
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
              <CuaButton
                onClick={() => setShowCreate(false)}
                disabled={creating}
              >
                Cancel
              </CuaButton>
              <CuaButton
                tone="primary"
                onClick={createClaim}
                loading={creating}
              >
                Create
              </CuaButton>
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
              <CuaButton
                onClick={() => setConfirmRelease(null)}
                disabled={releasing}
              >
                Cancel
              </CuaButton>
              <CuaButton
                tone="danger"
                onClick={() => confirmRelease && releaseClaim(confirmRelease)}
                loading={releasing}
              >
                Release
              </CuaButton>
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
