/**
 * CuaBot Utility Functions
 * Shared utilities for Docker and Xpra detection
 */

import { exec } from "child_process";
import { existsSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import { promisify } from "util";

const execAsync = promisify(exec);

/**
 * Generate a consistent color from a name using a hash
 */
export function nameToColor(name: string): string {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash) + name.charCodeAt(i);
    hash = hash & hash;
  }

  // Use golden ratio for nice hue distribution
  const hue = (Math.abs(hash) * 0.618033988749895) % 1;

  // Convert HSL to RGB (saturation=0.7, lightness=0.5 for vibrant colors)
  const s = 0.7;
  const l = 0.5;

  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs((hue * 6) % 2 - 1));
  const m = l - c / 2;

  let r: number, g: number, b: number;
  const h = hue * 6;

  if (h < 1) { r = c; g = x; b = 0; }
  else if (h < 2) { r = x; g = c; b = 0; }
  else if (h < 3) { r = 0; g = c; b = x; }
  else if (h < 4) { r = 0; g = x; b = c; }
  else if (h < 5) { r = x; g = 0; b = c; }
  else { r = c; g = 0; b = x; }

  const toHex = (n: number) => Math.round((n + m) * 255).toString(16).padStart(2, "0").toUpperCase();
  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

// Xpra paths by platform
const XPRA_PATHS = {
  win32: "C:\\Program Files\\Xpra\\xpra_cmd.exe",
  darwin: "/Applications/Xpra.app/Contents/MacOS/Xpra",
  linux: "xpra",
} as const;

/**
 * Get the Xpra binary path for the current platform
 */
export function getXpraBinPath(): string {
  const platform = process.platform as keyof typeof XPRA_PATHS;
  return XPRA_PATHS[platform] || "xpra";
}

/**
 * Get the path to an asset file
 */
function getAssetPath(filename: string): string {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  return join(currentDir, "..", "assets", filename);
}

/**
 * Get the Xpra attach command arguments
 */
export function getXpraAttachArgs(containerPort: number, sessionName?: string | null): string[] {
  const platform = process.platform;
  const iconFile = platform === "win32" ? "icon.ico" : "icon.png";
  const iconPath = getAssetPath(iconFile);
  const name = sessionName || "default";
  const displayName = `cuabot (${name})`;

  // Generate border color from session name to match overlay cursor
  const borderColor = "auto" // TODO: fix this nameToColor(name).substring(1); // remove #

  const args = [
    "attach",
    `tcp://localhost:${containerPort}`,
    "--splash=no",
    "--notifications=no",
    `--border=${borderColor},4`,
    "--sharing=yes",
    `--tray-icon=${iconPath}`,
    `--window-icon=${iconPath}`,
    `--session-name=${displayName}`,
  ];

  // Mac-only dock icon
  if (platform === "darwin") {
    args.push(`--dock-icon=${iconPath}`);
  }

  return args;
}

/**
 * Check if Docker is available and running
 */
export async function checkDocker(): Promise<{ ok: boolean; message: string }> {
  try {
    await execAsync("docker info");
    return { ok: true, message: "Docker is running" };
  } catch {
    try {
      await execAsync("docker --version");
      return { ok: false, message: "Docker is installed but not running. Please start Docker Desktop." };
    } catch {
      return { ok: false, message: "Docker is not installed. Please install Docker Desktop from https://www.docker.com/products/docker-desktop/" };
    }
  }
}

/**
 * Check if Xpra client is available
 */
export async function checkXpra(): Promise<{ ok: boolean; message: string; quarantined?: boolean }> {
  const xpraPath = getXpraBinPath();
  const platform = process.platform;

  // Platform-specific install instructions
  const installInstructions = platform === "darwin"
    ? "Please install from https://github.com/Xpra-org/xpra/wiki/Download and ensure it's installed at /Applications/Xpra.app"
    : platform === "linux"
      ? "Please install via your package manager (e.g., 'apt install xpra' or 'brew install xpra')"
      : "Please install from https://github.com/Xpra-org/xpra/wiki/Download";

  // For absolute paths, check if file exists
  if (xpraPath.includes("/") || xpraPath.includes("\\")) {
    if (existsSync(xpraPath)) {
      // On macOS, check for quarantine attribute
      if (platform === "darwin") {
        try {
          const { stdout } = await execAsync("xattr /Applications/Xpra.app");
          if (stdout.includes("com.apple.quarantine")) {
            return { ok: false, message: `Xpra is installed but quarantined. Run: xattr -d com.apple.quarantine /Applications/Xpra.app`, quarantined: true };
          }
        } catch {
          // xattr command failed, ignore
        }
      }
      return { ok: true, message: "Xpra client found" };
    }
    return {
      ok: false,
      message: `Xpra client not found. ${installInstructions}`
    };
  }

  // For command names, try to run with --version
  try {
    await execAsync(`${xpraPath} --version`);
    return { ok: true, message: "Xpra client found" };
  } catch {
    return {
      ok: false,
      message: `Xpra client not found. ${installInstructions}`
    };
  }
}

/**
 * Check if Playwright Chromium is installed
 */
export async function checkPlaywright(): Promise<{ ok: boolean; message: string }> {
  try {
    const { chromium } = await import("playwright");
    const browser = await chromium.launch({ headless: true });
    await browser.close();
    return { ok: true, message: "installed" };
  } catch {
    return { ok: false, message: "not installed" };
  }
}

/**
 * Check all dependencies required for cuabot to run
 */
export async function checkDependencies(): Promise<{ ok: boolean; errors: string[] }> {
  const errors: string[] = [];

  const [dockerCheck, xpraCheck] = await Promise.all([
    checkDocker(),
    checkXpra(),
  ]);

  if (!dockerCheck.ok) {
    errors.push(dockerCheck.message);
  }

  if (!xpraCheck.ok) {
    errors.push(xpraCheck.message);
  }

  return { ok: errors.length === 0, errors };
}
