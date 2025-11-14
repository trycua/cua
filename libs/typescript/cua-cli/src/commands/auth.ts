import { setApiKey, clearApiKey } from '../storage';
import { ensureApiKeyInteractive, loginViaBrowser } from '../auth';
import { writeEnvFile } from '../util';
import type { Argv } from 'yargs';

export function registerAuthCommands(y: Argv) {
  return y
    .command(
      'login',
      'Open browser to authorize and store API key',
      (y) =>
        y.option('api-key', {
          type: 'string',
          describe: 'API key to store directly',
        }),
      async (argv: Record<string, unknown>) => {
        if (argv['api-key']) {
          setApiKey(String(argv['api-key']));
          console.log('API key saved');
          return;
        }
        console.log('Opening browser for CLI auth...');
        const token = await loginViaBrowser();
        setApiKey(token);
        console.log('API key saved');
      }
    )
    .command(
      'env',
      'Create or update .env with CUA_API_KEY (login if needed)',
      () => {},
      async (_argv: Record<string, unknown>) => {
        const token = await ensureApiKeyInteractive();
        const out = await writeEnvFile(process.cwd(), token);
        console.log(`Wrote ${out}`);
      }
    )
    .command(
      'logout',
      'Remove stored API key',
      () => {},
      async (_argv: Record<string, unknown>) => {
        clearApiKey();
        console.log('Logged out');
      }
    );
}
