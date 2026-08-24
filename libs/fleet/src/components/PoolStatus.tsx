import StatusIndicator, {
  type StatusIndicatorProps,
} from "@cloudscape-design/components/status-indicator"
import type { PoolStatus } from "../sdk/status"

const POOL_STATUS_TYPE: Record<PoolStatus["kind"], StatusIndicatorProps.Type> = {
  healthy: "success",
  provisioning: "in-progress",
  terminating: "in-progress",
  unknown: "pending",
}

export function PoolStatusPill({
  status,
  compact = false,
}: {
  status: PoolStatus
  compact?: boolean
}) {
  return (
    <span title={compact ? status.label : undefined}>
      <StatusIndicator type={POOL_STATUS_TYPE[status.kind]}>
        <span className={compact ? "cua-visually-hidden" : undefined}>
          {status.label}
        </span>
      </StatusIndicator>
    </span>
  )
}
