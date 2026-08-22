import Select from "@cloudscape-design/components/select"
import { useCallback, useEffect, useMemo, useState } from "react"
import { CuaButton } from "../components/CuaButton"
import { PageEmpty, PageError } from "../components/PageState"
import { PageShell } from "../components/PageShell"
import {
  billingApi,
  type BillingUsage,
  type UsagePoint,
  type UsageRangeMonths,
} from "../sdk/billing"
import "./Usage.css"

const rangeOptions = [
  { label: "Last 3 months", value: "3" },
  { label: "Last 6 months", value: "6" },
  { label: "Last 12 months", value: "12" },
]

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
})
const monthFormatter = new Intl.DateTimeFormat(undefined, { month: "short" })
const quantityFormatter = new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 })

function formatMoney(amount: number, currency: string, compact = false) {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: currency.toUpperCase(),
    notation: compact ? "compact" : "standard",
    maximumFractionDigits: compact ? 1 : 2,
  }).format(amount / 100)
}

function formatDate(value: string | null) {
  return value ? dateFormatter.format(new Date(value)) : "Not available"
}

function periodLabel(usage: BillingUsage) {
  if (!usage.current_period_start || !usage.current_period_end) return "No active period"
  return `${formatDate(usage.current_period_start)} – ${formatDate(usage.current_period_end)}`
}

function percentChange(current: number, previous: number) {
  if (previous === 0) return null
  return ((current - previous) / previous) * 100
}

function UsageSkeleton() {
  return (
    <div className="usage-skeleton" role="status" aria-label="Loading usage">
      <div className="usage-skeleton__summary" aria-hidden="true">
        {Array.from({ length: 3 }, (_, index) => (
          <div className="usage-skeleton__block" key={index} />
        ))}
      </div>
      <div className="usage-skeleton__chart" aria-hidden="true" />
      <div className="usage-skeleton__rows" aria-hidden="true">
        {Array.from({ length: 4 }, (_, index) => (
          <div className="usage-skeleton__row" key={index} />
        ))}
      </div>
    </div>
  )
}

function UsageTrend({
  points,
  currency,
  referenceAmount,
}: {
  points: UsagePoint[]
  currency: string
  referenceAmount: number
}) {
  const width = 720
  const height = 280
  const padding = { top: 24, right: 34, bottom: 42, left: 54 }
  const plotWidth = width - padding.left - padding.right
  const plotHeight = height - padding.top - padding.bottom
  const maximum = Math.max(...points.map(point => point.amount), 1)
  const minimum = Math.min(...points.map(point => point.amount), 0)
  const span = Math.max(maximum - minimum, 1)
  const pointAt = (point: UsagePoint, index: number) => ({
    x: padding.left + (points.length === 1 ? plotWidth : (index / (points.length - 1)) * plotWidth),
    y: padding.top + plotHeight - ((point.amount - minimum) / span) * plotHeight,
  })
  const positions = points.map(pointAt)
  const referenceY =
    padding.top + plotHeight - ((referenceAmount - minimum) / span) * plotHeight
  const path = positions.map((point, index) => `${index === 0 ? "M" : "L"}${point.x},${point.y}`).join(" ")
  const latest = points.at(-1)
  const peak = points.reduce((best, point) => (point.amount > best.amount ? point : best))
  const summary = `${formatMoney(latest?.amount ?? 0, currency)} estimated this period. Peak spend was ${formatMoney(peak.amount, currency)} in ${monthFormatter.format(new Date(peak.period_start))}.`

  return (
    <div className="usage-trend" role="img" aria-label={summary}>
      <svg viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
        <line className="usage-trend__axis" x1={padding.left} x2={width - padding.right} y1={height - padding.bottom} y2={height - padding.bottom} />
        {referenceAmount >= minimum && referenceAmount <= maximum ? (
          <>
            <line className="usage-trend__reference" x1={padding.left} x2={width - padding.right} y1={referenceY} y2={referenceY} />
            <text className="usage-trend__tick" x={width - padding.right} y={referenceY - 8} textAnchor="end">Previous {formatMoney(referenceAmount, currency, true)}</text>
          </>
        ) : null}
        <text className="usage-trend__tick" x={padding.left - 12} y={padding.top + 4} textAnchor="end">{formatMoney(maximum, currency, true)}</text>
        <text className="usage-trend__tick" x={padding.left - 12} y={height - padding.bottom + 4} textAnchor="end">{formatMoney(minimum, currency, true)}</text>
        <path className="usage-trend__line" d={path} />
        {positions.map((position, index) => (
          <g key={points[index].period_start}>
            <circle className={points[index].estimate ? "usage-trend__point usage-trend__point--estimate" : "usage-trend__point"} cx={position.x} cy={position.y} r={points[index].estimate ? 5 : 3.5} />
            <text className="usage-trend__month" x={position.x} y={height - 15} textAnchor="middle">{monthFormatter.format(new Date(points[index].period_start))}</text>
          </g>
        ))}
        {latest ? (
          <text className="usage-trend__label" x={positions.at(-1)!.x - 8} y={positions.at(-1)!.y - 14} textAnchor="end">
            {formatMoney(latest.amount, currency, true)} est.
          </text>
        ) : null}
      </svg>
      <table className="cua-visually-hidden">
        <caption>Billing spend by period</caption>
        <thead><tr><th>Period</th><th>Spend</th><th>Type</th></tr></thead>
        <tbody>{points.map(point => <tr key={point.period_start}><td>{formatDate(point.period_start)}</td><td>{formatMoney(point.amount, currency)}</td><td>{point.estimate ? "Estimate" : "Invoice"}</td></tr>)}</tbody>
      </table>
    </div>
  )
}

export function Usage() {
  const [months, setMonths] = useState<UsageRangeMonths>(6)
  const [usage, setUsage] = useState<BillingUsage | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    setError(false)
    try {
      setUsage(await billingApi.usage(months))
    } catch {
      setError(true)
    } finally {
      setLoading(false)
    }
  }, [months])

  useEffect(() => {
    load()
  }, [load])

  const change = usage ? percentChange(usage.current_estimate, usage.previous_period_amount) : null
  const largestAmount = useMemo(
    () => Math.max(...(usage?.breakdown.map(item => Math.abs(item.amount)) ?? []), 1),
    [usage],
  )

  return (
    <PageShell
      eyebrow="Billing"
      title="Usage"
      description="Track invoiced spend and see which services drive the current period."
      actions={
        <Select
          ariaLabel="Usage history range"
          selectedOption={rangeOptions.find(option => option.value === String(months)) ?? rangeOptions[1]}
          onChange={({ detail }) => setMonths(Number(detail.selectedOption.value) as UsageRangeMonths)}
          options={rangeOptions}
        />
      }
    >
      {loading ? <UsageSkeleton /> : error ? (
        <PageError title="Usage is temporarily unavailable" action={<CuaButton tone="secondary" onClick={load}>Try again</CuaButton>}>
          We could not load Stripe billing data. No billing changes were made.
        </PageError>
      ) : !usage || (usage.trend.length === 0 && usage.breakdown.length === 0) ? (
        <div className="usage-empty">
          <PageEmpty title="No invoiced usage yet">
            Usage appears here after services begin reporting billable activity.
          </PageEmpty>
        </div>
      ) : (
        <div className="usage-layout">
          <section className="usage-summary" aria-label="Usage summary">
            <div className="usage-summary__metric usage-summary__metric--primary">
              <span className="usage-summary__label">Current estimate</span>
              <strong>{formatMoney(usage.current_estimate, usage.currency)}</strong>
              <span>{periodLabel(usage)}</span>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Previous period</span>
              <strong>{formatMoney(usage.previous_period_amount, usage.currency)}</strong>
              <span>{change === null ? "No comparison yet" : `${change > 0 ? "+" : ""}${change.toFixed(1)}% this period`}</span>
            </div>
            <div className="usage-summary__metric">
              <span className="usage-summary__label">Usage drivers</span>
              <strong>{usage.breakdown.length}</strong>
              <span>{usage.breakdown.length === 1 ? "billable service" : "billable services"}</span>
            </div>
          </section>

          <section className="usage-panel" aria-labelledby="usage-trend-title">
            <div className="usage-panel__heading">
              <div><p className="usage-panel__eyebrow">Spend trend</p><h2 id="usage-trend-title">Current pace is visible at a glance</h2></div>
              <p>Finalized invoices plus the current Stripe estimate</p>
            </div>
            <UsageTrend points={usage.trend} currency={usage.currency} referenceAmount={usage.previous_period_amount} />
          </section>

          <section className="usage-panel" aria-labelledby="usage-breakdown-title">
            <div className="usage-panel__heading">
              <div><p className="usage-panel__eyebrow">Breakdown</p><h2 id="usage-breakdown-title">Services driving this period</h2></div>
              <p>{periodLabel(usage)}</p>
            </div>
            <table className="usage-breakdown">
              <caption className="cua-visually-hidden">Current period usage breakdown</caption>
              <thead>
                <tr className="usage-breakdown__header">
                  <th scope="col">Service</th><th scope="col">Quantity</th><th scope="col">Spend</th>
                </tr>
              </thead>
              <tbody>
                {usage.breakdown.map(item => (
                  <tr className="usage-breakdown__row" key={item.name}>
                    <td>
                      <div className="usage-breakdown__service">
                        <span>{item.name}</span>
                        <span className="usage-breakdown__bar"><span style={{ width: `${(Math.abs(item.amount) / largestAmount) * 100}%` }} /></span>
                      </div>
                    </td>
                    <td className="usage-breakdown__quantity">{quantityFormatter.format(item.quantity)} units</td>
                    <td><strong>{formatMoney(item.amount, usage.currency)}</strong></td>
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
