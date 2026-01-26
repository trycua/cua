import { createHash } from 'node:crypto';
import pino from 'pino';
import { v4 as uuidv4 } from 'uuid';
import { type BaseComputerInterface, InterfaceFactory } from '../../interface/index';
import type { CloudComputerConfig, VMProviderType } from '../types';
import { BaseComputer } from './base';

/**
 * Hash API key using SHA256 for secure telemetry identification.
 */
function hashApiKey(apiKey: string): string {
  return createHash('sha256').update(apiKey).digest('hex');
}

const DEFAULT_API_BASE = process.env.CUA_API_BASE || 'https://api.cua.ai';

interface VMInfo {
  name: string;
  host?: string;
  status?: string;
}

/**
 * Cloud-specific computer implementation
 */
export class CloudComputer extends BaseComputer {
  protected static vmProviderType: VMProviderType.CLOUD;
  protected apiKey: string;
  private iface?: BaseComputerInterface;
  private initialized = false;
  private cachedHost?: string;
  private apiBase: string;

  // Session tracking
  private sessionId: string;
  private sessionStartTime?: number;
  private apiKeyHash: string;

  protected logger = pino({ name: 'computer.provider_cloud' });

  constructor(config: CloudComputerConfig) {
    super(config);
    this.apiKey = config.apiKey;
    this.apiBase = DEFAULT_API_BASE;
    this.sessionId = uuidv4();
    this.apiKeyHash = hashApiKey(config.apiKey);
  }

  /**
   * Get the host for this VM.
   * Returns cached host if available, otherwise falls back to default format.
   */
  get ip(): string {
    return this.cachedHost || `${this.name}.sandbox.cua.ai`;
  }

  /**
   * Fetch VM list from API and cache the host for this VM.
   */
  private async fetchAndCacheHost(): Promise<string> {
    try {
      const response = await fetch(`${this.apiBase}/v1/vms`, {
        headers: {
          Authorization: `Bearer ${this.apiKey}`,
          Accept: 'application/json',
        },
      });

      if (response.ok) {
        const vms = (await response.json()) as VMInfo[];
        const vm = vms.find((v) => v.name === this.name);
        if (vm?.host) {
          this.cachedHost = vm.host;
          this.logger.info(`Cached host from API: ${this.cachedHost}`);
          return this.cachedHost;
        }
      }
    } catch (error) {
      this.logger.warn(`Failed to fetch VM list for host lookup: ${error}`);
    }

    // Fall back to default format
    const fallbackHost = `${this.name}.sandbox.cua.ai`;
    this.cachedHost = fallbackHost;
    this.logger.info(`Using fallback host: ${fallbackHost}`);
    return fallbackHost;
  }

  /**
   * Initialize the cloud VM and interface
   */
  async run(): Promise<void> {
    if (this.initialized) {
      this.logger.info('Computer already initialized, skipping initialization');
      return;
    }

    try {
      // Fetch the host from API before connecting
      const ipAddress = await this.fetchAndCacheHost();
      this.logger.info(`Connecting to cloud VM at ${ipAddress}`);

      // Create the interface with API key authentication
      this.iface = InterfaceFactory.createInterfaceForOS(
        this.osType,
        ipAddress,
        this.apiKey,
        this.name
      );

      // Wait for the interface to be ready
      this.logger.info('Waiting for interface to be ready...');
      await this.iface.waitForReady();

      this.initialized = true;
      this.sessionStartTime = Date.now();

      // Track session start with hashed API key
      this.telemetry.recordEvent('ts_session_start', {
        session_id: this.sessionId,
        os_type: this.osType,
        connection_type: 'cloud',
        api_key_hash: this.apiKeyHash,
      });

      this.logger.info('Cloud computer ready');
    } catch (error) {
      this.logger.error(`Failed to initialize cloud computer: ${error}`);
      throw new Error(`Failed to initialize cloud computer: ${error}`);
    }
  }

  /**
   * Stop the cloud computer (disconnect interface)
   */
  async stop(): Promise<void> {
    this.logger.info('Disconnecting from cloud computer...');

    // Track session end with hashed API key
    if (this.sessionStartTime) {
      const durationSeconds = (Date.now() - this.sessionStartTime) / 1000;
      this.telemetry.recordEvent('ts_session_end', {
        session_id: this.sessionId,
        duration_seconds: durationSeconds,
        api_key_hash: this.apiKeyHash,
      });
    }

    if (this.iface) {
      this.iface.disconnect();
      this.iface = undefined;
    }

    this.initialized = false;
    this.logger.info('Disconnected from cloud computer');
  }

  /**
   * Get the computer interface
   */
  get interface(): BaseComputerInterface {
    if (!this.iface) {
      throw new Error('Computer not initialized. Call run() first.');
    }
    return this.iface;
  }

  /**
   * Disconnect from the cloud computer
   */
  async disconnect(): Promise<void> {
    await this.stop();
  }
}
