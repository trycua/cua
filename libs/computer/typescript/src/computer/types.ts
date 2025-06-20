import type { OSType, ScreenSize } from "../types";

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

export interface LumeComputerConfig extends BaseComputerConfig {
  /**
   * The display configuration. Can be:
   * - A Display object
   * - A dict with 'width' and 'height'
   * - A string in format "WIDTHxHEIGHT" (e.g. "1920x1080")
   * @default "1024x768"
   */
  display?: Display | string;

  /**
   * The VM memory allocation. (e.g. "8GB", "4GB", "1024MB")
   * @default "8GB"
   */
  memory?: string;

  /**
   * The VM CPU allocation.
   * @default 4
   */
  cpu?: number;

  /**
   * The VM image name
   * @default "macos-sequoia-cua:latest"
   */
  image?: string;

  /**
   * Optional list of directory paths to share with the VM
   */
  sharedDirectories?: string[];

  /**
   * If True, target localhost instead of starting a VM
   * @default false
   */
  useHostComputerServer?: boolean;

  /**
   * Whether to enable telemetry tracking.
   * @default true
   */
  telemetryEnabled?: boolean;

  /**
   * Optional port to use for the VM provider server
   * @default 7777
   */
  port?: number;

  /**
   * Optional port for the noVNC web interface
   * @default 8006
   */
  noVNCPort?: number;

  /**
   * Host to use for VM provider connections (e.g. "localhost", "host.docker.internal")
   * @default "localhost"
   */
  host?: string;

  /**
   * Optional path for persistent VM storage
   */
  storage?: string;

  /**
   * Whether to use ephemeral storage
   * @default false
   */
  ephemeral?: boolean;

  /**
   * Optional list of experimental features to enable (e.g. ["app-use"])
   */
  experiments?: string[];
}

export enum VMProviderType {
  CLOUD = "cloud",
  LUME = "lume",
}
