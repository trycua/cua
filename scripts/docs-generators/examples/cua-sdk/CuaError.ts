// docs: test="docs" prelude="spacesd,spacesd-vars"
import { CuaError, embedded } from '@trycua/cua';

const sb = await embedded().sandboxes().connectUrl(URL, 'wrong-token', undefined);
try {
  await sb.spacesd(5000);
} catch (e) {
  if (!CuaError.Unauthenticated.instanceOf(e)) throw e;
  console.log('rejected:', e.message);
}
