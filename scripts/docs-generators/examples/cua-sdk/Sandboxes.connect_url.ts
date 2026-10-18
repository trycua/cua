// docs: test="docs" prelude="spacesd,spacesd-vars"
import { SpacesdCommand, embedded } from '@trycua/cua';

const cua = embedded();
const sb = await cua.sandboxes().connectUrl(URL, TOKEN, 'dev'); // your spacesd's address and token
const guest = await sb.spacesd(undefined);
const out = await guest.run(SpacesdCommand.create({ program: 'echo', args: ['hi'] }));
console.log(new TextDecoder().decode(out.stdout));
