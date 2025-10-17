import { spawn } from 'node:child_process';
import pino from 'pino';
import {
  type BaseComputerInterface,
  InterfaceFactory,
} from '../../interface/index';
import { VMProviderType, type DockerComputerConfig } from '../types';
import { BaseComputer } from './base';

interface DockerContainerInfo {
  name: string;
  status: string;
  ip_address?: string;
  ports: Record<string, string>;
  container_id?: string;
}

/**
 * Docker-specific computer implementation
 */
export class DockerComputer extends BaseComputer {
  protected static vmProviderType = VMProviderType.DOCKER;
  private image: string;
  private port: number;
  private vncPort: number;
  private memory: string;
  private cpu: number;
  private storage?: string;
  private sharedPath?: string;
  private ephemeral: boolean;
  private iface?: BaseComputerInterface;
  private initialized = false;
  private containerId?: string;

  protected logger = pino({ name: 'computer.provider_docker' });

  constructor(config: DockerComputerConfig) {
    super(config);
    this.image = config.image || 'trycua/cua-ubuntu:latest';
    this.port = config.port || 8000;
    this.vncPort = config.vncPort || 6901;
    this.memory = config.memory || '4g';
    this.cpu = config.cpu || 2;
    this.storage = config.storage;
    this.sharedPath = config.sharedPath;
    this.ephemeral = config.ephemeral || false;
  }

  /**
   * Check if Docker is available on the system
   */
  private async hasDocker(): Promise<boolean> {
    return new Promise((resolve) => {
      const proc = spawn('docker', ['--version']);
      proc.on('error', () => resolve(false));
      proc.on('close', (code) => resolve(code === 0));
    });
  }

  /**
   * Execute a Docker command and return the output
   */
  private async execDocker(args: string[]): Promise<{ stdout: string; stderr: string; code: number }> {
    return new Promise((resolve) => {
      const proc = spawn('docker', args);
      let stdout = '';
      let stderr = '';

      proc.stdout?.on('data', (data) => {
        stdout += data.toString();
      });

      proc.stderr?.on('data', (data) => {
        stderr += data.toString();
      });

      proc.on('close', (code) => {
        resolve({ stdout, stderr, code: code || 0 });
      });

      proc.on('error', (err) => {
        resolve({ stdout, stderr: err.message, code: 1 });
      });
    });
  }

  /**
   * Parse memory string to Docker format
   */
  private parseMemory(memoryStr: string): string {
    const match = memoryStr.match(/^(\d+)([A-Za-z]*)$/);
    if (!match) {
      this.logger.warn(`Could not parse memory string '${memoryStr}', using 4g default`);
      return '4g';
    }

    const [, value, unit] = match;
    const upperUnit = unit.toUpperCase();

    if (upperUnit === 'GB' || upperUnit === 'G') {
      return `${value}g`;
    }
    if (upperUnit === 'MB' || upperUnit === 'M' || upperUnit === '') {
      return `${value}m`;
    }

    return '4g';
  }

  /**
   * Get container information
   */
  private async getContainerInfo(name: string): Promise<DockerContainerInfo | null> {
    const result = await this.execDocker(['inspect', name]);

    if (result.code !== 0) {
      return null;
    }

    try {
      const containers = JSON.parse(result.stdout);
      if (!containers || containers.length === 0) {
        return null;
      }

      const container = containers[0];
      const state = container.State;
      const networkSettings = container.NetworkSettings;

      let status = 'stopped';
      if (state.Running) {
        status = 'running';
      } else if (state.Paused) {
        status = 'paused';
      }

      let ipAddress = networkSettings.IPAddress || '';
      if (!ipAddress && networkSettings.Networks) {
        for (const network of Object.values(networkSettings.Networks)) {
          const net = network as { IPAddress?: string };
          if (net.IPAddress) {
            ipAddress = net.IPAddress;
            break;
          }
        }
      }

      const ports: Record<string, string> = {};
      if (networkSettings.Ports) {
        for (const [containerPort, portMappings] of Object.entries(networkSettings.Ports)) {
          if (Array.isArray(portMappings) && portMappings.length > 0) {
            const mapping = portMappings[0] as { HostPort?: string };
            if (mapping.HostPort) {
              ports[containerPort] = mapping.HostPort;
            }
          }
        }
      }

      return {
        name,
        status,
        ip_address: ipAddress || 'localhost',
        ports,
        container_id: container.Id.substring(0, 12),
      };
    } catch (error) {
      this.logger.error(`Failed to parse container info: ${error}`);
      return null;
    }
  }

  /**
   * Wait for container to be ready
   */
  private async waitForContainerReady(name: string, timeout = 60): Promise<boolean> {
    this.logger.info(`Waiting for container ${name} to be ready...`);
    const startTime = Date.now();

    while (Date.now() - startTime < timeout * 1000) {
      const info = await this.getContainerInfo(name);
      if (info?.status === 'running') {
        this.logger.info(`Container ${name} is running`);
        await new Promise((resolve) => setTimeout(resolve, 5000));
        return true;
      }
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }

    this.logger.warn(`Container ${name} did not become ready within ${timeout} seconds`);
    return false;
  }

  /**
   * Run or start the Docker container
   */
  private async runContainer(): Promise<void> {
    const existingInfo = await this.getContainerInfo(this.name);

    if (existingInfo?.status === 'running') {
      this.logger.info(`Container ${this.name} is already running`);
      this.containerId = existingInfo.container_id;
      return;
    }

    if (existingInfo && existingInfo.status !== 'running') {
      this.logger.info(`Starting existing container ${this.name}`);
      const result = await this.execDocker(['start', this.name]);
      if (result.code !== 0) {
        throw new Error(`Failed to start container: ${result.stderr}`);
      }
      await this.waitForContainerReady(this.name);
      const info = await this.getContainerInfo(this.name);
      this.containerId = info?.container_id;
      return;
    }

    this.logger.info(`Creating new container ${this.name} from image ${this.image}`);

    const args = ['run', '-d', '--name', this.name];

    if (this.memory) {
      const memoryLimit = this.parseMemory(this.memory);
      args.push('--memory', memoryLimit);
    }

    if (this.cpu) {
      args.push('--cpus', this.cpu.toString());
    }

    if (this.vncPort) {
      args.push('-p', `${this.vncPort}:6901`);
    }

    if (this.port) {
      args.push('-p', `${this.port}:8000`);
    }

    if (this.storage && !this.ephemeral) {
      args.push('-v', `${this.storage}:/home/kasm-user/storage`);
    }

    if (this.sharedPath) {
      args.push('-v', `${this.sharedPath}:/home/kasm-user/shared`);
    }

    args.push('-e', 'VNC_PW=password');
    args.push('-e', 'VNCOPTIONS=-disableBasicAuth');

    args.push(this.image);

    const result = await this.execDocker(args);

    if (result.code !== 0) {
      throw new Error(`Failed to run container: ${result.stderr}`);
    }

    this.containerId = result.stdout.trim();
    this.logger.info(`Container ${this.name} started with ID: ${this.containerId}`);

    await this.waitForContainerReady(this.name);
  }

  /**
   * Initialize the Docker container and interface
   */
  async run(): Promise<void> {
    if (this.initialized) {
      this.logger.info('Computer already initialized, skipping initialization');
      return;
    }

    const dockerAvailable = await this.hasDocker();
    if (!dockerAvailable) {
      throw new Error('Docker is not available. Please install Docker and ensure it is running.');
    }

    try {
      await this.runContainer();

      const ipAddress = 'localhost';
      this.logger.info(`Connecting to Docker container at ${ipAddress}`);

      this.iface = InterfaceFactory.createInterfaceForOS(
        this.osType,
        ipAddress,
        undefined,
        this.name
      );

      this.logger.info('Waiting for interface to be ready...');
      await this.iface.waitForReady();

      this.initialized = true;
      this.logger.info('Docker computer ready');
    } catch (error) {
      this.logger.error(`Failed to initialize Docker computer: ${error}`);
      throw new Error(`Failed to initialize Docker computer: ${error}`);
    }
  }

  /**
   * Stop the Docker container
   */
  async stop(): Promise<void> {
    this.logger.info('Stopping Docker container...');

    if (this.iface) {
      this.iface.disconnect();
      this.iface = undefined;
    }

    if (this.name) {
      const result = await this.execDocker(['stop', this.name]);
      if (result.code !== 0) {
        this.logger.error(`Failed to stop container: ${result.stderr}`);
      } else {
        this.logger.info(`Container ${this.name} stopped successfully`);
      }
    }

    this.initialized = false;
    this.logger.info('Docker computer stopped');
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
   * Disconnect from the Docker container
   */
  async disconnect(): Promise<void> {
    if (this.iface) {
      this.iface.disconnect();
      this.iface = undefined;
    }
    this.initialized = false;
  }
}
