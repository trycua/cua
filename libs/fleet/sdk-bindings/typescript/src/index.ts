import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import type { ClaimSpec, PoolSpec } from "./generated-crd.js"

export * from "./generated-crd.js"

export type OAuthConfiguration = { tokenUrl: string; clientId: string; clientSecret: string }
export type Configuration = { baseUrl: string; oauth: OAuthConfiguration }
export type ResourceMetadata = { namespace: string; name: string; labels?: Record<string, string> }
export type Pool = { apiVersion: "cua.ai/v1"; kind: "OSGymWorkspacePool"; metadata: ResourceMetadata; spec: Record<string, unknown>; status?: Record<string, unknown> }
export type Claim = { apiVersion: "osgym.cua.ai/v1alpha1"; kind: "OSGymSandboxClaim"; metadata: ResourceMetadata; spec: Record<string, unknown>; status?: Record<string, unknown> }
export type Sandbox = { namespace: string; claim: string; sandbox: string; services: string[] }
export type CreatePoolRequest = { namespace: string; spec: PoolSpec }
export type CreateClaimRequest = { pool: Pool; spec?: ClaimSpec }
type ServiceHttpResponse = { status: number; body: string }
export type ServiceFetch = (path: string, init?: RequestInit) => Promise<Response>

export class UnknownServiceError extends Error {
  readonly requested: string
  readonly available: readonly string[]

  constructor(requested: string, available: readonly string[]) {
    super(`unknown sandbox service ${JSON.stringify(requested)}; available services: ${JSON.stringify(available)}`)
    this.name = "UnknownServiceError"
    this.requested = requested
    this.available = [...available]
  }
}

type HostCall =
  | { kind: "http"; request: { method: string; url: string; headers: [string, string][]; body?: string } }
  | { kind: "sleep"; milliseconds: number }
  | { kind: "now_ms" }
  | { kind: "acquire_oauth_credentials" }
  | { kind: "load_access_token" }
  | { kind: "store_access_token"; token: { value: string; expires_at_ms: number } }

type Reply<T> = { ok: boolean; value?: T; error?: string }

export class SDK {
  readonly #configuration: Configuration
  readonly #process: ChildProcessWithoutNullStreams
  #buffer = ""
  #token?: { value: string; expires_at_ms: number }
  #pending: { resolve: (value: unknown) => void; reject: (error: Error) => void }[] = []

  private constructor(configuration: Configuration) {
    this.#configuration = configuration
    this.#process = spawn(process.env.CYCLOPS_CORE_RUNNER ?? "cyclops-core-runner", [], { stdio: "pipe" })
    this.#process.stdout.setEncoding("utf8")
    this.#process.stdout.on("data", chunk => this.#read(String(chunk)))
    this.#process.on("error", error => this.#fail(error))
    this.#process.on("exit", code => this.#fail(new Error(`core runner exited with ${code ?? "unknown"}`)))
  }

  static async connect(configuration: Configuration): Promise<SDK> {
    const sdk = new SDK(configuration)
    try {
      await sdk.#call({ op: "configure", configuration: { protocol_version: 7, base_url: configuration.baseUrl } })
      return sdk
    } catch (error) {
      sdk.close()
      throw error
    }
  }

  createPool(request: CreatePoolRequest): Promise<Pool> { return this.#call({ op: "create_pool", request }) }
  listPools(namespace: string): Promise<Pool[]> { return this.#call({ op: "list_pools", namespace }) }
  getPool(pool: Pool): Promise<Pool> { return this.#call({ op: "get_pool", pool }) }
  updatePool(pool: Pool): Promise<Pool> { return this.#call({ op: "update_pool", pool }) }
  deletePool(pool: Pool): Promise<void> { return this.#call({ op: "delete_pool", pool }) }
  createClaim(request: CreateClaimRequest): Promise<Claim> { return this.#call({ op: "create_claim", request }) }
  listClaims(namespace: string): Promise<Claim[]> { return this.#call({ op: "list_claims", namespace }) }
  getClaim(claim: Claim): Promise<Claim> { return this.#call({ op: "get_claim", claim }) }
  updateClaim(claim: Claim): Promise<Claim> { return this.#call({ op: "update_claim", claim }) }
  deleteClaim(claim: Claim): Promise<void> { return this.#call({ op: "delete_claim", claim }) }
  waitClaim(claim: Claim): Promise<Sandbox> { return this.#call({ op: "wait_claim", claim }) }
  serviceFetch(sandbox: Sandbox, service: string): ServiceFetch {
    const available = [...new Set(sandbox.services)].sort()
    if (!available.includes(service)) throw new UnknownServiceError(service, available)
    return async (path, init = {}) => {
      if (!path.startsWith("/") || path.startsWith("//") || path.includes("#")) {
        throw new TypeError(`service request path must be relative and start with '/': ${path}`)
      }
      const request = new Request(`https://cyclops.invalid${path}`, init)
      const body = init.body === undefined || init.body === null ? undefined : await request.text()
      const response = await this.#call<ServiceHttpResponse>({
        op: "service_request",
        sandbox,
        service,
        request: {
          method: request.method,
          path,
          headers: Array.from(request.headers.entries()),
          body,
        },
      })
      const responseBody = [204, 205, 304].includes(response.status) ? null : response.body
      return new Response(responseBody, { status: response.status })
    }
  }
  close(): void { this.#process.stdin.end(); this.#process.kill("SIGTERM") }

  #call<T>(request: unknown): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.#pending.push({ resolve: value => resolve(value as T), reject })
      this.#process.stdin.write(`${JSON.stringify(request)}\n`)
    })
  }

  async #read(chunk: string): Promise<void> {
    this.#buffer += chunk
    while (this.#buffer.includes("\n")) {
      const newline = this.#buffer.indexOf("\n")
      const line = this.#buffer.slice(0, newline)
      this.#buffer = this.#buffer.slice(newline + 1)
      if (!line) continue
      const message = JSON.parse(line) as Reply<unknown> | HostCall
      if ("kind" in message) {
        try { this.#process.stdin.write(`${JSON.stringify({ ok: true, value: await this.#dispatch(message) })}\n`) }
        catch (error) { this.#process.stdin.write(`${JSON.stringify({ ok: false, error: { transport: String(error) } })}\n`) }
        continue
      }
      const pending = this.#pending.shift()
      if (!pending) continue
      if (message.ok) pending.resolve(message.value)
      else pending.reject(new Error(message.error ?? "core operation failed"))
    }
  }

  async #dispatch(message: HostCall): Promise<unknown> {
    if (message.kind === "http") {
      const response = await fetch(message.request.url, { method: message.request.method, headers: Object.fromEntries(message.request.headers), body: message.request.body, redirect: "manual" })
      return { status: response.status, body: await response.text() }
    }
    if (message.kind === "sleep") { await new Promise(resolve => setTimeout(resolve, message.milliseconds)); return null }
    if (message.kind === "now_ms") return Date.now()
    if (message.kind === "acquire_oauth_credentials") return { token_url: this.#configuration.oauth.tokenUrl, client_id: this.#configuration.oauth.clientId, client_secret: this.#configuration.oauth.clientSecret }
    if (message.kind === "load_access_token") return this.#token ?? null
    this.#token = message.token
    return null
  }

  #fail(error: Error): void {
    for (const pending of this.#pending.splice(0)) pending.reject(error)
  }
}

export function connect(configuration: Configuration): Promise<SDK> { return SDK.connect(configuration) }
