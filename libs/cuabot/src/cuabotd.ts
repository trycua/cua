#!/usr/bin/env node
/**
 * CuaBot HTTP Server
 * Provides exec, screenshot, and input capabilities via HTTP API
 * Uses Playwright for screenshot/input on the xpra HTML5 interface
 * WebSocket support for interactive shell sessions
 */

import * as pty from "@lydell/node-pty";
import { exec, spawn } from "child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "fs";
import { openWindows } from "get-windows";
import { createServer, IncomingMessage, ServerResponse } from "http";
import { homedir } from "os";
import { join } from "path";
import { Browser, BrowserContext, chromium, Page } from "playwright";
import sharp from "sharp";
import { promisify } from "util";
import { WebSocket, WebSocketServer } from "ws";
import { getDefaultAgent, getTelemetryEnabled } from "./settings.js";
import {
  CuabotTelemetry,
  initTelemetry,
  startHistoryPolling,
  stopHistoryPolling,
  TelemetryEvent,
} from "./telemetry.js";
import { getXpraAttachArgs, getXpraBinPath, nameToColor } from "./utils.js";


const execAsync = promisify(exec);

// Config
const DEFAULT_PORT = 7842;
const CONFIG_DIR = join(homedir(), ".cuabot");
const CLAUDE_CONFIG_DIR = join(CONFIG_DIR, "user", ".claude");

const IMAGE_NAME = "trycua/cuabot:latest";
const BASE_CONTAINER_NAME = "cuabot-xpra";

// Session name support
let sessionName: string | null = null;

export function setSessionName(name: string | null): void {
  sessionName = name;
}

export function getSessionName(): string | null {
  return sessionName;
}

// Find an available HTTP server port
async function findAvailableServerPort(preferredPort: number): Promise<number> {
  const net = await import("net");

  const isPortAvailable = (port: number): Promise<boolean> => {
    return new Promise((resolve) => {
      const server = net.createServer();
      server.once("error", () => resolve(false));
      server.once("listening", () => {
        server.close();
        resolve(true);
      });
      server.listen(port);
    });
  };

  // Try preferred port first
  if (await isPortAvailable(preferredPort)) {
    return preferredPort;
  }

  // Find next available port starting from preferred + 1
  let port = preferredPort + 1;
  while (port < 65535) {
    if (await isPortAvailable(port)) {
      return port;
    }
    port++;
  }

  throw new Error("No available ports found");
}

function getContainerName(): string {
  return sessionName ? `${BASE_CONTAINER_NAME}-${sessionName}` : BASE_CONTAINER_NAME;
}

function getPidFile(): string {
  return sessionName ? join(CONFIG_DIR, `server.${sessionName}.pid`) : join(CONFIG_DIR, "server.pid");
}

function getPortFile(): string {
  return sessionName ? join(CONFIG_DIR, `server.${sessionName}.port`) : join(CONFIG_DIR, "server.port");
}
const SCREENSHOT_PATH = "F:\\Projects\\cuabot\\screenshot.jpg";
const SCREENSHOT_CLICKED_PATH = "F:\\Projects\\cuabot\\screenshot-clicked.jpg";
const DEBUG = false;

// Telemetry client (initialized at startup)
let telemetryClient: CuabotTelemetry | null = null;

// State
let browser: Browser | null = null;
let context: BrowserContext | null = null;
let page: Page | null = null;
let containerPort: number | null = null;
let serverPort: number | null = null; // HTTP server port for CUABOT_HOST
let screenshotScale: number = 1; // Scale factor applied to screenshots (for scaling coords back)
let xpraClientPid: number | null = null; // Track xpra client process for cleanup
let serverReady: boolean = false; // Track if server initialization is complete

const STARTING_ERROR = "cuabotd is still starting";

// Ensure config directory exists
function ensureConfigDir() {
  if (!existsSync(CONFIG_DIR)) {
    mkdirSync(CONFIG_DIR, { recursive: true });
  }
  // Ensure Claude config directory exists for volume mount
  if (!existsSync(CLAUDE_CONFIG_DIR)) {
    mkdirSync(CLAUDE_CONFIG_DIR, { recursive: true });
  }
}

// Docker helpers
async function testDockerConnection(): Promise<{ ok: boolean; message: string }> {
  try {
    await execAsync("docker info");
    return { ok: true, message: "Docker connected" };
  } catch {
    try {
      await execAsync("docker --version");
      return { ok: false, message: "Docker installed but not running - start Docker Desktop" };
    } catch {
      return { ok: false, message: "Docker not installed" };
    }
  }
}

async function findAvailablePort(): Promise<number> {
  const usedPorts = new Set<number>();
  try {
    const { stdout } = await execAsync('docker ps -a --format "{{json .Ports}}"');
    for (const line of stdout.trim().split("\n")) {
      if (!line) continue;
      const matches = line.matchAll(/(\d+)->(\d+)/g);
      for (const match of matches) {
        usedPorts.add(parseInt(match[1], 10));
      }
    }
  } catch { }
  let port = 10000;
  while (usedPorts.has(port)) port++;
  return port;
}

async function ensureImage(): Promise<void> {
  try {
    await execAsync(`docker image inspect ${IMAGE_NAME}`);
    console.log(`[server] Image ${IMAGE_NAME} already exists`);
  } catch {
    console.log(`[server] Pulling ${IMAGE_NAME}...`);
    const pullProcess = exec(`docker pull ${IMAGE_NAME}`);

    pullProcess.stdout?.on("data", (data: Buffer) => {
      const lines = data.toString().split("\n").filter(l => l.trim());
      for (const line of lines) {
        console.log(`[pull] ${line}`);
      }
    });

    pullProcess.stderr?.on("data", (data: Buffer) => {
      const lines = data.toString().split("\n").filter(l => l.trim());
      for (const line of lines) {
        console.error(`[pull] ${line}`);
      }
    });

    await new Promise<void>((resolve, reject) => {
      pullProcess.on("exit", (code) => {
        if (code === 0) {
          console.log(`[server] Pull completed successfully!`);
          resolve();
        } else {
          reject(new Error(`Docker pull failed with exit code ${code}`));
        }
      });
    });
  }
}

async function ensureContainer(): Promise<number> {
  // Check if container is running
  try {
    const { stdout: runningOut } = await execAsync(`docker inspect -f "{{.State.Running}}" ${getContainerName()}`);
    if (runningOut.trim() === "true") {
      const { stdout: portOut } = await execAsync(`docker port ${getContainerName()} 10000`);
      return parseInt(portOut.split(":")[1]?.trim() || "10000", 10);
    }
  } catch { }

  const dockerCheck = await testDockerConnection();
  if (!dockerCheck.ok) {
    throw new Error(dockerCheck.message);
  }
  await ensureImage();

  // Remove existing stopped container
  await execAsync(`docker rm -f ${getContainerName()}`).catch(() => { });

  const port = await findAvailablePort();

  // Convert Windows path to Docker-compatible path for volume mount
  const toDockerPath = (p: string) => p.replace(/\\/g, "/").replace(/^([A-Z]):/, (_, letter) => `/${letter.toLowerCase()}`);
  const claudeConfigPath = toDockerPath(CLAUDE_CONFIG_DIR);

  const telemetryEnabled = getTelemetryEnabled();
  const cuabotName = sessionName || "cuabot";
  const cuabotColor = nameToColor(cuabotName).slice(1); // Remove # prefix
  const createCmd = [
    "docker", "create",
    "--name", getContainerName(),
    "-p", `${port}:10000`,
    "-e", "DISPLAY=:100",
    "-e", "CLAUDE_CONFIG_DIR=/home/user/.claude",
    "-e", `CUABOT_HOST=http://host.docker.internal:${serverPort}`,
    "-e", `CUABOT_TELEMETRY=${telemetryEnabled ? "true" : "false"}`,
    "-e", `CUABOT_NAME=${cuabotName}`,
    "-e", `CUABOT_COLOR=${cuabotColor}`,
    "--add-host=host.docker.internal:host-gateway",
    "-v", `${claudeConfigPath}:/home/user/.claude`,
    IMAGE_NAME,
  ];

  await execAsync(createCmd.join(" "));
  await execAsync(`docker start ${getContainerName()}`);

  // Wait for xpra to be ready
  for (let i = 0; i < 30; i++) {
    try {
      const { stdout } = await execAsync(
        `docker exec ${getContainerName()} sh -c "netstat -tln 2>/dev/null | grep -q 10000 && echo ready || ss -tln 2>/dev/null | grep -q 10000 && echo ready"`
      );
      if (stdout.includes("ready")) break;
    } catch { }
    await new Promise(r => setTimeout(r, 500));
  }

  // Start overlay cursor in background
  const cursorName = sessionName || "cuabot";
  try {
    await execAsync(
      `docker exec -d ${getContainerName()} python3 /home/user/.cuabot/mcp/overlay-cursor.py --name=${cursorName}`
    );
  } catch { }

  // Start mask polling for cursor visibility
  startMaskPolling();

  return port;
}

async function execInContainer(
  command: string,
  options: { timeout?: number; runInBackground?: boolean } = {}
): Promise<{ stdout: string; stderr: string; pid?: number }> {
  const { timeout = 60000, runInBackground = false } = options;

  // Write command to a temp script file inside the container to avoid shell escaping issues
  const scriptId = Date.now() + Math.random().toString(36).slice(2);
  const scriptPath = `/tmp/cuabot-cmd-${scriptId}.sh`;

  // Create the script content with proper shebang and DISPLAY
  const scriptContent = `#!/bin/bash
export DISPLAY=:100
${command}
`;

  // Write script to container using docker exec with base64 to avoid any escaping issues
  const base64Script = Buffer.from(scriptContent).toString("base64");
  await execAsync(
    `docker exec ${getContainerName()} bash -c "echo '${base64Script}' | base64 -d > ${scriptPath} && chmod +x ${scriptPath}"`
  );

  try {
    if (runInBackground) {
      // Run in background with nohup, capture PID
      // Wait 1 second to catch any immediate errors (syntax errors, command not found, etc.)
      const bgCmd = `docker exec ${getContainerName()} bash -c "nohup ${scriptPath} > /tmp/cuabot-bg-${scriptId}.log 2>&1 & echo \\$!; sleep 1; if ! kill -0 \\$! 2>/dev/null; then cat /tmp/cuabot-bg-${scriptId}.log; exit 1; fi"`;

      const { stdout, stderr } = await execAsync(bgCmd, { timeout: 10000 });
      const pid = parseInt(stdout.trim().split("\n")[0], 10);

      // Clean up script after a delay (let it start first)
      setTimeout(async () => {
        await execAsync(`docker exec ${getContainerName()} rm -f ${scriptPath}`).catch(() => {});
      }, 5000);

      return { stdout: `Background process started with PID ${pid}`, stderr, pid };
    } else {
      // Run synchronously
      const { stdout, stderr } = await execAsync(
        `docker exec ${getContainerName()} ${scriptPath}`,
        { timeout }
      );

      // Clean up script
      await execAsync(`docker exec ${getContainerName()} rm -f ${scriptPath}`).catch(() => {});

      return { stdout, stderr };
    }
  } catch (err: any) {
    // Clean up script on error
    await execAsync(`docker exec ${getContainerName()} rm -f ${scriptPath}`).catch(() => {});
    return { stdout: err.stdout || "", stderr: err.stderr || err.message };
  }
}

// Overlay cursor management
const CURSOR_SOCKET = "/tmp/cuabot-overlay-cursor.sock";
let maskPollingInterval: ReturnType<typeof setInterval> | null = null;
let lastCursorPos = { x: 0, y: 0 };

async function sendCursorCommand(cmd: Record<string, unknown>): Promise<void> {
  const json = JSON.stringify(cmd).replace(/"/g, '\\"');
  await execAsync(
    `docker exec ${getContainerName()} python3 -c "import socket; s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.settimeout(0.1); s.connect('${CURSOR_SOCKET}'); s.send(b'${json}\\n'); s.close()"`
  ).catch(() => {});
}

async function notifyCursorMove(x: number, y: number, click = false): Promise<void> {
  lastCursorPos = { x, y };
  await sendCursorCommand({ type: "move", x: x, y: y, click });
}

// AABB types and operations for mask computation
interface AABB {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface AABBOp extends AABB {
  op: "add" | "subtract";
}

function subtractAABB(a: AABB, b: AABB): AABB[] {
  const aRight = a.x + a.width;
  const aBottom = a.y + a.height;
  const bRight = b.x + b.width;
  const bBottom = b.y + b.height;

  // No intersection - return A unchanged
  if (b.x >= aRight || bRight <= a.x || b.y >= aBottom || bBottom <= a.y) {
    return [a];
  }

  const result: AABB[] = [];

  // Top slice
  if (b.y > a.y) {
    result.push({ x: a.x, y: a.y, width: a.width, height: b.y - a.y });
  }

  // Bottom slice
  if (bBottom < aBottom) {
    result.push({ x: a.x, y: bBottom, width: a.width, height: aBottom - bBottom });
  }

  // Left/right slices (between top and bottom cuts)
  const midTop = Math.max(a.y, b.y);
  const midBottom = Math.min(aBottom, bBottom);
  if (midBottom > midTop) {
    if (b.x > a.x) {
      result.push({ x: a.x, y: midTop, width: b.x - a.x, height: midBottom - midTop });
    }
    if (bRight < aRight) {
      result.push({ x: bRight, y: midTop, width: aRight - bRight, height: midBottom - midTop });
    }
  }

  return result;
}

function tryMerge(a: AABB, b: AABB): AABB | null {
  const aRight = a.x + a.width, aBottom = a.y + a.height;
  const bRight = b.x + b.width, bBottom = b.y + b.height;

  // Horizontal merge
  if (a.y === b.y && a.height === b.height) {
    if (aRight === b.x) return { x: a.x, y: a.y, width: a.width + b.width, height: a.height };
    if (bRight === a.x) return { x: b.x, y: a.y, width: a.width + b.width, height: a.height };
  }

  // Vertical merge
  if (a.x === b.x && a.width === b.width) {
    if (aBottom === b.y) return { x: a.x, y: a.y, width: a.width, height: a.height + b.height };
    if (bBottom === a.y) return { x: a.x, y: b.y, width: a.width, height: a.height + b.height };
  }

  return null;
}

function mergeRegions(regions: AABB[]): AABB[] {
  if (regions.length <= 1) return regions;

  let merged = true;
  while (merged) {
    merged = false;
    outer: for (let i = 0; i < regions.length; i++) {
      for (let j = i + 1; j < regions.length; j++) {
        const m = tryMerge(regions[i], regions[j]);
        if (m) {
          regions.splice(j, 1);
          regions[i] = m;
          merged = true;
          break outer;
        }
      }
    }
  }

  return regions;
}

function computeAABBs(ops: AABBOp[]): AABB[] {
  let regions: AABB[] = [];

  for (const op of ops) {
    const box: AABB = { x: op.x, y: op.y, width: op.width, height: op.height };

    if (op.op === "add") {
      regions.push(box);
    } else {
      regions = regions.flatMap(r => subtractAABB(r, box));
    }
  }

  return mergeRegions(regions);
}

async function updateCursorMasks(): Promise<void> {
  try {
    const windows = await openWindows();
    const cursorName = sessionName || "cuabot";
    const cursorWindowPattern = `cuabot-cursor-${cursorName}`;

    // Build AABB operations from windows (all in absolute screen coordinates)
    // Process back-to-front so Xpra windows only cut holes in things BEHIND them
    const ops: AABBOp[] = [];

    // Store window info for debug visualization
    const windowsForDebug: { bounds: AABB; title: string; owner: string; isXpra: boolean }[] = [];

    // Reverse to go back-to-front (openWindows returns front-to-back)
    for (const win of [...windows].reverse()) {
      const b = win.bounds;
      const isXpra = win.owner.name === "Xpra" || win.owner.name.toLowerCase().includes("xpra");
      const isCursorWindow = win.title.includes(cursorWindowPattern);
      const ignoredWindows = ["cuabot-debug-masks", "Screen Studio", "recording-manager-widget"];
      const isIgnoredWindow = ignoredWindows.some(name => win.title.includes(name));

      if (isCursorWindow || isIgnoredWindow) {
        continue;
      }

      windowsForDebug.push({
        bounds: { x: b.x, y: b.y, width: b.width, height: b.height },
        title: win.title,
        owner: win.owner.name,
        isXpra,
      });

      if (isXpra) {
        // Xpra windows: subtract (cut holes in masks behind it)
        ops.push({ x: b.x, y: b.y, width: b.width, height: b.height, op: "subtract" });
      } else {
        // Non-Xpra windows: add (mask areas where cursor should hide)
        ops.push({ x: b.x, y: b.y, width: b.width, height: b.height, op: "add" });
      }
    }

    // Compute final mask regions in host screen coordinates.
    // Xpra runs in seamless mode - each app window is a separate Xpra window on the host,
    // and the xpra display coordinates map directly to host screen coordinates.
    const maskRects = computeAABBs(ops);

    // Check if cursor is inside any mask
    const cx = lastCursorPos.x;
    const cy = lastCursorPos.y;
    const isInMask = maskRects.some(r =>
      cx >= r.x && cx <= r.x + r.width && cy >= r.y && cy <= r.y + r.height
    );

    // Debug visualization
    if (DEBUG) {
      await drawDebugMasks(windowsForDebug, maskRects, cx, cy, isInMask);
    }

    // Send masked state to overlay cursor (not the rects - calculation done here)
    await sendCursorCommand({ type: "masked", value: isInMask });
  } catch (e) {
    // Silently ignore errors
  }
}

async function drawDebugMasks(
  windows: { bounds: AABB; title: string; owner: string; isXpra: boolean }[],
  maskRects: AABB[],
  cursorX: number,
  cursorY: number,
  isInMask: boolean
): Promise<void> {
  try {
    const width = 2400;
    const height = 1600;
    const crosshairSize = 20;
    const cursorColor = isInMask ? "red" : "lime";

    // Escape XML special characters
    const escapeXml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

    // Build SVG
    let svg = `<svg width="${width}" height="${height}" xmlns="http://www.w3.org/2000/svg">`;

    // Background
    svg += `<rect x="0" y="0" width="${width}" height="${height}" fill="#1a1a1a"/>`;

    // Draw mask rects (red filled, semi-transparent)
    for (const r of maskRects) {
      svg += `<rect x="${r.x}" y="${r.y}" width="${r.width}" height="${r.height}" fill="rgba(255,0,0,0.3)" stroke="red" stroke-width="2"/>`;
    }

    // Draw window rects (blue outlined) with labels
    for (const win of windows) {
      const b = win.bounds;
      const strokeColor = win.isXpra ? "cyan" : "blue";
      svg += `<rect x="${b.x}" y="${b.y}" width="${b.width}" height="${b.height}" fill="none" stroke="${strokeColor}" stroke-width="2"/>`;

      // Label background
      const label = `${escapeXml(win.owner)}: ${escapeXml(win.title.substring(0, 40))}`;
      svg += `<rect x="${b.x}" y="${b.y - 18}" width="${Math.min(b.width, 300)}" height="18" fill="rgba(0,0,0,0.7)"/>`;
      svg += `<text x="${b.x + 4}" y="${b.y - 4}" font-family="monospace" font-size="12" fill="${strokeColor}">${label}</text>`;
    }

    // Draw cursor crosshair
    svg += `<line x1="${cursorX - crosshairSize}" y1="${cursorY}" x2="${cursorX + crosshairSize}" y2="${cursorY}" stroke="${cursorColor}" stroke-width="3"/>`;
    svg += `<line x1="${cursorX}" y1="${cursorY - crosshairSize}" x2="${cursorX}" y2="${cursorY + crosshairSize}" stroke="${cursorColor}" stroke-width="3"/>`;
    svg += `<circle cx="${cursorX}" cy="${cursorY}" r="8" fill="none" stroke="${cursorColor}" stroke-width="2"/>`;

    // Legend
    svg += `<rect x="10" y="10" width="200" height="80" fill="rgba(0,0,0,0.8)" stroke="white"/>`;
    svg += `<text x="20" y="30" font-family="monospace" font-size="12" fill="white">Cursor: ${cursorX}, ${cursorY}</text>`;
    svg += `<text x="20" y="50" font-family="monospace" font-size="12" fill="${cursorColor}">Masked: ${isInMask}</text>`;
    svg += `<text x="20" y="70" font-family="monospace" font-size="12" fill="cyan">Cyan=Xpra, Blue=Other</text>`;

    svg += `</svg>`;

    await sharp(Buffer.from(svg)).png().toFile(SCREENSHOT_PATH.replace("screenshot.jpg", "masks.png"));
  } catch (e) {
    console.error("[debug] Failed to draw masks:", e);
  }
}

function startMaskPolling(): void {
  if (maskPollingInterval) return;
  maskPollingInterval = setInterval(updateCursorMasks, 500);
}

function stopMaskPolling(): void {
  if (maskPollingInterval) {
    clearInterval(maskPollingInterval);
    maskPollingInterval = null;
  }
}

// Playwright session management
let playwrightConnected = false;

async function launchPlaywright(): Promise<void> {
  if (browser?.isConnected()) return;

  console.log(`[server] Launching Playwright browser...`);
  browser = await chromium.launch({ headless: true });
  context = await browser.newContext({
    viewport: { width: 1920, height: 1080 },
    ignoreHTTPSErrors: true,
  });
  page = await context.newPage();
  console.log(`[server] Playwright browser ready (not yet connected to xpra)`);
}

async function ensurePlaywrightConnected(): Promise<Page> {
  if (!browser?.isConnected() || !page) {
    await launchPlaywright();
  }

  if (playwrightConnected && page) {
    return page;
  }

  const url = `http://localhost:${containerPort}/?steal=no&sharing=yes`;
  console.log(`[server] Connecting to xpra-html5 at ${url}`);
  await page!.goto(url);

  // Wait for xpra to be ready
  for (let i = 0; i < 60; i++) {
    const isReady = await page!.evaluate(() => {
      const canvas = document.querySelector("canvas");
      return canvas !== null;
    });
    if (isReady) break;
    await new Promise(r => setTimeout(r, 500));
  }

  playwrightConnected = true;
  console.log(`[server] Playwright connected to xpra`);
  return page!;
}

async function closePlaywrightSession(): Promise<void> {
  if (page) { await page.close().catch(() => { }); page = null; }
  if (context) { await context.close().catch(() => { }); context = null; }
  if (browser) { await browser.close().catch(() => { }); browser = null; }
  playwrightConnected = false;
}

// Constants for screenshot scaling
const MAX_SCREENSHOT_DIMENSION = 1280;

// Scale screenshot to max dimension and track scale factor
async function scaleScreenshot(buffer: Buffer): Promise<{ scaled: Buffer; scale: number }> {
  const metadata = await sharp(buffer).metadata();
  const width = metadata.width || 1920;
  const height = metadata.height || 1080;
  const maxDim = Math.max(width, height);

  if (maxDim <= MAX_SCREENSHOT_DIMENSION) {
    return { scaled: buffer, scale: 1 };
  }

  const scale = MAX_SCREENSHOT_DIMENSION / maxDim;
  const newWidth = Math.round(width * scale);
  const newHeight = Math.round(height * scale);

  const scaled = await sharp(buffer)
    .resize(newWidth, newHeight)
    .jpeg()
    .toBuffer();

  return { scaled, scale };
}

// Scale coordinates from scaled screenshot back to original screen coordinates
function scaleCoordinates(x: number, y: number): { x: number; y: number } {
  if (screenshotScale === 1) return { x, y };
  return {
    x: Math.round(x / screenshotScale),
    y: Math.round(y / screenshotScale),
  };
}

// Save screenshot with click coordinates marked
async function saveClickedScreenshot(p: Page, x: number, y: number): Promise<void> {
  const buffer = await p.screenshot({ type: "jpeg", fullPage: false });

  // Save the regular screenshot
  writeFileSync(SCREENSHOT_PATH, buffer);

  // Get image dimensions
  const metadata = await sharp(buffer).metadata();
  const width = metadata.width || 1920;
  const height = metadata.height || 1080;

  // Create a crosshair/marker SVG overlay
  const markerSize = 20;
  const svg = `
    <svg width="${width}" height="${height}">
      <circle cx="${x}" cy="${y}" r="${markerSize}" fill="none" stroke="red" stroke-width="3"/>
      <line x1="${x - markerSize}" y1="${y}" x2="${x + markerSize}" y2="${y}" stroke="red" stroke-width="2"/>
      <line x1="${x}" y1="${y - markerSize}" x2="${x}" y2="${y + markerSize}" stroke="red" stroke-width="2"/>
    </svg>
  `;

  await sharp(buffer)
    .composite([{ input: Buffer.from(svg), top: 0, left: 0 }])
    .jpeg()
    .toFile(SCREENSHOT_CLICKED_PATH);
}

// Check if server is ready, throw error if not
function requireReady() {
  if (!serverReady) {
    throw new Error(STARTING_ERROR);
  }
}

// HTTP Server handlers
type Handler = (body: any) => Promise<any>;

const handlers: Record<string, Handler> = {
  async status() {
    return {
      ok: true,
      ready: serverReady,
      container: containerPort ? `running on port ${containerPort}` : "not started",
      containerPort: containerPort,
      playwright: browser?.isConnected() ? "connected" : "not connected",
    };
  },

  async bash(body: { command: string; run_in_background?: boolean; timeout?: number }) {
    requireReady();
    await ensureContainer();
    const { stdout, stderr, pid } = await execInContainer(body.command, {
      runInBackground: body.run_in_background,
      timeout: body.timeout,
    });
    return { stdout, stderr, pid };
  },

  async screenshot() {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const buffer = await p.screenshot({ type: "jpeg", fullPage: false });
    const { scaled, scale } = await scaleScreenshot(buffer);
    screenshotScale = scale; // Store for coordinate scaling
    return { image: Buffer.from(scaled).toString("base64"), scale };
  },

  async click(body: { x: number; y: number; button?: "left" | "right" | "middle" }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y, true);
    await p.mouse.click(x, y, { button: body.button || "left" });
    // Save screenshot with click coordinates marked (DEBUG only)
    if (DEBUG) await saveClickedScreenshot(p, x, y);
    return { ok: true, scaledCoords: { x, y } };
  },

  async doubleClick(body: { x: number; y: number }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y, true);
    await p.mouse.dblclick(x, y);
    // Save screenshot with click coordinates marked (DEBUG only)
    if (DEBUG) await saveClickedScreenshot(p, x, y);
    return { ok: true, scaledCoords: { x, y } };
  },

  async type(body: { text: string; delay?: number }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    await p.keyboard.type(body.text, { delay: body.delay ?? 50 });
    return { ok: true };
  },

  async mouseMove(body: { x: number; y: number }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y);
    await p.mouse.move(x, y);
    return { ok: true, scaledCoords: { x, y } };
  },

  async mouseDown(body: { x: number; y: number; button?: "left" | "right" | "middle" }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y, true);
    await p.mouse.move(x, y);
    await p.mouse.down({ button: body.button || "left" });
    return { ok: true, scaledCoords: { x, y } };
  },

  async mouseUp(body: { x: number; y: number; button?: "left" | "right" | "middle" }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y);
    await p.mouse.move(x, y);
    await p.mouse.up({ button: body.button || "left" });
    return { ok: true, scaledCoords: { x, y } };
  },

  async scroll(body: { x: number; y: number; deltaX: number; deltaY: number }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const { x, y } = scaleCoordinates(body.x, body.y);
    notifyCursorMove(x, y);
    await p.mouse.move(x, y);
    // Note: deltaX/deltaY are scroll amounts, not screen coordinates, so they're not scaled
    await p.mouse.wheel(body.deltaX, body.deltaY);
    return { ok: true, scaledCoords: { x, y } };
  },

  async keyDown(body: { key: string }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    await p.keyboard.down(body.key);
    return { ok: true };
  },

  async keyUp(body: { key: string }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    await p.keyboard.up(body.key);
    return { ok: true };
  },

  async keyPress(body: { key: string }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    await p.keyboard.press(body.key);
    return { ok: true };
  },

  async drag(body: { fromX: number; fromY: number; toX: number; toY: number }) {
    requireReady();
    const p = await ensurePlaywrightConnected();
    const from = scaleCoordinates(body.fromX, body.fromY);
    const to = scaleCoordinates(body.toX, body.toY);
    await p.mouse.move(from.x, from.y);
    await p.mouse.down();
    await p.mouse.move(to.x, to.y, { steps: 10 });
    await p.mouse.up();
    return { ok: true, scaledCoords: { from, to } };
  },

  async telemetry(body: TelemetryEvent) {
    if (telemetryClient) {
      telemetryClient.recordEvent(body);
    }
    return { ok: true };
  },
};

// Cleanup
async function cleanup() {
  console.log("[server] Cleaning up...");

  // Shutdown telemetry (records shutdown event and flushes)
  if (telemetryClient) {
    await telemetryClient.shutdown();
    telemetryClient = null;
  }

  // Stop mask polling
  stopMaskPolling();

  // Stop history polling
  stopHistoryPolling();

  // Kill xpra client first (before container stops, so it exits cleanly)
  if (xpraClientPid) {
    try {
      process.kill(xpraClientPid, "SIGKILL");
      console.log(`[server] Killed xpra client (PID ${xpraClientPid})`);
    } catch { }
    xpraClientPid = null;
  }

  await closePlaywrightSession();
  await execAsync(`docker stop ${getContainerName()}`).catch(() => { });
  await execAsync(`docker rm ${getContainerName()}`).catch(() => { });
  try { unlinkSync(getPidFile()); } catch { }
  try { unlinkSync(getPortFile()); } catch { }
  console.log("[server] Cleanup complete");
}

// Server start
export async function startServer(preferredPort?: number): Promise<void> {
  ensureConfigDir();

  // Initialize telemetry
  telemetryClient = initTelemetry();

  // Check if already running
  if (existsSync(getPidFile())) {
    const pids = readFileSync(getPidFile(), "utf-8").trim().split("\n");
    const serverPid = parseInt(pids[0], 10);
    try {
      process.kill(serverPid, 0); // Check if process exists
      console.log(`Server already running (PID ${serverPid})`);
      process.exit(1);
    } catch {
      // Process doesn't exist, clean up stale files
      unlinkSync(getPidFile());
      if (existsSync(getPortFile())) unlinkSync(getPortFile());
    }
  }

  // Find available port
  const basePort = preferredPort ?? DEFAULT_PORT;
  const port = await findAvailableServerPort(basePort);
  serverPort = port; // Store for CUABOT_HOST in container

  // Write PID and port files
  writeFileSync(getPidFile(), process.pid.toString());
  writeFileSync(getPortFile(), port.toString());

  // Create HTTP server immediately (before initialization)
  const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    const url = new URL(req.url || "/", `http://localhost:${port}`);
    const path = url.pathname.slice(1); // Remove leading /

    // CORS headers
    res.setHeader("Content-Type", "application/json");
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "Content-Type");

    if (req.method === "OPTIONS") {
      res.writeHead(200);
      res.end();
      return;
    }

    // Handle stop request
    if (path === "stop") {
      res.writeHead(200);
      res.end(JSON.stringify({ ok: true, message: "Server stopping" }));
      setTimeout(async () => {
        await cleanup();
        process.exit(0);
      }, 100);
      return;
    }

    const handler = handlers[path];
    if (!handler) {
      res.writeHead(404);
      res.end(JSON.stringify({ error: `Unknown endpoint: ${path}` }));
      return;
    }

    try {
      let body = {};
      if (req.method === "POST") {
        const chunks: Buffer[] = [];
        for await (const chunk of req) {
          chunks.push(chunk as Buffer);
        }
        const text = Buffer.concat(chunks).toString();
        if (text) body = JSON.parse(text);
      }
      const result = await handler(body);
      res.writeHead(200);
      res.end(JSON.stringify(result));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      res.writeHead(500);
      res.end(JSON.stringify({ error: message }));
    }
  });

  // WebSocket server for interactive shell sessions
  const wss = new WebSocketServer({ server });
  const activeSessions = new Map<string, pty.IPty>();

  wss.on("connection", (ws: WebSocket, req: IncomingMessage) => {
    // Check if server is ready
    if (!serverReady) {
      ws.send(JSON.stringify({ type: "error", message: STARTING_ERROR }));
      ws.close();
      return;
    }

    const url = new URL(req.url || "/", `http://localhost:${port}`);
    const command = url.searchParams.get("command") || "bash";
    const cols = parseInt(url.searchParams.get("cols") || "80", 10);
    const rows = parseInt(url.searchParams.get("rows") || "24", 10);
    const sessionId = Date.now().toString(36) + Math.random().toString(36).slice(2);

    console.log(`[ws] New interactive session: ${sessionId}, command: ${command}`);

    let ptyProcess: pty.IPty;
    try {
      // Use node-pty to spawn docker exec with proper PTY support
      // Wrap command in bash -c to ensure proper shell environment
      const shell = process.platform === "win32" ? "docker.exe" : "docker";
      ptyProcess = pty.spawn(shell, [
        "exec",
        "-it",
        "-e", `TERM=${process.env.TERM || "xterm-256color"}`,
        "-e", "DISPLAY=:100",
        getContainerName(),
        "bash", "-c", command,
      ], {
        name: "xterm-256color",
        cols,
        rows,
        cwd: process.cwd(),
        env: process.env as Record<string, string>,
      });
    } catch (err) {
      console.error(`[ws] Failed to spawn PTY:`, err);
      ws.send(JSON.stringify({ type: "error", message: `Failed to spawn: ${err}` }));
      ws.close();
      return;
    }

    activeSessions.set(sessionId, ptyProcess);

    // Pipe PTY output to WebSocket
    ptyProcess.onData((data: string) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "stdout", data: Buffer.from(data).toString("base64") }));
      }
    });

    ptyProcess.onExit(({ exitCode }: { exitCode: number }) => {
      console.log(`[ws] Session ${sessionId} exited with code ${exitCode}`);
      activeSessions.delete(sessionId);
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "exit", code: exitCode }));
        ws.close();
      }
    });

    // Handle WebSocket messages (stdin from client)
    ws.on("message", (message: Buffer) => {
      try {
        const msg = JSON.parse(message.toString());
        if (msg.type === "stdin" && msg.data) {
          const data = Buffer.from(msg.data, "base64").toString();
          ptyProcess.write(data);
        } else if (msg.type === "resize" && msg.cols && msg.rows) {
          ptyProcess.resize(msg.cols, msg.rows);
        }
      } catch (err) {
        console.error(`[ws] Failed to parse message:`, err);
      }
    });

    ws.on("close", () => {
      console.log(`[ws] Session ${sessionId} WebSocket closed`);
      if (activeSessions.has(sessionId)) {
        ptyProcess.kill();
        activeSessions.delete(sessionId);
      }
    });

    ws.on("error", (err) => {
      console.error(`[ws] Session ${sessionId} WebSocket error:`, err);
    });

    // Send session info to client
    ws.send(JSON.stringify({ type: "session", id: sessionId, command }));
  });

  server.listen(port, () => {
    console.log(`[server] CuaBot server running on http://localhost:${port}`);
    console.log(`[server] PID: ${process.pid}`);
    console.log(`[server] Server is ready to accept connections`);
  });

  server.on("error", (err: NodeJS.ErrnoException) => {
    console.error(`[server] HTTP server error:`, err);
    if (err.code === "EADDRINUSE") {
      console.error(`[server] Port ${port} is already in use`);
    }
    process.exit(1);
  });

  // Cleanup on exit
  process.on("SIGINT", async () => { await cleanup(); process.exit(0); });
  process.on("SIGTERM", async () => { await cleanup(); process.exit(0); });

  // Initialize container, playwright, and xpra client in background
  (async () => {
    try {
      console.log(`[server] Initializing container...`);
      containerPort = await ensureContainer();

      // Initialize Playwright and connect to xpra first
      console.log(`[server] Initializing Playwright...`);
      await launchPlaywright();
      console.log(`[server] Connecting Playwright to xpra...`);
      await ensurePlaywrightConnected();

      // Launch xpra client after Playwright is connected
      console.log(`[server] Launching xpra client...`);
      const xpraCmd = getXpraBinPath();
      const xpraArgs = getXpraAttachArgs(containerPort, sessionName);
      const xpraClient = spawn(xpraCmd, xpraArgs, {
        detached: true,
        stdio: "ignore",
        windowsHide: true,
      });
      xpraClientPid = xpraClient.pid ?? null;
      xpraClient.unref();

      // Append xpra client PID to PID file
      if (xpraClientPid) {
        appendFileSync(getPidFile(), `\n${xpraClientPid}`);
      }

      serverReady = true;
      console.log(`[server] Ready!`);

      // Record startup telemetry
      if (telemetryClient) {
        telemetryClient.recordStartup(port, sessionName, getDefaultAgent() ?? null);
      }

      // Start history polling for telemetry (scrapes /home/user/.claude/history.jsonl)
      const historyPath = join(CLAUDE_CONFIG_DIR, "history.jsonl");
      startHistoryPolling(historyPath, 2000);
    } catch (err) {
      console.error(`[server] Initialization failed:`, err);
      process.exit(1);
    }
  })();
}

// Stop server
export async function stopServer(): Promise<boolean> {
  if (!existsSync(getPidFile())) {
    console.log("Server not running");
    return false;
  }

  const pids = readFileSync(getPidFile(), "utf-8").trim().split("\n").map(p => parseInt(p, 10)).filter(p => !isNaN(p));
  const serverPid = pids[0];
  const xpraPid = pids[1];
  const port = existsSync(getPortFile())
    ? parseInt(readFileSync(getPortFile(), "utf-8").trim(), 10)
    : DEFAULT_PORT;

  try {
    // Try graceful shutdown via HTTP
    const res = await fetch(`http://localhost:${port}/stop`, { method: "POST" });
    if (res.ok) {
      console.log("Server stopped");
      return true;
    }
  } catch {
    // HTTP failed, try killing processes directly
    let killed = false;

    // Kill server process
    try {
      process.kill(serverPid, "SIGTERM");
      console.log(`Sent SIGTERM to server (PID ${serverPid})`);
      killed = true;
    } catch { }

    // Kill xpra client process
    if (xpraPid) {
      try {
        process.kill(xpraPid, "SIGKILL");
        console.log(`Sent SIGKILL to xpra client (PID ${xpraPid})`);
        killed = true;
      } catch { }
    }

    // Clean up files
    try { unlinkSync(getPidFile()); } catch { }
    try { unlinkSync(getPortFile()); } catch { }

    if (killed) {
      return true;
    } else {
      console.log("Server not running (stale PID file)");
      return false;
    }
  }
  return false;
}

// Get server info
export function getServerInfo(): { running: boolean; port: number; pid: number } | null {
  if (!existsSync(getPidFile())) return null;

  const pids = readFileSync(getPidFile(), "utf-8").trim().split("\n");
  const pid = parseInt(pids[0], 10);
  const port = existsSync(getPortFile())
    ? parseInt(readFileSync(getPortFile(), "utf-8").trim(), 10)
    : DEFAULT_PORT;

  try {
    process.kill(pid, 0); // Check if process exists
    return { running: true, port, pid };
  } catch {
    return null;
  }
}

// CLI entry point
if (import.meta.main) {
  const port = parseInt(process.argv[2] || String(DEFAULT_PORT), 10);
  startServer(port);
}
