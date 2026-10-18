// The package is used from the repository, not a registry. Check both
// documented ways in: copying the built single-file ESM, and vendoring src/.
import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { expect, it } from 'vitest';

const pkg = fileURLToPath(new URL('..', import.meta.url));
const bin = (name: string) => join(pkg, 'node_modules/.bin', name);
const tsc = (dir: string, extra: string[] = []) =>
  execFileSync(
    bin('tsc'),
    [
      '--noEmit',
      '--strict',
      '--target',
      'es2022',
      '--lib',
      'es2022,dom',
      ...extra,
      join(dir, 'app.ts'),
    ],
    { cwd: dir, stdio: 'pipe' }
  );

const usage = (from: string) => `
import { planMove, planSpec, specForStyle, MotionPlayer, type Trajectory } from '${from}';
const t: Trajectory = planMove({ style: 'comet_swoop' }, { from: { x: 0, y: 0 }, to: { x: 600, y: 300 } });
const s = planSpec(specForStyle('spring_settle')!, { from: { x: 0, y: 0 }, to: { x: 600, y: 300 } });
export const ok: boolean = t.arrivalT > 0 && s.duration() > 0 && typeof MotionPlayer === 'function';
`;

it('the built single-file ESM can be copied into a project', async () => {
  execFileSync(bin('tsdown'), { cwd: pkg, stdio: 'pipe' });
  const dir = mkdtempSync(join(tmpdir(), 'cua-cursor-motion-'));
  cpSync(join(pkg, 'dist/index.js'), join(dir, 'cua-cursor-motion.js'));
  cpSync(join(pkg, 'dist/index.d.ts'), join(dir, 'cua-cursor-motion.d.ts'));
  writeFileSync(join(dir, 'app.ts'), usage('./cua-cursor-motion.js'));
  tsc(dir, ['--module', 'nodenext', '--moduleResolution', 'nodenext']);
  const mod = await import(pathToFileURL(join(dir, 'cua-cursor-motion.js')).href);
  const traj = mod.planMove({ style: 'magnetic' }, { from: { x: 0, y: 0 }, to: { x: 400, y: 0 } });
  expect(traj.end()).toMatchObject({ x: 400, y: 0 });
}, 60_000);

it('src/ can be vendored into a bundler project', () => {
  const dir = mkdtempSync(join(tmpdir(), 'cua-cursor-motion-src-'));
  cpSync(join(pkg, 'src'), join(dir, 'cua-cursor-motion'), { recursive: true });
  writeFileSync(join(dir, 'app.ts'), usage('./cua-cursor-motion'));
  tsc(dir, ['--module', 'esnext', '--moduleResolution', 'bundler']);
}, 60_000);
