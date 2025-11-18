import type { Argv } from 'yargs';
import { ensureApiKeyInteractive } from '../auth';
import { WEBSITE_URL } from '../config';
import { http } from '../http';
import { clearApiKey } from '../storage';
import type { SandboxItem } from '../util';
import { openInBrowser, printSandboxList } from '../util';

// Command handlers
const listHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const res = await http('/v1/vms', { token });
  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (!res.ok) {
    console.error(`Request failed: ${res.status}`);
    process.exit(1);
  }
  const data = (await res.json()) as SandboxItem[];
  printSandboxList(data, Boolean(argv['show-passwords']));
};

const createHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const { os, size, region } = argv as {
    os: string;
    size: string;
    region: string;
  };

  const res = await http('/v1/vms', {
    token,
    method: 'POST',
    body: { os, configuration: size, region },
  });

  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }

  if (res.status === 400) {
    console.error('Invalid request or unsupported configuration');
    process.exit(1);
  }

  if (res.status === 500) {
    console.error('Internal server error');
    process.exit(1);
  }

  if (res.status === 200) {
    const data = (await res.json()) as {
      status: string;
      name: string;
      password: string;
      host: string;
    };
    console.log(`Sandbox created and ready: ${data.name}`);
    console.log(`Password: ${data.password}`);
    console.log(`Host: ${data.host}`);
    return;
  }

  if (res.status === 202) {
    const data = (await res.json()) as {
      status: string;
      name: string;
      job_id: string;
    };
    console.log(`Sandbox provisioning started: ${data.name}`);
    console.log(`Job ID: ${data.job_id}`);
    console.log("Use 'cua list' to monitor provisioning progress");
    return;
  }

  console.error(`Unexpected status: ${res.status}`);
  process.exit(1);
};

const deleteHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const res = await http(`/v1/vms/${encodeURIComponent(name)}`, {
    token,
    method: 'DELETE',
  });

  if (res.status === 202) {
    const body = (await res.json().catch(() => ({}))) as {
      status?: string;
    };
    console.log(
      `Sandbox deletion initiated: ${body.status ?? 'deleting'}`
    );
    return;
  }

  if (res.status === 404) {
    console.error('Sandbox not found or not owned by you');
    process.exit(1);
  }

  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }

  console.error(`Unexpected status: ${res.status}`);
  process.exit(1);
};

const startHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const res = await http(`/v1/vms/${encodeURIComponent(name)}/start`, {
    token,
    method: 'POST',
  });
  if (res.status === 204) {
    console.log('Start accepted');
    return;
  }
  if (res.status === 404) {
    console.error('Sandbox not found');
    process.exit(1);
  }
  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  console.error(`Unexpected status: ${res.status}`);
  process.exit(1);
};

const stopHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const res = await http(`/v1/vms/${encodeURIComponent(name)}/stop`, {
    token,
    method: 'POST',
  });
  if (res.status === 202) {
    const body = (await res.json().catch(() => ({}))) as {
      status?: string;
    };
    console.log(body.status ?? 'stopping');
    return;
  }
  if (res.status === 404) {
    console.error('Sandbox not found');
    process.exit(1);
  }
  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  console.error(`Unexpected status: ${res.status}`);
  process.exit(1);
};

const restartHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const res = await http(`/v1/vms/${encodeURIComponent(name)}/restart`, {
    token,
    method: 'POST',
  });
  if (res.status === 202) {
    const body = (await res.json().catch(() => ({}))) as {
      status?: string;
    };
    console.log(body.status ?? 'restarting');
    return;
  }
  if (res.status === 404) {
    console.error('Sandbox not found');
    process.exit(1);
  }
  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  console.error(`Unexpected status: ${res.status}`);
  process.exit(1);
};

const openHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const listRes = await http('/v1/vms', { token });
  if (listRes.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (!listRes.ok) {
    console.error(`Request failed: ${listRes.status}`);
    process.exit(1);
  }
  const sandboxes = (await listRes.json()) as SandboxItem[];
  const sandbox = sandboxes.find((s) => s.name === name);
  if (!sandbox) {
    console.error('Sandbox not found');
    process.exit(1);
  }
  const host =
    sandbox.host && sandbox.host.length
      ? sandbox.host
      : `${sandbox.name}.containers.cloud.trycua.com`;
  const url = `https://${host}/vnc.html?autoconnect=true&password=${encodeURIComponent(sandbox.password)}`;
  console.log(`Opening NoVNC: ${url}`);
  await openInBrowser(url);
};

// Register commands in both flat and grouped structures
export function registerSandboxCommands(y: Argv) {
  // Flat structure (backwards compatible, hidden from help)
  y.command({
    command: ['list', 'ls', 'ps'],
    describe: 'List sandboxes',
    hidden: true,
    builder: (y: Argv) =>
      y.option('show-passwords', {
        type: 'boolean',
        default: false,
        describe: 'Show sandbox passwords in output',
      }),
    handler: listHandler,
  } as any)
    .command({
      command: 'create',
      describe: 'Create a new sandbox',
      hidden: true,
      builder: (y: Argv) =>
        y
          .option('os', {
            type: 'string',
            choices: ['linux', 'windows', 'macos'],
            demandOption: true,
            describe: 'Operating system',
          })
          .option('size', {
            type: 'string',
            choices: ['small', 'medium', 'large'],
            demandOption: true,
            describe: 'Sandbox size',
          })
          .option('region', {
            type: 'string',
            choices: [
              'north-america',
              'europe',
              'asia-pacific',
              'south-america',
            ],
            demandOption: true,
            describe: 'Sandbox region',
          }),
      handler: createHandler,
    } as any)
    .command({
      command: 'delete <name>',
      describe: 'Delete a sandbox',
      hidden: true,
      builder: (y: Argv) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: deleteHandler,
    } as any)
    .command({
      command: 'start <name>',
      describe: 'Start a sandbox',
      hidden: true,
      builder: (y: Argv) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: startHandler,
    } as any)
    .command({
      command: 'stop <name>',
      describe: 'Stop a sandbox',
      hidden: true,
      builder: (y: Argv) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: stopHandler,
    } as any)
    .command({
      command: 'restart <name>',
      describe: 'Restart a sandbox',
      hidden: true,
      builder: (y: Argv) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: restartHandler,
    } as any)
    .command({
      command: ['vnc <name>', 'open <name>'],
      describe: 'Open VNC desktop for a sandbox',
      hidden: true,
      builder: (y: Argv) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: openHandler,
    } as any);

  // Grouped structure: cua sandbox <command> or cua sb <command>
  y.command(
    ['sandbox', 'sb'],
    'Manage sandboxes',
    (y) => {
      return y
        .command(
          ['list', 'ls', 'ps'],
          'List sandboxes',
          (y) =>
            y.option('show-passwords', {
              type: 'boolean',
              default: false,
              describe: 'Show sandbox passwords in output',
            }),
          listHandler
        )
        .command(
          'create',
          'Create a new sandbox',
          (y) =>
            y
              .option('os', {
                type: 'string',
                choices: ['linux', 'windows', 'macos'],
                demandOption: true,
                describe: 'Operating system',
              })
              .option('size', {
                type: 'string',
                choices: ['small', 'medium', 'large'],
                demandOption: true,
                describe: 'Sandbox size',
              })
              .option('region', {
                type: 'string',
                choices: [
                  'north-america',
                  'europe',
                  'asia-pacific',
                  'south-america',
                ],
                demandOption: true,
                describe: 'Sandbox region',
              }),
          createHandler
        )
        .command(
          'delete <name>',
          'Delete a sandbox',
          (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          deleteHandler
        )
        .command(
          'start <name>',
          'Start a sandbox',
          (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          startHandler
        )
        .command(
          'stop <name>',
          'Stop a sandbox',
          (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          stopHandler
        )
        .command(
          'restart <name>',
          'Restart a sandbox',
          (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          restartHandler
        )
        .command(
          ['vnc <name>', 'open <name>'],
          'Open VNC desktop for a sandbox',
          (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          openHandler
        )
        .demandCommand(1, 'You must provide a sandbox command');
    },
    () => {}
  );

  return y;
}
