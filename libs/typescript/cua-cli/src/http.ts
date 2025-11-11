import { API_BASE } from './config';

export async function http(
  path: string,
  opts: { method?: string; token: string; body?: any }
): Promise<Response> {
  const url = `${API_BASE}${path}`;
  const headers: Record<string, string> = { Authorization: `Bearer ${opts.token}` };
  if (opts.body) headers['content-type'] = 'application/json';
  return fetch(url, {
    method: opts.method || 'GET',
    headers,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
}
