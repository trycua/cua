/**
 * CuaBot Server Client
 * Connects to the CuaBot server via HTTP
 */

import { spawn } from "child_process";
import { existsSync, readFileSync, openSync } from "fs";
import { join, dirname } from "path";
import { homedir } from "os";
import { fileURLToPath } from "url";
import { checkDependencies } from "./utils.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const CONFIG_DIR = join(homedir(), ".cuabot");

const STARTING_ERROR = "cuabotd is still starting";

// Session name support
let currentSessionName: string | null = null;

export function setSessionName(name: string | null): void {
  currentSessionName = name;
}

export function getSessionName(): string | null {
  return currentSessionName;
}

function getPidFile(): string {
  return currentSessionName
    ? join(CONFIG_DIR, `server.${currentSessionName}.pid`)
    : join(CONFIG_DIR, "server.pid");
}

function getPortFile(): string {
  return currentSessionName
    ? join(CONFIG_DIR, `server.${currentSessionName}.port`)
    : join(CONFIG_DIR, "server.port");
}

// Read port from port file, with optional wait
function readPortFromFile(): number | null {
  const portFile = getPortFile();
  if (existsSync(portFile)) {
    try {
      return parseInt(readFileSync(portFile, "utf-8").trim(), 10);
    } catch {
      return null;
    }
  }
  return null;
}

export class CuaBotClient {
  private baseUrl: string;

  constructor(port: number) {
    this.baseUrl = `http://localhost:${port}`;
  }

  private async request(endpoint: string, body?: any): Promise<any> {
    const maxRetries = 120; // 2 minutes max wait
    const retryDelay = 1000; // 1 second between retries

    for (let attempt = 0; attempt < maxRetries; attempt++) {
      const res = await fetch(`${this.baseUrl}/${endpoint}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: body ? JSON.stringify(body) : undefined,
      });

      const data = (await res.json()) as { error?: string; [key: string]: any };
      if (data.error) {
        // If server is still starting, wait and retry
        if (data.error === STARTING_ERROR) {
          if (attempt === 0) {
            console.error("Waiting for cuabotd to finish starting...");
          }
          await new Promise(r => setTimeout(r, retryDelay));
          continue;
        }
        throw new Error(data.error);
      }
      return data;
    }

    throw new Error("Timed out waiting for cuabotd to start");
  }

  async status(): Promise<{ ok: boolean; container: string; containerPort: number | null; playwright: string }> {
    return this.request("status");
  }

  async getContainerPort(): Promise<number | null> {
    const status = await this.status();
    return status.containerPort;
  }

  async bash(
    command: string,
    run_in_background?: boolean,
    timeout?: number
  ): Promise<{ stdout: string; stderr: string; pid?: number }> {
    return this.request("bash", { command, run_in_background, timeout });
  }

  async screenshot(): Promise<string> {
    const result = await this.request("screenshot");
    return result.image;
  }

  async click(x: number, y: number, button: "left" | "right" | "middle" = "left"): Promise<void> {
    await this.request("click", { x, y, button });
  }

  async doubleClick(x: number, y: number): Promise<void> {
    await this.request("doubleClick", { x, y });
  }

  async type(text: string, delay?: number): Promise<void> {
    await this.request("type", { text, delay });
  }

  async mouseMove(x: number, y: number): Promise<void> {
    await this.request("mouseMove", { x, y });
  }

  async mouseDown(x: number, y: number, button: "left" | "right" | "middle" = "left"): Promise<void> {
    await this.request("mouseDown", { x, y, button });
  }

  async mouseUp(x: number, y: number, button: "left" | "right" | "middle" = "left"): Promise<void> {
    await this.request("mouseUp", { x, y, button });
  }

  async scroll(x: number, y: number, deltaX: number, deltaY: number): Promise<void> {
    await this.request("scroll", { x, y, deltaX, deltaY });
  }

  async keyDown(key: string): Promise<void> {
    await this.request("keyDown", { key });
  }

  async keyUp(key: string): Promise<void> {
    await this.request("keyUp", { key });
  }

  async keyPress(key: string): Promise<void> {
    await this.request("keyPress", { key });
  }

  async drag(fromX: number, fromY: number, toX: number, toY: number): Promise<void> {
    await this.request("drag", { fromX, fromY, toX, toY });
  }
}

// Check if server is running and actually responding
export async function isServerRunning(): Promise<{ running: boolean; port: number | null; pid?: number }> {
  const pidFile = getPidFile();
  const portFile = getPortFile();

  if (!existsSync(pidFile)) {
    return { running: false, port: null };
  }

  const pid = parseInt(readFileSync(pidFile, "utf-8").trim(), 10);
  const port = existsSync(portFile)
    ? parseInt(readFileSync(portFile, "utf-8").trim(), 10)
    : null;

  try {
    process.kill(pid, 0);
    // Check if HTTP server is actually responding
    try {
      const res = await fetch(`http://localhost:${port}/status`, { signal: AbortSignal.timeout(2000) });
      if (res.ok) {
        return { running: true, port, pid };
      }
    } catch {
      // HTTP server not ready yet, but process exists
      return { running: false, port, pid };
    }
  } catch {
    return { running: false, port };
  }
  return { running: false, port };
}

// Start the server in background and wait for it to be ready
export async function ensureServerRunning(): Promise<number> {
  const info = await isServerRunning();
  if (info.running && info.port) {
    return info.port;
  }

  // Check dependencies before trying to start the server
  const deps = await checkDependencies();
  if (!deps.ok) {
    console.log("deps", deps);
    throw new Error(`Missing dependencies:\n${deps.errors.map(e => `  - ${e}`).join("\n")}\n\nSee https://github.com/trycua/cuabot#quick-start for installation instructions.`);
  }

  const sessionSuffix = currentSessionName ? ` (session: ${currentSessionName})` : "";
  console.error(`starting cuabotd in background${sessionSuffix}...`);

  // Start server as detached background process using npx cuabot serve
  const spawnArgs = ["cuabot", "--serve"];
  if (currentSessionName) {
    spawnArgs.push("--name", currentSessionName);
  }
  // Write stderr to a log file so we can debug issues
  const logFile = join(CONFIG_DIR, "server.log");
  const logFd = openSync(logFile, "w");

  // console.error(`Server logs will be written to: ${logFile}`);

  const child = spawn("npx", spawnArgs, {
    detached: true,
    stdio: ["ignore", "ignore", logFd],
    cwd: process.cwd(),
    windowsHide: true,
    shell: process.platform === "win32",
  });
  child.unref();

  // Wait for port file to be written, then poll for server ready
  let lastError: Error | null = null;
  for (let i = 0; i < 240; i++) {
    await new Promise(r => setTimeout(r, 500));

    // Try to read port from file
    const port = readPortFromFile();
    if (!port) continue;

    try {
      const res = await fetch(`http://localhost:${port}/status`, { signal: AbortSignal.timeout(1000) });
      if (res.ok) {
        const data = await res.json() as { ready?: boolean };
        if (data.ready) {
          console.error("Server ready!");
          return port;
        }
        // Server responded but not ready yet, keep waiting
      }
    } catch (err) {
      lastError = err instanceof Error ? err : new Error(String(err));
      // Show progress every 10 seconds
      if (i > 0 && i % 20 === 0) {
        console.error(`Waiting for server... (${i * 0.5}s)`);
      }
    }
  }

  const errorMsg = lastError ? `: ${lastError.message}` : "";
  throw new Error(`Failed to start server after 2 minutes${errorMsg}. The server may still be initializing (building Docker image, starting container, etc.). Check server logs at: ${logFile} or try again in a moment.`);
}
