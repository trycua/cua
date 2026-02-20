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

export interface DockerComputerConfig extends BaseComputerConfig {
  /**
   * Docker image to use
   * @default "trycua/cua-ubuntu:latest"
   */
  image?: string;

  /**
   * API server port
   * @default 8000
   */
  port?: number;

  /**
   * VNC port for remote desktop
   * @default 6901
   */
  vncPort?: number;

  /**
   * Memory limit (e.g., "4g", "2048m")
   * @default "4g"
   */
  memory?: string;

  /**
   * CPU count
   * @default 2
   */
  cpu?: number;

  /**
   * Storage path for persistent data
   */
  storage?: string;

  /**
   * Shared directory path to mount in container
   */
  sharedPath?: string;

  /**
   * Use ephemeral (temporary) storage
   * @default false
   */
  ephemeral?: boolean;
}

export enum VMProviderType {
  CLOUD = 'cloud',
  DOCKER = 'docker',
}
