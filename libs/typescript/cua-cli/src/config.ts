export const API_BASE = "https://api.cua.ai";
export const AUTH_PAGE = "https://cua.ai/cli-auth";
export const CALLBACK_HOST = "127.0.0.1";

export function getConfigDir(): string {
  const home = Bun.env.HOME || Bun.env.USERPROFILE || ".";
  const dir = `${home}/.cua/config`;
  try {
    Bun.spawnSync(["mkdir", "-p", dir]);
  } catch {}
  return dir;
}

export function getDbPath(): string {
  return `${getConfigDir()}/cli.sqlite`;
}
