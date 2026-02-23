import type { OSType, ScreenSize } from '../types';

/**
 * Display configuration for the computer.
 */
export interface Display extends ScreenSize {
  scale_factor?: number;
}

/**
 * Computer configuration model.
 */
export interface BaseComputerConfig {
  /**
   * The VM name
   * @default ""
   */
  name: string;

  /**
   * The operating system type ('macos', 'windows', or 'linux')
   * @default "macos"
   */
  osType: OSType;
}

export interface CloudComputerConfig extends BaseComputerConfig {
  /**
   * Optional API key for cloud providers
   */
  apiKey: string;
}

export enum VMProviderType {
  DOCKER = 'docker',
  LUME = 'lume',
  CLOUD = 'cloud',
  QEMU = 'qemu',
  WINDOWS_SANDBOX = 'windows-sandbox',
  HOST = 'host',
}
