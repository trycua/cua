import pino from 'pino';
import { type BaseComputerInterface, InterfaceFactory } from '../../interface/index';
import type { CloudComputerConfig, VMProviderType } from '../types';
import { BaseComputer } from './base';

const DEFAULT_API_BASE = process.env.CUA_API_BASE || 'https://api.cua.ai';

/**
 * Cloud-specific computer implementation
 */
export class CloudComputer extends BaseComputer {
  protected static vmProviderType: VMProviderType.CLOUD;
  protected apiKey: string;
  private iface?: BaseComputerInterface;
  private initialized = false;
  private resolvedIp?: string;

  protected logger = pino({ name: 'computer.provider_cloud' });

  constructor(config: CloudComputerConfig) {
    super(config);
    this.apiKey = config.apiKey;
  }

  get ip() {
    return this.resolvedIp || `${this.name}.containers.cloud.trycua.com`;
  }

  /**
   * Fetch the API endpoint hostname from the management API.
   * Looks for an 'api' endpoint in the VM's endpoints list.
   */
  private async fetchApiHost(): Promise<string | null> {
    const apiBase = DEFAULT_API_BASE.replace(/\/+$/, '');
    try {
      const resp = await fetch(`${apiBase}/v1/vms`, {
        headers: {
          Authorization: `Bearer ${this.apiKey}`,
          Accept: 'application/json',
        },
        signal: AbortSignal.timeout(5000),
      });
      if (!resp.ok) return null;
      const data = await resp.json();
      if (!Array.isArray(data)) return null;
      for (const item of data) {
        if (item?.name !== this.name) continue;
        for (const ep of item.endpoints || []) {
          if (ep?.name === 'api' && ep?.host) {
            return ep.host;
          }
        }
        break;
      }
    } catch (e) {
      this.logger.debug(`Failed to fetch VM endpoint info from API: ${e}`);
    }
    return null;
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
      // Resolve the correct endpoint hostname from the API
      const apiHost = await this.fetchApiHost();
      if (apiHost) {
        this.resolvedIp = apiHost;
        this.logger.info(`Using API endpoint hostname: ${apiHost}`);
      }

      // For cloud provider, the VM is already running, we just need to connect
      const ipAddress = this.ip;
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
