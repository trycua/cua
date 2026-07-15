import { useEffect, useMemo, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import BreadcrumbGroup from "@cloudscape-design/components/breadcrumb-group"
import Box from "@cloudscape-design/components/box"
import Button from "@cloudscape-design/components/button"
import ColumnLayout from "@cloudscape-design/components/column-layout"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import Modal from "@cloudscape-design/components/modal"
import ProgressBar from "@cloudscape-design/components/progress-bar"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Spinner from "@cloudscape-design/components/spinner"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import { api, parseMemoryBytes, sumPodUsage } from "../api/cyclops"
import type { PodMetrics, PodSummary } from "../api/cyclops"
import { useFlash } from "../components/FlashContext"
import { LogsPanel } from "../components/LogsPanel"

const LOG_CONTAINER = "guest-console-log"
const METRICS_INTERVAL_MS = 5000

export function PoolReplicaDetail() {
  const { namespace: nsParam = "", poolName = "", vmName = "" } = useParams()
  const navigate = useNavigate()
  const flash = useFlash()

  const [namespace, setNamespace] = useState<string | null>(null)
  const [launcher, setLauncher] = useState<PodSummary | null>(null)
  const [launcherLoading, setLauncherLoading] = useState(true)
  const [launcherError, setLauncherError] = useState<string | null>(null)

  const [metrics, setMetrics] = useState<PodMetrics | null>(null)
  const [metricsError, setMetricsError] = useState<string | null>(null)

  // Pool's spec.cpu / spec.ram (denominators for the progress bars).
  const [cpuLimit, setCpuLimit] = useState<number | null>(null)
  const [memLimit, setMemLimit] = useState<number | null>(null)

  // Restart confirmation dialog state.
  const [confirmingRestart, setConfirmingRestart] = useState(false)
  const [restarting, setRestarting] = useState(false)

  useEffect(() => {
    if (!nsParam || !poolName) return
    api
      .getPool(nsParam, poolName)
      .then(p => {
        setNamespace(p.namespace)
        setCpuLimit(p.cpu)
        setMemLimit(parseMemoryBytes(p.ram))
      })
      .catch(e => setLauncherError(String((e as Error).message)))
  }, [nsParam, poolName])

  // Once we have the namespace, find the launcher pod.
  useEffect(() => {
    if (!namespace || !vmName) return
    setLauncherLoading(true)
    setLauncherError(null)
    api
      .findLauncherPod(namespace, vmName)
      .then(p => setLauncher(p))
      .catch(e => setLauncherError(String((e as Error).message)))
      .finally(() => setLauncherLoading(false))
  }, [namespace, vmName])

  // Poll metrics every 5s once we have a launcher pod.
  useEffect(() => {
    if (!launcher) return
    let cancelled = false
    const tick = async () => {
      try {
        const m = await api.getPodMetrics(
          launcher.metadata.namespace,
          launcher.metadata.name,
        )
        if (!cancelled) {
          setMetrics(m)
          setMetricsError(null)
        }
      } catch (e) {
        if (!cancelled) setMetricsError(String((e as Error).message))
      }
    }
    tick()
    const id = window.setInterval(tick, METRICS_INTERVAL_MS)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [launcher])

  const usage = useMemo(() => (metrics ? sumPodUsage(metrics) : null), [metrics])

  const refresh = () => {
    if (!namespace) return
    setLauncherLoading(true)
    setLauncherError(null)
    api
      .findLauncherPod(namespace, vmName)
      .then(p => setLauncher(p))
      .catch(e => setLauncherError(String((e as Error).message)))
      .finally(() => setLauncherLoading(false))
  }

  const restart = async () => {
    if (!namespace) return
    setRestarting(true)
    try {
      await api.restartVm(namespace, vmName)
      flash.push({
        type: "success",
        header: `Restarting ${vmName}`,
        content:
          "The VM restart has been requested. The VMI will terminate and a fresh instance will start from the same spec.",
      })
      setConfirmingRestart(false)
      // Refresh after a short delay to pick up the new launcher pod.
      setTimeout(refresh, 3000)
    } catch (e) {
      flash.push({
        type: "error",
        header: "Restart failed",
        content: String((e as Error).message),
      })
    } finally {
      setRestarting(false)
      setConfirmingRestart(false)
    }
  }

  return (
    <SpaceBetween size="l">
      <BreadcrumbGroup
        items={[
          { text: "Pools", href: "#/pools" },
          { text: poolName, href: `#/pools/${nsParam}/${poolName}` },
          { text: vmName, href: "#" },
        ]}
        onFollow={e => {
          if (e.detail.href === "#") return
          e.preventDefault()
          navigate(e.detail.href.replace(/^#/, ""))
        }}
      />

      <Container
        header={
          <Header
            variant="h1"
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button iconName="refresh" onClick={refresh} disabled={launcherLoading}>
                  Refresh
                </Button>
                <Button
                  onClick={() => setConfirmingRestart(true)}
                  disabled={restarting}
                >
                  Restart
                </Button>
              </SpaceBetween>
            }
          >
            {vmName}
          </Header>
        }
      >
        <ColumnLayout columns={2}>
          <div>
            <Box variant="awsui-key-label">CPU</Box>
            <MetricTile
              error={metricsError}
              loading={!metrics && !metricsError}
              value={usage?.cpuCores}
              limit={cpuLimit}
              format={fmtCores}
            />
          </div>
          <div>
            <Box variant="awsui-key-label">Memory</Box>
            <MetricTile
              error={metricsError}
              loading={!metrics && !metricsError}
              value={usage?.memBytes}
              limit={memLimit}
              format={fmtBytes}
            />
          </div>
        </ColumnLayout>
      </Container>

      <LogsPanel
        namespace={launcher?.metadata.namespace}
        podName={launcher?.metadata.name}
        container={LOG_CONTAINER}
        title="Logs (guest console)"
        waitingText="Waiting for launcher pod…"
      />

      {launcherError && !launcherLoading && !launcher && (
        <Box color="text-status-warning" textAlign="center" padding="m">
          <SpaceBetween size="xs">
            <b>Couldn't find the launcher pod</b>
            <Box variant="small">
              <code>{launcherError}</code>
            </Box>
          </SpaceBetween>
        </Box>
      )}

      <Modal
        visible={confirmingRestart}
        onDismiss={() => { if (!restarting) setConfirmingRestart(false) }}
        header={`Restart ${vmName}?`}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <Button
                onClick={() => setConfirmingRestart(false)}
                disabled={restarting}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                onClick={restart}
                loading={restarting}
              >
                Restart
              </Button>
            </SpaceBetween>
          </Box>
        }
      >
        This will restart replica <b>{vmName}</b> via the KubeVirt subresource
        API (equivalent to <code>virtctl restart {vmName}</code>). The VM's
        current instance will be terminated and a fresh one started from the
        same spec. Any in-progress session on this replica will be interrupted.
      </Modal>
    </SpaceBetween>
  )
}

// ── Metric tile ────────────────────────────────────────────────────────────

function MetricTile({
  loading,
  error,
  value,
  limit,
  format,
}: {
  loading: boolean
  error: string | null
  value: number | undefined
  limit: number | null
  format: (n: number) => string
}) {
  if (loading) {
    return (
      <Box textAlign="center" padding="m">
        <Spinner /> Loading metrics…
      </Box>
    )
  }
  if (error) {
    return (
      <Box color="text-body-secondary" padding="m">
        <StatusIndicator type="warning">{error}</StatusIndicator>
      </Box>
    )
  }
  if (value === undefined) return null
  const denom = limit ?? 0
  const pct = denom > 0 ? Math.min(100, (value / denom) * 100) : 0
  return (
    <SpaceBetween size="xs">
      <Box variant="h2" fontSize="display-l">
        {format(value)}
      </Box>
      {limit ? (
        <ProgressBar
          value={pct}
          additionalInfo={`${format(value)} of ${format(denom)}`}
        />
      ) : (
        <Box variant="small" color="text-body-secondary">
          No limit configured.
        </Box>
      )}
    </SpaceBetween>
  )
}

function fmtCores(n: number): string {
  return `${n.toFixed(2)} cores`
}

function fmtBytes(n: number): string {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(2)} GiB`
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MiB`
  if (n >= 1024) return `${(n / 1024).toFixed(0)} KiB`
  return `${n} B`
}
