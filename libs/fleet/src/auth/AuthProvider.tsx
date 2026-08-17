// AuthProvider — wraps the app, gates rendering until Keycloak login
// resolves. With onLoad: "login-required" the user is redirected to
// Keycloak before the React tree mounts; this component only renders
// children once we have a valid session.

import { useEffect, useState } from "react"
import { initKc } from "./keycloak"
import { isLocalVisualPreview } from "../local-visual-preview"

interface Props {
  children: React.ReactNode
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
      })
      .catch(e => setError(String(e)))
  }, [visualPreview])

  if (error) {
    return (
      <div style={{ padding: 24, fontFamily: "monospace" }}>
        Auth error: {error}
      </div>
    )
  }
  if (!ready) {
    return (
      <div style={{ padding: 24, fontFamily: "monospace" }}>
        Signing in&hellip;
      </div>
    )
  }
  return <>{children}</>
}
