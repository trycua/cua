import { getToken } from "./keycloak"
import {
  CyclopsClient,
  CyclopsTokenProviderConfigurationBuilder,
  uniffiInitAsync,
  type CyclopsTokenProviderConfiguration,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"

const sdkInitialization = uniffiInitAsync()

export async function ensureSdkInitialized(): Promise<void> {
  await sdkInitialization
}

function baseUrl(): string {
  return window.location.origin
}

export function buildClientConfiguration(): CyclopsTokenProviderConfiguration {
  return new CyclopsTokenProviderConfigurationBuilder()
    .baseUrl(baseUrl())
    .poolPollIntervalMs(5_000n)
    .poolPollLimit(120)
    .claimPollIntervalMs(5_000n)
    .claimPollLimit(120)
    .build()
}

export async function withClient<T>(
  operation: (client: CyclopsClient) => Promise<T>,
): Promise<T> {
  await ensureSdkInitialized()
  const token = await getToken()
  if (!token) throw new Error("Authentication token is unavailable")

  const client = CyclopsClient.connectBrowserWithAccessToken(
    buildClientConfiguration(),
    token,
  ) as CyclopsClient
  try {
    return await operation(client)
  } finally {
    client.uniffiDestroy()
  }
}
