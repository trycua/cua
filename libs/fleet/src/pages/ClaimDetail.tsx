import { useEffect, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"
import Box from "@cloudscape-design/components/box"
import ColumnLayout from "@cloudscape-design/components/column-layout"
import Container from "@cloudscape-design/components/container"
import Header from "@cloudscape-design/components/header"
import Link from "@cloudscape-design/components/link"
import Modal from "@cloudscape-design/components/modal"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import { useFlash } from "../components/FlashContext"
import { DesktopPane } from "../components/DesktopPane"
import { deleteClaim, getClaim } from "../fleet/claims"
import type { Claim, PoolService } from "../fleet/models"
import { getPool } from "../fleet/pools"
import type { Sandbox } from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import { CuaButton } from "../components/CuaButton"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import { SignedServiceUrls } from "../components/SignedServiceUrls"

function phaseType(phase: string): "success" | "pending" | "error" | "info" {
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

function boundSandbox(claim: Claim, services: PoolService[]): Sandbox | null {
  if (claim.phase !== "Bound" || !claim.sandboxName) return null
  return {
    namespace: claim.namespace,
    claim: claim.name,
    name: claim.sandboxName,
    services: services.map(service => service.name),
  }
}

export function ClaimDetail() {
  const { namespace = "", poolName = "", claimName = "" } = useParams()

  return (
    <ClaimDetailContent
      key={`${namespace}/${poolName}/${claimName}`}
      namespace={namespace}
      poolName={poolName}
      claimName={claimName}
    />
  )
}

function ClaimDetailContent({
  namespace,
  poolName,
  claimName,
}: {
  namespace: string
  poolName: string
  claimName: string
}) {
  const navigate = useNavigate()
  const flash = useFlash()

  const [claim, setClaim] = useState<Claim | null>(null)
  const [services, setServices] = useState<PoolService[]>([])
  const [sandbox, setSandbox] = useState<Sandbox | null>(null)
  const [loading, setLoading] = useState(true)
  const [confirmRelease, setConfirmRelease] = useState(false)
  const [releasing, setReleasing] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const load = async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const [claimData, poolData] = await Promise.all([
        getClaim(namespace, claimName),
        getPool(namespace, poolName),
      ])
      const nextSandbox = boundSandbox(claimData, poolData.services)
      setClaim(claimData)
      setServices(poolData.services)
      setSandbox(nextSandbox)
    } catch (e) {
      const message = String((e as Error).message)
      setLoadError(message)
      flash.push({
        type: "error",
        header: "Failed to load claim",
        content: message,
      })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (namespace && poolName && claimName) load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [namespace, poolName, claimName])

  // Auto-refresh while Pending — only refetch the claim (pool data
  // doesn't change during binding). Errors are silent to avoid
  // spamming flash notifications every 3s.
  useEffect(() => {
    if (!claim || claim.phase !== "Pending") return
    const id = setInterval(async () => {
      try {
        const c = await getClaim(namespace, claimName)
        const nextSandbox = boundSandbox(c, services)
        setClaim(c)
        setSandbox(nextSandbox)
      } catch {
        // Silent — next tick will retry
      }
    }, 3000)
    return () => clearInterval(id)
  }, [claim?.phase, namespace, claimName, services])

  const release = async () => {
    setReleasing(true)
    try {
      await deleteClaim(namespace, claimName)
      flash.push({ type: "success", header: `Released claim ${claimName}` })
      navigate(`/pools/${namespace}/${poolName}`)
    } catch (e) {
      flash.push({
        type: "error",
        header: "Failed to release claim",
        content: String((e as Error).message),
      })
      setReleasing(false)
    }
  }

  if (loading && !claim) {
    return (
      <PageShell eyebrow="Fleet / Claim" title={claimName || "Claim"}>
        <PageEmpty title="Loading claim…" />
      </PageShell>
    )
  }
  if (!claim) {
    return (
      <PageShell eyebrow="Fleet / Claim" title={claimName || "Claim"}>
        <PageError
          title="Claim unavailable"
          action={<CuaButton onClick={load}>Try again</CuaButton>}
        >
          {loadError}
        </PageError>
      </PageShell>
    )
  }

  const sandboxName = claim.sandboxName
  const isBound = claim.phase === "Bound" && sandboxName

  return (
    <PageShell
      eyebrow="Fleet / Claim"
      title={claimName}
      description={
        <StatusIndicator type={phaseType(claim.phase)}>
          {claim.phase}
        </StatusIndicator>
      }
      secondaryActions={
        <SpaceBetween direction="horizontal" size="xs">
          <CuaButton
            tone="icon"
            ariaLabel="Refresh claim"
            iconName="refresh"
            onClick={load}
          />
          <CuaButton onClick={() => navigate(`/pools/${namespace}/${poolName}`)}>
            Back to pool
          </CuaButton>
          <CuaButton onClick={() => setConfirmRelease(true)}>
            Release
          </CuaButton>
        </SpaceBetween>
      }
    >
      <SpaceBetween size="l">
      <Container header={<Header variant="h2">Overview</Header>}>
        <ColumnLayout columns={2} variant="text-grid">
          <div>
            <Box variant="awsui-key-label">Status</Box>
            <StatusIndicator type={phaseType(claim.phase)}>
              {claim.phase}
            </StatusIndicator>
          </div>
          <div>
            <Box variant="awsui-key-label">Pool</Box>
            <Link
              href={`#/pools/${namespace}/${poolName}`}
              onFollow={e => {
                e.preventDefault()
                navigate(`/pools/${namespace}/${poolName}`)
              }}
            >
              {poolName}
            </Link>
          </div>
          <div>
            <Box variant="awsui-key-label">Sandbox</Box>
            <div>{sandboxName ?? "-"}</div>
          </div>
          <div>
            <Box variant="awsui-key-label">Template</Box>
            <div><code>{claim.templateRef}</code></div>
          </div>
        </ColumnLayout>
      </Container>

      {isBound && sandboxName && services.some(s => s.name === "vnc") && (
        <Container
          header={
            <Header variant="h2" description="Live macOS desktop streamed from the macOS sandbox via noVNC.">
              Desktop
            </Header>
          }
        >
          <DesktopPane namespace={namespace} sandboxName={sandboxName} vncService="vnc" />
        </Container>
      )}

      {isBound && services.length > 0 && (
        <Container header={<Header variant="h2">Services</Header>}>
          <SpaceBetween size="s">
            {services.map(svc => {
              const svcK8sName = `${sandboxName}-${svc.name}`
              const svcUrl = `/api/svc/${namespace}/${svcK8sName}/`
              return (
                <ColumnLayout key={svc.name} columns={3}>
                  <div>
                    <Box variant="awsui-key-label">{svc.name}</Box>
                    <div>port 80 → {svc.targetPort} ({svc.protocol})</div>
                  </div>
                  <div>
                    <Box variant="awsui-key-label">K8s Service</Box>
                    <div><code>{svcK8sName}</code></div>
                  </div>
                  <div>
                    <Box variant="awsui-key-label">URL</Box>
                    <Link href={svcUrl} external>{svcUrl}</Link>
                  </div>
                </ColumnLayout>
              )
            })}
          </SpaceBetween>
        </Container>
      )}

      {isBound && services.length === 0 && (
        <Container header={<Header variant="h2">Services</Header>}>
          <Box color="text-status-inactive">
            No services defined on this pool. Add services in the pool configuration to expose extra ports.
          </Box>
        </Container>
      )}

      {!isBound && (
        <Container header={<Header variant="h2">Services</Header>}>
          <Box color="text-status-inactive">
            Claim is not yet bound to a sandbox. Services will appear once a sandbox is assigned.
          </Box>
        </Container>
      )}

      {sandbox && <SignedServiceUrls key={`${sandbox.namespace}/${sandbox.name}`} sandbox={sandbox} />}

      <Modal
        visible={confirmRelease}
        onDismiss={() => { if (!releasing) setConfirmRelease(false) }}
        header={`Release claim ${claimName}?`}
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <CuaButton onClick={() => setConfirmRelease(false)} disabled={releasing}>
                Cancel
              </CuaButton>
              <CuaButton tone="danger" onClick={release} loading={releasing}>
                Release
              </CuaButton>
            </SpaceBetween>
          </Box>
        }
      >
        This will delete the claim and return its sandbox VM back to the warm pool.
        The VM will be restarted with a clean state.
      </Modal>
      </SpaceBetween>
    </PageShell>
  )
}
