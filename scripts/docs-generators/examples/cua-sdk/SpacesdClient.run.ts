// docs: test="docs" prelude="spacesd,spacesd-vars"
import { SpacesdCommand, embedded } from '@trycua/cua';

const guest = await embedded().spacesd(URL, TOKEN); // or: await sandbox.spacesd(undefined)
const out = await guest.run(SpacesdCommand.create({ program: 'echo', args: ['hello'] }));
console.log(out.exit.success, new TextDecoder().decode(out.stdout));
