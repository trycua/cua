export interface ClientCredentialsTokenProviderOptions {
  tokenUrl: string
  clientId: string
  clientSecret: string
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

interface TokenResponse {
  access_token: string
  expires_in?: number
}

export class ClientCredentialsTokenProvider {
  private readonly tokenUrl: string
  private readonly clientId: string
  private readonly clientSecret: string
  private readonly fetch: typeof fetch
  private readonly refreshBufferMs: number
  private accessToken?: string
  private tokenDeadline = 0
  private refreshPromise?: Promise<string>

  constructor(options: ClientCredentialsTokenProviderOptions) {
    this.tokenUrl = options.tokenUrl
    this.clientId = options.clientId
    this.clientSecret = options.clientSecret
    this.fetch = resolveFetch(options.fetch)
    this.refreshBufferMs = options.refreshBufferMs ?? 30_000
  }

  async getToken(): Promise<string> {
    if (this.accessToken && Date.now() < this.tokenDeadline - this.refreshBufferMs) {
      return this.accessToken
    }
    if (!this.refreshPromise) {
      this.refreshPromise = this.exchangeToken().finally(() => {
        this.refreshPromise = undefined
      })
    }
    return this.refreshPromise
  }

  private async exchangeToken(): Promise<string> {
    const response = await this.fetch(this.tokenUrl, {
      method: "POST",
      body: new URLSearchParams({
        grant_type: "client_credentials",
        client_id: this.clientId,
        client_secret: this.clientSecret,
      }),
    })
    if (!response.ok) {
      const body = await response.text().catch(() => "")
      throw new Error(`${response.status} ${response.statusText}${body ? `: ${body}` : ""}`)
    }

    const token = (await response.json()) as TokenResponse
    this.accessToken = token.access_token
    this.tokenDeadline = Date.now() + Number(token.expires_in ?? 300) * 1000
    return this.accessToken
  }
}
