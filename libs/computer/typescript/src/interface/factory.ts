/**
 * Factory for creating computer interfaces.
 */

import type { BaseComputerInterface } from "./base";
import { MacOSComputerInterface } from "./macos";
import { LinuxComputerInterface } from "./linux";
import { WindowsComputerInterface } from "./windows";
import type { OSType } from "../types";

export const InterfaceFactory = {
  /**
   * Create an interface for the specified OS.
   *
   * @param os Operating system type ('macos', 'linux', or 'windows')
   * @param ipAddress IP address of the computer to control
   * @param apiKey Optional API key for cloud authentication
   * @param vmName Optional VM name for cloud authentication
   * @returns The appropriate interface for the OS
   * @throws Error if the OS type is not supported
   */
  createInterfaceForOS(
    os: OSType,
    ipAddress: string,
    apiKey?: string,
    vmName?: string
  ): BaseComputerInterface {
    switch (os) {
      case "macos":
        return new MacOSComputerInterface(
          ipAddress,
          "lume",
          "lume",
          apiKey,
          vmName
        );
      case "linux":
        return new LinuxComputerInterface(
          ipAddress,
          "lume",
          "lume",
          apiKey,
          vmName
        );
      case "windows":
        return new WindowsComputerInterface(
          ipAddress,
          "lume",
          "lume",
          apiKey,
          vmName
        );
      default:
        throw new Error(`Unsupported OS type: ${os}`);
    }
  },
};
