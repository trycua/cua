import { randomUUID } from 'node:crypto';
import {
  CyclopsClient,
  CyclopsCredentials,
  SdkError,
  uniffiInitAsync,
  type CyclopsClientLike,
  type HttpClient,
  type HttpRequest,
  type HttpResponse,
  type Claim,
  type Sandbox,
} from '@trycua/fleet/node';

export interface FleetSession {
  start(): Promise<void>;
  command(name: string, params: Record<string, unknown>): Promise<Record<string, unknown>>;
  close(): Promise<void>;
}

export interface SessionOptions {
  poolName: string;
  clientId: string;
  clientSecret: string;
  claimTtlSeconds: number;
  startupTimeoutMs: number;
  requestTimeoutMs: number;
}

export class CuaFleetError extends Error {
  constructor(public readonly code: string) {
    super(`Cua Fleet: ${code}`);
    this.name = 'CuaFleetError';
  }
}

// Never forward raw SDK exceptions: they can include authenticated HTTP details.
export function safeError(error: unknown, fallback: string): CuaFleetError {
  return error instanceof CuaFleetError ? error : new CuaFleetError(fallback);
}

export class FleetHttpClient implements HttpClient {
  constructor(private readonly timeoutMs: number) {}
  async execute(request: HttpRequest, options?: { signal: AbortSignal }): Promise<HttpResponse> {
    const timeout = AbortSignal.timeout(this.timeoutMs);
    const response = await fetch(request.url, {
      method: request.method,
      headers: request.headers.map(({ name, value }) => [name, value]),
      body: request.body,
      redirect: 'error',
      signal: options?.signal ? AbortSignal.any([options.signal, timeout]) : timeout,
    });
    return {
      status: response.status,
      headers: [...response.headers].map(([name, value]) => ({ name, value })),
      body: await response.arrayBuffer(),
    };
  }
}

export function decodeGuestResponse(bytes: ArrayBuffer): Record<string, unknown> {
  const text = new TextDecoder().decode(bytes).trim();
  const data = text.startsWith('data:')
    ? text
        .split(/\r?\n/)
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).trim())
        .join('\n')
    : text;
  try {
    const result: unknown = JSON.parse(data);
    if (!result || typeof result !== 'object' || Array.isArray(result)) throw new Error();
    return result as Record<string, unknown>;
  } catch {
    throw new CuaFleetError('INVALID_GUEST_RESPONSE');
  }
}

export class CloudFleetSession implements FleetSession {
  #options: SessionOptions;
  #client?: CyclopsClientLike;
  #credentials?: CyclopsCredentials;
  #claim?: Claim;
  #sandbox?: Sandbox;
  #namespace?: string;
  #claimName = `mastra-${randomUUID()}`;
  #createAttempted = false;

  constructor(options: SessionOptions) {
    this.#options = options;
  }

  async start(): Promise<void> {
    const options = this.#options;
    await uniffiInitAsync();
    this.#credentials = new CyclopsCredentials(options.clientId, options.clientSecret);
    this.#client = CyclopsClient.connect(
      {
        baseUrl: 'https://run.cua.ai',
        tokenUrl: 'https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token',
        credentials: this.#credentials,
        poolPollIntervalMs: 2_000n,
        poolPollLimit: Math.ceil(options.startupTimeoutMs / 2_000),
        claimPollIntervalMs: 2_000n,
        claimPollLimit: Math.ceil(options.startupTimeoutMs / 2_000),
      },
      new FleetHttpClient(options.requestTimeoutMs)
    );
    const signal = AbortSignal.timeout(options.startupTimeoutMs);
    const pool = await this.#client.getPool(options.poolName, { signal });
    this.#namespace = pool.metadata.namespace;
    this.#createAttempted = true;
    try {
      this.#claim = await this.#client.createClaim(
        {
          pool,
          name: this.#claimName,
          spec: {
            sandboxTemplateRef: pool.spec.sandboxTemplateRef,
            bindDeadline: Math.ceil(options.startupTimeoutMs / 1000),
            ttlSecondsAfterCreated: options.claimTtlSeconds,
          },
        },
        { signal }
      );
    } catch (error) {
      if (
        SdkError.Status.instanceOf(error) &&
        [400, 401, 403, 404, 409, 422].includes(error.inner.status)
      ) {
        this.#createAttempted = false;
      }
      throw error;
    }
    this.#sandbox = await this.#client.waitClaim(this.#claim, { signal });
    while (!signal.aborted) {
      try {
        await this.#request('/status', undefined, signal);
        return;
      } catch (error) {
        if (signal.aborted) break;
        if (
          error instanceof CuaFleetError &&
          ['GUEST_HTTP_401', 'GUEST_HTTP_403'].includes(error.code)
        )
          throw error;
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }
    throw new CuaFleetError('STARTUP_TIMEOUT');
  }

  async #request(path: string, body?: object, signal?: AbortSignal) {
    if (!this.#client || !this.#sandbox) throw new CuaFleetError('NOT_RUNNING');
    const response = await this.#client.serviceRequest(
      this.#sandbox,
      'server',
      path,
      {
        method: body ? 'POST' : 'GET',
        url: `https://ignored.invalid${path}`,
        headers: body ? [{ name: 'content-type', value: 'application/json' }] : [],
        body: body ? new TextEncoder().encode(JSON.stringify(body)).buffer : undefined,
        timeoutSecs: BigInt(Math.ceil(this.#options.requestTimeoutMs / 1000)),
      },
      signal ? { signal } : undefined
    );
    if (response.status !== 200) throw new CuaFleetError(`GUEST_HTTP_${response.status}`);
    return decodeGuestResponse(response.body);
  }

  async command(name: string, params: Record<string, unknown>) {
    const response = await this.#request('/cmd', { command: name, params });
    if (response.success !== true) throw new CuaFleetError('GUEST_COMMAND_FAILED');
    return response;
  }

  async close(): Promise<void> {
    const client = this.#client;
    if (client && this.#createAttempted && this.#namespace) {
      // Recover an owned named claim even if its create response was lost.
      const deadline = Date.now() + this.#options.requestTimeoutMs;
      let claim = (await client.listClaims(this.#namespace)).find(
        (item) => item.metadata.name === this.#claimName
      );
      // Absence is not conclusive when a timed-out create may still complete.
      while (!claim && !this.#claim && Date.now() < deadline) {
        await new Promise((resolve) =>
          setTimeout(resolve, Math.min(1000, this.#options.requestTimeoutMs))
        );
        claim = (await client.listClaims(this.#namespace)).find(
          (item) => item.metadata.name === this.#claimName
        );
      }
      if (!claim && !this.#claim) throw new CuaFleetError('CLAIM_CREATION_OUTCOME_UNKNOWN');
      if (claim) await client.deleteClaim(claim);
      // Retain confirmed identity for a retry if deletion succeeded but verification failed.
      if (claim) this.#claim = claim;
      while (
        (await client.listClaims(this.#namespace)).some(
          (item) => item.metadata.name === this.#claimName
        )
      ) {
        if (Date.now() >= deadline) throw new CuaFleetError('CLAIM_CLEANUP_PENDING');
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }
    this.#claim = undefined;
    this.#sandbox = undefined;
    this.#createAttempted = false;
    if (client instanceof CyclopsClient) client.uniffiDestroy();
    this.#client = undefined;
    this.#credentials?.uniffiDestroy();
    this.#credentials = undefined;
  }
}
