import { AUTH_PAGE, CALLBACK_HOST } from './config';
import { setApiKey, getApiKey } from './storage';
import { openInBrowser } from './util';

const c = {
  reset: '\x1b[0m',
  bold: '\x1b[1m',
  dim: '\x1b[2m',
  underline: '\x1b[4m',
  cyan: '\x1b[36m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
};

export async function loginViaBrowser(): Promise<string> {
  let resolveToken!: (v: string) => void;
  const tokenPromise = new Promise<string>((resolve) => {
    resolveToken = resolve;
  });

  // dynamic port (0) -> OS chooses available port
  const server = Bun.serve({
    hostname: CALLBACK_HOST,
    port: 0,
    fetch(req) {
      const u = new URL(req.url);
      if (u.pathname !== '/callback') return new Response('Not found', { status: 404 });
      const token = u.searchParams.get('token');
      if (!token) return new Response('Missing token', { status: 400 });
      resolveToken(token);
      queueMicrotask(() => server.stop());
      return new Response('CLI authorized. You can close this window.', {
        status: 200,
        headers: { 'content-type': 'text/plain' },
      });
    },
  });

  const callbackURL = `http://${CALLBACK_HOST}:${server.port}/callback`;
  const url = `${AUTH_PAGE}?callback_url=${encodeURIComponent(callbackURL)}`;
  console.log(`${c.cyan}${c.bold}Opening your default browser to authorize the CLI...${c.reset}`);
  console.log(`${c.dim}If the browser does not open automatically, copy/paste this URL:${c.reset}`);
  console.log(`${c.yellow}${c.underline}${url}${c.reset}`);
  await openInBrowser(url);

  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<string>((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error('Timed out waiting for authorization')),
      2 * 60 * 1000
    );
  });
  try {
    const result = await Promise.race([tokenPromise, timeout]);
    if (timeoutId) clearTimeout(timeoutId);
    return result;
  } finally {
    try {
      server.stop();
    } catch {}
  }
}

export async function ensureApiKeyInteractive(): Promise<string> {
  const existing = getApiKey();
  if (existing) return existing;
  const token = await loginViaBrowser();
  setApiKey(token);
  return token;
}
