import {
  CreatePoolRequestBuilder,
  CreateTemplateRequestBuilder,
  Firmware,
  OsGymSandboxTemplateSpecBuilder,
  OsGymSandboxWarmPoolSpecBuilder,
  PreservedJson,
  poolDisplayStatus,
  SandboxServiceBuilder,
  SandboxTemplateRefBuilder,
  ServiceProtocol,
  TemplateBuilder,
  VmTemplateBuilder,
  WarmPoolAutoscalingBuilder,
  type CreatePoolRequest,
  type CreateTemplateRequest,
  type CyclopsClient,
  type Namespace,
  type OsGymSandboxWarmPoolSpecBuilderLike,
  type Pool,
  type SandboxService,
  type Template,
  type VmTemplate,
  type VmTemplateBuilderLike,
  type WarmPoolAutoscaling,
  type WarmPoolAutoscalingBuilderLike,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import { ensureSdkInitialized, withClient } from "../auth/cyclops-client"
import { applyPoolTombstones } from "./status"
import type {
  PoolData,
  PoolService,
  PoolSummary,
  PoolTemplateConfig,
} from "./models"
import {
  getLocalVisualPreviewPool,
  isLocalVisualPreview,
  listLocalVisualPreviewPools,
} from "../local-visual-preview"

function serviceProtocol(protocol: ServiceProtocol | undefined): string {
  return protocol === ServiceProtocol.Udp ? "UDP" : "TCP"
}

function sdkProtocol(protocol: string): ServiceProtocol {
  return protocol.toUpperCase() === "UDP"
    ? ServiceProtocol.Udp
    : ServiceProtocol.Tcp
}

function parseProbes(template: Template): PoolTemplateConfig["probes"] {
  const probes = template.spec.vmTemplate.probes
  return probes ? (JSON.parse(probes.toJson()) as PoolTemplateConfig["probes"]) : undefined
}

function poolSummary(pool: Pool): PoolSummary {
  return {
    name: pool.metadata.name,
    namespace: pool.metadata.namespace,
    replicas: pool.spec.replicas,
    availableCount: pool.status?.readyReplicas ?? 0,
    ttlSecondsAfterCreated: pool.spec.ttlSecondsAfterCreated,
    status: poolDisplayStatus(pool),
  }
}

function poolData(pool: Pool, template: Template): PoolData {
  const summary = poolSummary(pool)
  const vm = template.spec.vmTemplate
  const totalCount = pool.status?.replicas ?? pool.spec.replicas
  const availableCount = pool.status?.readyReplicas ?? 0
  return {
    ...summary,
    cpu: vm.cpuCores ?? 0,
    ram: vm.memory ?? "",
    ociImage: vm.containerDiskImage,
    ttlSecondsAfterCreated: pool.spec.ttlSecondsAfterCreated,
    firmware: vm.firmware === Firmware.Efi ? "efi" : undefined,
    services: (vm.services ?? []).map(service => ({
      name: service.name,
      targetPort: service.targetPort,
      protocol: serviceProtocol(service.protocol),
    })),
    probes: parseProbes(template),
    autoscaling: pool.spec.autoscaling,
    totalCount,
    availableCount,
    claimedCount: Math.max(totalCount - availableCount, 0),
  }
}

async function listNamespacesWith(client: CyclopsClient): Promise<Namespace[]> {
  return client.listNamespaces()
}

export async function listNamespaces(): Promise<Namespace[]> {
  if (isLocalVisualPreview()) {
    return [
      {
        name: "preview",
        status: "Active",
        createdAt: "2026-08-16T20:20:00.000Z",
        labels: undefined,
      },
    ]
  }
  return withClient(listNamespacesWith)
}

export async function listPools(): Promise<PoolSummary[]> {
  if (isLocalVisualPreview()) {
    await ensureSdkInitialized()
    return applyPoolTombstones(await listLocalVisualPreviewPools())
  }
  return withClient(async client => {
    const namespaces = await listNamespacesWith(client)
    const pools = await Promise.all(
      namespaces.map(async namespace => {
        try {
          return await client.listPools(namespace.name)
        } catch {
          return []
        }
      }),
    )
    return applyPoolTombstones(
      pools.flat().map(poolSummary).sort((a, b) => a.name.localeCompare(b.name)),
    )
  })
}

async function getPoolWith(
  client: CyclopsClient,
  namespace: string,
  name: string,
): Promise<{ pool: Pool; template: Template }> {
  const pool = await client.getPool(name)
  if (pool.metadata.namespace !== namespace) {
    throw new Error(`Pool ${name} belongs to namespace ${pool.metadata.namespace}`)
  }
  const template = await client.getTemplate(
    namespace,
    pool.spec.sandboxTemplateRef.name,
  )
  return { pool, template }
}

export async function getPool(namespace: string, name: string): Promise<PoolData> {
  if (isLocalVisualPreview()) {
    await ensureSdkInitialized()
    const pool = getLocalVisualPreviewPool(namespace, name)
    if (!pool) throw new Error(`Preview pool ${namespace}/${name} was not found`)
    return pool
  }
  return withClient(async client => {
    const resources = await getPoolWith(client, namespace, name)
    return poolData(resources.pool, resources.template)
  })
}

export function buildSdkServices(
  services: PoolService[] | undefined,
): SandboxService[] | undefined {
  return services?.map(service =>
    new SandboxServiceBuilder()
      .name(service.name)
      .targetPort(service.targetPort)
      .protocol(sdkProtocol(service.protocol))
      .build(),
  )
}

function buildAutoscaling(
  values: PoolTemplateConfig["autoscaling"],
): WarmPoolAutoscaling | undefined {
  if (!values) return undefined

  let builder: WarmPoolAutoscalingBuilderLike =
    new WarmPoolAutoscalingBuilder()
  if (values.minPoolSize !== undefined) {
    builder = builder.minPoolSize(values.minPoolSize)
  }
  if (values.initialPoolSize !== undefined) {
    builder = builder.initialPoolSize(values.initialPoolSize)
  }
  if (values.maxPoolSize !== undefined) {
    builder = builder.maxPoolSize(values.maxPoolSize)
  }
  return builder.build()
}

export function buildPoolRequest(
  namespace: string,
  templateName: string,
  values: PoolTemplateConfig,
): CreatePoolRequest {
  const reference = new SandboxTemplateRefBuilder()
    .name(templateName)
    .build()
  let specBuilder: OsGymSandboxWarmPoolSpecBuilderLike =
    new OsGymSandboxWarmPoolSpecBuilder()
      .replicas(values.replicas)
      .sandboxTemplateRef(reference)
  const autoscaling = buildAutoscaling(values.autoscaling)
  if (autoscaling) specBuilder = specBuilder.autoscaling(autoscaling)
  if (values.ttlSecondsAfterCreated !== undefined) {
    specBuilder = specBuilder.ttlSecondsAfterCreated(
      values.ttlSecondsAfterCreated,
    )
  }

  return new CreatePoolRequestBuilder()
    .namespace(namespace)
    .spec(specBuilder.build())
    .build()
}

export function buildTemplateRequest(
  namespace: string,
  templateName: string,
  values: PoolTemplateConfig,
): CreateTemplateRequest {
  let vmBuilder: VmTemplateBuilderLike = new VmTemplateBuilder()
    .containerDiskImage(values.ociImage)
    .cpuCores(values.cpu)
    .memory(values.ram)
  if (values.firmware === "efi") vmBuilder = vmBuilder.firmware(Firmware.Efi)
  if (values.probes) {
    vmBuilder = vmBuilder.probes(
      PreservedJson.fromJson(JSON.stringify(values.probes)),
    )
  }
  const services = buildSdkServices(values.services)
  if (services) vmBuilder = vmBuilder.services(services)

  const spec = new OsGymSandboxTemplateSpecBuilder()
    .vmTemplate(vmBuilder.build())
    .build()
  return new CreateTemplateRequestBuilder()
    .namespace(namespace)
    .name(templateName)
    .spec(spec)
    .build()
}

function seedVmTemplateBuilder(vm: VmTemplate): VmTemplateBuilderLike {
  let builder: VmTemplateBuilderLike = new VmTemplateBuilder()
    .containerDiskImage(vm.containerDiskImage)
  if (vm.command !== undefined) builder = builder.command(vm.command)
  if (vm.runtime !== undefined) builder = builder.runtime(vm.runtime)
  if (vm.runtimeClassName !== undefined) {
    builder = builder.runtimeClassName(vm.runtimeClassName)
  }
  if (vm.nodeSelector !== undefined) {
    builder = builder.nodeSelector(vm.nodeSelector)
  }
  if (vm.tolerations !== undefined) builder = builder.tolerations(vm.tolerations)
  if (vm.imagePullPolicy !== undefined) {
    builder = builder.imagePullPolicy(vm.imagePullPolicy)
  }
  if (vm.imagePullSecret !== undefined) {
    builder = builder.imagePullSecret(vm.imagePullSecret)
  }
  if (vm.cpuCores !== undefined) builder = builder.cpuCores(vm.cpuCores)
  if (vm.memory !== undefined) builder = builder.memory(vm.memory)
  if (vm.firmware !== undefined) builder = builder.firmware(vm.firmware)
  if (vm.probes !== undefined) builder = builder.probes(vm.probes)
  if (vm.services !== undefined) builder = builder.services(vm.services)
  if (vm.oidc !== undefined) builder = builder.oidc(vm.oidc)
  return builder
}

export function rebuildTemplateWithServices(
  template: Template,
  services: PoolService[],
): Template {
  const vmTemplate = seedVmTemplateBuilder(template.spec.vmTemplate)
    .services(buildSdkServices(services) ?? [])
    .build()
  const spec = new OsGymSandboxTemplateSpecBuilder()
    .vmTemplate(vmTemplate)
    .build()
  return new TemplateBuilder()
    .apiVersion(template.apiVersion)
    .kind(template.kind)
    .metadata(template.metadata)
    .spec(spec)
    .build()
}

export async function createPool(
  name: string,
  values: PoolTemplateConfig,
): Promise<PoolData> {
  return withClient(async client => {
    const templateName = `${name}-template`
    const pool = await client.createPool(
      buildPoolRequest(name, templateName, values),
    )
    try {
      const template = await client.createTemplate(
        buildTemplateRequest(name, templateName, values),
      )
      return poolData(pool, template)
    } catch (error) {
      await client.deletePool(pool).catch(() => undefined)
      throw error
    }
  })
}

export async function updatePoolServices(
  namespace: string,
  name: string,
  services: PoolService[],
): Promise<void> {
  await withClient(async client => {
    const { template } = await getPoolWith(client, namespace, name)
    await client.updateTemplate(rebuildTemplateWithServices(template, services))
  })
}

export async function deletePool(namespace: string, name: string): Promise<void> {
  await withClient(async client => {
    const { pool } = await getPoolWith(client, namespace, name)
    await client.deletePool(pool)
  })
}
