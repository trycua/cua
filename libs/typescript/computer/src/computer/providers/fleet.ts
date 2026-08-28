import { createHash } from 'node:crypto';
import {
  CreateClaimRequestBuilder,
  CyclopsTokenProviderConfigurationBuilder,
  createFleetClient,
  type Claim,
  type CyclopsClient,
} from '@trycua/fleet/node';
import pino from 'pino';
import { v4 as uuidv4 } from 'uuid';
import { type BaseComputerInterface, InterfaceFactory } from '../../interface/index';
import type { FleetComputerConfig } from '../types';
import { VMProviderType } from '../types';
import { BaseComputer } from './base';

const DEFAULT_FLEET_BASE_URL = process.env.CUA_FLEET_BASE_URL || 'https://run.cua.ai';
const DEFAULT_SERVICE_NAME = 'server';

function hashAccessToken(accessToken: string): string {
  return createHash('sha256').update(accessToken).digest('hex');
}

function websocketProxyUrl(
  baseUrl: string,
  namespace: string,
  sandboxName: string,
  serviceName: string
): string {
  const url = new URL(
    `/api/svc/${encodeURIComponent(namespace)}/${encodeURIComponent(`${sandboxName}-${serviceName}`)}/ws`,
    baseUrl
  );
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

/** Computer implementation backed by a claim from an existing Fleet pool. */
export class FleetComputer extends BaseComputer {
  private readonly config: FleetComputerConfig;
  private readonly sessionId = uuidv4();
  private readonly accessTokenHash: string;
  private iface?: BaseComputerInterface;
  private client?: CyclopsClient;
  private claim?: Claim;
  private proxyUrl?: string;
  private initialized = false;
  private sessionStartTime?: number;

  protected logger = pino({ name: 'computer.provider_fleet' });

  constructor(config: FleetComputerConfig) {
    super({ ...config, vmProvider: VMProviderType.FLEET });
    this.config = config;
    this.accessTokenHash = hashAccessToken(config.accessToken);
  }

  get ip(): string {
    return this.proxyUrl ?? this.config.fleetBaseUrl ?? DEFAULT_FLEET_BASE_URL;
  }

  async run(): Promise<void> {
    if (this.initialized) return;

    try {
      const configuration = new CyclopsTokenProviderConfigurationBuilder()
        .baseUrl(this.config.fleetBaseUrl ?? DEFAULT_FLEET_BASE_URL)
        .poolPollIntervalMs(this.config.poolPollIntervalMs ?? 5_000n)
        .poolPollLimit(this.config.poolPollLimit ?? 120)
        .claimPollIntervalMs(this.config.claimPollIntervalMs ?? 5_000n)
        .claimPollLimit(this.config.claimPollLimit ?? 120)
        .build();
      this.client = await createFleetClient(configuration, this.config.accessToken);

      const pool = await this.client.getPool(this.config.poolName);
      if (this.config.namespace && pool.metadata.namespace !== this.config.namespace) {
        throw new Error(
          `Pool ${this.config.poolName} belongs to namespace ${pool.metadata.namespace}`
        );
      }

      this.claim = await this.client.createClaim(
        new CreateClaimRequestBuilder().pool(pool).build()
      );
      const sandbox = await this.client.waitClaim(this.claim);
      this.proxyUrl = websocketProxyUrl(
        this.config.fleetBaseUrl ?? DEFAULT_FLEET_BASE_URL,
        sandbox.namespace,
        sandbox.name,
        this.config.serviceName ?? DEFAULT_SERVICE_NAME
      );
      this.iface = InterfaceFactory.createInterfaceForOS(
        this.osType,
        sandbox.name,
        undefined,
        undefined,
        {
          wsUrl: this.proxyUrl,
          headers: { Authorization: `Bearer ${this.config.accessToken}` },
        }
      );
      await this.iface.waitForReady();

      this.initialized = true;
      this.sessionStartTime = Date.now();
      this.telemetry.recordEvent('ts_session_start', {
        session_id: this.sessionId,
        os_type: this.osType,
        connection_type: 'fleet',
        access_token_hash: this.accessTokenHash,
        pool_name: this.config.poolName,
      });
    } catch (error) {
      try {
        await this.release();
      } catch (releaseError) {
        this.logger.warn(
          `Failed to release Fleet claim after initialization error: ${releaseError}`
        );
      }
      throw new Error(`Failed to initialize Fleet computer: ${error}`);
    }
  }

  async stop(): Promise<void> {
    if (this.sessionStartTime) {
      this.telemetry.recordEvent('ts_session_end', {
        session_id: this.sessionId,
        duration_seconds: (Date.now() - this.sessionStartTime) / 1000,
        access_token_hash: this.accessTokenHash,
        pool_name: this.config.poolName,
      });
    }
    await this.release();
  }

  get interface(): BaseComputerInterface {
    if (!this.iface) throw new Error('Computer not initialized. Call run() first.');
    return this.iface;
  }

  async disconnect(): Promise<void> {
    await this.stop();
  }

  private async release(): Promise<void> {
    this.iface?.disconnect();
    this.iface = undefined;

    const client = this.client;
    const claim = this.claim;
    this.client = undefined;
    this.claim = undefined;
    this.initialized = false;
    this.sessionStartTime = undefined;
    this.proxyUrl = undefined;

    try {
      if (client && claim) await client.deleteClaim(claim);
    } finally {
      client?.uniffiDestroy();
    }
  }
}
