import { useEffect, useMemo, useRef, useState } from "react"
import { useBeforeUnload, useLocation, useNavigate } from "react-router-dom"
import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import ColumnLayout from "@cloudscape-design/components/column-layout"
import Container from "@cloudscape-design/components/container"
import Form from "@cloudscape-design/components/form"
import FormField from "@cloudscape-design/components/form-field"
import Header from "@cloudscape-design/components/header"
import Input, { type InputProps } from "@cloudscape-design/components/input"
import Modal from "@cloudscape-design/components/modal"
import Select from "@cloudscape-design/components/select"
import SpaceBetween from "@cloudscape-design/components/space-between"
import Toggle from "@cloudscape-design/components/toggle"
import { billingApi } from "../api/billing"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { useFlash } from "../components/FlashContext"
import { errorMessage } from "../error-message"
import { createPool } from "../fleet/pools"
import type { PoolTemplateConfig } from "../fleet/models"
import { CuaButton } from "../components/CuaButton"
import { PageShell } from "../components/PageShell"
import { isReviewPreviewEnvironment } from "../preview-environment"

const NAME_PATTERN = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/

const DEFAULTS = {
  cpu: 4,
  ram: "4Gi",
  ociImage: "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest",
  replicas: 1,
}

const PROTOCOL_OPTIONS = [
  { label: "TCP", value: "TCP" },
  { label: "UDP", value: "UDP" },
]

const FIRMWARE_OPTIONS = [
  { label: "BIOS (default)", value: "bios", description: "Legacy boot — the Linux workspace images." },
  { label: "UEFI", value: "efi", description: "Required for GPT/UEFI-only images, e.g. the Windows desktop-workspace." },
]

interface ServiceEntry {
  id: string
  name: string
  targetPort: string
  protocol: string
}

interface PoolSource {
  name: string
  cpu?: number
  ram?: string
  ociImage?: string
  firmware?: "bios" | "efi"
  replicas?: number
  services?: { name: string; targetPort: number; protocol: string }[]
}

// Navigation state PoolNew accepts:
//   { source }   — duplicate an existing pool (from the Pools list)
interface PoolNewState {
  source?: PoolSource
}

let nextId = 0
function genId(): string {
  return String(++nextId)
}

// Recover the TCP port from a {tcpSocket:{port}} probe so the form's
// readiness/liveness fields round-trip through a duplicated pool.
function portFromProbe(probe?: Record<string, unknown>): string {
  const sock = probe?.tcpSocket as { port?: number } | undefined
  return sock?.port ? String(sock.port) : ""
}

export function PoolNew() {
  const navigate = useNavigate()
  const location = useLocation()
  const flash = useFlash()
  const { billing: billingEnabled } = useFeatureFlags()
  const paymentMethodGateEnabled = !isReviewPreviewEnvironment()

  const navState = location.state as PoolNewState | null
  const source = navState?.source ?? null
  // Seed from the duplicated pool, if any.
  const seed: Partial<PoolTemplateConfig> = source ?? {}

  const [name, setName] = useState(source ? `${source.name}-copy` : "")
  const [cpu, setCpu] = useState(String(seed.cpu ?? DEFAULTS.cpu))
  const [ram, setRam] = useState(seed.ram ?? DEFAULTS.ram)
  const [ociImage, setOciImage] = useState(seed.ociImage ?? DEFAULTS.ociImage)
  const [firmware, setFirmware] = useState<"bios" | "efi">(seed.firmware ?? "bios")
  const [replicas, setReplicas] = useState(String(seed.replicas ?? DEFAULTS.replicas))
  const [ttlSecondsAfterCreated, setTtlSecondsAfterCreated] = useState(
    seed.ttlSecondsAfterCreated === undefined
      ? ""
      : String(seed.ttlSecondsAfterCreated),
  )
  const [readinessPort, setReadinessPort] = useState(
    portFromProbe(seed.probes?.readinessProbe),
  )
  const [livenessPort, setLivenessPort] = useState(
    portFromProbe(seed.probes?.livenessProbe),
  )
  const [services, setServices] = useState<ServiceEntry[]>(
    (seed.services ?? []).map(s => ({
      id: genId(),
      name: s.name,
      targetPort: String(s.targetPort),
      protocol: s.protocol,
    })),
  )
  // Autoscaling extension (optional). When enabled, the pool gets a KEDA
  // ScaledObject that scales it to claim demand.
  const [autoscalingEnabled, setAutoscalingEnabled] = useState(!!seed.autoscaling)
  const [minPoolSize, setMinPoolSize] = useState(String(seed.autoscaling?.minPoolSize ?? 0))
  const [initialPoolSize, setInitialPoolSize] = useState(String(seed.autoscaling?.initialPoolSize ?? 0))
  const [maxPoolSize, setMaxPoolSize] = useState(String(seed.autoscaling?.maxPoolSize ?? 20))
  const [submitting, setSubmitting] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [submitAttempted, setSubmitAttempted] = useState(false)
  const [discardOpen, setDiscardOpen] = useState(false)
  // True once billing confirms there is no saved payment method or the
  // card-admission policy would deny this user's pool create. Stays false
  // while loading or when the lookup fails: the backend policy is still the
  // enforcement point, so the gate fails open on a billing hiccup.
  const [cardRequired, setCardRequired] = useState(false)
  const nameRef = useRef<InputProps.Ref>(null)
  const ttlRef = useRef<InputProps.Ref>(null)

  useEffect(() => {
    if (!billingEnabled || !paymentMethodGateEnabled) return
    let cancelled = false
    billingApi
      .summary()
      .then(summary => {
        if (!cancelled) {
          setCardRequired(
            !summary.payment_method_present ||
              summary.pool_create_card_required === true,
          )
        }
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [billingEnabled, paymentMethodGateEnabled])

  useBeforeUnload(event => {
    if (!dirty || submitting) return
    event.preventDefault()
  })

  const nameError = useMemo(() => {
    if (!name) return submitAttempted ? "Name is required." : undefined
    if (!NAME_PATTERN.test(name)) {
      return "Lowercase letters, digits, and dashes only; no leading/trailing dash."
    }
    return undefined
  }, [name, submitAttempted])

  const ttlError = useMemo(() => {
    if (!ttlSecondsAfterCreated.trim()) return undefined
    const value = Number(ttlSecondsAfterCreated)
    if (!Number.isInteger(value) || value < 0) {
      return "Time to live must be a non-negative whole number."
    }
    return undefined
  }, [ttlSecondsAfterCreated])

  const addService = () => {
    setDirty(true)
    setServices(prev => [...prev, { id: genId(), name: "", targetPort: "", protocol: "TCP" }])
  }

  const updateService = (id: string, field: keyof ServiceEntry, value: string) => {
    setServices(prev => prev.map(s => (s.id === id ? { ...s, [field]: value } : s)))
  }

  const removeService = (id: string) => {
    setDirty(true)
    setServices(prev => prev.filter(s => s.id !== id))
  }

  // Collect the current form into the SDK's pool/template configuration.
  const buildValues = (): PoolTemplateConfig => {
    const validServices = services
      .filter(s => s.name.trim() && s.targetPort)
      .map(s => ({
        name: s.name.trim(),
        targetPort: parseInt(s.targetPort, 10),
        protocol: s.protocol || "TCP",
      }))
    const readPort = parseInt(readinessPort, 10)
    const livePort = parseInt(livenessPort, 10)
    const probes: {
      readinessProbe?: Record<string, unknown>
      livenessProbe?: Record<string, unknown>
    } = {}
    if (readPort) probes.readinessProbe = { tcpSocket: { port: readPort } }
    if (livePort) probes.livenessProbe = { tcpSocket: { port: livePort } }
    return {
      cpu: parseInt(cpu, 10) || DEFAULTS.cpu,
      ram: ram.trim() || DEFAULTS.ram,
      // Trim pasted whitespace: K8s rejects pod images with leading/trailing
      // spaces, which wedges the pool's VM in Starting forever.
      ociImage: ociImage.trim() || DEFAULTS.ociImage,
      firmware: firmware !== "bios" ? firmware : undefined,
      replicas: parseInt(replicas, 10) || DEFAULTS.replicas,
      ttlSecondsAfterCreated: ttlSecondsAfterCreated.trim()
        ? Number(ttlSecondsAfterCreated)
        : undefined,
      services: validServices.length ? validServices : undefined,
      probes: Object.keys(probes).length ? probes : undefined,
      autoscaling: autoscalingEnabled
        ? {
            minPoolSize: parseInt(minPoolSize, 10) || 0,
            initialPoolSize: parseInt(initialPoolSize, 10) || 0,
            maxPoolSize: parseInt(maxPoolSize, 10) || 20,
          }
        : undefined,
    }
  }

  const create = async () => {
    setSubmitAttempted(true)
    if (!name || nameError || ttlError) {
      if (!name || nameError) {
        nameRef.current?.focus()
      } else {
        ttlRef.current?.focus()
      }
      return
    }
    setSubmitting(true)
    try {
      await createPool(name, buildValues())
      flash.push({ type: "success", header: `Created pool ${name}` })
      setDirty(false)
      // Pool name = namespace name (1:1 mapping).
      navigate(`/pools/${name}/${name}`)
    } catch (e) {
      flash.push({
        type: "error",
        header: "Create failed",
        content: errorMessage(e),
      })
    } finally {
      setSubmitting(false)
    }
  }

  const cancel = () => {
    if (dirty) setDiscardOpen(true)
    else navigate("/pools")
  }

  return (
    <PageShell
      eyebrow="Fleet / Pool"
      title={source ? "Duplicate pool" : "New pool"}
      description={
        source
          ? `Duplicating "${source.name}". Edit and save as a new pool.`
          : "Create a new pool."
      }
      secondaryActions={<CuaButton onClick={cancel}>Cancel</CuaButton>}
      primaryAction={
        <CuaButton
          tone="primary"
          loading={submitting}
          disabled={cardRequired}
          disabledReason="Add a payment method in Settings to create pools."
          onClick={create}
        >
          Create
        </CuaButton>
      }
    >
      <SpaceBetween size="l">
      {cardRequired && (
        <Alert
          type="warning"
          header="Payment method required"
          action={
            <CuaButton onClick={() => navigate("/settings")}>
              Add payment method
            </CuaButton>
          }
        >
          Pool creation is disabled because this account has no payment method
          on file. Add a card in Settings, then come back to create your pool.
        </Alert>
      )}
      <Container header={<Header variant="h2">Configuration</Header>}>
      <div onChangeCapture={() => setDirty(true)}>
      <Form>
        <SpaceBetween size="l">
          <FormField
            label="Name"
            description={
              name
                ? `Pool: ${name}`
                : "Lowercase letters, digits, and dashes."
            }
            errorText={nameError}
          >
            <Input
              ref={nameRef}
              value={name}
              onChange={({ detail }) => setName(detail.value)}
              placeholder="my-pool"
            />
          </FormField>

          <FormField label="vCPU cores">
            <Input
              type="number"
              value={cpu}
              onChange={({ detail }) => setCpu(detail.value)}
            />
          </FormField>

          <FormField label="RAM" description="Kubernetes quantity, e.g. 4Gi.">
            <Input
              value={ram}
              onChange={({ detail }) => setRam(detail.value)}
            />
          </FormField>

          <FormField
            label="OCI image"
            description="Workspace containerDisk image."
          >
            <Input
              value={ociImage}
              onChange={({ detail }) => setOciImage(detail.value)}
            />
          </FormField>

          <FormField
            label="Firmware"
            description="UEFI is required for GPT/UEFI-only images like the Windows desktop-workspace; Linux workspace images use BIOS."
          >
            <Select
              selectedOption={FIRMWARE_OPTIONS.find(o => o.value === firmware) ?? FIRMWARE_OPTIONS[0]}
              onChange={({ detail }) => setFirmware((detail.selectedOption.value as "bios" | "efi") ?? "bios")}
              options={FIRMWARE_OPTIONS}
            />
          </FormField>

          <FormField
            label="Replicas"
            description="Static number of pre-warmed VMs the orchestrator keeps. When lane autoscaling is on (below), KEDA takes over this value."
          >
            <Input
              type="number"
              value={replicas}
              onChange={({ detail }) => setReplicas(detail.value)}
            />
          </FormField>

          <FormField
            label="Time to live (seconds)"
            description="Optional. Automatically deletes the pool after this many seconds. Leave blank for no expiration."
            errorText={ttlError}
          >
            <Input
              ref={ttlRef}
              type="number"
              value={ttlSecondsAfterCreated}
              onChange={({ detail }) =>
                setTtlSecondsAfterCreated(detail.value)
              }
              placeholder="3600"
            />
          </FormField>

          <FormField
            label="Autoscaling"
            description="Optional. KEDA scales the pool to its claim demand (the number of claims/VMs in use), and back down when idle (never below Min pool size). Off = the static Replicas above."
          >
            <Toggle
              checked={autoscalingEnabled}
              onChange={({ detail }) => setAutoscalingEnabled(detail.checked)}
            >
              Scale to claim demand
            </Toggle>
          </FormField>

          {autoscalingEnabled && (
            <ColumnLayout columns={2}>
              <FormField
                label="Min pool size"
                description="Durable warm floor (KEDA minReplicaCount). Set > 0 to keep spare VMs so a fresh claim binds without a cold start."
              >
                <Input
                  type="number"
                  value={minPoolSize}
                  onChange={({ detail }) => setMinPoolSize(detail.value)}
                />
              </FormField>
              <FormField
                label="Initial pool size"
                description="One-time warm head-start seeded on create; KEDA then manages the size."
              >
                <Input
                  type="number"
                  value={initialPoolSize}
                  onChange={({ detail }) => setInitialPoolSize(detail.value)}
                />
              </FormField>
              <FormField
                label="Max pool size"
                description="Hard ceiling (KEDA maxReplicaCount)."
              >
                <Input
                  type="number"
                  value={maxPoolSize}
                  onChange={({ detail }) => setMaxPoolSize(detail.value)}
                />
              </FormField>
            </ColumnLayout>
          )}

          <FormField
            label="Readiness port"
            description="TCP port the VM must be listening on before it's marked ready. Default: 5000."
          >
            <Input
              type="number"
              value={readinessPort}
              onChange={({ detail }) => setReadinessPort(detail.value)}
              placeholder="5000"
            />
          </FormField>

          <FormField
            label="Liveness port"
            description="TCP port checked periodically to restart unresponsive VMs. Optional."
          >
            <Input
              type="number"
              value={livenessPort}
              onChange={({ detail }) => setLivenessPort(detail.value)}
              placeholder=""
            />
          </FormField>

          <FormField
            label="Services"
            description="Extra K8s Services created per sandbox. Each maps port 80 to the target port on the VM."
          >
            <SpaceBetween size="s">
              {services.map(svc => (
                <ColumnLayout key={svc.id} columns={4}>
                  <Input
                    value={svc.name}
                    onChange={({ detail }) => updateService(svc.id, "name", detail.value)}
                    placeholder="Service name"
                  />
                  <Input
                    type="number"
                    value={svc.targetPort}
                    onChange={({ detail }) => updateService(svc.id, "targetPort", detail.value)}
                    placeholder="Target port"
                  />
                  <Select
                    selectedOption={PROTOCOL_OPTIONS.find(o => o.value === svc.protocol) ?? PROTOCOL_OPTIONS[0]}
                    onChange={({ detail }) => updateService(svc.id, "protocol", detail.selectedOption.value ?? "TCP")}
                    options={PROTOCOL_OPTIONS}
                  />
                  <CuaButton
                    tone="icon"
                    ariaLabel={`Remove service ${svc.name || "row"}`}
                    iconName="remove"
                    onClick={() => removeService(svc.id)}
                  />
                </ColumnLayout>
              ))}
              {services.length === 0 && (
                <Box color="text-status-inactive">No services defined.</Box>
              )}
              <CuaButton iconName="add-plus" onClick={addService}>
                Add service
              </CuaButton>
            </SpaceBetween>
          </FormField>
        </SpaceBetween>
      </Form>
      </div>
      </Container>
      </SpaceBetween>
      <Modal
        visible={discardOpen}
        onDismiss={() => setDiscardOpen(false)}
        header="Discard changes?"
        footer={
          <Box float="right">
            <SpaceBetween direction="horizontal" size="xs">
              <CuaButton onClick={() => setDiscardOpen(false)}>
                Keep editing
              </CuaButton>
              <CuaButton
                tone="danger"
                onClick={() => {
                  setDirty(false)
                  navigate("/pools")
                }}
              >
                Discard
              </CuaButton>
            </SpaceBetween>
          </Box>
        }
      >
        Your unsaved pool configuration will be lost.
      </Modal>
    </PageShell>
  )
}
