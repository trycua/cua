/**
 * Factory for creating computer interfaces.
 */

import type { OSType } from '../types';
import type { BaseComputerInterface, ComputerInterfaceConnection } from './base';
import { LinuxComputerInterface } from './linux';
import { MacOSComputerInterface } from './macos';
import { WindowsComputerInterface } from './windows';

export const InterfaceFactory = {
  /** Create an interface for the specified operating system and connection. */
  createInterfaceForOS(
    os: OSType,
    ipAddress: string,
    apiKey?: string,
    vmName?: string,
    connection?: ComputerInterfaceConnection
  ): BaseComputerInterface {
    switch (os) {
      case 'macos':
        return new MacOSComputerInterface(ipAddress, 'lume', 'lume', apiKey, vmName, connection);
      case 'linux':
        return new LinuxComputerInterface(ipAddress, 'lume', 'lume', apiKey, vmName, connection);
      case 'windows':
        return new WindowsComputerInterface(ipAddress, 'lume', 'lume', apiKey, vmName, connection);
      default:
        throw new Error(`Unsupported OS type: ${os}`);
    }
  },
};
