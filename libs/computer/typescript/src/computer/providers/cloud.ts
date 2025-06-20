import { BaseComputer } from "./base";
import type { CloudComputerConfig, VMProviderType } from "../types";
import {
  InterfaceFactory,
  type BaseComputerInterface,
} from "../../interface/index";
import pino from "pino";

const logger = pino({ name: "computer-cloud" });

/**
 * Cloud-specific computer implementation
 */
export class CloudComputer extends BaseComputer {
  protected static vmProviderType: VMProviderType.CLOUD;
  protected apiKey: string;
  private iface?: BaseComputerInterface;
  private initialized = false;

  constructor(config: CloudComputerConfig) {
    super(config);
    this.apiKey = config.apiKey;
  }

  get ip() {
    return `${this.name}.containers.cloud.trycua.com`;
  }

  /**
   * Initialize the cloud VM and interface
   */
  async run(): Promise<void> {
    if (this.initialized) {
      logger.info("Computer already initialized, skipping initialization");
      return;
    }

    try {
      // For cloud provider, the VM is already running, we just need to connect
      const ipAddress = this.ip;
      logger.info(`Connecting to cloud VM at ${ipAddress}`);

      // Create the interface with API key authentication
      this.iface = InterfaceFactory.createInterfaceForOS(
        this.osType,
        ipAddress,
        this.apiKey,
        this.name
      );

      // Wait for the interface to be ready
      logger.info("Waiting for interface to be ready...");
      await this.iface.waitForReady();

      this.initialized = true;
      logger.info("Cloud computer ready");
    } catch (error) {
      logger.error(`Failed to initialize cloud computer: ${error}`);
      throw new Error(`Failed to initialize cloud computer: ${error}`);
    }
  }

  /**
   * Stop the cloud computer (disconnect interface)
   */
  async stop(): Promise<void> {
    logger.info("Disconnecting from cloud computer...");

    if (this.iface) {
      this.iface.disconnect();
      this.iface = undefined;
    }

    this.initialized = false;
    logger.info("Disconnected from cloud computer");
  }

  /**
   * Get the computer interface
   */
  get interface(): BaseComputerInterface {
    if (!this.iface) {
      throw new Error("Computer not initialized. Call run() first.");
    }
    return this.iface;
  }

  /**
   * Disconnect from the cloud computer
   */
  async disconnect(): Promise<void> {
    await this.stop();
  }
}
