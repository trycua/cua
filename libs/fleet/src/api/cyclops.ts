// © 2024 CUA - gateway proxy
// API client for the cyclops-cs SPA. All calls go through the
// kubectl-proxy sidecar (/k8s-api/) or the nginx orch-api proxy
// (/orch-api/).

// ── K8s API types (only the bits we use) ────────────────────────────────

export interface PodContainerStatus {
  name: string
  ready?: boolean
  restartCount?: number
  state?: {
    waiting?: { reason?: string; message?: string }
    running?: { startedAt?: string }
    terminated?: {
      exitCode?: number
      reason?: string
      message?: string
      startedAt?: string
      finishedAt?: string
    }
  }
}

export interface PodSummary {
  metadata: {
    name: string
    namespace: string
    labels?: Record<string, string>
    creationTimestamp?: string
  }
  spec?: {
    nodeName?: string
    containers?: { name: string }[]
  }
  status?: {
    phase?: string
    podIP?: string
    startTime?: string
    containerStatuses?: PodContainerStatus[]
  }
}

export interface PodList {
  items: PodSummary[]
}

export interface NodeSummary {
  metadata: {
    name: string
    labels?: Record<string, string>
    creationTimestamp?: string
  }
  status?: {
    capacity?: Record<string, string>
    allocatable?: Record<string, string>
    nodeInfo?: {
      kubeletVersion?: string
      osImage?: string
      architecture?: string
    }
    conditions?: { type: string; status: string }[]
  }
  spec?: {
    taints?: { key: string; effect: string }[]
    unschedulable?: boolean
  }
}

export interface NodeList {
  items: NodeSummary[]
}

// Pool VMs only schedule onto kubevirt worker nodes. Use this to scope any
// node listing or capacity math to the nodes that can actually host pools.
export const POOL_WORKER_NODE_PREFIX = "kubevirt-k3s-worker-"
export function isPoolWorkerNode(n: NodeSummary): boolean {
  return n.metadata.name.startsWith(POOL_WORKER_NODE_PREFIX)
}

export interface PodMetricsContainer {
  name: string
  usage: { cpu: string; memory: string } // e.g. "159687357n", "1450060Ki"
}

export interface PodMetrics {
  metadata: {
    name: string
    namespace: string
    timestamp?: string
    window?: string
  }
  containers: PodMetricsContainer[]
}

// One row from the osgym orchestrator's `GET /pool` (osgym/main.py).
export interface PoolVm {
  name: string
  // KubeVirt VM `printableStatus` — observed: "Running" | "Stopped" |
  // "Provisioning" | "Starting" | "Stopping" | "Terminating" | "Unknown".
  phase: string
  claimed_by: string | null
  available: boolean
}

export interface OrchestratorPoolStatus {
  count: number
  vms: PoolVm[]
}

// One row from osgym/observability.py's `GET /events`. Backed by a Redis
// LIST capped at 2000 entries.
export interface OrchestratorEvent {
  ts: number // unix seconds (float)
  method: string
  path: string
  status: number
  duration_ms: number
  vm_id: string | null
}

export interface OrchestratorEventsResponse {
  events: OrchestratorEvent[]
  pool: string
  key: string
}

// Prometheus HTTP API `/api/v1/query` response shape (only the bits we use).
export interface PromQueryResult {
  metric: Record<string, string>
  value: [number, string] // [unix_secs, sample_value_as_string]
}

export interface PromQueryResponse {
  status: "success" | "error"
  data: { resultType: "vector" | "scalar" | "matrix" | "string"; result: PromQueryResult[] }
  errorType?: string
  error?: string
}

export interface KubernetesEvent {
  metadata: {
    name: string
    namespace: string
    uid?: string
    creationTimestamp?: string
  }
  involvedObject: {
    kind: string
    name: string
    namespace?: string
  }
  reason?: string
  message?: string
  type?: string // "Normal" | "Warning"
  count?: number
  firstTimestamp?: string
  lastTimestamp?: string
  eventTime?: string
  source?: { component?: string; host?: string }
  reportingComponent?: string
  reportingInstance?: string
}

export interface KubernetesEventList {
  items: KubernetesEvent[]
}

async function fetchJson<T>(url: string, init?: RequestInit): Promise<T> {
  // Inject Authorization: Bearer ${kc.token} for backend-mediated paths
  // (everything served by the cyclops-cs sidecar).
  const authHeader: Record<string, string> = {}
  if (
    url.startsWith("/api/keys") ||
    url.startsWith("/api/user-keys") ||
    url.startsWith("/api/gateway") ||
    url.startsWith("/api/k8s") ||
    url.startsWith("/api/orch") ||
    url.startsWith("/api/namespaces") ||
    url.startsWith("/api/pool-templates")
  ) {
    const { getToken } = await import("../auth/keycloak")
    const token = await getToken()
    if (token) authHeader.Authorization = `Bearer ${token}`
  }
  const res = await fetch(url, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...authHeader,
      ...(init?.headers ?? {}),
    },
  })
  if (!res.ok) {
    const body = await res.text().catch(() => "")
    throw new Error(`${res.status} ${res.statusText}${body ? `: ${body}` : ""}`)
  }
  // Some endpoints return empty bodies on success.
  const text = await res.text()
  return text ? (JSON.parse(text) as T) : (undefined as T)
}

// `/api/orch/<ns>/<svc>/<path>` → osgym orchestrator, via the cyclops-cs
// backend sidecar (Keycloak SSO + OPA). Replaces the older /orch-api/
// nginx block; fetchJson injects Authorization: Bearer ${kc.token}.
const httpOrch = <T>(
  namespace: string,
  service: string,
  path: string,
  init?: RequestInit,
) =>
  fetchJson<T>(
    `/api/orch/${encodeURIComponent(namespace)}/` +
      `${encodeURIComponent(service)}/${path.replace(/^\//, "")}`,
    init,
  )

// `/api/k8s/<path>` → kubectl-proxy sidecar via the cyclops-cs backend
// (Keycloak SSO + OPA). Replaces /k8s-api/. Used for arbitrary K8s API
// reads — pod listing by label, PodMetrics, etc. The cyclops-cs
// ClusterRole on the pod's ServiceAccount limits what's reachable.
const httpK8s = <T>(path: string, init?: RequestInit) =>
  fetchJson<T>(`/api/k8s/${path.replace(/^\//, "")}`, init)

export const api = {
  // Pool CRUD via K8s API (kubectl-proxy sidecar with impersonation).
  // The proxy adds Impersonate-User/Group headers so Capsule enforces
  // tenant RBAC — users can only operate in their own namespaces.
  // The pool-operator watches OSGymWorkspacePool CRs and creates all
  // resources (orchestrator, RBAC, namespace, etc.) automatically.
  createPool: async (name: string, values: {
    cpu: number; ram: string; ociImage: string; replicas: number;
    firmware?: "bios" | "efi";
    // runtime=macos provisions uvisor macOS sandboxes (ADMIN-ONLY; the backend
    // rejects macos pool writes from non-admins). Omitted/kubevirt = a VM pool.
    runtime?: "kubevirt" | "macos" | "gvisor";
    services?: { name: string; targetPort: number; protocol: string }[];
    probes?: { readinessProbe?: Record<string, unknown>; livenessProbe?: Record<string, unknown> };
    autoscaling?: PoolAutoscalingConfig;
  }) => {
    // Pool name = namespace name (1:1 mapping).
    // Auto-create the namespace before creating the pool. Tolerate "already
    // exists": the namespace can be left over from a previous attempt that
    // failed at the pool step (e.g. an adoption race), and create-or-reuse
    // makes pool creation retryable under the same name.
    const namespace = name
    try {
      await namespacesApi.create(namespace)
    } catch (e) {
      if (!String((e as Error).message).startsWith("409")) throw e
    }
    return httpK8s<void>(
      `apis/cua.ai/v1/namespaces/${encodeURIComponent(namespace)}/osgymworkspacepools`,
      {
        method: "POST",
        body: JSON.stringify({
          apiVersion: "cua.ai/v1",
          kind: "OSGymWorkspacePool",
          metadata: { name, labels: { "cua.ai/pool": name } },
          spec: {
            replicas: values.replicas,
            template: {
              // A pod runtime (macos / gvisor) flows verbatim through the compat
              // shim into the OSGymSandboxTemplate vmTemplate, where the operator's
              // pod_backend dispatches on runtime and applies that runtime's
              // placement defaults (RuntimeClass, nodeSelector, tolerations).
              ...(values.runtime && values.runtime !== "kubevirt"
                ? { runtime: values.runtime }
                : {}),
              containerDiskImage: values.ociImage,
              imagePullSecret: "ecr-credentials",
              cpuCores: values.cpu,
              memory: values.ram,
              ...(values.firmware && values.firmware !== "bios" ? { firmware: values.firmware } : {}),
              ...(values.probes ? { probes: values.probes } : {}),
            },
            ...(values.services?.length ? { services: values.services } : {}),
            ...(values.autoscaling ? { autoscaling: values.autoscaling } : {}),
          },
        }),
      },
    )
  },

  updatePoolServices: (namespace: string, name: string, services: { name: string; targetPort: number; protocol: string }[]) =>
    httpK8s<void>(
      `apis/cua.ai/v1/namespaces/${encodeURIComponent(namespace)}/osgymworkspacepools/${encodeURIComponent(name)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/merge-patch+json" },
        body: JSON.stringify({ spec: { services } }),
      },
    ),

  deletePool: (namespace: string, name: string) =>
    httpK8s<void>(
      `apis/cua.ai/v1/namespaces/${encodeURIComponent(namespace)}/osgymworkspacepools/${encodeURIComponent(name)}`,
      { method: "DELETE" },
    ),

  getPool: async (namespace: string, name: string): Promise<{
    name: string
    namespace: string
    replicas: number
    cpu: number
    ram: string
    ociImage: string
    firmware?: "bios" | "efi"
    services: { name: string; targetPort: number; protocol: string }[]
    probes?: { readinessProbe?: Record<string, unknown>; livenessProbe?: Record<string, unknown> }
    phase: string
    totalCount: number
    availableCount: number
    claimedCount: number
  }> => {
    const pool = await httpK8s<{
      metadata: { name: string; namespace: string }
      spec: {
        replicas: number
        template: { containerDiskImage: string; cpuCores: number; memory: string; firmware?: string; probes?: Record<string, unknown> }
        services?: { name: string; targetPort: number; protocol?: string }[]
      }
      status?: {
        phase?: string
        totalCount?: number
        availableCount?: number
        claimedCount?: number
      }
    }>(`apis/cua.ai/v1/namespaces/${encodeURIComponent(namespace)}/osgymworkspacepools/${encodeURIComponent(name)}`)
    return {
      name: pool.metadata.name,
      namespace: pool.metadata.namespace,
      replicas: pool.spec.replicas,
      cpu: pool.spec.template.cpuCores,
      ram: pool.spec.template.memory,
      ociImage: pool.spec.template.containerDiskImage,
      firmware: pool.spec.template.firmware as "bios" | "efi" | undefined,
      services: (pool.spec.services ?? []).map(s => ({
        name: s.name,
        targetPort: s.targetPort,
        protocol: s.protocol ?? "TCP",
      })),
      probes: pool.spec.template.probes as { readinessProbe?: Record<string, unknown>; livenessProbe?: Record<string, unknown> } | undefined,
      phase: pool.status?.phase ?? "Unknown",
      totalCount: pool.status?.totalCount ?? 0,
      availableCount: pool.status?.availableCount ?? 0,
      claimedCount: pool.status?.claimedCount ?? 0,
    }
  },

  listPools: async (): Promise<{
    name: string
    namespace: string
    replicas: number
    phase: string
    availableCount: number
  }[]> => {
    // Capsule restricts users to their own namespaces — cluster-wide list
    // is forbidden. Instead, fetch the user's namespaces and list pools
    // in each one.
    const namespaces = await namespacesApi.list()
    const pools: {
      name: string
      namespace: string
      replicas: number
      phase: string
      availableCount: number
    }[] = []
    await Promise.all(
      namespaces.map(async (ns) => {
        try {
          const list = await httpK8s<{
            items: {
              metadata: { name: string; namespace: string }
              spec: { replicas: number }
              status?: { phase?: string; availableCount?: number }
            }[]
          }>(`apis/cua.ai/v1/namespaces/${encodeURIComponent(ns.name)}/osgymworkspacepools`)
          for (const p of list.items) {
            pools.push({
              name: p.metadata.name,
              namespace: p.metadata.namespace,
              replicas: p.spec.replicas,
              phase: p.status?.phase ?? "Unknown",
              availableCount: p.status?.availableCount ?? 0,
            })
          }
        } catch {
          // Namespace may not have pool CRD access — skip
        }
      }),
    )
    return pools.sort((a, b) => a.name.localeCompare(b.name))
  },

  // Per-pool orchestrator (osgym FastAPI inside the pool's namespace).
  listPoolVms: (namespace: string, poolName: string) =>
    httpOrch<OrchestratorPoolStatus>(
      namespace,
      `${poolName}-orchestrator`,
      "/pool",
    ),

  listOrchestratorEvents: (
    namespace: string,
    poolName: string,
    limit = 200,
  ) =>
    httpOrch<OrchestratorEventsResponse>(
      namespace,
      `${poolName}-orchestrator`,
      `/events?limit=${limit}`,
    ),

  // Shared Prom in cyclops-cs ns; Service listens on port 80 → 9090, so
  // the existing /orch-api/<ns>/<svc>/<path> proxy hits it unchanged.
  queryProm: (promql: string) =>
    httpOrch<PromQueryResponse>(
      "cyclops-cs",
      "prometheus",
      `/api/v1/query?query=${encodeURIComponent(promql)}`,
    ),

  // K8s direct (via kubectl-proxy sidecar). The replica detail page uses
  // these to find the virt-launcher pod for a given VM and read its metrics.
  findLauncherPod: async (
    namespace: string,
    vmName: string,
  ): Promise<PodSummary | null> => {
    const list = await httpK8s<PodList>(
      `api/v1/namespaces/${encodeURIComponent(namespace)}/pods` +
        `?labelSelector=${encodeURIComponent(`vm.kubevirt.io/name=${vmName}`)}` +
        "&limit=1",
    )
    return list.items[0] ?? null
  },

  // Locate the orchestrator (osgym) pod for a pool. The orchestrator
  // Deployment sets `app=<poolName>-orchestrator` (see
  // charts/osgym-workspace-test/templates/orchestrator-deploy.yaml).
  findPoolOrchestratorPod: async (
    namespace: string,
    poolName: string,
  ): Promise<PodSummary | null> => {
    const list = await httpK8s<PodList>(
      `api/v1/namespaces/${encodeURIComponent(namespace)}/pods` +
        `?labelSelector=${encodeURIComponent(`app=${poolName}-orchestrator`)}` +
        "&limit=1",
    )
    return list.items[0] ?? null
  },

  // Locate the centralized pool-operator pod. Deployed cluster-wide in the
  // `osgym` namespace with label `app=osgym-pool-operator` (see
  // clusters/kopf-k3s/osgym/pool-operator.yaml).
  findPoolOperatorPod: async (): Promise<PodSummary | null> => {
    const list = await httpK8s<PodList>(
      "api/v1/namespaces/osgym/pods" +
        `?labelSelector=${encodeURIComponent("app=osgym-pool-operator")}` +
        "&limit=1",
    )
    return list.items[0] ?? null
  },

  listNodes: () => httpK8s<NodeList>("api/v1/nodes"),

  // Cluster-wide list of KubeVirt launcher pods (one per VM replica).
  // Filters by the `vm.kubevirt.io/name` label so we only return virt-launcher
  // pods, not the orchestrator or other workloads.
  listVmPods: () =>
    httpK8s<PodList>(
      `api/v1/pods?labelSelector=${encodeURIComponent("vm.kubevirt.io/name")}`,
    ),

  getPodMetrics: (namespace: string, podName: string) =>
    httpK8s<PodMetrics>(
      "apis/metrics.k8s.io/v1beta1/namespaces/" +
        `${encodeURIComponent(namespace)}/pods/${encodeURIComponent(podName)}`,
    ),

  // K8s events for OSGymWorkspacePool resources — what the pool-operator
  // emits via kopf for handler outcomes (Normal/Warning, reason, message).
  // Cluster-wide; filtered server-side by involvedObject.kind.
  listOperatorEvents: () =>
    httpK8s<KubernetesEventList>(
      `api/v1/events?fieldSelector=${encodeURIComponent("involvedObject.kind=OSGymWorkspacePool")}&limit=500`,
    ),

  // Stream pod logs via K8s API (follow mode). Returns the URL for
  // fetch() with ReadableStream — the caller reads lines incrementally.
  // Routed through the cyclops-cs backend sidecar (Keycloak SSO + OPA),
  // which proxies to the kubectl-proxy sidecar.
  podLogsUrl: (namespace: string, podName: string, container: string) =>
    `/api/k8s/api/v1/namespaces/${encodeURIComponent(namespace)}/pods/` +
    `${encodeURIComponent(podName)}/log?container=${encodeURIComponent(container)}` +
    "&follow=true&tailLines=200",

  // Restart a KubeVirt VM replica via the subresources API.
  // This is equivalent to `virtctl restart <vm>` — the VMI is
  // terminated and a new one is started from the same VM spec.
  // Requires the subresources.kubevirt.io/virtualmachines/restart ClusterRole
  // rule added in clusters/kopf-k3s/cyclops-cs/rbac.yaml.
  restartVm: (namespace: string, vmName: string) =>
    httpK8s<void>(
      `apis/subresources.kubevirt.io/v1/namespaces/` +
        `${encodeURIComponent(namespace)}/virtualmachines/` +
        `${encodeURIComponent(vmName)}/restart`,
      { method: "PUT", body: JSON.stringify({}) },
    ),

  // Rollout-restart the pool's orchestrator Deployment.
  // Equivalent to `kubectl rollout restart deployment/<poolName>-orchestrator`.
  // Patches spec.template.metadata.annotations with a restartedAt timestamp
  // so K8s triggers a new rollout without changing the Deployment spec.
  // Requires apps/deployments patch permission in the ClusterRole.
  restartOrchestrator: (namespace: string, poolName: string) =>
    httpK8s<void>(
      `apis/apps/v1/namespaces/${encodeURIComponent(namespace)}/` +
        `deployments/${encodeURIComponent(`${poolName}-orchestrator`)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/strategic-merge-patch+json" },
        body: JSON.stringify({
          spec: {
            template: {
              metadata: {
                annotations: {
                  "kubectl.kubernetes.io/restartedAt": new Date().toISOString(),
                },
              },
            },
          },
        }),
      },
    ),
}

// ── Namespace types ────────────────────────────────────────────────────

export interface Namespace {
  name: string
  status: string
  createdAt: string
  labels: Record<string, string> | null
}

// `/api/namespaces` — Capsule-backed per-user namespace CRUD.
const httpNamespaces = <T>(path: string, init?: RequestInit) =>
  fetchJson<T>(`/api/namespaces${path}`, init)

export const namespacesApi = {
  list: () => httpNamespaces<Namespace[]>(""),

  create: (name: string) =>
    httpNamespaces<Namespace>("", {
      method: "POST",
      body: JSON.stringify({ name }),
    }),

  remove: (name: string) =>
    httpNamespaces<void>(`/${encodeURIComponent(name)}`, {
      method: "DELETE",
    }),
}

// ── Pool templates ─────────────────────────────────────────────────────
//
// Per-user saved pool configs, persisted in Redis by the backend
// (/api/pool-templates). The stored `config` is exactly the object the SPA
// passes to api.createPool — so creating a pool from a template just feeds
// it back into the New pool form.

// Optional autoscaling extension. Threaded verbatim onto the pool's
// generated OSGymSandboxWarmPool.spec.autoscaling by the compat shim,
// opting the pool into KEDA autoscaling on claim demand.
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
  // VM firmware: "efi" for GPT/UEFI-only images (e.g. the Windows
  // desktop-workspace); omitted/"bios" boots KubeVirt's legacy-BIOS default.
  firmware?: "bios" | "efi"
  // runtime=macos provisions uvisor macOS sandboxes (admin-only). Omitted =
  // a KubeVirt VM pool (the default for all customer pools).
  runtime?: "kubevirt" | "macos" | "gvisor"
  services?: { name: string; targetPort: number; protocol: string }[]
  probes?: {
    readinessProbe?: Record<string, unknown>
    livenessProbe?: Record<string, unknown>
  }
  autoscaling?: PoolAutoscalingConfig
}

export interface PoolTemplate {
  user: string
  name: string
  createdAt: string
  config: PoolTemplateConfig
}

const httpPoolTemplates = <T>(path: string, init?: RequestInit) =>
  fetchJson<T>(`/api/pool-templates${path}`, init)

export const poolTemplatesApi = {
  list: async (): Promise<PoolTemplate[]> => {
    const res = await httpPoolTemplates<PoolTemplate[]>("")
    return res ?? []
  },

  create: (name: string, config: PoolTemplateConfig) =>
    httpPoolTemplates<PoolTemplate>("", {
      method: "POST",
      body: JSON.stringify({ name, config }),
    }),

  remove: (name: string) =>
    httpPoolTemplates<void>(`/${encodeURIComponent(name)}`, {
      method: "DELETE",
    }),
}

// ── User API keys ─────────────────────────────────────────────────────

export interface UserApiKey {
  id: string
  client_id: string
  name: string
  scope: string[]
}

export interface NewUserApiKey {
  client_id: string
  client_secret: string
  token_url: string
  name: string
  scope: string[]
}

export const userKeysApi = {
  list: async (): Promise<UserApiKey[]> => {
    const res = await fetchJson<{ keys: UserApiKey[] }>("/api/user-keys")
    return res.keys ?? []
  },

  create: async (name: string, scope?: string[]): Promise<NewUserApiKey> => {
    return fetchJson<NewUserApiKey>("/api/user-keys", {
      method: "POST",
      body: JSON.stringify({ name, scope: scope ?? [] }),
    })
  },

  remove: async (id: string): Promise<void> => {
    await fetchJson<void>(`/api/user-keys/${encodeURIComponent(id)}`, {
      method: "DELETE",
    })
  },
}

// ── OSGymSandboxClaim (osgym.cua.ai/v1alpha1) ────────────────────────

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

function parseClaim(raw: any): Claim {
  const meta = raw.metadata || {}
  const spec = raw.spec || {}
  const status = raw.status || {}
  return {
    name: meta.name ?? "",
    namespace: meta.namespace ?? "",
    templateRef: (spec.sandboxTemplateRef || {}).name ?? "",
    warmpool: spec.warmpool ?? "default",
    phase: status.phase ?? "Pending",
    sandboxName: (status.sandbox || {}).name,
    sandboxService: (status.sandbox || {}).service,
    createdAt: meta.creationTimestamp ?? "",
  }
}

const CLAIM_GROUP = "osgym.cua.ai"
const CLAIM_VERSION = "v1alpha1"
const CLAIM_PLURAL = "osgymsandboxclaims"

export const claimsApi = {
  list: async (namespace: string): Promise<Claim[]> => {
    const data = await httpK8s<{ items: any[] }>(
      `apis/${CLAIM_GROUP}/${CLAIM_VERSION}/namespaces/${encodeURIComponent(namespace)}/${CLAIM_PLURAL}`,
    )
    return (data.items || []).map(parseClaim)
  },

  create: async (
    namespace: string,
    name: string,
    templateRef: string,
  ): Promise<Claim> => {
    const raw = await httpK8s<any>(
      `apis/${CLAIM_GROUP}/${CLAIM_VERSION}/namespaces/${encodeURIComponent(namespace)}/${CLAIM_PLURAL}`,
      {
        method: "POST",
        body: JSON.stringify({
          apiVersion: `${CLAIM_GROUP}/${CLAIM_VERSION}`,
          kind: "OSGymSandboxClaim",
          metadata: { name },
          spec: {
            sandboxTemplateRef: { name: templateRef },
          },
        }),
      },
    )
    return parseClaim(raw)
  },

  get: async (namespace: string, name: string): Promise<Claim> => {
    const raw = await httpK8s<any>(
      `apis/${CLAIM_GROUP}/${CLAIM_VERSION}/namespaces/${encodeURIComponent(namespace)}/${CLAIM_PLURAL}/${encodeURIComponent(name)}`,
    )
    return parseClaim(raw)
  },

  remove: async (namespace: string, name: string): Promise<void> => {
    await httpK8s<void>(
      `apis/${CLAIM_GROUP}/${CLAIM_VERSION}/namespaces/${encodeURIComponent(namespace)}/${CLAIM_PLURAL}/${encodeURIComponent(name)}`,
      { method: "DELETE" },
    )
  },
}

// ── Unit parsing ────────────────────────────────────────────────────────
//
// metrics-server returns CPU as `<n>n` (nanocores) and memory as
// `<n>Ki|Mi|Gi` (binary suffix). Helpers convert to floats.

const MEM_SUFFIX: Record<string, number> = {
  "": 1,
  Ki: 1024,
  Mi: 1024 ** 2,
  Gi: 1024 ** 3,
  Ti: 1024 ** 4,
  // Decimal suffixes (rare) — handled defensively.
  K: 1000,
  M: 1000 ** 2,
  G: 1000 ** 3,
  T: 1000 ** 4,
}

export function parseCpuCores(v: string): number {
  // Forms seen: "159687357n", "0.250", "100m"
  if (!v) return 0
  if (v.endsWith("n")) return parseInt(v.slice(0, -1), 10) / 1e9
  if (v.endsWith("u")) return parseInt(v.slice(0, -1), 10) / 1e6
  if (v.endsWith("m")) return parseInt(v.slice(0, -1), 10) / 1000
  return parseFloat(v)
}

export function parseMemoryBytes(v: string): number {
  if (!v) return 0
  const m = v.match(/^(\d+(?:\.\d+)?)([KMGTP]i?)?$/)
  if (!m) return parseFloat(v) || 0
  const mult = MEM_SUFFIX[m[2] ?? ""] ?? 1
  return parseFloat(m[1]) * mult
}

export function sumPodUsage(metrics: PodMetrics): {
  cpuCores: number
  memBytes: number
} {
  let cpu = 0
  let mem = 0
  for (const c of metrics.containers ?? []) {
    cpu += parseCpuCores(c.usage?.cpu ?? "0")
    mem += parseMemoryBytes(c.usage?.memory ?? "0")
  }
  return { cpuCores: cpu, memBytes: mem }
}

