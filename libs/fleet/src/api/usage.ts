import { getToken } from "../auth/keycloak"
import { isLocalVisualPreview } from "../local-visual-preview"

export type UsageTimeframe = "24h" | "7d" | "30d"

export interface UsageMetricTotals {
  consumed: number
  provisioned: number
}

export interface UsagePoolSummary {
  id: string
  name: string
  cpu: UsageMetricTotals
  memory: UsageMetricTotals
  cost_usd: number
}

export interface UsageOverviewResponse {
  data_as_of: string
  partial: boolean
  pools: UsagePoolSummary[]
}

export interface UsageBucket {
  start: string
  end: string
  cpu_consumed: number
  cpu_provisioned: number
  memory_consumed: number
  memory_provisioned: number
}

export interface UsagePoolDetailResponse {
  data_as_of: string
  partial: boolean
  pool: { id: string; name: string }
  buckets: UsageBucket[]
}

export interface UsageBrowserTimings {
  initial_load_ms: number
  dashboard_ready_ms: number
}

const previewPools: UsagePoolSummary[] = [
  {
    id: "preview:browser-agent-fleet",
    name: "browser-agent-fleet",
    cpu: { consumed: 842.6, provisioned: 1_344 },
    memory: { consumed: 3_218.4, provisioned: 5_376 },
    cost_usd: 18.42,
  },
  {
    id: "preview:desktop-agent-fleet",
    name: "desktop-agent-fleet",
    cpu: { consumed: 396.8, provisioned: 672 },
    memory: { consumed: 1_786.2, provisioned: 2_688 },
    cost_usd: 9.75,
  },
  {
    id: "preview:eval-runner-fleet",
    name: "eval-runner-fleet",
    cpu: { consumed: 271.4, provisioned: 448 },
    memory: { consumed: 986.7, provisioned: 1_792 },
    cost_usd: 6.2,
  },
  {
    id: "preview:batch-jobs-fleet",
    name: "batch-jobs-fleet",
    cpu: { consumed: 188.9, provisioned: 320 },
    memory: { consumed: 624.5, provisioned: 1_280 },
    cost_usd: 3.1,
  },
]

function previewState() {
  return new URLSearchParams(window.location.search).get("cua-preview-state")
}

async function preparePreview() {
  const state = previewState()
  if (state === "error") throw new Error("Preview usage is unavailable")
  if (state === "loading") {
    await new Promise(resolve => window.setTimeout(resolve, 2_000))
  }
  return state
}

async function previewOverview(
  timeframe: UsageTimeframe,
): Promise<UsageOverviewResponse> {
  const state = await preparePreview()
  const multiplier = timeframe === "24h" ? 1 : timeframe === "7d" ? 7 : 30
  return {
    data_as_of: "2026-08-22T02:00:00Z",
    partial: false,
    pools: state === "empty"
      ? []
      : previewPools.map(pool => ({
          ...pool,
          cost_usd: pool.cost_usd * multiplier,
        })),
  }
}

function previewBucketCount(timeframe: UsageTimeframe) {
  if (timeframe === "24h") return { count: 24, stepMs: 60 * 60 * 1_000 }
  if (timeframe === "7d") return { count: 28, stepMs: 6 * 60 * 60 * 1_000 }
  return { count: 30, stepMs: 24 * 60 * 60 * 1_000 }
}

async function previewPool(
  timeframe: UsageTimeframe,
  poolId: string,
): Promise<UsagePoolDetailResponse> {
  const state = await preparePreview()
  const pool = previewPools.find(candidate => candidate.id === poolId) ?? previewPools[0]
  const { count, stepMs } = previewBucketCount(timeframe)
  const end = new Date("2026-08-22T02:00:00Z").getTime()
  const poolScale = Math.max(
    previewPools.findIndex(candidate => candidate.id === pool.id) + 1,
    1,
  )
  const buckets = state === "empty"
    ? []
    : Array.from({ length: count }, (_, index) => {
        const start = end - (count - index) * stepMs
        const wave = 0.72 + Math.sin(index / 2.8) * 0.12 + index / count * 0.1
        const cpuProvisioned = 38 / poolScale
        const memoryProvisioned = 152 / poolScale
        return {
          start: new Date(start).toISOString(),
          end: new Date(start + stepMs).toISOString(),
          cpu_consumed: Number((cpuProvisioned * wave).toFixed(2)),
          cpu_provisioned: Number(cpuProvisioned.toFixed(2)),
          memory_consumed: Number((memoryProvisioned * (wave - 0.06)).toFixed(2)),
          memory_provisioned: Number(memoryProvisioned.toFixed(2)),
        }
      })
  return {
    data_as_of: "2026-08-22T02:00:00Z",
    partial: false,
    pool: { id: pool.id, name: pool.name },
    buckets,
  }
}

async function request<T>(path: string, query: URLSearchParams): Promise<T> {
  const token = await getToken()
  const response = await fetch(`${path}?${query}`, {
    headers: { Authorization: token ? `Bearer ${token}` : "" },
  })
  if (!response.ok) {
    const body = await response.json().catch(() => null) as { error?: string } | null
    throw new Error(body?.error ?? `usage request failed: ${response.status}`)
  }
  return response.json() as Promise<T>
}

function params(timeframe: UsageTimeframe, subject?: string) {
  const query = new URLSearchParams({ timeframe })
  if (subject) query.set("subject", subject)
  return query
}

async function recordBrowserTimings(
  timeframe: UsageTimeframe,
  timings: UsageBrowserTimings,
): Promise<void> {
  if (isLocalVisualPreview()) return
  const token = await getToken()
  await fetch(`/api/usage/browser-timings?${params(timeframe)}`, {
    method: "POST",
    headers: {
      Authorization: token ? `Bearer ${token}` : "",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(timings),
    keepalive: true,
  })
}

export const usageApi = {
  overview: (timeframe: UsageTimeframe, subject?: string) =>
    isLocalVisualPreview()
      ? previewOverview(timeframe)
      : request<UsageOverviewResponse>(
          "/api/usage/overview",
          params(timeframe, subject),
        ),
  pool: (timeframe: UsageTimeframe, poolId: string, subject?: string) => {
    if (isLocalVisualPreview()) return previewPool(timeframe, poolId)
    const query = params(timeframe, subject)
    query.set("pool", poolId)
    return request<UsagePoolDetailResponse>("/api/usage/pool", query)
  },
  recordBrowserTimings,
}
