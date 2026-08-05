import {
  Firmware,
  PreservedJson,
  ServiceProtocol,
  type CyclopsClient,
  type Namespace,
  type Pool,
  type Template,
} from "./generated"
import { withClient } from "./client"
import { deriveWarmPoolPhase } from "./status"
import type {
  PoolData,
  PoolService,
  PoolSummary,
  PoolTemplateConfig,
} from "./models"

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
    phase: deriveWarmPoolPhase(pool.spec.replicas, pool.status),
    availableCount: pool.status?.readyReplicas ?? 0,
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
  return withClient(listNamespacesWith)
}

export async function listPools(): Promise<PoolSummary[]> {
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
    return pools.flat().map(poolSummary).sort((a, b) => a.name.localeCompare(b.name))
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
  return withClient(async client => {
    const resources = await getPoolWith(client, namespace, name)
    return poolData(resources.pool, resources.template)
  })
}

function sdkServices(services: PoolService[] | undefined) {
  return services?.map(service => ({
    name: service.name,
    targetPort: service.targetPort,
    protocol: sdkProtocol(service.protocol),
  }))
}

export async function createPool(
  name: string,
  values: PoolTemplateConfig,
): Promise<PoolData> {
  return withClient(async client => {
    const templateName = `${name}-template`
    const pool = await client.createPool({
      namespace: name,
      spec: {
        replicas: values.replicas,
        sandboxTemplateRef: { name: templateName },
        autoscaling: values.autoscaling,
      },
    })
    try {
      const template = await client.createTemplate({
        namespace: name,
        name: templateName,
        spec: {
          vmTemplate: {
            containerDiskImage: values.ociImage,
            imagePullSecret: "ecr-credentials",
            cpuCores: values.cpu,
            memory: values.ram,
            firmware: values.firmware === "efi" ? Firmware.Efi : undefined,
            probes: values.probes
              ? PreservedJson.fromJson(JSON.stringify(values.probes))
              : undefined,
            services: sdkServices(values.services),
          },
        },
      })
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
    template.spec.vmTemplate.services = sdkServices(services)
    await client.updateTemplate(template)
  })
}

export async function deletePool(namespace: string, name: string): Promise<void> {
  await withClient(async client => {
    const { pool } = await getPoolWith(client, namespace, name)
    await client.deletePool(pool)
  })
}
