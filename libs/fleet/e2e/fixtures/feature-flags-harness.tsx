import { createRoot } from "react-dom/client"
import { BrowserRouter, Route, Routes } from "react-router-dom"
import { useState } from "react"
import { AdminRoute } from "../../src/components/AdminRoute"
import { fetchFeatureFlags } from "../../src/api/featureFlags"
import {
  FeatureFlagProvider,
  useFeatureFlags,
} from "../../src/components/FeatureFlagContext"

function Controls() {
  const { admin, refresh, resolved } = useFeatureFlags()
  const [outcome, setOutcome] = useState("idle")
  const [cached, setCached] = useState("idle")
  const refreshFlags = async () => {
    try {
      await refresh()
      setOutcome("fulfilled")
    } catch {
      setOutcome("rejected")
    }
  }

  return (
    <>
      {admin && resolved ? <a href="#admin">Feature flags</a> : null}
      <button onClick={() => void refreshFlags()}>Refresh config</button>
      <output data-testid="flags-state">{`${admin}:${resolved}`}</output>
      <button onClick={() => void fetchFeatureFlags().then(flags => setCached(`${flags.admin}:${flags.billing}`))}>
        Read cached config
      </button>
      <output data-testid="refresh-outcome">{outcome}</output>
      <output data-testid="cached-state">{cached}</output>
    </>
  )
}

function Harness() {
  return (
    <FeatureFlagProvider>
      <BrowserRouter>
        <Controls />
        <Routes>
          <Route element={<AdminRoute />}>
            <Route path="*" element={<h1>Feature flags</h1>} />
          </Route>
        </Routes>
      </BrowserRouter>
    </FeatureFlagProvider>
  )
}

const root = document.getElementById("root")
if (!root) throw new Error("missing root")
createRoot(root).render(<Harness />)
