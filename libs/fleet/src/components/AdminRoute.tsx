import Spinner from "@cloudscape-design/components/spinner"
import { Navigate, Outlet } from "react-router-dom"
import { useFeatureFlags } from "./FeatureFlagContext"

export function AdminRoute() {
  const { admin, resolved } = useFeatureFlags()

  if (!resolved) return <Spinner nativeAttributes={{ "aria-label": "Loading admin access" }} />
  if (!admin) return <Navigate to="/pools" replace />
  return <Outlet />
}
