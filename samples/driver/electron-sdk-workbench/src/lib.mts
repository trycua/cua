import type { ToolResult } from '@trycua/cua-driver';

export const allowedTools = [
  'list_apps',
  'list_windows',
  'launch_app',
  'get_window_state',
  'get_desktop_state',
  'click',
  'type_text',
  'scroll',
  'press_key',
] as const;

export type AllowedTool = (typeof allowedTools)[number];
export type PermissionModeName = 'standard' | 'bounded' | 'unrestricted';

const allowedToolSet = new Set<string>(allowedTools);

export function assertAllowedTool(value: unknown): asserts value is AllowedTool {
  if (typeof value !== 'string' || !allowedToolSet.has(value)) {
    throw new Error('Unsupported tool. Choose one of the workbench allowlisted tools.');
  }
}

export function parseArgumentsJson(value: unknown): Record<string, unknown> {
  if (typeof value !== 'string' || value.length > 100_000) {
    throw new Error('Tool arguments must be a JSON object under 100 KB.');
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error('Tool arguments are not valid JSON.');
  }
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== 'object') {
    throw new Error('Tool arguments must be a JSON object.');
  }
  return parsed as Record<string, unknown>;
}

export function validateSessionRequest(value: unknown): {
  name: string;
  mode: PermissionModeName;
  manifestPath?: string;
} {
  if (value === null || typeof value !== 'object') throw new Error('Invalid session request.');
  const record = value as Record<string, unknown>;
  const name = typeof record.name === 'string' ? record.name.trim() : '';
  const mode = record.mode;
  const manifestPath = typeof record.manifestPath === 'string' ? record.manifestPath.trim() : '';
  if (name.length < 2 || name.length > 80) throw new Error('Session name must be 2 to 80 characters.');
  if (mode !== 'standard' && mode !== 'bounded' && mode !== 'unrestricted') {
    throw new Error('Unknown permission mode.');
  }
  if (mode === 'bounded' && manifestPath.length === 0) {
    throw new Error('Bounded mode requires an absolute manifest path.');
  }
  if (manifestPath.length > 4096) throw new Error('Manifest path is too long.');
  return manifestPath ? { name, mode, manifestPath } : { name, mode };
}

function jsonSafe(value: unknown): unknown {
  if (typeof value === 'bigint') return value.toString();
  if (Array.isArray(value)) return value.map(jsonSafe);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, jsonSafe(item)]));
  }
  return value;
}

function parseOptionalJson(value?: string): unknown {
  if (!value) return undefined;
  try {
    return jsonSafe(JSON.parse(value));
  } catch {
    return value;
  }
}

export function serializeToolResult(result: ToolResult) {
  return {
    text: result.text,
    isError: result.isError,
    errorCode: result.errorCode,
    degraded: result.degraded,
    structured: parseOptionalJson(result.structuredJson),
    raw: parseOptionalJson(result.rawJson),
    action: jsonSafe(result.action),
    verification: jsonSafe(result.verification),
    images: result.images.map((item) => ({
      mimeType: item.mimeType,
      dataUrl: `data:${item.mimeType};base64,${item.dataBase64}`,
    })),
  };
}
