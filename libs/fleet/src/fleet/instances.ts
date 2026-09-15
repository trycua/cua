import { getToken } from "../auth/keycloak"
import { isLocalVisualPreview } from "../local-visual-preview"
import type { InstancePod, PoolInstance } from "./models"

const GUEST_CONSOLE_CONTAINER = "guest-console-log"
const WARM_POOL_LABEL = "osgym.cua.ai/warmpool"
const ORIGIN_WARM_POOL_ANNOTATION = "osgym.cua.ai/origin-warmpool"
const ORIGIN_WARM_POOL_NAMESPACE_ANNOTATION =
  "osgym.cua.ai/origin-warmpool-namespace"

interface SandboxResource {
  metadata?: {
    name?: string
    namespace?: string
    creationTimestamp?: string
    labels?: Record<string, string>
    annotations?: Record<string, string>
    ownerReferences?: Array<{
      kind?: string
      name?: string
      controller?: boolean
    }>
  }
  spec?: {
    vmTemplate?: {
      runtime?: string
    }
  }
  status?: {
    phase?: string
    runtime?: string
    ready?: boolean
    vmName?: string
    message?: string
  }
}

interface SandboxResourceList {
  items?: SandboxResource[]
}

interface PodResource {
  metadata?: {
    name?: string
    namespace?: string
    creationTimestamp?: string
  }
  spec?: {
    nodeName?: string
    containers?: Array<{ name?: string }>
    initContainers?: Array<{ name?: string }>
    ephemeralContainers?: Array<{ name?: string }>
  }
  status?: {
    phase?: string
  }
}

interface PodResourceList {
  items?: PodResource[]
}

class KubernetesApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message)
  }
}

async function k8sRequest(path: string): Promise<Response> {
  const token = await getToken()
  return fetch(`/api/k8s/${path}`, {
    headers: { Authorization: token ? `Bearer ${token}` : "" },
  })
}

async function k8sJson<T>(path: string): Promise<T> {
  const response = await k8sRequest(path)
  if (!response.ok) {
    const body = await response.text()
    throw new KubernetesApiError(
      body || `Kubernetes request failed: ${response.status}`,
      response.status,
    )
  }
  return response.json() as Promise<T>
}

function belongsToPool(
  sandbox: SandboxResource,
  namespace: string,
  poolName: string,
): boolean {
  const metadata = sandbox.metadata
  const labels = metadata?.labels ?? {}
  const annotations = metadata?.annotations ?? {}
  const originNamespace = annotations[ORIGIN_WARM_POOL_NAMESPACE_ANNOTATION]

  return (
    (labels[WARM_POOL_LABEL] === poolName ||
      annotations[ORIGIN_WARM_POOL_ANNOTATION] === poolName) &&
    (!originNamespace || originNamespace === namespace)
  )
}

function instanceModel(sandbox: SandboxResource): PoolInstance | null {
  const metadata = sandbox.metadata
  if (!metadata?.name || !metadata.namespace) return null

  const claimName = metadata.ownerReferences?.find(
    reference =>
      reference.kind === "OSGymSandboxClaim" && reference.controller === true,
  )?.name

  return {
    name: metadata.name,
    namespace: metadata.namespace,
    phase: sandbox.status?.phase ?? "Pending",
    runtime: sandbox.status?.runtime ?? sandbox.spec?.vmTemplate?.runtime,
    vmName: sandbox.status?.vmName,
    ready: sandbox.status?.ready,
    message: sandbox.status?.message,
    claimName,
    createdAt: metadata.creationTimestamp ?? "",
  }
}

function podModel(pod: PodResource): InstancePod | null {
  const metadata = pod.metadata
  if (!metadata?.name || !metadata.namespace) return null

  const containers = [
    ...(pod.spec?.initContainers ?? []),
    ...(pod.spec?.containers ?? []),
    ...(pod.spec?.ephemeralContainers ?? []),
  ]
    .map(container => container.name)
    .filter((name): name is string => Boolean(name))

  return {
    name: metadata.name,
    namespace: metadata.namespace,
    phase: pod.status?.phase ?? "Unknown",
    nodeName: pod.spec?.nodeName,
    createdAt: metadata.creationTimestamp ?? "",
    containers,
  }
}

export async function listPoolInstances(
  namespace: string,
  poolName: string,
): Promise<PoolInstance[]> {
  if (isLocalVisualPreview()) return []

  const list = await k8sJson<SandboxResourceList>(
    `apis/osgym.cua.ai/v1alpha1/namespaces/${encodeURIComponent(namespace)}/osgymsandboxes`,
  )
  return (list.items ?? [])
    .filter(sandbox => belongsToPool(sandbox, namespace, poolName))
    .map(instanceModel)
    .filter((instance): instance is PoolInstance => instance !== null)
}

export async function getPoolInstance(
  namespace: string,
  instanceName: string,
): Promise<PoolInstance> {
  const sandbox = await k8sJson<SandboxResource>(
    `apis/osgym.cua.ai/v1alpha1/namespaces/${encodeURIComponent(namespace)}/osgymsandboxes/${encodeURIComponent(instanceName)}`,
  )
  const instance = instanceModel(sandbox)
  if (!instance) throw new Error("Instance response is missing metadata")
  return instance
}

async function getPodByName(
  namespace: string,
  podName: string,
): Promise<InstancePod | null> {
  try {
    const pod = await k8sJson<PodResource>(
      `api/v1/namespaces/${encodeURIComponent(namespace)}/pods/${encodeURIComponent(podName)}`,
    )
    return podModel(pod)
  } catch (error) {
    if (error instanceof KubernetesApiError && error.status === 404) return null
    throw error
  }
}

async function getKubeVirtPod(
  namespace: string,
  vmName: string,
): Promise<InstancePod | null> {
  const selector = encodeURIComponent(`kubevirt.io/domain=${vmName}`)
  const list = await k8sJson<PodResourceList>(
    `api/v1/namespaces/${encodeURIComponent(namespace)}/pods?labelSelector=${selector}`,
  )
  const pods = (list.items ?? [])
    .map(podModel)
    .filter((pod): pod is InstancePod => pod !== null)
  return pods.find(pod => pod.phase === "Running") ?? pods[0] ?? null
}

export async function getInstancePod(
  instance: PoolInstance,
): Promise<InstancePod | null> {
  const workloadName = instance.vmName ?? instance.name
  if (instance.runtime === "macos" || instance.runtime === "gvisor") {
    return getPodByName(instance.namespace, workloadName)
  }

  const kubeVirtPod = await getKubeVirtPod(instance.namespace, workloadName)
  if (kubeVirtPod) return kubeVirtPod

  // Older resources may not have status.runtime populated yet.
  return getPodByName(instance.namespace, workloadName)
}

export async function getInstanceLogs(
  pod: InstancePod,
  tailLines = 1000,
): Promise<string> {
  const query = new URLSearchParams({
    container: GUEST_CONSOLE_CONTAINER,
    tailLines: String(tailLines),
    timestamps: "true",
  })
  const response = await k8sRequest(
    `api/v1/namespaces/${encodeURIComponent(pod.namespace)}/pods/${encodeURIComponent(pod.name)}/log?${query}`,
  )
  if (!response.ok) {
    const body = await response.text()
    throw new Error(body || `Log request failed: ${response.status}`)
  }
  return response.text()
}
