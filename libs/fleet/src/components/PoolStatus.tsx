import type { CSSProperties } from "react"
import type { PoolDisplayStatus } from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import "./PoolStatus.css"

function indicatorStyle(status: PoolDisplayStatus): CSSProperties {
  return { backgroundColor: status.indicator }
}

export function PoolStatusDot({ status }: { status: PoolDisplayStatus }) {
  return (
    <span
      aria-label={status.label}
      className="cua-pool-status__dot"
      role="img"
      style={indicatorStyle(status)}
      title={status.label}
    />
  )
}

export function PoolStatusPill({
  status,
  compact = false,
}: {
  status: PoolDisplayStatus
  compact?: boolean
}) {
  return (
    <span className="cua-pool-status" title={compact ? status.label : undefined}>
      <span
        aria-hidden="true"
        className="cua-pool-status__dot"
        style={indicatorStyle(status)}
      />
      <span className={compact ? "cua-visually-hidden" : undefined}>
        {status.label}
      </span>
    </span>
  )
}
