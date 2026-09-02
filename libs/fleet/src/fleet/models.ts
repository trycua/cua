import type { PoolDisplayStatus } from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"

export interface PoolService {
  name: string
  targetPort: number
  protocol: string
}

export interface PoolAutoscalingConfig {
  minPoolSize?: number
  initialPoolSize?: number
  maxPoolSize?: number
}

export interface PoolTemplateConfig {
  cpu: number
  ram: string
  ociImage: string
  replicas: number
  ttlSecondsAfterCreated?: number
  firmware?: "bios" | "efi"
  services?: PoolService[]
  probes?: {
    readinessProbe?: Record<string, unknown>
    livenessProbe?: Record<string, unknown>
  }
  autoscaling?: PoolAutoscalingConfig
}

export interface PoolSummary {
  name: string
  namespace: string
  replicas: number
  availableCount: number
  ttlSecondsAfterCreated?: number
  status: PoolDisplayStatus
}

export interface PoolData extends PoolSummary, PoolTemplateConfig {
  services: PoolService[]
  totalCount: number
  claimedCount: number
}

export interface Claim {
  name: string
  namespace: string
  templateRef: string
  warmpool: string
  phase: string
  sandboxName?: string
  sandboxService?: string
  ttlSecondsAfterCreated?: number
  createdAt: string
}

export interface PoolInstance {
  name: string
  namespace: string
  phase: string
  runtime?: string
  vmName?: string
  ready?: boolean
  message?: string
  claimName?: string
  createdAt: string
}

export interface InstancePod {
  name: string
  namespace: string
  phase: string
  nodeName?: string
  createdAt: string
  containers: string[]
}
