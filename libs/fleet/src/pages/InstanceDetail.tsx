import { useCallback, useEffect, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Toggle from "@cloudscape-design/components/toggle"
import { LazyLog } from "@melloware/react-logviewer"
import { CuaButton } from "../components/CuaButton"
import { useFlash } from "../components/FlashContext"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import {
  getInstanceLogs,
  getInstancePod,
  getPoolInstance,
} from "../fleet/instances"
import type { InstancePod, PoolInstance } from "../fleet/models"

const LOG_REFRESH_INTERVAL_MS = 5000

function statusType(
  phase: string,
): "success" | "pending" | "error" | "info" | "stopped" {
  switch (phase) {
    case "Ready":
    case "Running":
      return "success"
    case "Pending":
    case "Resetting":
      return "pending"
    case "Failed":
    case "Terminating":
      return "error"
    case "Succeeded":
      return "stopped"
    default:
      return "info"
  }
}

export function InstanceDetail() {
  const { namespace = "", poolName = "", instanceName = "" } = useParams()
  const navigate = useNavigate()
  const flash = useFlash()
  const [instance, setInstance] = useState<PoolInstance | null>(null)
  const [pod, setPod] = useState<InstancePod | null>(null)
  const [logs, setLogs] = useState("")
  const [loading, setLoading] = useState(true)
  const [logsLoading, setLogsLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [logsError, setLogsError] = useState<string | null>(null)
  const [autoRefresh, setAutoRefresh] = useState(true)
  const [copied, setCopied] = useState(false)
  const [lastRefreshed, setLastRefreshed] = useState<Date | null>(null)
  const logRequestRef = useRef(0)
  const copyResetRef = useRef<number | null>(null)
  const logViewerRef = useRef<LazyLog | null>(null)

  const loadInstance = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const nextInstance = await getPoolInstance(namespace, instanceName)
      const nextPod = await getInstancePod(nextInstance)
      setInstance(nextInstance)
      setPod(nextPod)
    } catch (error) {
      setLoadError(String((error as Error).message))
    } finally {
      setLoading(false)
    }
  }, [instanceName, namespace])

  useEffect(() => {
    if (namespace && instanceName) void loadInstance()
  }, [instanceName, loadInstance, namespace])

  const loadLogs = useCallback(async () => {
    if (!pod) return
    const requestId = ++logRequestRef.current
    setLogsLoading(true)
    setLogsError(null)
    try {
      const nextLogs = await getInstanceLogs(pod)
      if (requestId !== logRequestRef.current) return
      setLogs(nextLogs)
      setLastRefreshed(new Date())
    } catch (error) {
      if (requestId !== logRequestRef.current) return
      setLogsError(String((error as Error).message))
    } finally {
      if (requestId === logRequestRef.current) setLogsLoading(false)
    }
  }, [pod])

  useEffect(() => {
    if (!pod) return
    void loadLogs()
  }, [loadLogs, pod])

  useEffect(() => {
    if (!autoRefresh || !pod) return
    const timer = window.setInterval(() => void loadLogs(), LOG_REFRESH_INTERVAL_MS)
    return () => window.clearInterval(timer)
  }, [autoRefresh, loadLogs, pod])

  useEffect(() => {
    return () => {
      if (copyResetRef.current !== null) {
        window.clearTimeout(copyResetRef.current)
      }
    }
  }, [])

  const copyLogs = async () => {
    if (!logs) return
    try {
      await navigator.clipboard.writeText(logs)
      setCopied(true)
      if (copyResetRef.current !== null) {
        window.clearTimeout(copyResetRef.current)
      }
      copyResetRef.current = window.setTimeout(() => {
        setCopied(false)
        copyResetRef.current = null
      }, 2000)
    } catch (error) {
      flash.push({
        type: "error",
        header: "Failed to copy logs",
        content: String((error as Error).message),
      })
    }
  }

  const scrollLogsToTop = () => {
    logViewerRef.current?.handleScrollToLine(1)
  }

  const scrollLogsToBottom = () => {
    const lineCount = logs ? logs.split(/\r?\n/).length : 1
    logViewerRef.current?.handleScrollToLine(lineCount)
  }

  if (loading && !instance) {
    return (
      <PageShell eyebrow="Fleet / Pool / Instance" title={instanceName || "Instance"}>
        <PageEmpty title="Loading instance…" />
      </PageShell>
    )
  }

  if (!instance) {
    return (
      <PageShell eyebrow="Fleet / Pool / Instance" title={instanceName || "Instance"}>
        <PageError
          title="Instance unavailable"
          action={<CuaButton onClick={loadInstance}>Try again</CuaButton>}
        >
          {loadError}
        </PageError>
      </PageShell>
    )
  }

  return (
    <PageShell
      eyebrow="Fleet / Pool / Instance"
      title={instance.name}
      description={
        <StatusIndicator type={statusType(instance.phase)}>
          {instance.phase}
        </StatusIndicator>
      }
      secondaryActions={
        <SpaceBetween direction="horizontal" size="xs">
          <CuaButton
            tone="icon"
            ariaLabel="Refresh instance"
            iconName="refresh"
            loading={loading}
            onClick={loadInstance}
          />
          <CuaButton onClick={() => navigate(`/pools/${namespace}/${poolName}`)}>
            Back to pool
          </CuaButton>
        </SpaceBetween>
      }
    >
      <Container
        header={
          <Header
            variant="h2"
            description={
              lastRefreshed
                ? `Last refreshed ${lastRefreshed.toLocaleTimeString()}`
                : "Most recent 1,000 lines from the guest console."
            }
            actions={
              <SpaceBetween direction="horizontal" size="s" alignItems="center">
                <Toggle
                  checked={autoRefresh}
                  onChange={({ detail }) => setAutoRefresh(detail.checked)}
                >
                  Auto-refresh
                </Toggle>
                <CuaButton
                  tone="icon"
                  ariaLabel="Scroll logs to top"
                  iconName="angle-up"
                  disabled={!logs}
                  onClick={scrollLogsToTop}
                />
                <CuaButton
                  tone="icon"
                  ariaLabel="Scroll logs to bottom"
                  iconName="angle-down"
                  disabled={!logs}
                  onClick={scrollLogsToBottom}
                />
                <CuaButton
                  iconName="copy"
                  disabled={!logs}
                  onClick={copyLogs}
                >
                  {copied ? "Copied" : "Copy logs"}
                </CuaButton>
                <CuaButton
                  tone="icon"
                  ariaLabel="Refresh logs"
                  iconName="refresh"
                  loading={logsLoading}
                  disabled={!pod}
                  onClick={loadLogs}
                />
              </SpaceBetween>
            }
          >
            Guest console logs
          </Header>
        }
      >
        {!pod ? (
          <PageEmpty title="Workload pod not available">
            The instance may still be starting. Refresh the instance to try again.
          </PageEmpty>
        ) : logsError ? (
          <PageError
            title="Unable to load guest console logs"
            action={<CuaButton onClick={loadLogs}>Try again</CuaButton>}
          >
            {logsError}
          </PageError>
        ) : logsLoading && !logs ? (
          <PageEmpty title="Loading guest console logs…" />
        ) : (
          <section aria-label="Guest console logs">
            <div className="cua-instance-log-view">
              <LazyLog
                ref={logViewerRef}
                className="cua-instance-log-lines"
                searchBarClassName="cua-instance-log-search"
                text={logs || "No guest console output."}
                enableLineNumbers
                enableSearch
                selectableLines
                wrapLines
                height="auto"
                rowHeight={20}
              />
            </div>
          </section>
        )}
      </Container>
    </PageShell>
  )
}
