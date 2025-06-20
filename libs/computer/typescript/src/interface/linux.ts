/**
 * Linux computer interface implementation.
 */

import { MacOSComputerInterface } from "./macos";

/**
 * Linux interface implementation.
 * Since the cloud provider uses the same WebSocket protocol for all OS types,
 * we can reuse the macOS implementation.
 */
export class LinuxComputerInterface extends MacOSComputerInterface {
  // Linux uses the same WebSocket interface as macOS for cloud provider
}
