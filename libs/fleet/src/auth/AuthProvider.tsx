// AuthProvider — wraps the app, gates rendering until Keycloak login
// resolves. With onLoad: "login-required" the user is redirected to
// Keycloak before the React tree mounts; this component only renders
// children once we have a valid session.

import { useEffect, useLayoutEffect, useState } from "react"
import { initKc, kc } from "./keycloak"
import { bindFleetAttribution, recordFleetLogin } from "./analytics"
import { isLocalVisualPreview } from "../local-visual-preview"

interface Props {
  children: React.ReactNode
}

function removeBootSurface() {
  document.getElementById("cua-boot")?.remove()
}

export function AuthProvider({ children }: Props) {
  const visualPreview = isLocalVisualPreview()
  const [ready, setReady] = useState(visualPreview)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (visualPreview) return
    initKc()
      .then(authed => {
        if (!authed) {
          setError("Not authenticated")
          return
        }
        setReady(true)
        void Promise.allSettled([
          bindFleetAttribution(),
          recordFleetLogin(kc.sessionId),
        ])
      })
      .catch(e => setError(String(e)))
  }, [visualPreview])

  useLayoutEffect(() => {
    if (ready || error) removeBootSurface()
  }, [error, ready])

  if (error) {
    return (
      <main className="cua-auth-error">
        <div className="cua-auth-error__panel">
          <p className="cua-auth-error__brand">Cua</p>
          <h1>We couldn&apos;t sign you in</h1>
          <p>
            Your session didn&apos;t complete. Try again, or contact Cua support
            if this keeps happening.
          </p>
          <code>{error}</code>
          <button type="button" onClick={() => window.location.reload()}>
            Try again
          </button>
        </div>
      </main>
    )
  }
  if (!ready) return null
  return <>{children}</>
}
