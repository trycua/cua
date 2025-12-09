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
        '    start              Start or resume a sandbox\n' +
        '    stop               Stop a sandbox (preserves disk)\n' +
        '    suspend            Suspend a sandbox (preserves memory)\n' +
        '    vnc                Open remote desktop\n' +
        '\n' +
        'Documentation: https://docs.cua.ai/libraries/cua-cli/commands'
    );
  // Override the default --version behavior
  argv = argv.version(false).option('version', {
    alias: 'v',
    describe: 'Show CUA CLI version',
    type: 'boolean',
    global: false,
  });
  argv = registerAuthCommands(argv);
  argv = registerSandboxCommands(argv);

  // Check for version flag before command validation
  const args = process.argv.slice(2);
  if (args.includes('--version') || args.includes('-v')) {
    try {
      const home = process.env.HOME || process.env.USERPROFILE || '';
      const path = `${home}/.cua/bin/.version`;
      const version = await Bun.file(path).text();
      const v = version.trim();
      if (v) {
        console.log(v);
      } else {
        console.log('unknown');
      }
    } catch {
      console.log('unknown');
    }
    process.exit(0);
  }

  await argv.demandCommand(1).strict().help().parseAsync();
}
