import process from 'node:process';

import { getDefaultEnvironment } from '@modelcontextprotocol/sdk/client/stdio.js';

// The MCP SDK stdio transport passes only a small default allowlist (HOME,
// PATH, USER, ...) on POSIX. On Linux that drops the desktop session, so the
// Driver cannot reach the display or session bus and the isolated browser exits
// before exposing DevTools. Forward the SDK defaults plus the desktop-session
// and Cua Driver variables only; provider credentials such as TYPESAFE_API_KEY
// stay in the agent process.
export const DESKTOP_SESSION_VARS = [
  'DISPLAY',
  'WAYLAND_DISPLAY',
  'XAUTHORITY',
  'XDG_RUNTIME_DIR',
  'DBUS_SESSION_BUS_ADDRESS',
  'AT_SPI_BUS_ADDRESS',
  'XDG_SESSION_TYPE',
  'XDG_CURRENT_DESKTOP',
] as const;
const DRIVER_VAR_PREFIX = 'CUA_DRIVER_';

export function driverEnvironment(
  source: NodeJS.ProcessEnv = process.env,
): Record<string, string> {
  const env = getDefaultEnvironment();
  const desktop: ReadonlySet<string> = new Set(DESKTOP_SESSION_VARS);
  for (const [key, value] of Object.entries(source)) {
    if (value === undefined) continue;
    if (desktop.has(key) || key.startsWith(DRIVER_VAR_PREFIX)) env[key] = value;
  }
  return env;
}
