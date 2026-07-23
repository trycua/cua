import { createHash } from 'node:crypto';
import os from 'node:os';
import pino from 'pino';
import { v4 as uuidv4 } from 'uuid';
import { cuaVersionHeaders } from '@trycua/core';
import { type BaseComputerInterface, InterfaceFactory } from '../../interface/index';
import { OSType } from '../../types';
import type { ComputerConfig } from '../types';
import { VMProviderType } from '../types';
import { BaseComputer } from './base';

/**
 * Hash API key using SHA256 for secure telemetry identification.
 */
function hashApiKey(apiKey: string): string {
  return createHash('sha256').update(apiKey).digest('hex');
}

const DEFAULT_API_BASE = process.env.CUA_API_BASE || 'https://api.cua.ai';
const DEFAULT_HOST = 'localhost';
const DEFAULT_HOST_PORT = 8000;

interface VMInfo {
  name: string;
  host?: string;
  status?: string;
}

/**
 * Detect the OS type of the host this process is running on.
 */
function detectHostOSType(): OSType {
  switch (os.platform()) {
    case 'darwin':
      return OSType.MACOS;
    case 'win32':
      return OSType.WINDOWS;
    default:
      return OSType.LINUX;
  }
}

/**
 * Unified Computer client.
 *
 * Supports two connection modes:
 *
 * - **Cloud VM** — provide `apiKey` and the VM `name`. Resolves the VM host via
 *   the Cua API and connects over an authenticated `wss://` channel.
 *
 *   ```ts
 *   const computer = new Computer({ apiKey, name: 's-linux-1234', osType: OSType.LINUX });
 *   await computer.run();
 *   ```
 *
 * - **Host Computer Server** — set `useHostComputerServer: true` to drive the
 *   local desktop through a Computer Server running on this machine (no VM, no
 *   API key). Mirrors the Python SDK's `use_host_computer_server=True`.
 *
 *   ```ts
 *   const computer = new Computer({ useHostComputerServer: true });
 *   await computer.run(); // Connect to the host desktop
 *   ```
 */
export class Computer extends BaseComputer {
  protected apiKey?: string;
  protected useHostComputerServer: boolean;
  protected host: string;
  protected port: number;

  private iface?: BaseComputerInterface;
  private initialized = false;
  private cachedHost?: string;
  private apiBase: string;

  // Session tracking
  private sessionId: string;
  private sessionStartTime?: number;
  private apiKeyHash?: string;

  protected logger = pino({ name: 'computer.provider' });

  constructor(config: ComputerConfig) {
    const useHost =
      ('useHostComputerServer' in config && config.useHostComputerServer === true) ||
      config.vmProvider === VMProviderType.HOST;

    const apiKey = 'apiKey' in config ? config.apiKey : undefined;
    const host = 'host' in config && config.host ? config.host : DEFAULT_HOST;
    const port = 'port' in config && config.port ? config.port : DEFAULT_HOST_PORT;

    // Normalize required base fields. In host mode `name`/`osType` are optional
    // (name is just a label; osType defaults to the current platform).
    const osType = config.osType ?? (useHost ? detectHostOSType() : OSType.MACOS);
    const name = config.name ?? (useHost ? 'host' : '');
    const vmProvider = config.vmProvider ?? (useHost ? VMProviderType.HOST : undefined);

    // Forward apiKey to the base ctor (cloud only) so its init telemetry keeps
    // recording the hashed key, matching the previous CloudComputer behavior.
    super({ name, osType, vmProvider, ...(apiKey ? { apiKey } : {}) });

    this.useHostComputerServer = useHost;
    this.host = host;
    this.port = port;
    this.apiBase = DEFAULT_API_BASE;
    this.sessionId = uuidv4();

    if (!useHost) {
      if (!apiKey) {
        throw new Error(
          'A cloud Computer requires an `apiKey`. To control the local desktop instead, ' +
            'pass `{ useHostComputerServer: true }`.'
        );
      }
      this.apiKey = apiKey;
      this.apiKeyHash = hashApiKey(apiKey);
    }
  }

  /**
   * Get the host for this computer.
   *
   * - Host mode: the configured local host (default `localhost`).
   * - Cloud mode: the cached VM host if resolved, otherwise the default format.
   */
  get ip(): string {
    if (this.useHostComputerServer) {
      return this.host;
    }
    return this.cachedHost || `${this.name}.sandbox.cua.ai`;
  }

  /**
   * Fetch VM list from API and cache the host for this VM (cloud mode).
   */
  private async fetchAndCacheHost(): Promise<string> {
    try {
      const response = await fetch(`${this.apiBase}/v1/vms`, {
        headers: {
          Authorization: `Bearer ${this.apiKey}`,
          Accept: 'application/json',
          ...cuaVersionHeaders(
            'computer',
            typeof __CUA_VERSION__ !== 'undefined' ? __CUA_VERSION__ : ''
          ),
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
   * Initialize the computer and connect its interface.
   */
  async run(): Promise<void> {
    if (this.initialized) {
      this.logger.info('Computer already initialized, skipping initialization');
      return;
    }

    try {
      if (this.useHostComputerServer) {
        const address = `${this.host}:${this.port}`;
        this.logger.info(`Connecting to host Computer Server at ${address}`);

        // No API key: the interface connects over plain ws:// and skips the
        // authentication handshake.
        this.iface = InterfaceFactory.createInterfaceForOS(this.osType, address);
      } else {
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
      }

      // Wait for the interface to be ready
      this.logger.info('Waiting for interface to be ready...');
      await this.iface.waitForReady();

      this.initialized = true;
      this.sessionStartTime = Date.now();

      // Track session start
      this.telemetry.recordEvent('ts_session_start', {
        session_id: this.sessionId,
        os_type: this.osType,
        connection_type: this.useHostComputerServer ? 'host' : 'cloud',
        ...(this.apiKeyHash ? { api_key_hash: this.apiKeyHash } : {}),
      });

      this.logger.info(this.useHostComputerServer ? 'Host computer ready' : 'Cloud computer ready');
    } catch (error) {
      this.logger.error(`Failed to initialize computer: ${error}`);
      throw new Error(`Failed to initialize computer: ${error}`);
    }
  }

  /**
   * Stop the computer (disconnect interface).
   */
  async stop(): Promise<void> {
    this.logger.info('Disconnecting from computer...');

    // Track session end
    if (this.sessionStartTime) {
      const durationSeconds = (Date.now() - this.sessionStartTime) / 1000;
      this.telemetry.recordEvent('ts_session_end', {
        session_id: this.sessionId,
        duration_seconds: durationSeconds,
        ...(this.apiKeyHash ? { api_key_hash: this.apiKeyHash } : {}),
      });
    }

    if (this.iface) {
      this.iface.disconnect();
      this.iface = undefined;
    }

    this.initialized = false;
    this.logger.info('Disconnected from computer');
  }

  /**
   * Get the computer interface.
   */
  get interface(): BaseComputerInterface {
    if (!this.iface) {
      throw new Error('Computer not initialized. Call run() first.');
    }
    return this.iface;
  }

  /**
   * Disconnect from the computer.
   */
  async disconnect(): Promise<void> {
    await this.stop();
  }
}
