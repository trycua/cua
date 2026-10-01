/** Node-only composition of an existing Fleet client and the shared Rust Driver. */
import { randomUUID } from 'node:crypto';
import {
  openMcpDriverChannel,
  DriverServiceTransportError,
  type CuaDriverLike,
  type DriverServiceRequest,
  type DriverServiceResponse,
} from './index.js';

const FLEET_DRIVER_MAX_RESPONSE_BYTES = 16n * 1024n * 1024n;

/** Structural subset of Fleet's generated Sandbox record. */
export interface FleetDriverSandbox {
  namespace: string;
  claim: string;
  name: string;
  services: string[];
}

/** Compatible with the generated Fleet client; no shared native pointers. */
export interface FleetDriverClient {
  serviceRequest(
    sandbox: FleetDriverSandbox,
    service: string,
    path: string,
    request: {
      method: string;
      url: string;
      headers: { name: string; value: string }[];
      body?: ArrayBuffer;
      timeoutSecs?: bigint;
      maxResponseBytes?: bigint;
    },
    options?: { signal: AbortSignal }
  ): Promise<{
    status: number;
    headers: { name: string; value: string }[];
    body: ArrayBuffer;
  }>;
}

export interface FleetDriverConnection {
  readonly driver: CuaDriverLike;
  readonly sessionName: string;
  /** Close before releasing the Fleet claim. This never releases the claim. */
  close(): Promise<void>;
}

/**
 * Connect to an advertised service on the caller's live Fleet claim.
 * The caller owns the client and claim, and must await close before releasing
 * either. Abort closes this connection; it does not undo desktop actions.
 */
export async function connectFleetDriver({
  client,
  sandbox,
  service = 'mcp',
  signal,
}: {
  client: FleetDriverClient;
  sandbox: FleetDriverSandbox;
  service?: string;
  signal?: AbortSignal;
}): Promise<FleetDriverConnection> {
  const target: FleetDriverSandbox = {
    namespace: sandbox.namespace,
    claim: sandbox.claim,
    name: sandbox.name,
    services: [...sandbox.services],
  };
  if (!target.services.includes(service)) {
    throw new Error('Fleet sandbox does not advertise the requested Driver service');
  }
  if (signal?.aborted) throw new Error('Fleet Driver connection aborted');
  const serviceRequest = client.serviceRequest.bind(client);
  let closing: Promise<void> | undefined;

  async function send(request: DriverServiceRequest): Promise<DriverServiceResponse> {
    // Keep routing authority in Fleet even if the native boundary changes.
    if (
      !request.path.startsWith('/') ||
      request.path.length > 2048 ||
      request.path.startsWith('//') ||
      /[\\?#\s%]/u.test(request.path) ||
      request.path.split('/').some((part) => part === '.' || part === '..') ||
      request.timeoutMs <= 0n ||
      request.timeoutMs > 2_147_483_647n
    ) {
      throw new DriverServiceTransportError.Failed({
        reason: 'Invalid bounded Driver service request',
      });
    }
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), Number(request.timeoutMs));
    let onAbort: () => void = () => {};
    const aborted = new Promise<never>((_, reject) => {
      onAbort = () => reject(new Error('Fleet Driver service request canceled'));
      controller.signal.addEventListener('abort', onAbort, { once: true });
    });
    try {
      const response = await Promise.race([
        serviceRequest(
          { ...target, services: [...target.services] },
          service,
          request.path,
          {
            method: request.method,
            url: `https://service.invalid${request.path}`,
            headers: request.headers.map((header) => ({ ...header })),
            body: request.body,
            timeoutSecs: (request.timeoutMs + 999n) / 1000n,
            maxResponseBytes: FLEET_DRIVER_MAX_RESPONSE_BYTES,
          },
          { signal: controller.signal }
        ),
        aborted,
      ]);
      if (BigInt(response.body.byteLength) > FLEET_DRIVER_MAX_RESPONSE_BYTES) {
        throw new Error('Fleet Driver response exceeds the configured size limit');
      }
      return response;
    } catch {
      // Native callback errors must not expose Fleet credentials or response bodies.
      throw new DriverServiceTransportError.Failed({
        reason: 'Fleet Driver service request failed',
      });
    } finally {
      clearTimeout(timeout);
      controller.signal.removeEventListener('abort', onAbort);
    }
  }

  const channel = openMcpDriverChannel({ send }, `fleet:${randomUUID()}`);
  const close = (): Promise<void> => {
    if (!closing) {
      signal?.removeEventListener('abort', onAbort);
      // Retain the promise so cancellation cannot orphan cleanup or hide errors
      // from a later explicit close. Rust bounds its own session teardown.
      closing = channel.close();
    }
    return closing;
  };
  const onAbort = () => {
    void close().catch(() => {});
  };
  signal?.addEventListener('abort', onAbort, { once: true });
  try {
    if (signal?.aborted) throw new Error('Fleet Driver connection aborted');
    await channel.open();
    if (signal?.aborted || closing) throw new Error('Fleet Driver connection aborted');
    return Object.freeze({ driver: channel.driver(), sessionName: channel.publicSession(), close });
  } catch (error) {
    await close();
    throw error;
  }
}
