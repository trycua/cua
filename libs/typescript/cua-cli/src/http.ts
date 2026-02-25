import { readFileSync } from 'node:fs';
import { API_BASE } from './config';

const { version: cliVersion } = JSON.parse(
  readFileSync(new URL('../../package.json', import.meta.url), 'utf-8'),
) as { version: string };

export const CUA_VERSION_HEADERS: Record<string, string> = {
  'X-Cua-Client-Version': `cli:${cliVersion}`,
};

export async function http(
  path: string,
  opts: { method?: string; token: string; body?: any }
): Promise<Response> {
  const url = `${API_BASE}${path}`;
  const headers: Record<string, string> = {
    Authorization: `Bearer ${opts.token}`,
    ...CUA_VERSION_HEADERS,
  };
  if (opts.body) headers['content-type'] = 'application/json';
  return fetch(url, {
    method: opts.method || 'GET',
    headers,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
}
