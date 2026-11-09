// Hidden docs prelude `spacesd-3211`: the page's "Start a spacesd to try it"
// step (canonical Linux image, spacesd on 127.0.0.1:3211, CUA_ENV_TOKEN set).
// Container lane only; the container is removed when the block exits.
import { execFileSync as __cuaDocsExec } from 'node:child_process';
import { randomBytes as __cuaDocsRandom } from 'node:crypto';

process.env.CUA_ENV_TOKEN = __cuaDocsRandom(24).toString('hex');
const __cuaDocsBox = `cua-e2e-${process.env.CUA_E2E_RUN ?? 'docs'}-envbox-ts`.slice(0, 63);
try { __cuaDocsExec('docker', ['rm', '-f', __cuaDocsBox], { stdio: 'ignore' }); } catch {}
__cuaDocsExec('docker', ['run', '-d', '--name', __cuaDocsBox, '--shm-size=512m', '--memory=2g',
  '-e', 'CUA_ENV_TOKEN', '-p', '127.0.0.1:3211:3211', 'ghcr.io/trycua/linux:24.04'], { stdio: 'ignore' });
process.on('exit', () => {
  try { __cuaDocsExec('docker', ['rm', '-f', __cuaDocsBox], { stdio: 'ignore' }); } catch {}
});
for (let i = 0; i < 120; i++) { // bounded: at most ~2 minutes
  try { await fetch('http://127.0.0.1:3211/'); break; } catch { await new Promise((r) => setTimeout(r, 1000)); }
}
