import type { OSType } from "../../types";
import type { BaseComputerConfig, Display, VMProviderType } from "../types";
import pino from "pino";

const logger = pino({ name: "computer-base" });

/**
 * Base Computer class with shared functionality
 */
export abstract class BaseComputer {
  protected name: string;
  protected osType: OSType;
  protected vmProvider?: VMProviderType;

  constructor(config: BaseComputerConfig) {
    this.name = config.name;
    this.osType = config.osType;
  }

  /**
   * Get the name of the computer
   */
  getName(): string {
    return this.name;
  }

  /**
   * Get the OS type of the computer
   */
  getOSType(): OSType {
    return this.osType;
  }

  /**
   * Get the VM provider type
   */
  getVMProviderType(): VMProviderType | undefined {
    return this.vmProvider;
  }

  /**
   * Shared method available to all computer types
   */
  async disconnect(): Promise<void> {
    logger.info(`Disconnecting from ${this.name}`);
    // Implementation would go here
  }

  /**
   * Parse display string into Display object
   * @param display Display string in format "WIDTHxHEIGHT"
   * @returns Display object
   */
  public static parseDisplayString(display: string): Display {
    const match = display.match(/^(\d+)x(\d+)$/);
    if (!match) {
      throw new Error(
        `Invalid display format: ${display}. Expected format: WIDTHxHEIGHT`
      );
    }

    return {
      width: Number.parseInt(match[1], 10),
      height: Number.parseInt(match[2], 10),
    };
  }

  /**
   * Parse memory string to MB integer.
   *
   * Examples:
   *   "8GB" -> 8192
   *   "1024MB" -> 1024
   *   "512" -> 512
   *
   * @param memoryStr - Memory string to parse
   * @returns Memory value in MB
   */
  public static parseMemoryString(memoryStr: string): number {
    if (!memoryStr) {
      return 0;
    }

    // Convert to uppercase for case-insensitive matching
    const upperStr = memoryStr.toUpperCase().trim();

    // Extract numeric value and unit
    const match = upperStr.match(/^(\d+(?:\.\d+)?)\s*(GB|MB)?$/);
    if (!match) {
      throw new Error(`Invalid memory format: ${memoryStr}`);
    }

    const value = Number.parseFloat(match[1]);
    const unit = match[2] || "MB"; // Default to MB if no unit specified

    // Convert to MB
    if (unit === "GB") {
      return Math.round(value * 1024);
    }
    return Math.round(value);
  }
}
