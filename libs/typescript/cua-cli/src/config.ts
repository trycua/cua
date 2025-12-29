export const WEBSITE_URL =
  Bun.env.CUA_WEBSITE_URL?.replace(/\/$/, '') || 'https://cua.ai';
export const API_BASE =
  Bun.env.CUA_API_BASE?.replace(/\/$/, '') || 'https://api.cua.ai';
export const AUTH_PAGE = `${WEBSITE_URL}/cli-auth`;
export const CALLBACK_HOST = '127.0.0.1';

export function getConfigDir(): string {
  const home = Bun.env.HOME || Bun.env.USERPROFILE || '.';
  const dir = `${home}/.cua`;
  try {
    Bun.spawnSync(['mkdir', '-p', dir]);
  } catch {}
  return dir;
}

export function getDbPath(): string {
  return `${getConfigDir()}/cli.sqlite`;
}
