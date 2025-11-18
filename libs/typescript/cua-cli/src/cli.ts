import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';
import { registerAuthCommands } from './commands/auth';
import { registerSandboxCommands } from './commands/sandbox';

export async function runCli() {
  let argv = yargs(hideBin(process.argv))
    .scriptName('cua')
    .usage('Usage: $0 <command> [options]')
    .epilogue(
      'Grouped Commands (recommended):\n' +
        '  cua auth <command>   Manage authentication (login, env, logout)\n' +
        '  cua sb <command>     Manage sandboxes (list, create, start, stop, etc.)\n' +
        '\n' +
        'For more information: https://docs.cua.ai/libraries/cua-cli/commands'
    );
  argv = registerAuthCommands(argv);
  argv = registerSandboxCommands(argv);
  await argv.demandCommand(1).strict().help().parseAsync();
}
