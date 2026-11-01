// Agents in a Space: the docs agree with the code on the facts they used to
// contradict each other about.
//
//   pnpm exec tsx --test scripts/agents-docs.test.ts
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { SKILL_PAGES, rendered, sha256 } from './agent-skill-docs';

const repo = path.resolve(__dirname, '../..');
const content = path.join(repo, 'docs/content/docs');
const page = (rel: string) => readFileSync(path.join(content, rel), 'utf8');

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : full.endsWith('.mdx') ? [full] : [];
  });
}

test('the agents page names every harness the code supports', () => {
  const src = readFileSync(path.join(repo, 'libs/cua/crates/cua-agents/src/harness.rs'), 'utf8');
  const table = src.slice(src.indexOf('pub const HARNESSES'));
  const ids = [...table.matchAll(/^\s+id: "([a-z0-9-]+)",/gm)].map((m) => m[1]);
  assert.ok(ids.length >= 2, 'read the harness ids from harness.rs');
  const text = page('spaces/guides/agents-in-a-space.mdx');
  const missing = ids.filter((id) => !text.includes(`\`${id}\``));
  assert.deepEqual(missing, [], 'harnesses missing from agents-in-a-space.mdx');
});

test('no page says the daemon copies host credential files into a Space', () => {
  const src = readFileSync(path.join(repo, 'libs/cua/crates/cua-spaces/src/agents/mod.rs'), 'utf8');
  assert.match(src, /Host credential files are never copied/, 'the code still says so');
  const bad = walk(content).filter((f) =>
    /copies your agent's credential|CUA_SPACES_AGENT_CREDENTIALS_HOME/.test(readFileSync(f, 'utf8'))
  );
  assert.deepEqual(bad.map((f) => path.relative(content, f)), []);
});

test('the sample is CuaBots, never Grok Bot', () => {
  const bad = walk(content).filter((f) => /grok ?bot/i.test(readFileSync(f, 'utf8')));
  assert.deepEqual(bad.map((f) => path.relative(content, f)), []);
});

// The agent pages (cua.ai/docs/agents/*): one prompt line links to each, so
// they keep one shape and stay short enough for an agent to read in one go.
const AGENT_PAGES = ['join-a-space', 'add-a-computer', 'connect-a-cloud', 'volume-s3', 'add-a-machine'];
const AGENT_PARTS = [
  /^## You run on your user's computer/m,
  /^## You run in the cloud/m,
  /^## Check it worked$/m,
  /^## Don't$/m,
  /^## What you get$/m,
];

test('the agent pages have the same parts in order and stay under 80 lines', () => {
  const meta = JSON.parse(page('agents/meta.json')) as { pages: string[] };
  assert.deepEqual(meta.pages, AGENT_PAGES);
  for (const name of AGENT_PAGES) {
    const text = page(`agents/${name}.mdx`);
    assert.ok(text.split('\n').length - 1 < 80, `${name} is 80 lines or more`);
    let at = -1;
    for (const part of AGENT_PARTS) {
      const found = text.search(part);
      assert.ok(found > at, `${name}: ${part} is missing or out of order`);
      at = found;
    }
    assert.match(text.split('## ')[0], /\n\n(?!---)\S/, `${name} opens with what it is`);
  }
});

test('the agent pages never ask for or show a secret', () => {
  for (const name of AGENT_PAGES) {
    const text = page(`agents/${name}.mdx`);
    assert.doesNotMatch(text, /AKIA[0-9A-Z]{12}|--secret|--token-stdin\b.*<token>/, name);
  }
  const s3 = page('agents/volume-s3.mdx');
  assert.match(s3, /create-access-key[^\n]*\| cua volume config set-keys/, 'keys are piped, never printed');
});

test('the prompts in the app link to pages that exist', () => {
  const core = readFileSync(path.join(repo, 'libs/cua/crates/cua-spaces-app-core/src/agent_prompts.rs'), 'utf8');
  for (const [, slug] of core.matchAll(/https:\/\/cua\.ai\/docs\/agents\/([a-z0-9-]+)/g)) {
    assert.ok(AGENT_PAGES.includes(slug), `a prompt links to /docs/agents/${slug}, which is not a page`);
  }
});

test('the human page the join doc points at exists', () => {
  assert.match(page('agents/join-a-space.mdx'), /https:\/\/cua\.ai\/docs\/spaces\/agents/);
  const { redirects } = JSON.parse(page('redirects.json')) as { redirects: Record<string, string> };
  assert.ok(page(`${redirects['spaces/agents']}.mdx`).includes('cua join'));
});

test('the skill docs served at cua.ai/join and /add are pinned', () => {
  // trycua/cloud vendors these exact texts (src/spaces-site/api/_lib/skills) and
  // checks its copy against the same hashes. A page edit fails here until the
  // pin is updated, which is the reminder to send the copy over.
  const pin = page('agents/skill-docs.sha256').trim().split('\n');
  assert.deepEqual(
    pin,
    SKILL_PAGES.map((p) => `${sha256(rendered(p))}  ${p}.md`),
    'run `pnpm exec tsx scripts/agent-skill-docs.ts --write <cloud>/src/spaces-site/api/_lib/skills`, copy the hashes into agents/skill-docs.sha256 and open the trycua/cloud PR'
  );
  for (const p of SKILL_PAGES) {
    const text = rendered(p);
    assert.ok(!/skip=|test=|<[A-Z][a-z]+ /.test(text), `${p} keeps MDX out of the served text`);
    assert.ok(!/Space name|member names/i.test(text), `${p} carries no user content`);
  }
});
