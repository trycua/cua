import FormField from "@cloudscape-design/components/form-field"
import Input from "@cloudscape-design/components/input"
import Select from "@cloudscape-design/components/select"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { CuaButton } from "../components/CuaButton"
import { PoolStatusDot } from "../components/PoolStatus"
import { useFeatureFlags } from "../components/FeatureFlagContext"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import type { PoolSummary } from "../fleet/models"
import { listPools } from "../fleet/pools"
import { historicalPoolStatus } from "../fleet/status"
import {
  usageApi,
  type UsageOverviewResponse,
  type UsageTimeframe,
} from "../api/usage"
import "./BillingUsage.css"
import { reservedResourceCostUSD } from "../usagePricing"

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
const rateFormatter = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 0,
  maximumFractionDigits: 7,
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
  const { admin, usagePricing } = useFeatureFlags()
  const [timeframe, setTimeframe] = useState<UsageTimeframe>("24h")
  const [subjectInput, setSubjectInput] = useState("")
  const [activeSubject, setActiveSubject] = useState<string | null>(null)
  const [subjectError, setSubjectError] = useState("")
  const [overview, setOverview] = useState<UsageOverviewResponse | null>(null)
  const [currentPools, setCurrentPools] = useState<PoolSummary[] | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const loadStarted = useRef(0)
  const requestedQuery = useRef<string | null>(null)

  const load = useCallback(async () => {
    loadStarted.current = performance.now()
    setLoading(true)
    setError(false)
    try {
      const [value, poolsResult] = await Promise.all([
        usageApi.overview(timeframe, activeSubject ?? undefined),
        listPools().catch(() => null),
      ])
      const initialLoadMS = performance.now() - loadStarted.current
      setOverview(value)
      setCurrentPools(poolsResult)
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
  }, [activeSubject, timeframe])

  useEffect(() => {
    const query = `${timeframe}\0${activeSubject ?? ""}`
    if (requestedQuery.current === query) return
    requestedQuery.current = query
    load()
  }, [activeSubject, load, timeframe])

  const viewSubjectUsage = () => {
    const subject = subjectInput.trim()
    if (!subject) {
      setSubjectError("Enter a user subject ID.")
      return
    }
    if (subject.length > 255) {
      setSubjectError("User subject ID must be 255 characters or fewer.")
      return
    }
    setSubjectError("")
    if (subject === activeSubject) {
      void load()
      return
    }
    setActiveSubject(subject)
  }

  const resetSubjectUsage = () => {
    setSubjectInput("")
    setSubjectError("")
    setActiveSubject(null)
  }

  const pools = useMemo(
    () => (overview?.pools ?? [])
      .map(pool => ({
        ...pool,
        reservedCostUSD: reservedResourceCostUSD(
          pool.cpu.provisioned,
          pool.memory.provisioned,
          usagePricing,
        ),
      }))
      .sort((left, right) => right.reservedCostUSD - left.reservedCostUSD),
    [overview, usagePricing],
  )
  const totals = useMemo(
    () => pools.reduce(
      (total, pool) => ({
        cpuProvisioned: total.cpuProvisioned + pool.cpu.provisioned,
        memoryProvisioned: total.memoryProvisioned + pool.memory.provisioned,
        cost: total.cost + pool.reservedCostUSD,
      }),
      { cpuProvisioned: 0, memoryProvisioned: 0, cost: 0 },
    ),
    [pools],
  )

  return (
    <PageShell
      eyebrow="Billing"
      title="Usage"
      description={
        <span className="usage-pricing">
          <strong>Pricing:</strong>
          <span>CPU: {rateFormatter.format(usagePricing.vcpuHourUSD)}/vCPU/hour</span>
          <span>Memory: {rateFormatter.format(usagePricing.memoryGiBHourUSD)}/GB/hour</span>
        </span>
      }
      actions={
        <Select
          ariaLabel="Usage history range"
          selectedOption={rangeOptions.find(option => option.value === timeframe) ?? rangeOptions[0]}
          onChange={({ detail }) => setTimeframe(detail.selectedOption.value as UsageTimeframe)}
          options={rangeOptions}
        />
      }
    >
      {admin && (
        <section className="usage-admin" aria-labelledby="usage-admin-title">
          <div className="usage-admin__copy">
            <h2 id="usage-admin-title">View usage by subject</h2>
          </div>
          <div className="usage-admin__controls">
            <FormField
              label="Keycloak user subject ID"
              errorText={subjectError || undefined}
              description={activeSubject ? `Showing usage for ${activeSubject}` : undefined}
            >
              <Input
                value={subjectInput}
                placeholder="30a53246-881d-4f1a-8005-979f2a07933e"
                onChange={({ detail }) => {
                  setSubjectInput(detail.value)
                  if (subjectError) setSubjectError("")
                }}
                onKeyDown={({ detail }) => {
                  if (detail.key === "Enter") viewSubjectUsage()
                }}
              />
            </FormField>
            <div className="usage-admin__actions">
              <CuaButton tone="primary" onClick={viewSubjectUsage}>View usage</CuaButton>
              {activeSubject && (
                <CuaButton tone="secondary" onClick={resetSubjectUsage}>Reset to my usage</CuaButton>
              )}
            </div>
          </div>
        </section>
      )}
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
              <div className="usage-summary__value">
                <strong>{resourceFormatter.format(totals.cpuProvisioned)}</strong>
                <span>Core-hours</span>
              </div>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Reserved memory</span>
              <div className="usage-summary__value">
                <strong>{resourceFormatter.format(totals.memoryProvisioned)}</strong>
                <span>GiB-hours</span>
              </div>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Cost</span>
              <div className="usage-summary__value">
                <strong>{moneyFormatter.format(totals.cost)}</strong>
              </div>
            </div>
          </section>

          <section className="usage-panel" aria-labelledby="usage-breakdown-title">
            <div className="usage-panel__heading">
              <div>
                <h2 id="usage-breakdown-title">Cost by pool</h2>
              </div>
            </div>
            <table className="usage-breakdown">
              <caption className="cua-visually-hidden">Reserved resource hours and cost by pool</caption>
              <thead>
                <tr className="usage-breakdown__header">
                  <th scope="col">Pool name</th>
                  <th scope="col">Reserved CPU</th>
                  <th scope="col">Reserved memory</th>
                  <th scope="col">Cost</th>
                </tr>
              </thead>
              <tbody>
                {pools.map(pool => {
                  const status = historicalPoolStatus(pool.id, currentPools)
                  return (
                    <tr className="usage-breakdown__row" key={pool.id}>
                      <td>
                        <span className="usage-pool-name">
                          <PoolStatusDot status={status} />
                          <span>{pool.name}</span>
                        </span>
                      </td>
                      <td data-label="Reserved CPU"><strong>{resourceHours(pool.cpu.provisioned, "core-h")}</strong></td>
                      <td data-label="Reserved memory"><strong>{resourceHours(pool.memory.provisioned, "GiB-h")}</strong></td>
                      <td data-label="Cost"><strong>{moneyFormatter.format(pool.reservedCostUSD)}</strong></td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </section>
        </div>
      )}
    </PageShell>
  )
}
