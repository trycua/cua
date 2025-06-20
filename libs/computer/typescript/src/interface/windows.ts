/**
 * Windows computer interface implementation.
 */

import { MacOSComputerInterface } from "./macos";

/**
 * Windows interface implementation.
 * Since the cloud provider uses the same WebSocket protocol for all OS types,
 * we can reuse the macOS implementation.
 */
export class WindowsComputerInterface extends MacOSComputerInterface {
  // Windows uses the same WebSocket interface as macOS for cloud provider
}
