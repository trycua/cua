import type { Argv } from 'yargs';
import { ensureApiKeyInteractive, loginViaBrowser } from '../auth';
import { clearApiKey, setApiKey } from '../storage';
import { writeEnvFile } from '../util';

// Command handlers
const loginHandler = async (argv: Record<string, unknown>) => {
  if (argv['api-key']) {
    setApiKey(String(argv['api-key']));
    console.log('API key saved');
    return;
  }
  console.log('Opening browser for CLI auth...');
  const token = await loginViaBrowser();
  setApiKey(token);
  console.log('API key saved');
};

const envHandler = async (_argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const out = await writeEnvFile(process.cwd(), token);
  console.log(`Wrote ${out}`);
};

const logoutHandler = async (_argv: Record<string, unknown>) => {
  clearApiKey();
  console.log('Logged out');
};

export function registerAuthCommands(y: Argv) {
  // Grouped structure: cua auth <command> (register first to appear first in help)
  y.command(
    'auth',
    'Authenticate with Cua (login, logout, or export credentials)',
    (y) => {
      return y
        .command(
          'login',
          'Authenticate via browser or API key and save credentials locally',
          (y) =>
            y.option('api-key', {
              type: 'string',
              describe: 'API key to store directly',
            }),
          loginHandler
        )
        .command(
          'env',
          'Export your API key to a .env file in the current directory',
          () => {},
          envHandler
        )
        .command(
          'logout',
          'Clear stored API credentials from this machine',
          () => {},
          logoutHandler
        )
        .demandCommand(1, 'You must provide an auth command');
    },
    () => {}
  );

  // Flat structure (backwards compatible, hidden from help)
  y.command({
    command: 'login',
    describe: false as any, // Hide from help
    builder: (y: Argv) =>
      y.option('api-key', {
        type: 'string',
        describe: 'API key to store directly',
      }),
    handler: loginHandler,
  } as any)
    .command({
      command: 'env',
      describe: false as any, // Hide from help
      builder: (y: Argv) => y,
      handler: envHandler,
    } as any)
    .command({
      command: 'logout',
      describe: false as any, // Hide from help
      builder: (y: Argv) => y,
      handler: logoutHandler,
    } as any);

  return y;
}
