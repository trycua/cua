/**
 * Shared API utilities for Lume providers.
 *
 * This module contains shared functions for interacting with the Lume API,
 * used by LumeProvider.
 *
 * WARNING: This file was created as a test/example implementation and is not maintained or expected to work.
 * It serves as a reference for how a provider might be implemented but should not be used in production.
 */

import pino from "pino";

// Setup logging
const logger = pino({ name: "lume_api" });

// Types for API responses and options
// These are lume-specific
export interface SharedDirectory {
  hostPath: string;
  tag: string;
  readOnly: boolean;
}

export interface VMInfo {
  status?: string;
  name: string;
  diskSize: {
    allocated: number;
    total: number;
  };
  memorySize: number;
  os: string;
  display: string;
  locationName: string;
  cpuCount?: number;
  // started state results
  vncUrl?: string;
  ipAddress?: string;
  sharedDirectories?: SharedDirectory[];
}

export interface RunOptions {
  [key: string]: unknown;
}

export interface UpdateOptions {
  [key: string]: unknown;
}

/**
 * Use fetch to get VM information from Lume API.
 *
 * @param vmName - Name of the VM to get info for
 * @param host - API host
 * @param port - API port
 * @param storage - Storage path for the VM
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with VM status information parsed from JSON response
 */
export async function lumeApiGet(
  vmName: string,
  host: string,
  port: number,
  storage?: string,
  debug = false,
  verbose = false
): Promise<VMInfo[]> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    // First encode the storage path properly
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL with encoded storage parameter if needed
  const apiUrl = `http://${host}:${port}/lume/vms${
    vmName ? `/${vmName}` : ""
  }${storageParam}`;

  // Only print the fetch URL when debug is enabled
  logger.info(`Executing API request: ${apiUrl}`);

  try {
    // Execute the request with timeouts
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 20000); // 20 second timeout

    const response = await fetch(apiUrl, {
      method: "GET",
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok) {
      // Handle HTTP errors
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    // Parse JSON response
    // If vmName is provided, API returns a single object; otherwise it returns an array
    const data = await response.json();
    const result = vmName ? [data as VMInfo] : (data as VMInfo[]);

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(result, null, 2)}`);
    }

    return result;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}

/**
 * Run a VM using fetch.
 *
 * @param vmName - Name of the VM to run
 * @param host - API host
 * @param port - API port
 * @param runOpts - Dictionary of run options
 * @param storage - Storage path for the VM
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with API response
 */
export async function lumeApiRun(
  vmName: string,
  host: string,
  port: number,
  runOpts: RunOptions,
  storage?: string,
  debug = false,
  verbose = false
): Promise<VMInfo> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL
  const apiUrl = `http://${host}:${port}/lume/vms/${vmName}/run${storageParam}`;

  // Convert run options to JSON
  const jsonData = JSON.stringify(runOpts);

  if (debug || verbose) {
    console.log(`Executing fetch API call: POST ${apiUrl}`);
    console.log(`Request body: ${jsonData}`);
  }
  logger.info(`Executing API request: POST ${apiUrl}`);

  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 20000);

    const response = await fetch(apiUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: jsonData,
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok) {
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    const data = (await response.json()) as VMInfo;

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(data, null, 2)}`);
    }

    return data;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}

/**
 * Stop a VM using fetch.
 *
 * @param vmName - Name of the VM to stop
 * @param host - API host
 * @param port - API port
 * @param storage - Storage path for the VM
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with API response
 */
export async function lumeApiStop(
  vmName: string,
  host: string,
  port: number,
  storage?: string,
  debug = false,
  verbose = false
): Promise<VMInfo> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL
  const apiUrl = `http://${host}:${port}/lume/vms/${vmName}/stop${storageParam}`;

  if (debug || verbose) {
    console.log(`DEBUG: Executing fetch API call: POST ${apiUrl}`);
  }
  logger.debug(`Executing API request: POST ${apiUrl}`);

  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 20000);

    const response = await fetch(apiUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: "{}",
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok) {
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    const data = (await response.json()) as VMInfo;

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(data, null, 2)}`);
    }

    return data;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}

/**
 * Update VM settings using fetch.
 *
 * @param vmName - Name of the VM to update
 * @param host - API host
 * @param port - API port
 * @param updateOpts - Dictionary of update options
 * @param storage - Storage path for the VM
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with API response
 */
export async function lumeApiUpdate(
  vmName: string,
  host: string,
  port: number,
  updateOpts: UpdateOptions,
  storage?: string,
  debug = false,
  verbose = false
): Promise<VMInfo> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL
  const apiUrl = `http://${host}:${port}/lume/vms/${vmName}/update${storageParam}`;

  // Convert update options to JSON
  const jsonData = JSON.stringify(updateOpts);

  if (debug || verbose) {
    console.log(`DEBUG: Executing fetch API call: POST ${apiUrl}`);
    console.log(`DEBUG: Request body: ${jsonData}`);
  }
  logger.debug(`Executing API request: POST ${apiUrl}`);

  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 20000);

    const response = await fetch(apiUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: jsonData,
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok) {
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    const data = (await response.json()) as VMInfo;

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(data, null, 2)}`);
    }

    return data;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}

/**
 * Pull a VM image from a registry using fetch.
 *
 * @param image - Name/tag of the image to pull
 * @param name - Name to give the VM after pulling
 * @param host - API host
 * @param port - API port
 * @param storage - Storage path for the VM
 * @param registry - Registry to pull from (default: ghcr.io)
 * @param organization - Organization in registry (default: trycua)
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with pull status and information
 */
export async function lumeApiPull(
  image: string,
  name: string,
  host: string,
  port: number,
  storage?: string,
  registry = "ghcr.io",
  organization = "trycua",
  debug = false,
  verbose = false
): Promise<VMInfo> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL
  const apiUrl = `http://${host}:${port}/lume/pull${storageParam}`;

  // Construct pull options
  const pullOpts = {
    image,
    name,
    registry,
    organization,
  };

  const jsonData = JSON.stringify(pullOpts);

  if (debug || verbose) {
    console.log(`DEBUG: Executing fetch API call: POST ${apiUrl}`);
    console.log(`DEBUG: Request body: ${jsonData}`);
  }
  logger.debug(`Executing API request: POST ${apiUrl}`);

  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 60000); // 60 second timeout for pulls

    const response = await fetch(apiUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: jsonData,
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok) {
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    const data = (await response.json()) as VMInfo;

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(data, null, 2)}`);
    }

    return data;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}

/**
 * Delete a VM using fetch.
 *
 * @param vmName - Name of the VM to delete
 * @param host - API host
 * @param port - API port
 * @param storage - Storage path for the VM
 * @param debug - Whether to show debug output
 * @param verbose - Enable verbose logging
 * @returns Dictionary with API response
 */
export async function lumeApiDelete(
  vmName: string,
  host: string,
  port: number,
  storage?: string,
  debug = false,
  verbose = false
): Promise<VMInfo | null> {
  // URL encode the storage parameter for the query
  let storageParam = "";

  if (storage) {
    const encodedStorage = encodeURIComponent(storage);
    storageParam = `?storage=${encodedStorage}`;
  }

  // Construct API URL
  const apiUrl = `http://${host}:${port}/lume/vms/${vmName}${storageParam}`;

  if (debug || verbose) {
    console.log(`DEBUG: Executing fetch API call: DELETE ${apiUrl}`);
  }
  logger.debug(`Executing API request: DELETE ${apiUrl}`);

  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 20000);

    const response = await fetch(apiUrl, {
      method: "DELETE",
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    if (!response.ok && response.status !== 404) {
      const errorMsg = `HTTP error returned from API server (status: ${response.status})`;
      throw new Error(`API request failed: ${errorMsg}`);
    }

    // For 404, return null (VM already deleted)
    if (response.status === 404) {
      if (debug || verbose) {
        console.log("DEBUG: VM not found (404) - treating as already deleted");
      }
      return null;
    }

    // Try to parse JSON response, but handle empty responses
    let data: VMInfo | null = null;
    const contentType = response.headers.get("content-type");
    if (contentType?.includes("application/json")) {
      try {
        data = (await response.json()) as VMInfo;
      } catch {
        // Empty response is OK for DELETE
      }
    } else {
      // No JSON response expected
    }

    if (debug || verbose) {
      console.log(`DEBUG: API response: ${JSON.stringify(data, null, 2)}`);
    }

    return data;
  } catch (err) {
    const error = err as Error;
    let errorMsg = "Unknown error";

    if (error.name === "AbortError") {
      errorMsg =
        "Operation timeout - the API server is taking too long to respond";
    } else if (error.message.includes("ECONNREFUSED")) {
      errorMsg =
        "Failed to connect to the API server - it might still be starting up";
    } else if (error.message.includes("ENOTFOUND")) {
      errorMsg = "Failed to resolve host - check the API server address";
    } else if (error.message) {
      errorMsg = error.message;
    }

    logger.error(`API request failed: ${errorMsg}`);
    throw new Error(`API request failed: ${errorMsg}`);
  }
}
