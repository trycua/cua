import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';
import { registerAuthCommands } from './commands/auth';
import { registerVmCommands } from './commands/vm';

export async function runCli() {
  let argv = yargs(hideBin(process.argv)).scriptName('cua');
  argv = registerAuthCommands(argv);
  argv = registerVmCommands(argv);
  await argv.demandCommand(1).strict().help().parseAsync();
}
