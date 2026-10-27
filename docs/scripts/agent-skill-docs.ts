// The agent pages as plain Markdown, the exact text cua.ai/join/<code> and
// cua.ai/add/<code> serve to an agent (trycua/cloud, src/spaces-site, a vendored
// copy). One source: the pages under content/docs/agents.
//
//   pnpm exec tsx scripts/agent-skill-docs.ts            # print the vendored files' sha256
//   pnpm exec tsx scripts/agent-skill-docs.ts --write DIR  # write DIR/<page>.md
//
// `agents-docs.test.ts` pins the hashes in content/docs/agents/skill-docs.sha256.
// When you edit a page, run `--write` into src/spaces-site/api/_lib/skills/ of
// trycua/cloud, update the pin, and open that PR too.
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const agents = path.resolve(__dirname, '../content/docs/agents');

/** The pages the site serves as the skill doc behind a link with a code. */
export const SKILL_PAGES = ['join-a-space', 'add-a-computer'] as const;

/** An agent page as Markdown: its title as the heading, no frontmatter, no
 * fence attributes (` ```bash skip="..." ` becomes ` ```bash `), links absolute. */
export function renderMarkdown(mdx: string): string {
  const match = mdx.match(/^---\n([\s\S]*?)\n---\n/);
  if (!match) throw new Error('page has no frontmatter');
  const title = match[1].match(/^title:\s*(.+)$/m)?.[1].trim().replace(/^'(.*)'$/, '$1');
  if (!title) throw new Error('page has no title');
  const body = mdx
    .slice(match[0].length)
    .replace(/^(\s*```\w*)[^\n]*$/gm, '$1')
    .replace(/\]\((\/[^)]*)\)/g, '](https://cua.ai/docs$1)')
    .trim();
  return `# ${title}\n\n${body}\n`;
}

export function rendered(page: string): string {
  return renderMarkdown(readFileSync(path.join(agents, `${page}.mdx`), 'utf8'));
}

export const sha256 = (text: string) => createHash('sha256').update(text).digest('hex');

if (require.main === module) {
  const at = process.argv.indexOf('--write');
  for (const page of SKILL_PAGES) {
    const text = rendered(page);
    if (at > 0) {
      const dir = process.argv[at + 1];
      mkdirSync(dir, { recursive: true });
      writeFileSync(path.join(dir, `${page}.md`), text);
    }
    console.log(`${sha256(text)}  ${page}.md`);
  }
}
