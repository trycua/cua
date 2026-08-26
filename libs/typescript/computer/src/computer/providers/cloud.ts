import type { CloudComputerConfig } from '../types';
import { Computer } from './computer';

/**
 * Cloud-specific computer implementation.
 *
 * Connects to a Cua cloud VM using an API key. This is a thin, explicitly-typed
 * wrapper over the unified {@link Computer} class — `new Computer(cloudConfig)`
 * behaves identically.
 */
export class CloudComputer extends Computer {
  constructor(config: CloudComputerConfig) {
    super(config);
  }
}
