import { getToken } from "../auth/keycloak"
import { isLocalVisualPreview } from "../local-visual-preview"
import type { PoolInstance } from "./models"

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
  status?: {
    phase?: string
  }
}

interface SandboxResourceList {
  items?: SandboxResource[]
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
    claimName,
    createdAt: metadata.creationTimestamp ?? "",
  }
}

export async function listPoolInstances(
  namespace: string,
  poolName: string,
): Promise<PoolInstance[]> {
  if (isLocalVisualPreview()) return []

  const token = await getToken()
  const response = await fetch(
    `/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/${encodeURIComponent(namespace)}/osgymsandboxes`,
    { headers: { Authorization: token ? `Bearer ${token}` : "" } },
  )
  if (!response.ok) {
    const body = await response.text()
    throw new Error(body || `instance request failed: ${response.status}`)
  }

  const list = (await response.json()) as SandboxResourceList
  return (list.items ?? [])
    .filter(sandbox => belongsToPool(sandbox, namespace, poolName))
    .map(instanceModel)
    .filter((instance): instance is PoolInstance => instance !== null)
}
