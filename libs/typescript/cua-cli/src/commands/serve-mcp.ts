import type { Argv } from 'yargs';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';
import { getApiKey } from '../storage';
import { http } from '../http';
import type { SandboxItem } from '../util';
import { homedir } from 'os';
import { join } from 'path';
import { spawn } from 'child_process';

// Permission levels for the MCP server
type Permission =
  | 'list_sandboxes'
  | 'create_sandbox'
  | 'delete_sandbox'
  | 'start_sandbox'
  | 'stop_sandbox'
  | 'restart_sandbox'
  | 'suspend_sandbox'
  | 'get_sandbox'
  | 'computer_screenshot'
  | 'computer_click'
  | 'computer_type'
  | 'computer_key'
  | 'computer_scroll'
  | 'computer_drag'
  | 'computer_hotkey'
  | 'computer_clipboard'
  | 'computer_file'
  | 'computer_shell'
  | 'computer_window'
  | 'skill_list'
  | 'skill_read'
  | 'skill_record'
  | 'skill_delete';

const ALL_PERMISSIONS: Permission[] = [
  'list_sandboxes',
  'create_sandbox',
  'delete_sandbox',
  'start_sandbox',
  'stop_sandbox',
  'restart_sandbox',
  'suspend_sandbox',
  'get_sandbox',
  'computer_screenshot',
  'computer_click',
  'computer_type',
  'computer_key',
  'computer_scroll',
  'computer_drag',
  'computer_hotkey',
  'computer_clipboard',
  'computer_file',
  'computer_shell',
  'computer_window',
  'skill_list',
  'skill_read',
  'skill_record',
  'skill_delete',
];

// Skills directory path
const SKILLS_DIR = join(homedir(), '.cua', 'skills');

// Ensure skills directory exists
async function ensureSkillsDir(): Promise<void> {
  const fs = await import('fs/promises');
  await fs.mkdir(SKILLS_DIR, { recursive: true });
}

// Get skill directory path
function getSkillDir(name: string): string {
  return join(SKILLS_DIR, name);
}

// List all skills
async function listSkills(): Promise<string[]> {
  const fs = await import('fs/promises');
  await ensureSkillsDir();

  try {
    const entries = await fs.readdir(SKILLS_DIR, { withFileTypes: true });
    const skills: string[] = [];

    for (const entry of entries) {
      if (entry.isDirectory()) {
        const skillMdPath = join(SKILLS_DIR, entry.name, 'SKILL.md');
        try {
          await fs.access(skillMdPath);
          skills.push(entry.name);
        } catch {
          // Skip directories without SKILL.md
        }
      }
    }

    return skills;
  } catch {
    return [];
  }
}

// Recursively get all files in a directory
async function getAllFiles(dir: string): Promise<string[]> {
  const fs = await import('fs/promises');
  const files: string[] = [];

  try {
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = join(dir, entry.name);
      if (entry.isDirectory()) {
        const subFiles = await getAllFiles(fullPath);
        files.push(...subFiles);
      } else {
        files.push(fullPath);
      }
    }
  } catch {
    // Directory doesn't exist or can't be read
  }

  return files;
}

// Read a specific skill - returns raw SKILL.md content with file paths prefixed
async function readSkill(name: string): Promise<string | null> {
  const fs = await import('fs/promises');
  const skillDir = getSkillDir(name);
  const skillMdPath = join(skillDir, 'SKILL.md');

  try {
    const content = await fs.readFile(skillMdPath, 'utf-8');
    const files = await getAllFiles(skillDir);

    // Prefix with file paths
    const filesList = files.map((f) => f).join('\n');
    return `## Files\n\n${filesList}\n\n---\n\n${content}`;
  } catch {
    return null;
  }
}

// Parse permissions from environment variable
function parsePermissions(permEnv: string | undefined): Set<Permission> {
  if (!permEnv) {
    return new Set(ALL_PERMISSIONS);
  }
  const perms = permEnv.split(',').map((p) => p.trim() as Permission);
  return new Set(perms.filter((p) => ALL_PERMISSIONS.includes(p)));
}

// Send command to sandbox's computer-server via /cmd endpoint
async function sendCommand(
  sandbox: string,
  command: string,
  params: Record<string, unknown>,
  token: string
): Promise<{ success: boolean; data?: unknown; error?: string }> {
  // First, get the sandbox details to find the host
  const listRes = await http('/v1/vms', { token });
  if (!listRes.ok) {
    return {
      success: false,
      error: `Failed to list sandboxes: ${listRes.status}`,
    };
  }

  const sandboxes = (await listRes.json()) as SandboxItem[];
  const sb = sandboxes.find((s) => s.name === sandbox);

  if (!sb) {
    return { success: false, error: `Sandbox not found: ${sandbox}` };
  }

  if (sb.status !== 'running') {
    return {
      success: false,
      error: `Sandbox is not running (status: ${sb.status})`,
    };
  }

  const host = sb.host || `${sb.name}.sandbox.cua.ai`;
  const cmdUrl = `https://${host}:8443/cmd`;

  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 30000);

    const res = await fetch(cmdUrl, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'X-Container-Name': sandbox,
        'X-API-Key': token,
      },
      body: JSON.stringify({ command, params }),
      signal: controller.signal,
    });

    clearTimeout(timeout);

    if (!res.ok) {
      const errorText = await res.text();
      return {
        success: false,
        error: `Command failed: ${res.status} - ${errorText}`,
      };
    }

    // Parse SSE response format: "data: {...}"
    const text = await res.text();
    if (text.startsWith('data: ')) {
      const jsonStr = text.slice(6);
      const data = JSON.parse(jsonStr);
      return { success: true, data };
    }

    // Try parsing as regular JSON
    try {
      const data = JSON.parse(text);
      return { success: true, data };
    } catch {
      return { success: true, data: text };
    }
  } catch (err) {
    return { success: false, error: `Command error: ${err}` };
  }
}

// Create and run the MCP server
async function runMcpServer(permissions: Set<Permission>) {
  const token = getApiKey();
  if (!token) {
    console.error('Error: No API key found. Run "cua auth login" first.');
    process.exit(1);
  }

  const server = new McpServer({
    name: 'cua-cloud',
    version: '1.0.0',
  });

  // ============================================================
  // SANDBOX MANAGEMENT TOOLS
  // ============================================================

  if (permissions.has('list_sandboxes')) {
    server.tool(
      'list_sandboxes',
      'List all cloud sandboxes in your account with their status and details',
      {},
      async () => {
        const res = await http('/v1/vms', { token });
        if (!res.ok) {
          return {
            content: [
              { type: 'text', text: `Failed to list sandboxes: ${res.status}` },
            ],
          };
        }
        const sandboxes = (await res.json()) as SandboxItem[];
        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(
                sandboxes.map((s) => ({
                  name: s.name,
                  status: s.status,
                  host: s.host || `${s.name}.sandbox.cua.ai`,
                })),
                null,
                2
              ),
            },
          ],
        };
      }
    );
  }

  if (permissions.has('create_sandbox')) {
    server.tool(
      'create_sandbox',
      'Create a new cloud sandbox with specified OS, size, and region',
      {
        os: z
          .enum(['linux', 'windows', 'macos'])
          .describe('Operating system for the sandbox'),
        size: z
          .enum(['small', 'medium', 'large'])
          .describe('Size/configuration of the sandbox'),
        region: z
          .enum(['north-america', 'europe', 'asia-pacific', 'south-america'])
          .describe('Geographic region for the sandbox'),
      },
      async ({ os, size, region }) => {
        const res = await http('/v1/vms', {
          token,
          method: 'POST',
          body: { os, configuration: size, region },
        });

        if (res.status === 200) {
          const data = (await res.json()) as {
            name: string;
            password: string;
            host: string;
          };
          return {
            content: [
              {
                type: 'text',
                text: `Sandbox created: ${data.name}\nHost: ${data.host}\nPassword: ${data.password}`,
              },
            ],
          };
        }

        if (res.status === 202) {
          const data = (await res.json()) as { name: string; job_id: string };
          return {
            content: [
              {
                type: 'text',
                text: `Sandbox provisioning started: ${data.name}\nJob ID: ${data.job_id}`,
              },
            ],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to create sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('delete_sandbox')) {
    server.tool(
      'delete_sandbox',
      'Permanently delete a sandbox and all its data',
      {
        sandbox: z.string().describe('Name of the sandbox to delete'),
      },
      async ({ sandbox }) => {
        const res = await http(`/v1/vms/${encodeURIComponent(sandbox)}`, {
          token,
          method: 'DELETE',
        });

        if (res.status === 202) {
          return {
            content: [
              { type: 'text', text: `Sandbox deletion initiated: ${sandbox}` },
            ],
          };
        }

        if (res.status === 404) {
          return {
            content: [{ type: 'text', text: `Sandbox not found: ${sandbox}` }],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to delete sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('start_sandbox')) {
    server.tool(
      'start_sandbox',
      'Start a stopped sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox to start'),
      },
      async ({ sandbox }) => {
        const res = await http(`/v1/vms/${encodeURIComponent(sandbox)}/start`, {
          token,
          method: 'POST',
        });

        if (res.status === 204) {
          return {
            content: [{ type: 'text', text: `Start accepted for: ${sandbox}` }],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to start sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('stop_sandbox')) {
    server.tool(
      'stop_sandbox',
      'Stop a running sandbox (data is preserved)',
      {
        sandbox: z.string().describe('Name of the sandbox to stop'),
      },
      async ({ sandbox }) => {
        const res = await http(`/v1/vms/${encodeURIComponent(sandbox)}/stop`, {
          token,
          method: 'POST',
        });

        if (res.status === 202) {
          return {
            content: [{ type: 'text', text: `Stop initiated for: ${sandbox}` }],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to stop sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('restart_sandbox')) {
    server.tool(
      'restart_sandbox',
      'Restart a sandbox (reboot the system)',
      {
        sandbox: z.string().describe('Name of the sandbox to restart'),
      },
      async ({ sandbox }) => {
        const res = await http(
          `/v1/vms/${encodeURIComponent(sandbox)}/restart`,
          {
            token,
            method: 'POST',
          }
        );

        if (res.status === 202) {
          return {
            content: [
              { type: 'text', text: `Restart initiated for: ${sandbox}` },
            ],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to restart sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('suspend_sandbox')) {
    server.tool(
      'suspend_sandbox',
      'Suspend a sandbox, preserving memory state',
      {
        sandbox: z.string().describe('Name of the sandbox to suspend'),
      },
      async ({ sandbox }) => {
        const res = await http(
          `/v1/vms/${encodeURIComponent(sandbox)}/suspend`,
          {
            token,
            method: 'POST',
          }
        );

        if (res.status === 202) {
          return {
            content: [
              { type: 'text', text: `Suspend initiated for: ${sandbox}` },
            ],
          };
        }

        return {
          content: [
            { type: 'text', text: `Failed to suspend sandbox: ${res.status}` },
          ],
        };
      }
    );
  }

  if (permissions.has('get_sandbox')) {
    server.tool(
      'get_sandbox',
      'Get detailed information about a specific sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox to get info for'),
      },
      async ({ sandbox }) => {
        const res = await http('/v1/vms', { token });
        if (!res.ok) {
          return {
            content: [
              { type: 'text', text: `Failed to get sandbox: ${res.status}` },
            ],
          };
        }

        const sandboxes = (await res.json()) as SandboxItem[];
        const sb = sandboxes.find((s) => s.name === sandbox);

        if (!sb) {
          return {
            content: [{ type: 'text', text: `Sandbox not found: ${sandbox}` }],
          };
        }

        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(
                {
                  name: sb.name,
                  status: sb.status,
                  host: sb.host || `${sb.name}.sandbox.cua.ai`,
                },
                null,
                2
              ),
            },
          ],
        };
      }
    );
  }

  // ============================================================
  // COMPUTER CONTROL TOOLS (proxied to sandbox /cmd endpoint)
  // ============================================================

  if (permissions.has('computer_screenshot')) {
    server.tool(
      'computer_screenshot',
      'Take a screenshot of the sandbox screen. Returns a base64-encoded image.',
      {
        sandbox: z.string().describe('Name of the sandbox to screenshot'),
      },
      async ({ sandbox }) => {
        const result = await sendCommand(sandbox, 'screenshot', {}, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Screenshot failed' },
            ],
          };
        }

        const data = result.data as { image_data?: string; success?: boolean };
        if (data?.image_data) {
          return {
            content: [
              {
                type: 'image',
                data: data.image_data,
                mimeType: 'image/png',
              },
            ],
          };
        }

        return {
          content: [{ type: 'text', text: JSON.stringify(result.data) }],
        };
      }
    );

    server.tool(
      'computer_get_screen_size',
      'Get the screen dimensions of the sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox'),
      },
      async ({ sandbox }) => {
        const result = await sendCommand(sandbox, 'get_screen_size', {}, token);
        if (!result.success) {
          return {
            content: [
              {
                type: 'text',
                text: result.error || 'Failed to get screen size',
              },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result.data) }],
        };
      }
    );
  }

  if (permissions.has('computer_click')) {
    server.tool(
      'computer_click',
      'Click at the specified screen coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z.number().describe('X coordinate in pixels'),
        y: z.number().describe('Y coordinate in pixels'),
        button: z
          .enum(['left', 'right', 'middle'])
          .optional()
          .default('left')
          .describe('Mouse button to click'),
      },
      async ({ sandbox, x, y, button }) => {
        // Use the appropriate click command based on button
        let command: string;
        if (button === 'right') {
          command = 'right_click';
        } else if (button === 'middle') {
          // Middle click is mouse_down + mouse_up with middle button
          await sendCommand(
            sandbox,
            'mouse_down',
            { x, y, button: 'middle' },
            token
          );
          const result = await sendCommand(
            sandbox,
            'mouse_up',
            { x, y, button: 'middle' },
            token
          );
          if (!result.success) {
            return {
              content: [
                { type: 'text', text: result.error || 'Middle click failed' },
              ],
            };
          }
          return {
            content: [
              {
                type: 'text',
                text: `Middle-clicked at (${x}, ${y})`,
              },
            ],
          };
        } else {
          command = 'left_click';
        }

        const result = await sendCommand(sandbox, command, { x, y }, token);
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Click failed' }],
          };
        }
        return {
          content: [
            {
              type: 'text',
              text: `Clicked at (${x}, ${y}) with ${button} button`,
            },
          ],
        };
      }
    );

    server.tool(
      'computer_double_click',
      'Double-click at the specified screen coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z.number().describe('X coordinate in pixels'),
        y: z.number().describe('Y coordinate in pixels'),
      },
      async ({ sandbox, x, y }) => {
        const result = await sendCommand(
          sandbox,
          'double_click',
          { x, y },
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Double click failed' },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: `Double-clicked at (${x}, ${y})` }],
        };
      }
    );

    server.tool(
      'computer_move',
      'Move the cursor to the specified screen coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z.number().describe('X coordinate in pixels'),
        y: z.number().describe('Y coordinate in pixels'),
      },
      async ({ sandbox, x, y }) => {
        const result = await sendCommand(
          sandbox,
          'move_cursor',
          { x, y },
          token
        );
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Move failed' }],
          };
        }
        return {
          content: [{ type: 'text', text: `Moved cursor to (${x}, ${y})` }],
        };
      }
    );

    server.tool(
      'computer_mousedown',
      'Press and hold a mouse button at the specified coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z
          .number()
          .optional()
          .describe(
            'X coordinate in pixels (uses current position if not specified)'
          ),
        y: z
          .number()
          .optional()
          .describe(
            'Y coordinate in pixels (uses current position if not specified)'
          ),
        button: z
          .enum(['left', 'right', 'middle'])
          .optional()
          .default('left')
          .describe('Mouse button to press'),
      },
      async ({ sandbox, x, y, button }) => {
        const params: Record<string, unknown> = { button };
        if (x !== undefined) params.x = x;
        if (y !== undefined) params.y = y;
        const result = await sendCommand(sandbox, 'mouse_down', params, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Mouse down failed' },
            ],
          };
        }
        const posStr =
          x !== undefined && y !== undefined ? ` at (${x}, ${y})` : '';
        return {
          content: [
            { type: 'text', text: `Mouse ${button} button pressed${posStr}` },
          ],
        };
      }
    );

    server.tool(
      'computer_mouseup',
      'Release a mouse button at the specified coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z
          .number()
          .optional()
          .describe(
            'X coordinate in pixels (uses current position if not specified)'
          ),
        y: z
          .number()
          .optional()
          .describe(
            'Y coordinate in pixels (uses current position if not specified)'
          ),
        button: z
          .enum(['left', 'right', 'middle'])
          .optional()
          .default('left')
          .describe('Mouse button to release'),
      },
      async ({ sandbox, x, y, button }) => {
        const params: Record<string, unknown> = { button };
        if (x !== undefined) params.x = x;
        if (y !== undefined) params.y = y;
        const result = await sendCommand(sandbox, 'mouse_up', params, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Mouse up failed' },
            ],
          };
        }
        const posStr =
          x !== undefined && y !== undefined ? ` at (${x}, ${y})` : '';
        return {
          content: [
            { type: 'text', text: `Mouse ${button} button released${posStr}` },
          ],
        };
      }
    );

    server.tool(
      'computer_mousemove',
      'Move the mouse cursor to the specified coordinates without clicking',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z.number().describe('X coordinate in pixels'),
        y: z.number().describe('Y coordinate in pixels'),
      },
      async ({ sandbox, x, y }) => {
        const result = await sendCommand(
          sandbox,
          'move_cursor',
          { x, y },
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Mouse move failed' },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: `Mouse moved to (${x}, ${y})` }],
        };
      }
    );
  }

  if (permissions.has('computer_type')) {
    server.tool(
      'computer_type',
      'Type text at the current cursor position',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        text: z.string().describe('The text to type'),
      },
      async ({ sandbox, text }) => {
        const result = await sendCommand(sandbox, 'type_text', { text }, token);
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Type failed' }],
          };
        }
        return { content: [{ type: 'text', text: `Typed: "${text}"` }] };
      }
    );
  }

  if (permissions.has('computer_key')) {
    server.tool(
      'computer_press_key',
      'Press a single key',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        key: z
          .string()
          .describe('The key to press (e.g., enter, tab, escape, a, b)'),
      },
      async ({ sandbox, key }) => {
        const result = await sendCommand(sandbox, 'press_key', { key }, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Key press failed' },
            ],
          };
        }
        return { content: [{ type: 'text', text: `Pressed key: ${key}` }] };
      }
    );

    server.tool(
      'computer_keydown',
      'Press and hold a key (use with computer_keyup to release)',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        key: z
          .string()
          .describe('The key to hold down (e.g., shift, ctrl, alt, cmd)'),
      },
      async ({ sandbox, key }) => {
        const result = await sendCommand(sandbox, 'key_down', { key }, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Key down failed' },
            ],
          };
        }
        return { content: [{ type: 'text', text: `Key held down: ${key}` }] };
      }
    );

    server.tool(
      'computer_keyup',
      'Release a held key',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        key: z.string().describe('The key to release'),
      },
      async ({ sandbox, key }) => {
        const result = await sendCommand(sandbox, 'key_up', { key }, token);
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Key up failed' }],
          };
        }
        return { content: [{ type: 'text', text: `Key released: ${key}` }] };
      }
    );
  }

  if (permissions.has('computer_hotkey')) {
    server.tool(
      'computer_hotkey',
      'Press a key combination (hotkey)',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        keys: z
          .array(z.string())
          .describe(
            'List of keys to press together (e.g., ["ctrl", "c"] for copy)'
          ),
      },
      async ({ sandbox, keys }) => {
        const result = await sendCommand(sandbox, 'hotkey', { keys }, token);
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Hotkey failed' }],
          };
        }
        return {
          content: [
            { type: 'text', text: `Pressed hotkey: ${keys.join('+')}` },
          ],
        };
      }
    );
  }

  if (permissions.has('computer_scroll')) {
    server.tool(
      'computer_scroll',
      'Scroll at the specified position',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        x: z.number().describe('X coordinate where to scroll'),
        y: z.number().describe('Y coordinate where to scroll'),
        scroll_x: z
          .number()
          .optional()
          .default(0)
          .describe('Horizontal scroll amount (positive = right)'),
        scroll_y: z
          .number()
          .optional()
          .default(0)
          .describe('Vertical scroll amount (positive = down)'),
      },
      async ({ sandbox, x, y, scroll_x, scroll_y }) => {
        // First move cursor to position
        await sendCommand(sandbox, 'move_cursor', { x, y }, token);
        const result = await sendCommand(
          sandbox,
          'scroll',
          { scroll_x, scroll_y },
          token
        );
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Scroll failed' }],
          };
        }
        return {
          content: [
            {
              type: 'text',
              text: `Scrolled at (${x}, ${y}) by (${scroll_x}, ${scroll_y})`,
            },
          ],
        };
      }
    );
  }

  if (permissions.has('computer_drag')) {
    server.tool(
      'computer_drag',
      'Drag from start coordinates to end coordinates',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        start_x: z.number().describe('Starting X coordinate'),
        start_y: z.number().describe('Starting Y coordinate'),
        end_x: z.number().describe('Ending X coordinate'),
        end_y: z.number().describe('Ending Y coordinate'),
        button: z
          .enum(['left', 'right'])
          .optional()
          .default('left')
          .describe('Mouse button to use'),
      },
      async ({ sandbox, start_x, start_y, end_x, end_y, button }) => {
        // Move to start position first
        await sendCommand(
          sandbox,
          'move_cursor',
          { x: start_x, y: start_y },
          token
        );
        const result = await sendCommand(
          sandbox,
          'drag_to',
          { x: end_x, y: end_y, button },
          token
        );
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Drag failed' }],
          };
        }
        return {
          content: [
            {
              type: 'text',
              text: `Dragged from (${start_x}, ${start_y}) to (${end_x}, ${end_y})`,
            },
          ],
        };
      }
    );
  }

  if (permissions.has('computer_clipboard')) {
    server.tool(
      'computer_clipboard_get',
      'Get the current clipboard content',
      {
        sandbox: z.string().describe('Name of the sandbox'),
      },
      async ({ sandbox }) => {
        const result = await sendCommand(
          sandbox,
          'copy_to_clipboard',
          {},
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Failed to get clipboard' },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result.data) }],
        };
      }
    );

    server.tool(
      'computer_clipboard_set',
      'Set the clipboard content',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        text: z.string().describe('Text to copy to clipboard'),
      },
      async ({ sandbox, text }) => {
        const result = await sendCommand(
          sandbox,
          'set_clipboard',
          { text },
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Failed to set clipboard' },
            ],
          };
        }
        return { content: [{ type: 'text', text: 'Clipboard set' }] };
      }
    );
  }

  if (permissions.has('computer_shell')) {
    server.tool(
      'computer_run_command',
      'Execute a shell command and return the output',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        command: z.string().describe('The shell command to execute'),
      },
      async ({ sandbox, command }) => {
        const result = await sendCommand(
          sandbox,
          'run_command',
          { command },
          token
        );
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Command failed' }],
          };
        }
        return {
          content: [
            { type: 'text', text: JSON.stringify(result.data, null, 2) },
          ],
        };
      }
    );
  }

  if (permissions.has('computer_file')) {
    server.tool(
      'computer_file_read',
      'Read the text content of a file on the sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        path: z.string().describe('Path to the file to read'),
      },
      async ({ sandbox, path }) => {
        const result = await sendCommand(sandbox, 'read_text', { path }, token);
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Failed to read file' },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result.data) }],
        };
      }
    );

    server.tool(
      'computer_file_write',
      'Write text content to a file on the sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        path: z.string().describe('Path to the file to write'),
        content: z.string().describe('Text content to write'),
      },
      async ({ sandbox, path, content }) => {
        const result = await sendCommand(
          sandbox,
          'write_text',
          { path, content },
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Failed to write file' },
            ],
          };
        }
        return { content: [{ type: 'text', text: `File written: ${path}` }] };
      }
    );

    server.tool(
      'computer_list_directory',
      'List the contents of a directory on the sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        path: z.string().describe('Path to the directory'),
      },
      async ({ sandbox, path }) => {
        const result = await sendCommand(sandbox, 'list_dir', { path }, token);
        if (!result.success) {
          return {
            content: [
              {
                type: 'text',
                text: result.error || 'Failed to list directory',
              },
            ],
          };
        }
        return {
          content: [
            { type: 'text', text: JSON.stringify(result.data, null, 2) },
          ],
        };
      }
    );
  }

  if (permissions.has('computer_window')) {
    server.tool(
      'computer_open',
      'Open a file or URL with the default application',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        target: z.string().describe('File path or URL to open'),
      },
      async ({ sandbox, target }) => {
        const result = await sendCommand(sandbox, 'open', { target }, token);
        if (!result.success) {
          return {
            content: [{ type: 'text', text: result.error || 'Failed to open' }],
          };
        }
        return { content: [{ type: 'text', text: `Opened: ${target}` }] };
      }
    );

    server.tool(
      'computer_launch_app',
      'Launch an application on the sandbox',
      {
        sandbox: z.string().describe('Name of the sandbox'),
        app: z.string().describe('Application name or path'),
        args: z
          .array(z.string())
          .optional()
          .describe('Optional arguments to pass to the app'),
      },
      async ({ sandbox, app, args }) => {
        const result = await sendCommand(
          sandbox,
          'launch',
          { app, args: args || [] },
          token
        );
        if (!result.success) {
          return {
            content: [
              { type: 'text', text: result.error || 'Failed to launch app' },
            ],
          };
        }
        return { content: [{ type: 'text', text: `Launched: ${app}` }] };
      }
    );

    server.tool(
      'computer_get_active_window',
      'Get the currently active window ID',
      {
        sandbox: z.string().describe('Name of the sandbox'),
      },
      async ({ sandbox }) => {
        const result = await sendCommand(
          sandbox,
          'get_current_window_id',
          {},
          token
        );
        if (!result.success) {
          return {
            content: [
              {
                type: 'text',
                text: result.error || 'Failed to get active window',
              },
            ],
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result.data) }],
        };
      }
    );
  }

  // ============================================================
  // SKILL MANAGEMENT TOOLS
  // ============================================================

  if (permissions.has('skill_list')) {
    server.tool(
      'skill_list',
      'List all saved skills (human demonstrations that can guide agent behavior)',
      {},
      async () => {
        const skills = await listSkills();
        if (skills.length === 0) {
          return {
            content: [
              {
                type: 'text',
                text: 'No skills found. Use skill_record to record a skill from a sandbox.',
              },
            ],
          };
        }
        return {
          content: [
            {
              type: 'text',
              text: skills.join('\n'),
            },
          ],
        };
      }
    );
  }

  if (permissions.has('skill_read')) {
    server.tool(
      'skill_read',
      'Read a skill and get its skill_prompt for guiding agent behavior. Returns all trajectory file paths followed by the SKILL.md content.',
      {
        name: z.string().describe('Name of the skill to read'),
      },
      async ({ name }) => {
        const content = await readSkill(name);
        if (!content) {
          return {
            content: [{ type: 'text', text: `Skill not found: ${name}` }],
          };
        }
        return {
          content: [
            {
              type: 'text',
              text: content,
            },
          ],
        };
      }
    );
  }

  if (permissions.has('skill_record')) {
    server.tool(
      'skill_record',
      'Record a human demonstration from a sandbox and save it as a reusable skill. Opens the VNC interface in a browser for recording. The user performs the task, then clicks Stop Recording. The recording is automatically processed with VLM captions and saved.',
      {
        sandbox: z.string().describe('Name of the sandbox to record from'),
        name: z
          .string()
          .regex(/^[a-z0-9_-]+$/)
          .describe(
            'Unique name for the skill (lowercase, hyphens, underscores only)'
          ),
        description: z
          .string()
          .describe(
            'Human-readable description of what this skill demonstrates'
          ),
      },
      async ({ sandbox, name, description }) => {
        // Invoke the cua skills record CLI command
        return new Promise((resolve) => {
          const args = [
            'skills',
            'record',
            '--sandbox',
            sandbox,
            '--name',
            name,
            '--description',
            description,
          ];

          const proc = spawn('cua', args, {
            stdio: ['inherit', 'pipe', 'pipe'],
            env: { ...process.env },
          });

          let stdout = '';
          let stderr = '';

          proc.stdout?.on('data', (data) => {
            stdout += data.toString();
          });

          proc.stderr?.on('data', (data) => {
            stderr += data.toString();
          });

          proc.on('close', (code) => {
            if (code !== 0) {
              resolve({
                content: [
                  {
                    type: 'text',
                    text: `Recording failed (exit ${code}):\n${stderr || stdout}`,
                  },
                ],
              });
            } else {
              const skillDir = getSkillDir(name);
              resolve({
                content: [
                  {
                    type: 'text',
                    text: `Skill recorded successfully: ${name}\nDirectory: ${skillDir}\n\n${stdout}`,
                  },
                ],
              });
            }
          });

          proc.on('error', (err) => {
            resolve({
              content: [
                {
                  type: 'text',
                  text: `Failed to run cua skills record: ${err.message}`,
                },
              ],
            });
          });
        });
      }
    );
  }

  if (permissions.has('skill_delete')) {
    server.tool(
      'skill_delete',
      'Delete a skill and all its trajectory files',
      {
        name: z.string().describe('Name of the skill to delete'),
      },
      async ({ name }) => {
        const fs = await import('fs/promises');
        const skillDir = getSkillDir(name);

        try {
          await fs.access(join(skillDir, 'SKILL.md'));
        } catch {
          return {
            content: [{ type: 'text', text: `Skill not found: ${name}` }],
          };
        }

        await fs.rm(skillDir, { recursive: true, force: true });

        return {
          content: [
            {
              type: 'text',
              text: `Skill deleted: ${name}`,
            },
          ],
        };
      }
    );
  }

  // Connect via stdio transport
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

// Command handler
const serveMcpHandler = async (argv: Record<string, unknown>) => {
  const permEnv = argv.permissions as string | undefined;
  const permissions = parsePermissions(
    permEnv || process.env.CUA_MCP_PERMISSIONS
  );

  await runMcpServer(permissions);
};

// Register command
export function registerServeMcpCommands(y: Argv) {
  y.command(
    'serve-mcp',
    'Start an MCP server for Claude Code integration',
    (yargs) =>
      yargs
        .option('permissions', {
          alias: 'p',
          type: 'string',
          description:
            'Comma-separated list of allowed permissions (default: all). ' +
            'Available: list_sandboxes, create_sandbox, delete_sandbox, start_sandbox, ' +
            'stop_sandbox, restart_sandbox, suspend_sandbox, get_sandbox, ' +
            'computer_screenshot, computer_click, computer_type, computer_key, ' +
            'computer_scroll, computer_drag, computer_hotkey, computer_clipboard, ' +
            'computer_file, computer_shell, computer_window, ' +
            'skill_list, skill_read, skill_record, skill_delete',
        })
        .example(
          'claude mcp add cua-server -- cua serve-mcp',
          'Add CUA as an MCP server to Claude Code'
        )
        .example(
          'claude mcp add -e CUA_MCP_PERMISSIONS=list_sandboxes,computer_screenshot cua-server -- cua serve-mcp',
          'Add with limited permissions'
        ),
    serveMcpHandler
  );

  return y;
}
