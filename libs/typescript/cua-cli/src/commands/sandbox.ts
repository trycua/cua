import type { Argv } from 'yargs';
import { ensureApiKeyInteractive } from '../auth';
import { http } from '../http';
import { clearApiKey } from '../storage';
import type { SandboxItem } from '../util';
import { openInBrowser, printSandboxList } from '../util';

// Helper function to fetch sandbox details with computer-server probes
async function fetchSandboxDetails(
  name: string,
  token: string,
  options: {
    showPasswords?: boolean;
    showVncUrl?: boolean;
    probeComputerServer?: boolean;
  } = {}
) {
  // Fetch sandbox list
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

  // Build result object
  const result: any = {
    name: sandbox.name,
    status: sandbox.status,
    host: sandbox.host || `${sandbox.name}.sandbox.cua.ai`,
  };

  if (options.showPasswords) {
    result.password = sandbox.password;
  }

  // Compute VNC URL if requested
  if (options.showVncUrl) {
    const host = sandbox.host || `${sandbox.name}.sandbox.cua.ai`;
    result.vnc_url = `https://${host}/vnc.html?autoconnect=true&password=${encodeURIComponent(sandbox.password)}&show_dot=true`;
  }

  // Probe computer-server if requested and sandbox is running
  if (
    options.probeComputerServer &&
    sandbox.status === 'running' &&
    sandbox.host
  ) {
    let statusProbeSuccess = false;
    let versionProbeSuccess = false;

    try {
      // Probe OS type
      const statusUrl = `https://${sandbox.host}:8443/status`;
      const statusController = new AbortController();
      const statusTimeout = setTimeout(() => statusController.abort(), 3000);

      try {
        const statusRes = await fetch(statusUrl, {
          signal: statusController.signal,
        });
        clearTimeout(statusTimeout);

        if (statusRes.ok) {
          const statusData = (await statusRes.json()) as {
            status: string;
            os_type: string;
            features?: string[];
          };
          result.os_type = statusData.os_type;
          statusProbeSuccess = true;
        }
      } catch (err) {
        // Timeout or connection error - skip
      }

      // Probe computer-server version
      const versionUrl = `https://${sandbox.host}:8443/cmd`;
      const versionController = new AbortController();
      const versionTimeout = setTimeout(() => versionController.abort(), 3000);

      try {
        const versionRes = await fetch(versionUrl, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'X-Container-Name': sandbox.name,
            'X-API-Key': token,
          },
          body: JSON.stringify({
            command: 'version',
            params: {},
          }),
          signal: versionController.signal,
        });
        clearTimeout(versionTimeout);

        if (versionRes.ok) {
          const versionDataRaw = await versionRes.text();
          if (versionDataRaw.startsWith('data: ')) {
            const jsonStr = versionDataRaw.slice(6);
            const versionData = JSON.parse(jsonStr) as {
              success: boolean;
              protocol: number;
              package: string;
            };
            if (versionData.package) {
              result.computer_server_version = versionData.package;
              versionProbeSuccess = true;
            }
          }
        }
      } catch (err) {
        // Timeout or connection error - skip
      }
    } catch (err) {
      // General error - skip probing
    }

    // Set computer server status based on probe results
    if (statusProbeSuccess && versionProbeSuccess) {
      result.computer_server_status = 'healthy';
    }
  }

  return result;
}

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
    console.log(`Sandbox deletion initiated: ${body.status ?? 'deleting'}`);
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

const suspendHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const res = await http(`/v1/vms/${encodeURIComponent(name)}/suspend`, {
    token,
    method: 'POST',
  });
  if (res.status === 202) {
    const body = (await res.json().catch(() => ({}))) as {
      status?: string;
    };
    console.log(body.status ?? 'suspending');
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
  if (res.status === 400 || res.status === 500) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    console.error(
      body.error ??
        "Suspend not supported for this VM. Use 'cua sb stop' instead."
    );
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
      : `${sandbox.name}.sandbox.cua.ai`;
  const url = `https://${host}/vnc.html?autoconnect=true&password=${encodeURIComponent(sandbox.password)}&show_dot=true`;
  console.log(`Opening NoVNC: ${url}`);
  await openInBrowser(url);
};

const getHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String((argv as any).name);
  const showPasswords = Boolean(argv['show-passwords']);
  const showVncUrl = Boolean(argv['show-vnc-url']);
  const json = Boolean(argv.json);

  const details = await fetchSandboxDetails(name, token, {
    showPasswords,
    showVncUrl,
    probeComputerServer: true,
  });

  if (json) {
    console.log(JSON.stringify(details, null, 2));
  } else {
    // Pretty print the details
    console.log(`Name: ${details.name}`);
    console.log(`Status: ${details.status}`);
    console.log(`Host: ${details.host}`);

    if (showPasswords) {
      console.log(`Password: ${details.password}`);
    }

    if (details.os_type) {
      console.log(`OS Type: ${details.os_type}`);
    }

    if (details.computer_server_version) {
      console.log(
        `Computer Server Version: ${details.computer_server_version}`
      );
    }

    if (details.computer_server_status) {
      console.log(`Computer Server Status: ${details.computer_server_status}`);
    }

    if (showVncUrl) {
      console.log(`VNC URL: ${details.vnc_url}`);
    }
  }
};

// Register commands in both flat and grouped structures
export function registerSandboxCommands(y: Argv) {
  // Grouped structure: cua sandbox <command> or cua sb <command> (register first to appear first in help)
  y.command(
    ['sandbox', 'sb'],
    'Create and manage cloud sandboxes (Linux, Windows, or macOS)',
    (y) => {
      return y
        .command(
          ['list', 'ls', 'ps'],
          'List all your sandboxes with status and connection details',
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
          'Provision a new cloud sandbox in your chosen OS, size, and region',
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
          'Permanently delete a sandbox and all its data',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          deleteHandler
        )
        .command(
          'start <name>',
          'Start a stopped sandbox',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          startHandler
        )
        .command(
          'stop <name>',
          'Stop a running sandbox (data is preserved)',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          stopHandler
        )
        .command(
          'restart <name>',
          'Restart a sandbox (reboot the system)',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          restartHandler
        )
        .command(
          'suspend <name>',
          'Suspend a sandbox, preserving memory state (use start to resume)',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          suspendHandler
        )
        .command(
          ['vnc <name>', 'open <name>'],
          'Open remote desktop (VNC) connection in your browser',
          (y) =>
            y.positional('name', { type: 'string', describe: 'Sandbox name' }),
          openHandler
        )
        .command(
          'get <name>',
          'Get detailed information about a specific sandbox',
          (y) =>
            y
              .positional('name', { type: 'string', describe: 'Sandbox name' })
              .option('json', {
                type: 'boolean',
                default: false,
                describe: 'Output in JSON format',
              })
              .option('show-passwords', {
                type: 'boolean',
                default: false,
                describe: 'Include password in output',
              })
              .option('show-vnc-url', {
                type: 'boolean',
                default: false,
                describe: 'Include computed NoVNC URL in output',
              }),
          getHandler
        )
        .demandCommand(1, 'You must provide a sandbox command');
    },
    () => {}
  );

  // Flat structure (backwards compatible, hidden from help)
  y.command({
    command: ['list', 'ls', 'ps'],
    describe: false as any, // Hide from help
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
      describe: false as any, // Hide from help
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
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: deleteHandler,
    } as any)
    .command({
      command: 'start <name>',
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: startHandler,
    } as any)
    .command({
      command: 'stop <name>',
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: stopHandler,
    } as any)
    .command({
      command: 'restart <name>',
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: restartHandler,
    } as any)
    .command({
      command: 'suspend <name>',
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: suspendHandler,
    } as any)
    .command({
      command: ['vnc <name>', 'open <name>'],
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      handler: openHandler,
    } as any)
    .command({
      command: 'get <name>',
      describe: false as any, // Hide from help
      builder: (y: Argv) =>
        y
          .positional('name', { type: 'string', describe: 'Sandbox name' })
          .option('json', {
            type: 'boolean',
            default: false,
            describe: 'Output in JSON format',
          })
          .option('show-passwords', {
            type: 'boolean',
            default: false,
            describe: 'Include password in output',
          })
          .option('show-vnc-url', {
            type: 'boolean',
            default: false,
            describe: 'Include computed NoVNC URL in output',
          }),
      handler: getHandler,
    } as any);

  return y;
}
