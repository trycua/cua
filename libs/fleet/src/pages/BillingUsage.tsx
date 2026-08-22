import Select from "@cloudscape-design/components/select"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { CuaButton } from "../components/CuaButton"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import {
  usageApi,
  type UsageOverviewResponse,
  type UsageTimeframe,
} from "../sdk/usage"
import "./BillingUsage.css"

const rangeOptions = [
  { label: "Last 24 hours", value: "24h" },
  { label: "Last 7 days", value: "7d" },
  { label: "Last 30 days", value: "30d" },
]

const moneyFormatter = new Intl.NumberFormat(undefined, {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
})
const resourceFormatter = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
})

function resourceHours(value: number, unit: "core-h" | "GiB-h") {
  return `${resourceFormatter.format(value)} ${unit}`
}
function UsageSkeleton() {
  return (
    <div className="usage-skeleton" role="status" aria-label="Loading usage">
      <div className="usage-skeleton__summary" aria-hidden="true">
        {Array.from({ length: 3 }, (_, index) => (
          <div className="usage-skeleton__block" key={index} />
        ))}
      </div>
      <div className="usage-skeleton__rows" aria-hidden="true">
        {Array.from({ length: 4 }, (_, index) => (
          <div className="usage-skeleton__row" key={index} />
        ))}
      </div>
    </div>
  )
}

export function BillingUsagePage() {
  const [timeframe, setTimeframe] = useState<UsageTimeframe>("24h")
  const [overview, setOverview] = useState<UsageOverviewResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const loadStarted = useRef(0)

  const load = useCallback(async () => {
    loadStarted.current = performance.now()
    setLoading(true)
    setError(false)
    try {
      const value = await usageApi.overview(timeframe)
      const initialLoadMS = performance.now() - loadStarted.current
      setOverview(value)
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          performance.mark("usage-dashboard-ready")
          void usageApi
            .recordBrowserTimings(timeframe, {
              initial_load_ms: initialLoadMS,
              dashboard_ready_ms: performance.now() - loadStarted.current,
            })
            .catch(() => undefined)
        })
      })
    } catch {
      setOverview(null)
      setError(true)
    } finally {
      setLoading(false)
    }
  }, [timeframe])

  useEffect(() => {
    load()
  }, [load])

  const pools = useMemo(
    () => [...(overview?.pools ?? [])].sort((left, right) =>
      right.cpu.provisioned - left.cpu.provisioned ||
      right.memory.provisioned - left.memory.provisioned ||
      right.cost_usd - left.cost_usd
    ),
    [overview],
  )
  const totals = useMemo(
    () => pools.reduce(
      (total, pool) => ({
        cpuProvisioned: total.cpuProvisioned + pool.cpu.provisioned,
        memoryProvisioned: total.memoryProvisioned + pool.memory.provisioned,
        cost: total.cost + pool.cost_usd,
      }),
      { cpuProvisioned: 0, memoryProvisioned: 0, cost: 0 },
    ),
    [pools],
  )

  return (
    <PageShell
      eyebrow="Billing"
      title="Usage"
      actions={
        <Select
          ariaLabel="Usage history range"
          selectedOption={rangeOptions.find(option => option.value === timeframe) ?? rangeOptions[0]}
          onChange={({ detail }) => setTimeframe(detail.selectedOption.value as UsageTimeframe)}
          options={rangeOptions}
        />
      }
    >
      {loading ? <UsageSkeleton /> : error ? (
        <PageError title="Usage is temporarily unavailable" action={<CuaButton tone="secondary" onClick={load}>Try again</CuaButton>}>
          We could not load resource cost data. Try again in a moment.
        </PageError>
      ) : !overview || pools.length === 0 ? (
        <div className="usage-empty">
          <PageEmpty title="No resource usage yet">
            Reserved resource hours appear here after pools begin reporting activity.
          </PageEmpty>
        </div>
      ) : (
        <div className="usage-layout">
          <section className="usage-summary" aria-label="Usage summary">
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Reserved CPU</span>
              <strong>{resourceFormatter.format(totals.cpuProvisioned)}</strong>
              <span>Core-hours across the selected range</span>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Reserved memory</span>
              <strong>{resourceFormatter.format(totals.memoryProvisioned)}</strong>
              <span>GiB-hours across the selected range</span>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">OpenCost incurred</span>
              <strong>{moneyFormatter.format(totals.cost)}</strong>
              <span>Across {pools.length} {pools.length === 1 ? "pool" : "pools"}</span>
            </div>
          </section>

          <section className="usage-panel" aria-labelledby="usage-breakdown-title">
            <div className="usage-panel__heading">
              <div>
                <p className="usage-panel__eyebrow">Breakdown</p>
                <h2 id="usage-breakdown-title">Reserved resources by pool</h2>
              </div>
            </div>
            <table className="usage-breakdown">
              <caption className="cua-visually-hidden">Reserved resource hours and OpenCost by pool</caption>
              <thead>
                <tr className="usage-breakdown__header">
                  <th scope="col">Pool name</th>
                  <th scope="col">Reserved CPU</th>
                  <th scope="col">Reserved memory</th>
                  <th scope="col">OpenCost</th>
                </tr>
              </thead>
              <tbody>
                {pools.map(pool => (
                  <tr className="usage-breakdown__row" key={pool.id}>
                    <td>{pool.name}</td>
                    <td data-label="Reserved CPU"><strong>{resourceHours(pool.cpu.provisioned, "core-h")}</strong></td>
                    <td data-label="Reserved memory"><strong>{resourceHours(pool.memory.provisioned, "GiB-h")}</strong></td>
                    <td data-label="OpenCost"><strong>{moneyFormatter.format(pool.cost_usd)}</strong></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        </div>
      )}
    </PageShell>
  )
}
