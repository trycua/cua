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
  phase: string
  availableCount: number
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
  createdAt: string
}
