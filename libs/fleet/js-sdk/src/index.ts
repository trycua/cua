import { Api, type ApiConfig } from "./generated/cyclops-cs-backend.js"
import {
  ClientCredentialsTokenProvider,
  type ClientCredentialsTokenProviderOptions,
} from "./token.js"

export * from "./generated/cyclops-cs-backend.js"
export * from "./token.js"

export const DEFAULT_TOKEN_URL =
  "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
export const DEFAULT_BASE_URL = "https://run.cua.ai"

export interface CyclopsClientOptions {
  clientId: string
  clientSecret: string
  tokenUrl?: string
  baseUrl?: string
  fetch?: typeof fetch
  refreshBufferMs?: number
}

function resolveFetch(customFetch?: typeof fetch): typeof fetch {
  const runtimeFetch = customFetch ?? globalThis.fetch
  if (typeof runtimeFetch !== "function") {
    throw new Error(
      "Fetch API is unavailable. @trycua/cyclops requires Node.js 18+ or an injected fetch implementation.",
    )
  }
  return runtimeFetch
}

export class CyclopsClient extends Api<unknown> {
  readonly tokenProvider: ClientCredentialsTokenProvider

  constructor(options: CyclopsClientOptions) {
    const fetch = resolveFetch(options.fetch)
    const tokenOptions: ClientCredentialsTokenProviderOptions = {
      tokenUrl: options.tokenUrl ?? DEFAULT_TOKEN_URL,
      clientId: options.clientId,
      clientSecret: options.clientSecret,
      fetch,
      refreshBufferMs: options.refreshBufferMs,
    }
    const tokenProvider = new ClientCredentialsTokenProvider(tokenOptions)
    const config: ApiConfig = {
      baseUrl: options.baseUrl ?? DEFAULT_BASE_URL,
      customFetch: fetch,
      securityWorker: async () => ({
        headers: { Authorization: `Bearer ${await tokenProvider.getToken()}` },
      }),
    }
    super(config)
    this.tokenProvider = tokenProvider
  }

  static async fromKey(options: CyclopsClientOptions): Promise<CyclopsClient> {
    const client = new CyclopsClient(options)
    await client.tokenProvider.getToken()
    return client
  }
}
