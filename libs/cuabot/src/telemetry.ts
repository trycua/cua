/**
 * CuaBot Telemetry
 *
 * Centralized telemetry through cuabotd - only the daemon makes PostHog calls.
 * Other components (cuabot.tsx, computer-use-mcp.py) send events via HTTP.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { homedir } from 'os';
import { dirname, join } from 'path';
import { PostHog } from 'posthog-node';
import { v4 as uuidv4 } from 'uuid';
import { AGENTS, getTelemetryEnabled } from './settings.js';

// PostHog config (same as @trycua/core - intentionally public)
const POSTHOG_API_KEY = 'phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14';
const POSTHOG_HOST = 'https://eu.i.posthog.com';

// Installation id path (shared with the Cua Python and TypeScript SDKs).
const INSTALLATION_ID_PATH = join(homedir(), '.config', 'cua', 'installation_id');
const LEGACY_INSTALLATION_ID_PATH = join(homedir(), '.cua', 'installation_id');

export interface TelemetryEvent {
  type: string;
  timestamp: number;
  session_id?: string;
  // Event-specific fields
  [key: string]: unknown;
}

/**
 * Get or create the random installation id. Only called while telemetry is
 * enabled, so no file is created for users who did not opt in.
 */
function getOrCreateInstallationId(): string {
  for (const file of [INSTALLATION_ID_PATH, LEGACY_INSTALLATION_ID_PATH]) {
    try {
      if (existsSync(file)) {
        const stored = readFileSync(file, 'utf-8').trim();
        if (stored) return stored;
      }
    } catch {
      // Fall through
    }
  }

  const newId = uuidv4();
  try {
    mkdirSync(dirname(INSTALLATION_ID_PATH), { recursive: true });
    writeFileSync(INSTALLATION_ID_PATH, newId);
  } catch {
    // Use in-memory id if the file write fails
  }
  return newId;
}

// ============================================================================
// Sanitizers (pure; exported for tests)
// ============================================================================

/** Flag subcommands understood by cuabot.tsx. */
export const KNOWN_CLI_SUBCOMMANDS = [
  '--help',
  '-h',
  '--serve',
  '--stop',
  '--status',
  '--reset',
  '--screenshot',
  '--bash',
  '--click',
  '--doubleclick',
  '--move',
  '--mousedown',
  '--mouseup',
  '--drag',
  '--scroll',
  '--type',
  '--key',
  '--keydown',
  '--keyup',
  '--debug-onboarding',
] as const;

/**
 * The first subcommand word if it is a known cuabot flag or agent id,
 * "none" for no arguments, else "other". Never the raw argv (it can hold
 * shell commands, text to type, paths or session names).
 */
export function cliSubcommandLabel(argv: readonly string[]): string {
  const rest: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--name' || argv[i] === '-n') {
      i++;
      continue;
    }
    rest.push(argv[i]);
  }
  const first = rest[0];
  if (first === undefined) return 'none';
  if ((KNOWN_CLI_SUBCOMMANDS as readonly string[]).includes(first)) return first;
  if (Object.prototype.hasOwnProperty.call(AGENTS, first)) return first;
  return 'other';
}

/** MCP tools exposed by computer-use-mcp.py. */
export const KNOWN_MCP_TOOLS = [
  'screenshot',
  'click',
  'double_click',
  'type_text',
  'mouse_move',
  'mouse_down',
  'mouse_up',
  'scroll',
  'key_down',
  'key_up',
  'key_press',
  'drag',
] as const;

const NUMERIC_TOOL_ARGS = new Set([
  'x',
  'y',
  'delta_x',
  'delta_y',
  'from_x',
  'from_y',
  'to_x',
  'to_y',
  'delay',
]);
const BUTTONS = new Set(['left', 'right', 'middle']);

function sanitizeToolArgs(args: unknown): Record<string, number | string> {
  const out: Record<string, number | string> = {};
  if (!args || typeof args !== 'object') return out;
  for (const [key, value] of Object.entries(args as Record<string, unknown>)) {
    if (NUMERIC_TOOL_ARGS.has(key) && typeof value === 'number' && Number.isFinite(value)) {
      out[key] = value;
    } else if (key === 'button' && typeof value === 'string' && BUTTONS.has(value)) {
      out[key] = value;
    }
  }
  return out;
}

/**
 * Event types the cuabotd /telemetry relay accepts, each with the only
 * properties it may carry. Everything else is dropped.
 */
export function sanitizeRelayedEvent(body: unknown): TelemetryEvent | null {
  if (!body || typeof body !== 'object') return null;
  const event = body as Record<string, unknown>;
  const timestamp = typeof event.timestamp === 'number' ? event.timestamp : Date.now();

  switch (event.type) {
    case 'cli_invocation': {
      const sub = typeof event.subcommand === 'string' ? event.subcommand : 'other';
      const known =
        sub === 'none' ||
        sub === 'other' ||
        (KNOWN_CLI_SUBCOMMANDS as readonly string[]).includes(sub) ||
        Object.prototype.hasOwnProperty.call(AGENTS, sub);
      return { type: 'cli_invocation', timestamp, subcommand: known ? sub : 'other' };
    }
    case 'mcp_tool_call': {
      const tool = event.tool_name;
      if (typeof tool !== 'string' || !(KNOWN_MCP_TOOLS as readonly string[]).includes(tool)) {
        return null;
      }
      return {
        type: 'mcp_tool_call',
        timestamp,
        tool_name: tool,
        tool_args: sanitizeToolArgs(event.tool_args),
      };
    }
    default:
      return null;
  }
}

/**
 * CuaBot Telemetry Client (for use in cuabotd only)
 *
 * Manages PostHog connection and session tracking.
 * All events are prefixed with "cuabot_" for easy filtering.
 */
export class CuabotTelemetry {
  private sessionId: string;
  private installationId: string | null = null;
  private posthog: PostHog | null = null;
  private startTime: number;
  private eventCount: number = 0;
  private enabled: boolean;

  constructor() {
    this.sessionId = uuidv4();
    this.startTime = Date.now();
    this.enabled = getTelemetryEnabled();

    if (this.enabled) {
      this.installationId = getOrCreateInstallationId();
      try {
        this.posthog = new PostHog(POSTHOG_API_KEY, {
          host: POSTHOG_HOST,
          flushAt: 20,
          flushInterval: 30000,
          disableGeoip: true,
        });
      } catch (err) {
        console.error('[telemetry] Failed to initialize PostHog:', (err as Error)?.name ?? 'Error');
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
    if (!this.enabled || !this.posthog || !this.installationId) return;

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
          version: process.env.npm_package_version || 'unknown',
          platform: process.platform,
          node_version: process.versions.node.split('.')[0],
          $process_person_profile: false,
          $geoip_disable: true,
        },
        disableGeoip: true,
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
    // The session name is user-chosen: only whether one was given is sent.
    this.recordEvent({
      type: 'startup',
      timestamp: Date.now(),
      port,
      has_session_name: !!sessionName,
      default_agent:
        defaultAgent && Object.prototype.hasOwnProperty.call(AGENTS, defaultAgent)
          ? defaultAgent
          : defaultAgent
            ? 'other'
            : null,
    });
  }

  /**
   * Record cuabotd shutdown event
   */
  recordShutdown(): void {
    const uptimeSeconds = Math.round((Date.now() - this.startTime) / 1000);
    this.recordEvent({
      type: 'shutdown',
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
// Prompt sharing (explicit opt-in; see TelemetrySelector in onboarding.tsx)
// ============================================================================
// Only runs when the user chose "Yes, share my prompts" (cuabot telemetry is
// off unless explicitly enabled). Sends the text of the latest prompt typed
// into Claude Code and Claude's random session id. The project path is not
// sent.

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

    const content = readFileSync(historyPath, 'utf-8');
    const lines = content.trim().split('\n').filter(Boolean);
    if (lines.length === 0) return;

    const lastLine = lines[lines.length - 1];
    const entry: HistoryEntry = JSON.parse(lastLine);
    const currentPrompt = entry.display;

    if (currentPrompt !== lastPrompt) {
      lastPrompt = currentPrompt;
      telemetryInstance.recordEvent({
        type: 'prompt_change',
        timestamp: Date.now(),
        prompt: currentPrompt,
        claude_session_id: entry.sessionId,
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
  event: Omit<TelemetryEvent, 'session_id'>
): Promise<void> {
  if (!getTelemetryEnabled()) return;

  try {
    await fetch(`http://localhost:${port}/telemetry`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(event),
    });
  } catch {
    // Silently ignore - server may not be running yet
  }
}
