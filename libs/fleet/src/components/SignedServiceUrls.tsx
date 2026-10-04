import { useEffect, useRef, useState } from "react"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import Container from "@cloudscape-design/components/container"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import Form from "@cloudscape-design/components/form"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input from "@cloudscape-design/components/input"
import RadioGroup from "@cloudscape-design/components/radio-group"
import Select, { type SelectProps } from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import StatusIndicator from "@cloudscape-design/components/status-indicator"
import Table from "@cloudscape-design/components/table"
import {
  CreateSignedServiceUrlRequestBuilder,
  SdkError,
  type Sandbox,
  type SignedServiceUrl,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import { withClient } from "../auth/cyclops-client"
import { errorMessage } from "../error-message"
import { CuaButton } from "./CuaButton"
import { useFlash } from "./FlashContext"

const expirationOptions = [
  { value: "900", label: "15 minutes" },
  { value: "3600", label: "1 hour" },
  { value: "86400", label: "24 hours" },
]

function formatDate(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value))
}

function signedUrlStatus(item: SignedServiceUrl, now: number) {
  if (item.revokedAt) {
    return <StatusIndicator type="stopped">Revoked</StatusIndicator>
  }
  if (new Date(item.expiresAt).getTime() <= now) {
    return <StatusIndicator type="error">Expired</StatusIndicator>
  }
  return <StatusIndicator type="success">Expires {formatDate(item.expiresAt)}</StatusIndicator>
}

export function SignedServiceUrls({ sandbox }: { sandbox: Sandbox }) {
  const flash = useFlash()
  const [signedUrls, setSignedUrls] = useState<SignedServiceUrl[]>([])
  const [loading, setLoading] = useState(true)
  const [unavailable, setUnavailable] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [revokingId, setRevokingId] = useState<string | null>(null)
  const [service, setService] = useState<SelectProps.Option | null>(null)
  const [label, setLabel] = useState("")
  const [expiresInSeconds, setExpiresInSeconds] = useState("3600")
  const [now, setNow] = useState(() => Date.now())
  const sandboxIdentity = `${sandbox.namespace}\u0000${sandbox.claim}\u0000${sandbox.name}`
  const currentSandboxIdentity = useRef(sandboxIdentity)
  const mounted = useRef(false)
  const requestGeneration = useRef(0)

  if (currentSandboxIdentity.current !== sandboxIdentity) {
    currentSandboxIdentity.current = sandboxIdentity
  }

  const isCurrent = (identity: string) => (
    mounted.current && currentSandboxIdentity.current === identity
  )

  const markUnavailable = (identity: string) => {
    if (!isCurrent(identity)) return
    setUnavailable(true)
    setCreating(false)
    setSubmitting(false)
    setRevokingId(null)
  }

  const refresh = async (
    requestSandbox: Sandbox = sandbox,
    identity: string = sandboxIdentity,
  ) => {
    const generation = ++requestGeneration.current
    if (!isCurrent(identity)) return
    setLoading(true)
    try {
      const urls = await withClient(client => client.listSignedServiceUrls(requestSandbox))
      if (!isCurrent(identity) || generation !== requestGeneration.current) return
      setSignedUrls(urls)
      setUnavailable(false)
      setLoadError(null)
    } catch (error) {
      if (!isCurrent(identity) || generation !== requestGeneration.current) return
      if (SdkError.SignedServiceUrlsUnavailable.instanceOf(error)) {
        markUnavailable(identity)
        setLoadError(null)
      } else {
        setLoadError(errorMessage(error))
      }
    } finally {
      if (isCurrent(identity) && generation === requestGeneration.current) setLoading(false)
    }
  }

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      requestGeneration.current += 1
    }
  }, [])
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 30_000)
    return () => window.clearInterval(interval)
  }, [])


  useEffect(() => {
    const identity = sandboxIdentity
    currentSandboxIdentity.current = identity
    requestGeneration.current += 1
    if (!isCurrent(identity)) return
    setSignedUrls([])
    setUnavailable(false)
    setLoadError(null)
    setCreating(false)
    setSubmitting(false)
    setRevokingId(null)
    setService(null)
    setLabel("")
    setExpiresInSeconds("3600")
    void refresh(sandbox, identity)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sandbox.namespace, sandbox.claim, sandbox.name])

  const create = async () => {
    const identity = sandboxIdentity
    const requestSandbox = sandbox
    const selectedService = service?.value
    if (!selectedService || !isCurrent(identity)) return
    setSubmitting(true)
    try {
      let requestBuilder = new CreateSignedServiceUrlRequestBuilder()
        .sandbox(requestSandbox)
        .service(selectedService)
        .expiresInSeconds(Number(expiresInSeconds))
      const trimmedLabel = label.trim()
      if (trimmedLabel) requestBuilder = requestBuilder.label(trimmedLabel)
      await withClient(client => client.createSignedServiceUrl(requestBuilder.build()))
      if (!isCurrent(identity)) return
      flash.push({
        type: "success",
        header: "Signed URL created",
        content: "Share this bearer link only with people who need temporary access.",
      })
      setCreating(false)
      setService(null)
      setLabel("")
      setExpiresInSeconds("3600")
      await refresh(requestSandbox, identity)
    } catch (error) {
      if (!isCurrent(identity)) return
      if (SdkError.SignedServiceUrlsUnavailable.instanceOf(error)) {
        markUnavailable(identity)
        return
      }
      flash.push({
        type: "error",
        header: "Failed to create signed URL",
        content: errorMessage(error),
      })
    } finally {
      if (isCurrent(identity)) setSubmitting(false)
    }
  }

  const revoke = async (item: SignedServiceUrl) => {
    const identity = sandboxIdentity
    const requestSandbox = sandbox
    if (!isCurrent(identity)) return
    setRevokingId(item.id)
    try {
      await withClient(client => client.revokeSignedServiceUrl(item))
      if (!isCurrent(identity)) return
      flash.push({ type: "success", header: "Signed URL revoked" })
      await refresh(requestSandbox, identity)
    } catch (error) {
      if (!isCurrent(identity)) return
      if (SdkError.SignedServiceUrlsUnavailable.instanceOf(error)) {
        markUnavailable(identity)
        return
      }
      flash.push({
        type: "error",
        header: "Failed to revoke signed URL",
        content: errorMessage(error),
      })
    } finally {
      if (isCurrent(identity)) setRevokingId(null)
    }
  }

  const createAction = (
    <SpaceBetween direction="horizontal" size="xs">
      <CuaButton
        tone="icon"
        ariaLabel="Refresh signed URLs"
        iconName="refresh"
        disabled={loading}
        onClick={() => void refresh()}
      />
      <CuaButton onClick={() => setCreating(true)}>Create signed URL</CuaButton>
    </SpaceBetween>
  )

  if (unavailable) {
    return (
      <Container header={<Header variant="h2">Signed URLs</Header>}>
        <Alert type="info">
          Signed URLs are unavailable for this deployment. Other claim details remain available.
        </Alert>
      </Container>
    )
  }

  return (
    <SpaceBetween size="m">
      {creating && (
        <Container header={<Header variant="h2">Create signed URL</Header>}>
          <Form
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <CuaButton disabled={submitting} onClick={() => setCreating(false)}>
                  Cancel
                </CuaButton>
                <CuaButton tone="primary" loading={submitting} disabled={!service?.value} onClick={create}>
                  Create signed URL
                </CuaButton>
              </SpaceBetween>
            }
          >
            <SpaceBetween size="m">
              <Alert type="warning">
                Anyone with this bearer link can access the selected service until it expires or is revoked.
              </Alert>
              <FormField label="Service">
                <Select
                  selectedOption={service}
                  onChange={event => setService(event.detail.selectedOption)}
                  options={sandbox.services.map(value => ({ label: value, value }))}
                  placeholder="Choose a service"
                />
              </FormField>
              <FormField label="Label (optional)">
                <Input value={label} onChange={event => setLabel(event.detail.value)} />
              </FormField>
              <FormField label="Expiration">
                <RadioGroup
                  value={expiresInSeconds}
                  onChange={event => setExpiresInSeconds(event.detail.value)}
                  items={expirationOptions}
                />
              </FormField>
            </SpaceBetween>
          </Form>
        </Container>
      )}

      {loadError ? (
        <Container header={<Header variant="h2" actions={createAction}>Signed URLs</Header>}>
          <Alert type="error" action={<CuaButton onClick={() => void refresh()}>Retry</CuaButton>}>
            {loadError}
          </Alert>
        </Container>
      ) : (
        <section className="cua-page-table">
          <Table
            variant="borderless"
            loading={loading}
            loadingText="Loading signed URLs"
            items={signedUrls}
            header={<Header variant="h2" counter={`(${signedUrls.length})`} actions={createAction}>Signed URLs</Header>}
            empty={
              <Box textAlign="center" color="inherit">
                <SpaceBetween size="s">
                  <Box variant="p">No signed URLs yet.</Box>
                  <Box color="text-status-inactive">
                    Create a temporary bearer link to share service access without authentication.
                  </Box>
                  <div><CuaButton onClick={() => setCreating(true)}>Create signed URL</CuaButton></div>
                </SpaceBetween>
              </Box>
            }
            columnDefinitions={[
              { id: "label", header: "Label", cell: item => item.label ?? item.id.slice(0, 8) },
              { id: "service", header: "Service", cell: item => item.service },
              { id: "url", header: "URL", cell: item => <CopyToClipboard textToCopy={item.url} copyButtonText="Copy URL" copyButtonAriaLabel={`Copy signed URL ${item.id}`} copySuccessText="Copied" copyErrorText="Unable to copy URL" /> },
              { id: "created", header: "Created", cell: item => formatDate(item.createdAt) },
              { id: "expires", header: "Expires", cell: item => signedUrlStatus(item, now) },
              {
                id: "actions",
                header: "",
                cell: item => !item.revokedAt && new Date(item.expiresAt).getTime() > now && (
                  <CuaButton ariaLabel={`Revoke signed URL ${item.id}`} loading={revokingId === item.id} onClick={() => void revoke(item)}>
                    Revoke
                  </CuaButton>
                ),
              },
            ]}
          />
        </section>
      )}
    </SpaceBetween>
  )
}
