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

  /**
   * The VM provider type
   */
  vmProvider?: VMProviderType;
}

export interface CloudComputerConfig extends BaseComputerConfig {
  /**
   * Optional API key for cloud providers
   */
  apiKey: string;
}

/**
 * Configuration for connecting to a Computer Server running on the local host
 * (no VM, no cloud). Mirrors the Python SDK's `use_host_computer_server=True`.
 */
export interface HostComputerConfig {
  /**
   * Connect to a Computer Server on the local host instead of a VM/cloud VM.
   */
  useHostComputerServer: true;

  /**
   * Optional label for the host computer.
   * @default "host"
   */
  name?: string;

  /**
   * The operating system type. Defaults to the OS this process is running on.
   */
  osType?: OSType;

  /**
   * Host the Computer Server is reachable at.
   * @default "localhost"
   */
  host?: string;

  /**
   * Port the Computer Server is listening on.
   * @default 8000
   */
  port?: number;

  /**
   * The VM provider type. Defaults to `host` in this mode.
   */
  vmProvider?: VMProviderType;
}

/**
 * Unified configuration accepted by the {@link Computer} class. Either connect
 * to a cloud VM (provide `apiKey` + `name`) or to a local Computer Server
 * (set `useHostComputerServer: true`).
 */
export type ComputerConfig = CloudComputerConfig | HostComputerConfig;

export enum VMProviderType {
  DOCKER = 'docker',
  LUME = 'lume',
  CLOUD = 'cloud',
  QEMU = 'qemu',
  WINDOWS_SANDBOX = 'windows-sandbox',
  HOST = 'host',
}
