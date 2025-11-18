import type { Argv } from 'yargs';
import { ensureApiKeyInteractive } from '../auth';
import { WEBSITE_URL } from '../config';
import { http } from '../http';
import { clearApiKey } from '../storage';
import type { SandboxItem } from '../util';
import { openInBrowser, printSandboxList } from '../util';

export function registerSandboxCommands(y: Argv) {
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
      async (argv: Record<string, unknown>) => {
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
      }
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
          .option('configuration', {
            type: 'string',
            choices: ['small', 'medium', 'large'],
            demandOption: true,
            describe: 'Sandbox size configuration',
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
      async (argv: Record<string, unknown>) => {
        const token = await ensureApiKeyInteractive();
        const { os, configuration, region } = argv as {
          os: string;
          configuration: string;
          region: string;
        };

        const res = await http('/v1/vms', {
          token,
          method: 'POST',
          body: { os, configuration, region },
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
          // Sandbox ready immediately
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
          // Sandbox provisioning started
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
      }
    )
    .command(
      'delete <name>',
      'Delete a sandbox',
      (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      async (argv: Record<string, unknown>) => {
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
      }
    )
    .command(
      'start <name>',
      'Start a sandbox',
      (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      async (argv: Record<string, unknown>) => {
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
      }
    )
    .command(
      'stop <name>',
      'Stop a sandbox',
      (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      async (argv: Record<string, unknown>) => {
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
      }
    )
    .command(
      'restart <name>',
      'Restart a sandbox',
      (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      async (argv: Record<string, unknown>) => {
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
      }
    )
    .command(
      'open <name>',
      'Open NoVNC for a sandbox in your browser',
      (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
      async (argv: Record<string, unknown>) => {
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
      }
    );
  // .command(
  //   'chat <name>',
  //     'Open CUA playground for a sandbox',
  //     (y) => y.positional('name', { type: 'string', describe: 'Sandbox name' }),
  //     async (argv: Record<string, unknown>) => {
  //       const token = await ensureApiKeyInteractive();
  //       const name = String((argv as any).name);
  //       const listRes = await http('/v1/vms', { token });
  //       if (listRes.status === 401) {
  //         clearApiKey();
  //         console.error("Unauthorized. Try 'cua login' again.");
  //         process.exit(1);
  //       }
  //       if (!listRes.ok) {
  //         console.error(`Request failed: ${listRes.status}`);
  //         process.exit(1);
  //       }
  //       const sandboxes = (await listRes.json()) as SandboxItem[];
  //       const sandbox = sandboxes.find((s) => s.name === name);
  //       if (!sandbox) {
  //         console.error('Sandbox not found');
  //         process.exit(1);
  //       }
  //       const host =
  //         sandbox.host && sandbox.host.length
  //           ? sandbox.host
  //           : `${sandbox.name}.containers.cloud.trycua.com`;
  //       const base = WEBSITE_URL.replace(/\/$/, '');
  //       const url = `${base}/dashboard/playground?host=${encodeURIComponent(host)}&id=${encodeURIComponent(sandbox.name)}&name=${encodeURIComponent(sandbox.name)}&vnc_password=${encodeURIComponent(sandbox.password)}&fullscreen=true`;
  //       console.log(`Opening Playground: ${url}`);
  //       await openInBrowser(url);
  //     }
  //   );
}
