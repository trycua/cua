import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';
import { registerAuthCommands } from './commands/auth';
import { registerSandboxCommands } from './commands/sandbox';

export async function runCli() {
  let argv = yargs(hideBin(process.argv)).scriptName('cua');
  argv = registerAuthCommands(argv);
  argv = registerSandboxCommands(argv);
  await argv.demandCommand(1).strict().help().parseAsync();
}
