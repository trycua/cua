export async function writeEnvFile(cwd: string, key: string) {
  const path = `${cwd}/.env`;
  let content = '';
  try {
    content = await Bun.file(path).text();
  } catch {}
  const lines = content.split(/\r?\n/).filter(Boolean);
  const idx = lines.findIndex((l) => l.startsWith('CUA_API_KEY='));
  if (idx >= 0) lines[idx] = `CUA_API_KEY=${key}`;
  else lines.push(`CUA_API_KEY=${key}`);
  await Bun.write(path, lines.join('\n') + '\n');
  return path;
}

export type SandboxStatus =
  | 'pending'
  | 'running'
  | 'stopped'
  | 'terminated'
  | 'failed';
export type SandboxItem = {
  name: string;
  password: string;
  status: SandboxStatus;
  host?: string;
};

export function printSandboxList(items: SandboxItem[], showPasswords: boolean = false) {
  const headers = showPasswords
    ? ['NAME', 'STATUS', 'PASSWORD', 'HOST']
    : ['NAME', 'STATUS', 'HOST'];

  const rows: string[][] = [
    headers,
    ...items.map((v) =>
      showPasswords
        ? [v.name, String(v.status), v.password, v.host || '']
        : [v.name, String(v.status), v.host || '']
    ),
  ];

  const numCols = headers.length;
  const widths: number[] = new Array(numCols).fill(0);

  for (const r of rows)
    for (let i = 0; i < numCols; i++)
      widths[i] = Math.max(widths[i] ?? 0, (r[i] ?? '').length);

  for (const r of rows)
    console.log(r.map((c, i) => (c ?? '').padEnd(widths[i] ?? 0)).join('  '));

  if (items.length === 0) console.log('No sandboxes found');
}

export async function openInBrowser(url: string) {
  const platform = process.platform;
  let cmd: string;
  let args: string[] = [];
  if (platform === 'darwin') {
    cmd = 'open';
    args = [url];
  } else if (platform === 'win32') {
    cmd = 'cmd';
    args = ['/c', 'start', '', url];
  } else {
    cmd = 'xdg-open';
    args = [url];
  }
  try {
    await Bun.spawn({ cmd: [cmd, ...args] }).exited;
  } catch {
    console.error(`Failed to open browser. Please visit: ${url}`);
  }
}
