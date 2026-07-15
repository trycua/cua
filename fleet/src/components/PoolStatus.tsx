import StatusIndicator, {
  StatusIndicatorProps,
} from "@cloudscape-design/components/status-indicator"
import { PoolStatus } from "../api/pools"

const POOL_STATUS_TYPE: Record<PoolStatus["kind"], StatusIndicatorProps.Type> = {
  healthy: "success",
  provisioning: "in-progress",
  terminating: "in-progress",
  unknown: "pending",
}

export function PoolStatusPill({ status }: { status: PoolStatus }) {
  return (
    <StatusIndicator type={POOL_STATUS_TYPE[status.kind]}>
      {status.label}
    </StatusIndicator>
  )
}
