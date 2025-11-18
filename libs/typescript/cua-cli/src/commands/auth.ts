import { setApiKey, clearApiKey } from '../storage';
import { ensureApiKeyInteractive, loginViaBrowser } from '../auth';
import { writeEnvFile } from '../util';
import type { Argv } from 'yargs';

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
  // Flat structure (backwards compatible)
  y.command(
    'login',
    'Open browser to authorize and store API key',
    (y) =>
      y.option('api-key', {
        type: 'string',
        describe: 'API key to store directly',
      }),
    loginHandler
  )
    .command(
      'env',
      'Create or update .env with CUA_API_KEY (login if needed)',
      () => {},
      envHandler
    )
    .command('logout', 'Remove stored API key', () => {}, logoutHandler);

  // Grouped structure: cua auth <command>
  y.command(
    'auth',
    'Manage authentication',
    (y) => {
      return y
        .command(
          'login',
          'Open browser to authorize and store API key',
          (y) =>
            y.option('api-key', {
              type: 'string',
              describe: 'API key to store directly',
            }),
          loginHandler
        )
        .command(
          'env',
          'Create or update .env with CUA_API_KEY (login if needed)',
          () => {},
          envHandler
        )
        .command('logout', 'Remove stored API key', () => {}, logoutHandler)
        .demandCommand(1, 'You must provide an auth command');
    },
    () => {}
  );

  return y;
}
