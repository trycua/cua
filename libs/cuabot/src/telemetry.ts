/**
 * CuaBot Telemetry
 * Collects usage data to help improve computer-use technology
 */

import { appendFileSync, readFileSync, existsSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";
import { getTelemetryEnabled } from "./settings.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Log file path: __dirname/../telemetry.log
const TELEMETRY_LOG_PATH = join(__dirname, "..", "telemetry.log");

export interface TelemetryEvent {
  type: "mcp_tool_call" | "prompt_change" | "cli_invocation" | "telemetry_onboard";
  timestamp: number;
  // MCP tool call fields
  tool_name?: string;
  tool_args?: Record<string, unknown>;
  // Prompt change fields
  prompt?: string;
  session_id?: string;
  project?: string;
  // CLI invocation fields
  cli_args?: string[];
  cwd?: string;
  // Telemetry onboard fields
  default_agent?: string;
}

/**
 * Log a telemetry event to telemetry.log
 */
export function log_event(event: TelemetryEvent): void {
  if (!getTelemetryEnabled()) return;
  try {
    const line = JSON.stringify(event) + "\n";
    appendFileSync(TELEMETRY_LOG_PATH, line);
  } catch (err) {
    console.error("[telemetry] Failed to log event:", err);
  }
}

/**
 * Log an MCP tool call
 */
export function log_mcp_tool_call(tool_name: string, tool_args: Record<string, unknown>): void {
  if (!getTelemetryEnabled()) return;
  log_event({
    type: "mcp_tool_call",
    timestamp: Date.now(),
    tool_name,
    tool_args,
  });
}

/**
 * Log a CLI invocation
 */
export function log_cli_invocation(args: string[]): void {
  if (!getTelemetryEnabled()) return;
  log_event({
    type: "cli_invocation",
    timestamp: Date.now(),
    cli_args: args,
    cwd: process.cwd(),
  });
}

/**
 * Log telemetry onboarding (when user opts in)
 */
export function log_telemetry_onboard(default_agent: string | undefined): void {
  // Note: We log this even though telemetry was just enabled
  // This is the first event after opting in
  log_event({
    type: "telemetry_onboard",
    timestamp: Date.now(),
    default_agent,
  });
}

// History scraper state
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
  try {
    if (!existsSync(historyPath)) return;

    const content = readFileSync(historyPath, "utf-8");
    const lines = content.trim().split("\n").filter(Boolean);
    if (lines.length === 0) return;

    // Get the last entry
    const lastLine = lines[lines.length - 1];
    const entry: HistoryEntry = JSON.parse(lastLine);
    const currentPrompt = entry.display;

    // Log if prompt changed
    if (currentPrompt !== lastPrompt) {
      lastPrompt = currentPrompt;
      log_event({
        type: "prompt_change",
        timestamp: Date.now(),
        prompt: currentPrompt,
        session_id: entry.sessionId,
        project: entry.project,
      });
    }
  } catch (err) {
    // Silently ignore errors (file might be in use, etc.)
  }
}

/**
 * Start periodic history scraping
 */
export function startHistoryPolling(historyPath: string, intervalMs: number = 2000): void {
  if (historyPollingInterval) return;
  if (!getTelemetryEnabled()) return;

  // Initial scrape
  scrapeHistory(historyPath);

  // Poll periodically
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
