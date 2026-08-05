import { getToken } from "../auth/keycloak"
import { CyclopsClient, uniffiInitAsync } from "./generated"

const sdkInitialization = uniffiInitAsync()

function baseUrl(): string {
  return window.location.origin
}

export async function withClient<T>(
  operation: (client: CyclopsClient) => Promise<T>,
): Promise<T> {
  await sdkInitialization
  const token = await getToken()
  if (!token) throw new Error("Authentication token is unavailable")

  const client = CyclopsClient.connectBrowserWithAccessToken(
    {
      baseUrl: baseUrl(),
      poolPollIntervalMs: 5_000n,
      poolPollLimit: 120,
      claimPollIntervalMs: 5_000n,
      claimPollLimit: 120,
    },
    token,
  ) as CyclopsClient
  try {
    return await operation(client)
  } finally {
    client.uniffiDestroy()
  }
}
