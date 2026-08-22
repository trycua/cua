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
function UsageSkeleton() {
  return (
    <div className="usage-skeleton" role="status" aria-label="Loading usage">
      <div className="usage-skeleton__summary" aria-hidden="true">
        <div className="usage-skeleton__block" />
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
    () => [...(overview?.pools ?? [])].sort((left, right) => right.cost_usd - left.cost_usd),
    [overview],
  )
  const totalCost = useMemo(
    () => pools.reduce((total, pool) => total + pool.cost_usd, 0),
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
            Costs appear here after pools begin reporting resource activity.
          </PageEmpty>
        </div>
      ) : (
        <div className="usage-layout">
          <section className="usage-summary" aria-label="Usage summary">
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Cost incurred</span>
              <strong>{moneyFormatter.format(totalCost)}</strong>
              <span>Across {pools.length} {pools.length === 1 ? "pool" : "pools"}</span>
            </div>
          </section>

          <section className="usage-panel" aria-labelledby="usage-breakdown-title">
            <div className="usage-panel__heading">
              <div>
                <p className="usage-panel__eyebrow">Breakdown</p>
                <h2 id="usage-breakdown-title">Cost by pool</h2>
              </div>
            </div>
            <table className="usage-breakdown">
              <caption className="cua-visually-hidden">Resource cost by pool</caption>
              <thead>
                <tr className="usage-breakdown__header">
                  <th scope="col">Pool name</th>
                  <th scope="col">Cost incurred</th>
                </tr>
              </thead>
              <tbody>
                {pools.map(pool => (
                  <tr className="usage-breakdown__row" key={pool.id}>
                    <td>{pool.name}</td>
                    <td><strong>{moneyFormatter.format(pool.cost_usd)}</strong></td>
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
