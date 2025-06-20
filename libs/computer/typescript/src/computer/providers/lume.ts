/**
 * WARNING: This file was created as a test/example implementation and is not maintained or expected to work.
 * It serves as a reference for how a provider might be implemented but should not be used in production.
 */

import type { Display, LumeComputerConfig } from "../types.ts";
import { BaseComputer } from "./base.ts";
import {
  lumeApiGet,
  lumeApiRun,
  lumeApiPull,
  lumeApiStop,
  lumeApiDelete,
  lumeApiUpdate,
  type VMInfo,
} from "../../util/lume.ts";
import pino from "pino";

const logger = pino({ name: "computer-lume" });

/**
 * Lume-specific computer implementation
 */
export class LumeComputer extends BaseComputer {
  private display: string | Display;
  private memory: string;
  private cpu: number;
  private image: string;
  private port: number;
  private host: string;
  private ephemeral: boolean;

  constructor(config: LumeComputerConfig) {
    super(config);

    this.display = config.display ?? "1024x768";
    this.memory = config.memory ?? "8GB";
    this.cpu = config.cpu ?? 2;
    this.image = config.image ?? "macos-sequoia-cua:latest";
    this.port = config.port ?? 7777;
    this.host = config.host ?? "localhost";
    this.ephemeral = config.ephemeral ?? false;
  }

  /**
   * Lume-specific method to get a VM
   */

  async getVm(name: string, storage?: string): Promise<VMInfo> {
    try {
      const vmInfo = (await lumeApiGet(name, this.host, this.port, storage))[0];
      if (!vmInfo) throw new Error("VM Not Found.");
      if (vmInfo.status === "stopped") {
        logger.info(
          `VM ${name} is in '${vmInfo.status}' state - not waiting for IP address`
        );
        return {
          ...vmInfo,
          name,
          status: vmInfo.status,
        };
      }
      if (!vmInfo.ipAddress) {
        logger.info(
          `VM ${name} is in '${vmInfo.status}' state but no IP address found - reporting as still starting`
        );
      }
      return vmInfo;
    } catch (e) {
      logger.error(`Failed to get VM status: ${e}`);
      throw e;
    }
  }

  /**
   * Lume-specific method to list availalbe VMs
   */

  async listVm() {
    const vms = await lumeApiGet("", this.host, this.port);
    return vms;
  }

  /**
   * Lume-specific method to run the VM
   */
  async runVm(
    image: string,
    name: string,
    runOpts: { [key: string]: unknown } = {},
    storage?: string
  ): Promise<VMInfo> {
    logger.info(
      `Running Lume computer ${this.name} with ${this.memory} memory and ${this.cpu} CPUs`
    );
    logger.info(
      `Using image ${this.image} with display ${
        typeof this.display === "string"
          ? this.display
          : `${this.display.width}x${this.display.height}`
      }`
    );
    // Lume-specific implementation
    try {
      await this.getVm(name, storage);
    } catch {
      logger.info(
        `VM ${name} not found, attempting to pull image ${image} from registry...`
      );
      // Call pull_vm with the image parameter
      try {
        const pullRes = await this.pullVm(name, image, storage);
        logger.info(pullRes);
      } catch (e) {
        logger.info(`Failed to pull VM image: ${e}`);
        throw e;
      }
    }
    logger.info(`Running VM ${name} with options: ${runOpts}`);
    return await lumeApiRun(name, this.host, this.port, runOpts, storage);
  }

  /**
   * Lume-specific method to stop a VM
   */
  async stopVm(name: string, storage?: string): Promise<VMInfo> {
    // Stop the VM first
    const stopResult = await lumeApiStop(name, this.host, this.port, storage);

    // If ephemeral mode is enabled, delete the VM after stopping
    if (this.ephemeral && (!stopResult || !("error" in stopResult))) {
      logger.info(
        `Ephemeral mode enabled - deleting VM ${name} after stopping`
      );
      try {
        const deleteResult = await this.deleteVm(name, storage);

        // Return combined result
        return {
          ...stopResult,
          deleted: true,
          deleteResult: deleteResult,
        } as VMInfo;
      } catch (e) {
        logger.error(`Failed to delete ephemeral VM ${name}: ${e}`);
        throw new Error(`Failed to delete ephemeral VM ${name}: ${e}`);
      }
    }

    // Just return the stop result if not ephemeral
    return stopResult;
  }

  /**
   * Lume-specific method to pull a VM image from the registry
   */
  async pullVm(
    name: string,
    image: string,
    storage?: string,
    registry = "ghcr.io",
    organization = "trycua"
  ): Promise<VMInfo> {
    // Validate image parameter
    if (!image) {
      throw new Error("Image parameter is required for pullVm");
    }

    logger.info(`Pulling VM image '${image}' as '${name}'`);
    logger.info("You can check the pull progress using: lume logs -f");
    logger.debug(`Pull storage location: ${storage || "default"}`);

    try {
      const result = await lumeApiPull(
        image,
        name,
        this.host,
        this.port,
        storage,
        registry,
        organization
      );

      logger.info(`Successfully pulled VM image '${image}' as '${name}'`);
      return result;
    } catch (e) {
      logger.error(`Failed to pull VM image '${image}': ${e}`);
      throw new Error(`Failed to pull VM: ${e}`);
    }
  }

  /**
   * Lume-specific method to delete a VM permanently
   */
  async deleteVm(name: string, storage?: string): Promise<VMInfo | null> {
    logger.info(`Deleting VM ${name}...`);

    try {
      const result = await lumeApiDelete(
        name,
        this.host,
        this.port,
        storage,
        false,
        false
      );

      logger.info(`Successfully deleted VM '${name}'`);
      return result;
    } catch (e) {
      logger.error(`Failed to delete VM '${name}': ${e}`);
      throw new Error(`Failed to delete VM: ${e}`);
    }
  }

  /**
   * Lume-specific method to update VM configuration
   */
  async updateVm(
    name: string,
    updateOpts: { [key: string]: unknown },
    storage?: string
  ): Promise<VMInfo> {
    return await lumeApiUpdate(
      name,
      this.host,
      this.port,
      updateOpts,
      storage,
      false,
      false
    );
  }

  /**
   * Lume-specific method to get the IP address of a VM, waiting indefinitely until it's available
   */
  async getIp(name: string, storage?: string, retryDelay = 2): Promise<string> {
    // Track total attempts for logging purposes
    let attempts = 0;

    while (true) {
      attempts++;

      try {
        const vmInfo = await this.getVm(name, storage);

        // Check if VM has an IP address
        if (vmInfo.ipAddress) {
          logger.info(
            `Got IP address for VM ${name} after ${attempts} attempts: ${vmInfo.ipAddress}`
          );
          return vmInfo.ipAddress;
        }

        // Check if VM is in a state where it won't get an IP
        if (vmInfo.status === "stopped" || vmInfo.status === "error") {
          throw new Error(
            `VM ${name} is in '${vmInfo.status}' state and will not get an IP address`
          );
        }

        // Log progress every 10 attempts
        if (attempts % 10 === 0) {
          logger.info(
            `Still waiting for IP address for VM ${name} (${attempts} attempts)...`
          );
        }

        // Wait before retrying
        await new Promise((resolve) => setTimeout(resolve, retryDelay * 1000));
      } catch (e) {
        logger.error(`Error getting IP for VM ${name}: ${e}`);
        throw e;
      }
    }
  }
}
