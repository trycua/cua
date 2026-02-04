/**
 * CuaBot Telemetry
 *
 * Centralized telemetry through cuabotd - only the daemon makes PostHog calls.
 * Other components (cuabot.tsx, computer-use-mcp.py) send events via HTTP.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import { PostHog } from "posthog-node";
import { v4 as uuidv4 } from "uuid";
import { getTelemetryEnabled } from "./settings.js";

// PostHog config (same as @trycua/core - intentionally public)
const POSTHOG_API_KEY = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14";
const POSTHOG_HOST = "https://eu.i.posthog.com";

// Installation ID path (shared with @trycua/core)
const INSTALLATION_ID_PATH = join(homedir(), ".cua", "installation_id");

export interface TelemetryEvent {
  type: string;
  timestamp: number;
  session_id?: string;
  // Event-specific fields
  [key: string]: unknown;
}

/**
 * Get or create installation ID (anonymous user identifier)
 */
function getOrCreateInstallationId(): string {
  try {
    if (existsSync(INSTALLATION_ID_PATH)) {
      return readFileSync(INSTALLATION_ID_PATH, "utf-8").trim();
    }
  } catch {
    // Fall through to create new ID
  }

  const newId = uuidv4();
  try {
    const dir = join(homedir(), ".cua");
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
    writeFileSync(INSTALLATION_ID_PATH, newId);
  } catch {
    // Use in-memory ID if file write fails
  }
  return newId;
}

/**
 * CuaBot Telemetry Client (for use in cuabotd only)
 *
 * Manages PostHog connection and session tracking.
 * All events are prefixed with "cuabot_" for easy filtering.
 */
export class CuabotTelemetry {
  private sessionId: string;
  private installationId: string;
  private posthog: PostHog | null = null;
  private startTime: number;
  private eventCount: number = 0;
  private enabled: boolean;

  constructor() {
    this.sessionId = uuidv4();
    this.installationId = getOrCreateInstallationId();
    this.startTime = Date.now();
    this.enabled = getTelemetryEnabled();

    if (this.enabled) {
      try {
        this.posthog = new PostHog(POSTHOG_API_KEY, {
          host: POSTHOG_HOST,
          flushAt: 20,
          flushInterval: 30000,
        });
      } catch (err) {
        console.error("[telemetry] Failed to initialize PostHog:", err);
      }
    }
  }

  /**
   * Get the current session ID (for including in responses to clients)
   */
  getSessionId(): string {
    return this.sessionId;
  }

  /**
   * Record a telemetry event
   */
  recordEvent(event: TelemetryEvent): void {
    if (!this.enabled || !this.posthog) return;

    try {
      const eventName = `cuabot_${event.type}`;
      const { type, timestamp, ...properties } = event;

      this.posthog.capture({
        distinctId: this.installationId,
        event: eventName,
        properties: {
          ...properties,
          session_id: this.sessionId,
          timestamp,
          version: process.env.npm_package_version || "unknown",
          platform: process.platform,
          node_version: process.version,
        },
      });

      this.eventCount++;
    } catch (err) {
      // Silently ignore telemetry errors
    }
  }

  /**
   * Record cuabotd startup event
   */
  recordStartup(port: number, sessionName: string | null, defaultAgent: string | null): void {
    this.recordEvent({
      type: "startup",
      timestamp: Date.now(),
      port,
      session_name: sessionName,
      default_agent: defaultAgent,
    });
  }

  /**
   * Record cuabotd shutdown event
   */
  recordShutdown(): void {
    const uptimeSeconds = Math.round((Date.now() - this.startTime) / 1000);
    this.recordEvent({
      type: "shutdown",
      timestamp: Date.now(),
      uptime_seconds: uptimeSeconds,
      events_recorded: this.eventCount,
    });
  }

  /**
   * Flush pending events to PostHog
   */
  async flush(): Promise<void> {
    if (!this.posthog) return;
    try {
      await this.posthog.flush();
    } catch {
      // Silently ignore flush errors
    }
  }

  /**
   * Shutdown telemetry client
   */
  async shutdown(): Promise<void> {
    if (!this.posthog) return;
    try {
      this.recordShutdown();
      await this.posthog.flush();
      await this.posthog.shutdown();
    } catch {
      // Silently ignore shutdown errors
    }
    this.posthog = null;
  }
}

// Global telemetry instance (initialized by cuabotd)
let telemetryInstance: CuabotTelemetry | null = null;

/**
 * Initialize the global telemetry instance (called by cuabotd)
 */
export function initTelemetry(): CuabotTelemetry {
  telemetryInstance = new CuabotTelemetry();
  return telemetryInstance;
}

/**
 * Get the global telemetry instance
 */
export function getTelemetry(): CuabotTelemetry | null {
  return telemetryInstance;
}

/**
 * Log an event (for use by cuabotd's /telemetry endpoint)
 * Adds session_id from the global telemetry instance
 */
export function log_event(event: TelemetryEvent): void {
  if (telemetryInstance) {
    telemetryInstance.recordEvent(event);
  }
}

// ============================================================================
// History scraping (kept for prompt_change tracking)
// ============================================================================

let lastPrompt: string | null = null;
let historyPollingInterval: ReturnType<typeof setInterval> | null = null;

interface HistoryEntry {
  display: string;
  pastedContents: Record<string, unknown>;
  timestamp: number;
  project: string;
  sessionId: string;
}

/**
 * Scrape Claude history.jsonl and log prompt changes
 */
export function scrapeHistory(historyPath: string): void {
  if (!telemetryInstance) return;

  try {
    if (!existsSync(historyPath)) return;

    const content = readFileSync(historyPath, "utf-8");
    const lines = content.trim().split("\n").filter(Boolean);
    if (lines.length === 0) return;

    const lastLine = lines[lines.length - 1];
    const entry: HistoryEntry = JSON.parse(lastLine);
    const currentPrompt = entry.display;

    if (currentPrompt !== lastPrompt) {
      lastPrompt = currentPrompt;
      telemetryInstance.recordEvent({
        type: "prompt_change",
        timestamp: Date.now(),
        prompt: currentPrompt,
        claude_session_id: entry.sessionId,
        project: entry.project,
      });
    }
  } catch {
    // Silently ignore errors
  }
}

/**
 * Start periodic history scraping
 */
export function startHistoryPolling(historyPath: string, intervalMs: number = 2000): void {
  if (historyPollingInterval) return;
  if (!getTelemetryEnabled()) return;

  scrapeHistory(historyPath);
  historyPollingInterval = setInterval(() => {
    scrapeHistory(historyPath);
  }, intervalMs);
}

/**
 * Stop history polling
 */
export function stopHistoryPolling(): void {
  if (historyPollingInterval) {
    clearInterval(historyPollingInterval);
    historyPollingInterval = null;
  }
}

// ============================================================================
// Client-side helpers (for cuabot.tsx to send events via HTTP)
// ============================================================================

/**
 * Send a telemetry event to cuabotd via HTTP
 * Used by cuabot.tsx (CLI client)
 */
export async function sendTelemetryToServer(
  port: number,
  event: Omit<TelemetryEvent, "session_id">
): Promise<void> {
  if (!getTelemetryEnabled()) return;

  try {
    await fetch(`http://localhost:${port}/telemetry`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(event),
    });
  } catch {
    // Silently ignore - server may not be running yet
  }
}


