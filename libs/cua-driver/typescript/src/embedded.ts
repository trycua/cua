import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmod, rm } from 'node:fs/promises';
import { createConnection } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const EMBEDDED_ENV = 'CUA_DRIVER_EMBEDDED';
const HOST_BUNDLE_ID_ENV = 'CUA_DRIVER_HOST_BUNDLE_ID';

export type EmbeddedDriverErrorCode =
  'spawn-failed' | 'startup-timeout' | 'startup-cancelled' | 'exited-before-ready';

export class EmbeddedDriverError extends Error {
  readonly code: EmbeddedDriverErrorCode;

  constructor(code: EmbeddedDriverErrorCode, message: string, cause?: unknown) {
    super(message, { cause });
    this.name = 'EmbeddedDriverError';
    this.code = code;
  }
}

export interface EmbeddedMcpConfiguration {
  readonly command: string;
  readonly args: readonly string[];
  readonly env: Readonly<Record<string, string>>;
}

export interface EmbeddedDriverConnection {
  readonly socketPath: string;
  readonly pid: number;
  readonly mcp: EmbeddedMcpConfiguration;
}

export interface EmbeddedDriverOptions {
  readonly binaryPath: string;
  readonly hostBundleId: string;
  readonly socketPath?: string;
  readonly startupTimeoutMs?: number;
  readonly shutdownTimeoutMs?: number;
  /** Additional variables for the child; embedded-mode variables always win. */
  readonly environment?: NodeJS.ProcessEnv;
  readonly stderr?: 'inherit' | 'ignore';
}

const defaultSocketPath = (): string => {
  const nonce = randomBytes(6).toString('hex');
  return process.platform === 'win32'
    ? `\\\\.\\pipe\\cua-${process.pid}-${nonce}`
    : join(tmpdir(), `cua-${process.pid}-${nonce}.sock`);
};

const removeSocket = async (socketPath: string): Promise<void> => {
  if (process.platform !== 'win32') await rm(socketPath, { force: true });
};

const canConnect = (socketPath: string): Promise<boolean> =>
  new Promise((resolve) => {
    const socket = createConnection(socketPath);
    let settled = false;
    const finish = (connected: boolean) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(connected);
    };
    socket.setTimeout(200, () => finish(false));
    socket.once('connect', () => finish(true));
    socket.once('error', () => finish(false));
  });

const startupCancelled = (): EmbeddedDriverError =>
  new EmbeddedDriverError('startup-cancelled', 'embedded cua-driver startup was cancelled');

const delay = (milliseconds: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(startupCancelled());
      return;
    }
    const onAbort = () => {
      clearTimeout(timeout);
      reject(startupCancelled());
    };
    const timeout = setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', onAbort, { once: true });
  });

const waitForExit = (child: ChildProcess, timeoutMs: number): Promise<boolean> =>
  new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve(true);
      return;
    }
    const timeout = setTimeout(() => {
      child.removeListener('exit', onExit);
      resolve(false);
    }, timeoutMs);
    const onExit = () => {
      clearTimeout(timeout);
      resolve(true);
    };
    child.once('exit', onExit);
  });

const terminateChild = async (child: ChildProcess, timeoutMs: number): Promise<void> => {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill('SIGTERM');
  if (!(await waitForExit(child, timeoutMs))) {
    child.kill('SIGKILL');
    await waitForExit(child, timeoutMs);
  }
};

export class EmbeddedCuaDriver {
  readonly #options: Required<
    Pick<EmbeddedDriverOptions, 'startupTimeoutMs' | 'shutdownTimeoutMs' | 'stderr'>
  > &
    Omit<EmbeddedDriverOptions, 'startupTimeoutMs' | 'shutdownTimeoutMs' | 'stderr'>;
  #child: ChildProcess | undefined;
  #connection: EmbeddedDriverConnection | undefined;
  #starting: Promise<EmbeddedDriverConnection> | undefined;
  #startupAbort: AbortController | undefined;
  #stopping: Promise<void> | undefined;

  constructor(options: EmbeddedDriverOptions) {
    if (!options.binaryPath.trim()) throw new TypeError('binaryPath must not be empty');
    if (!options.hostBundleId.trim()) throw new TypeError('hostBundleId must not be empty');
    this.#options = {
      ...options,
      startupTimeoutMs: options.startupTimeoutMs ?? 10_000,
      shutdownTimeoutMs: options.shutdownTimeoutMs ?? 2_000,
      stderr: options.stderr ?? 'inherit',
    };
  }

  get connection(): EmbeddedDriverConnection | undefined {
    return this.#connection;
  }

  start(): Promise<EmbeddedDriverConnection> {
    if (this.#connection) return Promise.resolve(this.#connection);
    if (this.#starting) return this.#starting;
    const waitForStop = this.#stopping ?? Promise.resolve();
    const startupAbort = new AbortController();
    this.#startupAbort = startupAbort;
    this.#starting = waitForStop
      .then(() => this.#start(startupAbort.signal))
      .finally(() => {
        if (this.#startupAbort === startupAbort) this.#startupAbort = undefined;
        this.#starting = undefined;
      });
    return this.#starting;
  }

  async #start(signal: AbortSignal): Promise<EmbeddedDriverConnection> {
    if (signal.aborted) throw startupCancelled();
    const socketPath = this.#options.socketPath ?? defaultSocketPath();
    await removeSocket(socketPath);
    if (signal.aborted) throw startupCancelled();
    const embeddedEnv = {
      [EMBEDDED_ENV]: '1',
      [HOST_BUNDLE_ID_ENV]: this.#options.hostBundleId,
    } as const;
    const args = [
      'serve',
      '--embedded',
      '--socket',
      socketPath,
      '--host-bundle-id',
      this.#options.hostBundleId,
    ];

    let child: ChildProcess;
    try {
      child = spawn(this.#options.binaryPath, args, {
        env: { ...process.env, ...this.#options.environment, ...embeddedEnv },
        shell: false,
        stdio: ['ignore', 'ignore', this.#options.stderr],
      });
    } catch (cause) {
      throw new EmbeddedDriverError(
        'spawn-failed',
        `failed to spawn embedded cua-driver at ${this.#options.binaryPath}`,
        cause
      );
    }
    this.#child = child;

    const spawnFailure = new Promise<never>((_, reject) => {
      child.once('error', (cause) =>
        reject(
          new EmbeddedDriverError(
            'spawn-failed',
            `failed to spawn embedded cua-driver at ${this.#options.binaryPath}`,
            cause
          )
        )
      );
    });
    const exited = new Promise<never>((_, reject) => {
      child.once('exit', (code, signal) => {
        if (this.#child !== child) return;
        if (this.#connection) {
          this.#child = undefined;
          this.#connection = undefined;
          void removeSocket(socketPath);
          return;
        }
        reject(
          new EmbeddedDriverError(
            'exited-before-ready',
            `embedded cua-driver exited before its socket was ready (code=${String(code)}, signal=${String(signal)})`
          )
        );
      });
    });

    const readinessAbort = new AbortController();
    const cancelReadiness = () => readinessAbort.abort();
    signal.addEventListener('abort', cancelReadiness, { once: true });
    try {
      try {
        await Promise.race([
          this.#waitForSocket(socketPath, readinessAbort.signal),
          spawnFailure,
          exited,
        ]);
      } finally {
        signal.removeEventListener('abort', cancelReadiness);
        readinessAbort.abort();
      }
      if (signal.aborted) throw startupCancelled();
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new EmbeddedDriverError(
          'exited-before-ready',
          'embedded cua-driver exited while its socket became ready'
        );
      }
      if (process.platform !== 'win32') await chmod(socketPath, 0o600);
      if (child.pid === undefined) {
        throw new EmbeddedDriverError('spawn-failed', 'embedded cua-driver has no process id');
      }
      const connection: EmbeddedDriverConnection = {
        socketPath,
        pid: child.pid,
        mcp: {
          command: this.#options.binaryPath,
          args: ['mcp', '--embedded', '--socket', socketPath],
          env: embeddedEnv,
        },
      };
      this.#connection = connection;
      return connection;
    } catch (cause) {
      await terminateChild(child, this.#options.shutdownTimeoutMs);
      this.#child = undefined;
      await removeSocket(socketPath);
      throw cause;
    }
  }

  async #waitForSocket(socketPath: string, signal: AbortSignal): Promise<void> {
    const deadline = Date.now() + this.#options.startupTimeoutMs;
    while (Date.now() < deadline) {
      if (signal.aborted) throw startupCancelled();
      if (await canConnect(socketPath)) return;
      await delay(50, signal);
    }
    throw new EmbeddedDriverError(
      'startup-timeout',
      `embedded cua-driver did not accept connections within ${this.#options.startupTimeoutMs}ms`
    );
  }

  stop(): Promise<void> {
    if (this.#stopping) return this.#stopping;
    this.#stopping = this.#stop().finally(() => {
      this.#stopping = undefined;
    });
    return this.#stopping;
  }

  async #stop(): Promise<void> {
    this.#startupAbort?.abort();
    if (this.#child && this.#child.exitCode === null && this.#child.signalCode === null) {
      this.#child.kill('SIGTERM');
    }
    if (this.#starting) await this.#starting.catch(() => undefined);

    const child = this.#child;
    const socketPath = this.#connection?.socketPath ?? this.#options.socketPath;
    this.#connection = undefined;
    this.#child = undefined;

    if (child) await terminateChild(child, this.#options.shutdownTimeoutMs);
    if (socketPath) await removeSocket(socketPath);
  }

  async restart(): Promise<EmbeddedDriverConnection> {
    await this.stop();
    return this.start();
  }
}
