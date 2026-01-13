import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';
import { registerAuthCommands } from './commands/auth';
import { registerSandboxCommands } from './commands/sandbox';
import { http } from './http';
import { getApiKey } from './storage';
import type { SandboxItem } from './util';

// Fetch sandbox names for shell completion
async function getSandboxNames(): Promise<string[]> {
  try {
    const token = getApiKey();
    if (!token) return [];
    const res = await http('/v1/vms', { token });
    if (!res.ok) return [];
    const sandboxes = (await res.json()) as SandboxItem[];
    return sandboxes.map((s) => s.name);
  } catch {
    return [];
  }
}

export async function runCli() {
  let argv = yargs(hideBin(process.argv))
    .scriptName('cua')
    .usage('Usage: $0 <command> [options]')
    .epilogue(
      'Commands:\n' +
        '  cua auth <command>     Authenticate and manage credentials\n' +
        '    login              Login via browser or with API key\n' +
        '    env                Export API key to .env file\n' +
        '    logout             Clear stored credentials\n' +
        '\n' +
        '  cua sb <command>       Create and manage cloud sandboxes\n' +
        '    list               View all your sandboxes\n' +
        '    create             Provision a new sandbox\n' +
        '    get                Get detailed info about a sandbox\n' +
        '    delete             Permanently delete a sandbox\n' +
        '    start              Start a stopped sandbox\n' +
        '    stop               Stop a sandbox (preserves disk)\n' +
        '    restart            Restart/reboot a sandbox\n' +
        '    suspend            Suspend a sandbox (preserves memory)\n' +
        '    vnc                Open remote desktop\n' +
        '\n' +
        '  cua completion         Generate shell completion script\n' +
        '\n' +
        'Documentation: https://docs.cua.ai/libraries/cua-cli/commands'
    )
    .completion('completion', 'Generate shell completion script', async function (current, argv) {
      // Provide dynamic completions for sandbox names
      const commands = ['auth', 'sandbox', 'sb', 'completion'];
      const authCommands = ['login', 'env', 'logout'];
      const sbCommands = ['list', 'ls', 'ps', 'create', 'delete', 'start', 'stop', 'restart', 'suspend', 'vnc', 'open', 'get'];

      // If completing a sandbox name argument
      const args = process.argv.slice(2);
      const needsSandboxName = ['delete', 'start', 'stop', 'restart', 'suspend', 'vnc', 'open', 'get'];

      // Check if we're completing after a command that needs a sandbox name
      if (args.length >= 2) {
        const lastCmd = args[args.length - 2];
        if (needsSandboxName.includes(lastCmd) ||
            (args.length >= 3 && args[0] === 'sb' && needsSandboxName.includes(args[1]))) {
          return await getSandboxNames();
        }
      }

      // Top-level completion
      if (args.length <= 1) {
        return commands;
      }

      // Sub-command completion
      if (args[0] === 'auth') {
        return authCommands;
      }
      if (args[0] === 'sandbox' || args[0] === 'sb') {
        return sbCommands;
      }

      return [];
    });
  // Override the default --version behavior
  argv = argv.version(false).option('version', {
    alias: 'v',
    describe: 'Show Cua CLI version',
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
