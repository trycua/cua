// Code generated from clusters/base/osgym/crd.yaml; DO NOT EDIT.
export type ClaimSpec = {
  bindDeadline?: number
  lifecycle?: ClaimSpecLifecycle
  sandboxTemplateRef: SandboxTemplateRef
  warmpool?: string
}
export type ClaimSpecLifecycle = {
  autoRenew?: boolean
  shutdownPolicy?: string
  shutdownTime?: string
}
export type PoolSpec = {
  autoscaling?: PoolSpecAutoscaling
  replicas: number
  services?: PoolSpecService[]
  template: PoolTemplate
}
export type PoolSpecAutoscaling = {
  initialPoolSize?: number
  maxPoolSize?: number
  minPoolSize?: number
}
export type PoolSpecService = {
  name: string
  protocol?: PoolSpecServiceProtocol
  targetPort: number
}
export type PoolSpecServiceProtocol = "TCP" | "UDP"
export type PoolTemplate = {
  command?: string[]
  containerDiskImage: string
  cpuCores?: number
  firmware?: PoolTemplateFirmware
  imagePullSecret?: string
  memory?: string
  nodeSelector?: Record<string, string>
  oidc?: PoolTemplateOidc
  probes?: unknown
  runtime?: PoolTemplateRuntime
  runtimeClassName?: string
  tolerations?: unknown[]
}
export type PoolTemplateFirmware = "bios" | "efi"
export type PoolTemplateOidc = {
  awsRegion?: string
  awsRoleArn?: string
  credentialsSecret: string
  refreshIntervalSeconds?: number
  tokenUrl: string
}
export type PoolTemplateRuntime = "kubevirt" | "macos" | "gvisor"
export type SandboxTemplateRef = {
  name: string
}
