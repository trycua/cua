import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';
import { registerAuthCommands } from './commands/auth';
import { registerSandboxCommands } from './commands/sandbox';

export async function runCli() {
  let argv = yargs(hideBin(process.argv))
    .scriptName('cua')
    .usage('Usage: $0 <command> [options]')
    .epilogue(
      'Recommended Command Structure:\n' +
        '  cua auth <command>     Authenticate and manage credentials\n' +
        '    login              Login via browser or with API key\n' +
        '    env                Export API key to .env file\n' +
        '    logout             Clear stored credentials\n' +
        '\n' +
        '  cua sb <command>       Create and manage cloud sandboxes\n' +
        '    list               View all your sandboxes\n' +
        '    create             Provision a new sandbox\n' +
        '    start/stop         Control sandbox state\n' +
        '    vnc                Open remote desktop\n' +
        '\n' +
        'Documentation: https://docs.cua.ai/libraries/cua-cli/commands'
    );
  argv = registerAuthCommands(argv);
  argv = registerSandboxCommands(argv);
  await argv.demandCommand(1).strict().help().parseAsync();
}
