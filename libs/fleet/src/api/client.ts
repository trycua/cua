// Singleton instance of the generated cyclops-cs-backend Api client.
//
// The generated client (src/api/generated/cyclops-cs-backend.ts) is
// produced by `pnpm gen:api` from backend/docs/swagger.json. We don't
// hand-edit it — instead we configure it here with a securityWorker
// that pulls a fresh Keycloak access token before every authenticated
// request, so all SPA → backend calls share one auth pipeline.
//
// `baseUrl` is "" so the generated `/api/...` paths resolve against the
// SPA's own origin (nginx routes /api/{keys,gateway,k8s,orch,swagger}
// to the backend sidecar).

import { Api } from "./generated/cyclops-cs-backend"
import { getToken } from "../auth/keycloak"

export const apiClient = new Api({
  baseUrl: "",
  securityWorker: async () => {
    const token = await getToken()
    if (!token) return {}
    return { headers: { Authorization: `Bearer ${token}` } }
  },
})
