import type { HostComputerConfig } from '../types';
import { Computer } from './computer';

/**
 * Host computer implementation.
 *
 * Connects to a Computer Server running on the local host (no VM, no cloud, no
 * API key) to drive the local desktop. This is a thin, explicitly-typed wrapper
 * over the unified {@link Computer} class — `new Computer({ useHostComputerServer:
 * true })` behaves identically.
 *
 * @example
 * ```ts
 * const computer = new HostComputer();
 * await computer.run(); // Connect to the host desktop
 * ```
 */
export class HostComputer extends Computer {
  constructor(config: Omit<HostComputerConfig, 'useHostComputerServer'> = {}) {
    super({ ...config, useHostComputerServer: true });
  }
}
