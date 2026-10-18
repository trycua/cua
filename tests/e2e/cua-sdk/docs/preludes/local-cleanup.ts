// Hidden docs prelude `local-cleanup`: delete the local sandboxes this block
// created, and only those, before Node exits. `Sandboxes.prototype.create` is
// wrapped to record every sandbox the block makes; the exit hook deletes
// exactly those through the SDK. It never lists and deletes, so a reader's
// (or another run's) sandboxes are never touched, whatever CUA_HOME says.
import {
  embedded as __cuaDocsCleanupCua,
  Sandboxes as __cuaDocsSandboxes,
} from '@trycua/cua';

const __cuaDocsCreated: string[] = [];
const __cuaDocsCreate = __cuaDocsSandboxes.prototype.create;
__cuaDocsSandboxes.prototype.create = async function (
  this: __cuaDocsSandboxes,
  ...args: Parameters<typeof __cuaDocsCreate>
) {
  const sandbox = await __cuaDocsCreate.apply(this, args);
  __cuaDocsCreated.push(sandbox.name());
  return sandbox;
};

let __cuaDocsCleaned = false;
process.on('beforeExit', async () => {
  if (__cuaDocsCleaned || __cuaDocsCreated.length === 0) return;
  __cuaDocsCleaned = true;
  const sandboxes = __cuaDocsCleanupCua().sandboxes();
  for (const name of new Set(__cuaDocsCreated)) {
    try {
      await sandboxes.delete_(name);
    } catch (error) {
      if (!String(error).toLowerCase().includes('not found')) {
        console.log(`docs cleanup: ${name}: ${error}`);
      }
    }
  }
});
