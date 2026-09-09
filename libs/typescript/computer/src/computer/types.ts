import type { OSType, ScreenSize } from '../types';

/** Display configuration for the computer. */
export interface Display extends ScreenSize {
  scale_factor?: number;
}

export interface BaseComputerConfig {
  name: string;
  osType: OSType;
  vmProvider?: VMProviderType;
}

export interface CloudComputerConfig extends BaseComputerConfig {
  apiKey: string;
}

export interface FleetComputerConfig extends BaseComputerConfig {
  accessToken: string;
  poolName: string;
  namespace?: string;
  fleetBaseUrl?: string;
  serviceName?: string;
  poolPollIntervalMs?: bigint;
  poolPollLimit?: number;
  claimPollIntervalMs?: bigint;
  claimPollLimit?: number;
}

export enum VMProviderType {
  DOCKER = 'docker',
  LUME = 'lume',
  CLOUD = 'cloud',
  FLEET = 'fleet',
  QEMU = 'qemu',
  WINDOWS_SANDBOX = 'windows-sandbox',
  HOST = 'host',
}
