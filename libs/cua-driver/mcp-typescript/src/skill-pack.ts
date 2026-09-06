import { realpath, readFile, readdir } from 'node:fs/promises';
import { basename, isAbsolute, relative, resolve, sep } from 'node:path';
import type { CallToolResult, Resource, Tool } from '@modelcontextprotocol/sdk/types.js';

export const SKILL_NAME = 'cua-driver';
export const LOAD_SKILL_TOOL = 'load-skill';
export const SKILL_URI_PREFIX = 'cua-driver-skill://cua-driver/';

function parseDescription(skill: string): string {
  const frontmatter = skill.match(/^---\n([\s\S]*?)\n---/);
  const metadata = frontmatter?.[1];
  return metadata?.match(/^description:\s*(.+)$/m)?.[1]?.trim() ?? 'Operate Cua Driver.';
}

export async function buildSkillInstructions(skillRoot: string): Promise<string> {
  const description = parseDescription(await readFile(resolve(skillRoot, 'SKILL.md'), 'utf8'));
  return [
    'This server exposes the release-matched Cua Driver skill through `load-skill`.',
    'When relevant, call `load-skill` before acting. Load companion files only when SKILL.md references them.',
    '',
    '<available_skills>',
    `<skill><name>${SKILL_NAME}</name><description>${description}</description></skill>`,
    '</available_skills>',
  ].join('\n');
}

async function resolveSkillFile(skillRoot: string, requestedPath: string): Promise<string> {
  if (!requestedPath || isAbsolute(requestedPath)) throw new Error('skill path must be relative');
  const normalized = requestedPath.replaceAll('\\', '/');
  if (normalized.split('/').includes('..')) throw new Error('skill path traversal is not allowed');

  const root = await realpath(skillRoot);
  const candidate = await realpath(resolve(root, normalized));
  const child = relative(root, candidate);
  if (!child || child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child)) {
    throw new Error('skill path escapes the bundled skill root');
  }
  return candidate;
}

export const loadSkillTool: Tool = {
  name: LOAD_SKILL_TOOL,
  title: 'Load Cua Driver Skill',
  description: 'Load bundled Cua Driver SKILL.md or a referenced companion Markdown file.',
  inputSchema: {
    type: 'object',
    additionalProperties: false,
    properties: {
      name: { type: 'string', enum: [SKILL_NAME] },
      path: { type: 'string', default: 'SKILL.md' },
    },
    required: ['name'],
  },
  annotations: {
    readOnlyHint: true,
    destructiveHint: false,
    idempotentHint: true,
    openWorldHint: false,
  },
};

export async function loadSkill(
  skillRoot: string,
  args: Record<string, unknown>
): Promise<CallToolResult> {
  const extraKeys = Object.keys(args).filter((key) => key !== 'name' && key !== 'path');
  if (extraKeys.length > 0) {
    return {
      content: [{ type: 'text', text: `Unknown load-skill fields: ${extraKeys.join(', ')}` }],
      isError: true,
    };
  }
  if (args.name !== SKILL_NAME) {
    return {
      content: [{ type: 'text', text: `Unknown skill: ${String(args.name)}` }],
      isError: true,
    };
  }
  if (args.path !== undefined && typeof args.path !== 'string') {
    return { content: [{ type: 'text', text: 'skill path must be a string' }], isError: true };
  }
  try {
    const requestedPath = args.path ?? 'SKILL.md';
    const path = await resolveSkillFile(skillRoot, requestedPath);
    if (!path.endsWith('.md')) throw new Error('only Markdown skill files are readable');
    return { content: [{ type: 'text', text: await readFile(path, 'utf8') }] };
  } catch (error) {
    return {
      content: [{ type: 'text', text: error instanceof Error ? error.message : String(error) }],
      isError: true,
    };
  }
}

export async function listSkillResources(skillRoot: string): Promise<Resource[]> {
  const files = (await readdir(skillRoot, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith('.md') && entry.name !== 'SKILL.md')
    .map((entry) => entry.name)
    .sort();
  return files.map((file) => ({
    uri: `${SKILL_URI_PREFIX}${file}`,
    name: `${SKILL_NAME}/${file}`,
    title: basename(file, '.md'),
    description: `Bundled companion file for the ${SKILL_NAME} skill.`,
    mimeType: 'text/markdown',
  }));
}

export async function readSkillResource(skillRoot: string, uri: string) {
  if (!uri.startsWith(SKILL_URI_PREFIX)) throw new Error('unknown skill resource URI');
  const requestedPath = decodeURIComponent(uri.slice(SKILL_URI_PREFIX.length));
  const path = await resolveSkillFile(skillRoot, requestedPath);
  if (!path.endsWith('.md') || path.endsWith(`${sep}SKILL.md`)) {
    throw new Error('resource URI must identify a companion Markdown file');
  }
  return { uri, mimeType: 'text/markdown', text: await readFile(path, 'utf8') };
}
