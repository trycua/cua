// docs: test="docs"
import { connect, embedded } from '@trycua/cua';

const cua = embedded({ fleetFromSession: true }); // SDK runtime in this process
const viaDaemon = connect(); // client of a running `cua daemon`, connected on first use
console.log(cua.mode(), typeof viaDaemon.sandboxes);
