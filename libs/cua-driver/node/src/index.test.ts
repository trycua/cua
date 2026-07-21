import { chmod, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import { EmbeddedCuaDriver, buildCodexLaunchArgs } from './index.js';

const drivers: EmbeddedCuaDriver[] = [];
const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(drivers.splice(0).map((driver) => driver.stop()));
  await Promise.all(
    temporaryDirectories
      .splice(0)
      .map((directory) => rm(directory, { recursive: true, force: true }))
  );
});

const makeFakeDriver = async (): Promise<{ binaryPath: string; journalPath: string }> => {
  const directory = await mkdtemp(join(tmpdir(), 'cua embedded test '));
  temporaryDirectories.push(directory);
  const binaryPath = join(directory, 'fake cua-driver');
  const journalPath = join(directory, 'argv.json');
  await writeFile(
    binaryPath,
    `#!/usr/bin/env node
const fs = require("node:fs");
const net = require("node:net");
const args = process.argv.slice(2);
fs.writeFileSync(${JSON.stringify(journalPath)}, JSON.stringify(args));
const socketIndex = args.indexOf("--socket");
const socketPath = args[socketIndex + 1];
const server = net.createServer((socket) => socket.end());
server.listen(socketPath);
const stop = () => server.close(() => {
  if (process.platform !== "win32") try { fs.unlinkSync(socketPath); } catch {}
  process.exit(0);
});
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
`
  );
  await chmod(binaryPath, 0o755);
  return { binaryPath, journalPath };
};

const makeDriver = (binaryPath: string, socketPath: string): EmbeddedCuaDriver => {
  const driver = new EmbeddedCuaDriver({
    binaryPath,
    hostBundleId: 'com.example.host',
    socketPath,
    startupTimeoutMs: 2_000,
    shutdownTimeoutMs: 1_000,
    stderr: 'ignore',
  });
  drivers.push(driver);
  return driver;
};

describe.skipIf(process.platform === 'win32')('embedded cua-driver lifecycle', () => {
  it('starts a direct private daemon and returns its MCP configuration', async () => {
    const { binaryPath, journalPath } = await makeFakeDriver();
    const socketPath = join(tmpdir(), `cua driver ${process.pid}.sock`);
    const driver = makeDriver(binaryPath, socketPath);

    const [first, second] = await Promise.all([driver.start(), driver.start()]);

    expect(second).toEqual(first);
    expect(first.socketPath).toBe(socketPath);
    expect(first.mcp).toEqual({
      command: binaryPath,
      args: ['mcp', '--embedded', '--socket', socketPath],
      env: {
        CUA_DRIVER_EMBEDDED: '1',
        CUA_DRIVER_HOST_BUNDLE_ID: 'com.example.host',
      },
    });
    expect(JSON.parse(await readFile(journalPath, 'utf8'))).toEqual([
      'serve',
      '--embedded',
      '--socket',
      socketPath,
      '--host-bundle-id',
      'com.example.host',
    ]);
    expect((await stat(socketPath)).mode & 0o777).toBe(0o600);

    await driver.stop();
    await expect(stat(socketPath)).rejects.toThrow();
  });

  it('keeps its default Unix socket below the sockaddr length limit', async () => {
    const { binaryPath } = await makeFakeDriver();
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: 'com.example.host',
      startupTimeoutMs: 2_000,
      stderr: 'ignore',
    });
    drivers.push(driver);

    const connection = await driver.start();
    expect(Buffer.byteLength(connection.socketPath)).toBeLessThan(104);
  });

  it('restarts with a new child and cleans up on repeated stop', async () => {
    const { binaryPath } = await makeFakeDriver();
    const socketPath = join(tmpdir(), `cua-driver-restart-${process.pid}.sock`);
    const driver = makeDriver(binaryPath, socketPath);
    const first = await driver.start();
    const second = await driver.restart();

    expect(second.pid).not.toBe(first.pid);
    await driver.stop();
    await driver.stop();
    await expect(stat(socketPath)).rejects.toThrow();
  });

  it('recovers after the daemon exits unexpectedly', async () => {
    const { binaryPath } = await makeFakeDriver();
    const socketPath = join(tmpdir(), `cua-driver-crash-${process.pid}.sock`);
    const driver = makeDriver(binaryPath, socketPath);
    const first = await driver.start();

    process.kill(first.pid, 'SIGTERM');
    await expect.poll(() => driver.connection).toBeUndefined();

    const second = await driver.start();
    expect(second.pid).not.toBe(first.pid);
  });

  it('reports an early process exit instead of waiting for the timeout', async () => {
    const socketPath = join(tmpdir(), `cua-driver-exit-${process.pid}.sock`);
    const driver = makeDriver('/usr/bin/true', socketPath);

    await expect(driver.start()).rejects.toMatchObject({ code: 'exited-before-ready' });
  });

  it('times out when the daemon never creates its socket', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'cua embedded timeout '));
    temporaryDirectories.push(directory);
    const binaryPath = join(directory, 'unready-driver');
    await writeFile(binaryPath, '#!/usr/bin/env node\nsetInterval(() => {}, 1000);\n');
    await chmod(binaryPath, 0o755);
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: 'com.example.host',
      startupTimeoutMs: 100,
      shutdownTimeoutMs: 100,
      stderr: 'ignore',
    });
    drivers.push(driver);

    await expect(driver.start()).rejects.toMatchObject({ code: 'startup-timeout' });
  });

  it('force-kills an unresponsive child after a startup failure', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'cua-embedded-unresponsive-'));
    temporaryDirectories.push(directory);
    const binaryPath = join(directory, 'unresponsive-driver');
    const pidPath = join(directory, 'pid');
    await writeFile(
      binaryPath,
      `#!/usr/bin/env node
const fs = require("node:fs");
fs.writeFileSync(${JSON.stringify(pidPath)}, String(process.pid));
process.on("SIGTERM", () => process.stderr.write("ignored SIGTERM\\n"));
setInterval(() => {}, 1000);
`
    );
    await chmod(binaryPath, 0o755);
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: 'com.example.host',
      startupTimeoutMs: 1_000,
      shutdownTimeoutMs: 100,
      stderr: 'ignore',
    });
    drivers.push(driver);

    await expect(driver.start()).rejects.toMatchObject({ code: 'startup-timeout' });
    const pid = Number(await readFile(pidPath, 'utf8'));
    expect(() => process.kill(pid, 0)).toThrow();
  });

  it('can stop while startup is still waiting for the socket', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'cua embedded stop '));
    temporaryDirectories.push(directory);
    const binaryPath = join(directory, 'unready-driver');
    await writeFile(binaryPath, '#!/usr/bin/env node\nsetInterval(() => {}, 1000);\n');
    await chmod(binaryPath, 0o755);
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: 'com.example.host',
      startupTimeoutMs: 10_000,
      shutdownTimeoutMs: 100,
      stderr: 'ignore',
    });
    drivers.push(driver);

    const starting = driver.start();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await driver.stop();
    await expect(starting).rejects.toMatchObject({ code: 'exited-before-ready' });
  });
});

describe('Codex configuration', () => {
  it('quotes paths and preserves the embedded environment', () => {
    expect(
      buildCodexLaunchArgs({
        socketPath: '/tmp/t3 code.sock',
        pid: 123,
        mcp: {
          command: '/Applications/T3 Code/cua-driver',
          args: ['mcp', '--embedded', '--socket', '/tmp/t3 code.sock'],
          env: {
            CUA_DRIVER_EMBEDDED: '1',
            CUA_DRIVER_HOST_BUNDLE_ID: 'com.t3tools.t3code',
          },
        },
      })
    ).toBe(
      '-c "mcp_servers.cua-driver.command=\\"/Applications/T3 Code/cua-driver\\"" -c "mcp_servers.cua-driver.args=[\\"mcp\\",\\"--embedded\\",\\"--socket\\",\\"/tmp/t3 code.sock\\"]" -c "mcp_servers.cua-driver.env={CUA_DRIVER_EMBEDDED=\\"1\\",CUA_DRIVER_HOST_BUNDLE_ID=\\"com.t3tools.t3code\\"}"'
    );
  });
});
