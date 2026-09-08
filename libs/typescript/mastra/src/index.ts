import type {
  ProviderStatus,
  SandboxComputer,
  SandboxStartResult,
  WorkspaceSandbox,
} from '@mastra/core/workspace';
import { PNG } from 'pngjs';
import {
  CloudFleetSession,
  CuaFleetError,
  safeError,
  type FleetSession,
  type SessionOptions,
} from './fleet-session.js';

export { CuaFleetError } from './fleet-session.js';

export interface CuaFleetSandboxOptions {
  /** Logical session identity, not a reconnectable Fleet sandbox identifier. */
  id: string;
  /** Existing Linux pool with a computer-server service named `server`. */
  poolName: string;
  clientId: string;
  clientSecret: string;
  /** Creation-age deadline; does not renew automatically. Default: 3600. */
  claimTtlSeconds?: number;
  /** Total acquisition/readiness deadline. Default: 900000. */
  startupTimeoutMs?: number;
  /** Per-HTTP-request deadline. Default: 60000. */
  requestTimeoutMs?: number;
}

function integer(value: number, min = 0, max = 100_000): number {
  if (!Number.isSafeInteger(value) || value < min || value > max)
    throw new CuaFleetError('INVALID_ARGUMENT');
  return value;
}

const aliases: Record<string, string> = {
  enter: 'enter',
  return: 'enter',
  escape: 'esc',
  esc: 'esc',
  space: 'space',
  spacebar: 'space',
  control: 'ctrl',
  ctrl: 'ctrl',
  alt: 'alt',
  shift: 'shift',
  meta: 'cmd',
  super: 'cmd',
  win: 'cmd',
  arrowup: 'up',
  arrowdown: 'down',
  arrowleft: 'left',
  arrowright: 'right',
  pageup: 'page_up',
  pagedown: 'page_down',
  backspace: 'backspace',
  delete: 'delete',
  tab: 'tab',
  home: 'home',
  end: 'end',
  insert: 'insert',
  capslock: 'caps_lock',
};

function keyName(key: string): string {
  if (typeof key !== 'string' || !key.length) throw new CuaFleetError('INVALID_KEY');
  if (key.length === 1) return key;
  const lower = key.toLowerCase();
  if (aliases[lower]) return aliases[lower];
  if (/^(f([1-9]|1[0-9]|2[0-4])|up|down|left|right|page_up|page_down|caps_lock)$/.test(lower))
    return lower;
  throw new CuaFleetError('UNSUPPORTED_KEY');
}

/** Linux-only Mastra computer capability; each instance owns a separate Fleet claim. */
export class CuaFleetSandbox implements WorkspaceSandbox {
  readonly id: string;
  readonly name = 'Cua Fleet Linux';
  readonly provider = 'cua-fleet';
  readonly supportsCheckpoints = false;
  readonly computer: SandboxComputer;
  #status: ProviderStatus = 'pending';
  #options: SessionOptions;
  #session?: FleetSession;
  #startPromise?: Promise<SandboxStartResult>;
  #destroyPromise?: Promise<void>;
  #closing = false;
  #queue: Promise<unknown> = Promise.resolve();

  constructor(options: CuaFleetSandboxOptions) {
    if (!options.id || !options.poolName || !options.clientId || !options.clientSecret)
      throw new CuaFleetError('MISSING_CONFIGURATION');
    this.id = options.id;
    this.#options = {
      poolName: options.poolName,
      clientId: options.clientId,
      clientSecret: options.clientSecret,
      claimTtlSeconds: integer(options.claimTtlSeconds ?? 3600, 1, 86_400),
      startupTimeoutMs: integer(options.startupTimeoutMs ?? 900_000, 1, 3_600_000),
      requestTimeoutMs: integer(options.requestTimeoutMs ?? 60_000, 1, 300_000),
    };
    if (this.#options.claimTtlSeconds * 1000 <= this.#options.startupTimeoutMs)
      throw new CuaFleetError('TTL_SHORTER_THAN_STARTUP');
    const point = (x: number, y: number) => ({ x: integer(x), y: integer(y) });
    const action = async (command: string, params: Record<string, unknown>) => {
      await this.#command(command, params);
    };
    this.computer = {
      screenshot: async () => {
        const result = await this.#command('screenshot', { format: 'png' });
        if (typeof result.image_data !== 'string') throw new CuaFleetError('INVALID_SCREENSHOT');
        const data = Buffer.from(result.image_data, 'base64');
        if (
          data.length < 24 ||
          data.length > 20 * 1024 * 1024 ||
          data.subarray(0, 8).toString('hex') !== '89504e470d0a1a0a' ||
          !data.readUInt32BE(16) ||
          !data.readUInt32BE(20) ||
          data.readUInt32BE(16) * data.readUInt32BE(20) > 16_000_000
        )
          throw new CuaFleetError('INVALID_SCREENSHOT');
        try {
          PNG.sync.read(data, { checkCRC: true });
        } catch {
          throw new CuaFleetError('INVALID_SCREENSHOT');
        }
        return { data: new Uint8Array(data), mediaType: 'image/png' };
      },
      leftClick: (x, y) => action('left_click', point(x, y)),
      rightClick: (x, y) => action('right_click', point(x, y)),
      doubleClick: (x, y) => action('double_click', point(x, y)),
      moveMouse: (x, y) => action('move_cursor', point(x, y)),
      drag: (from, to) => {
        point(from.x, from.y);
        point(to.x, to.y);
        return action('drag', {
          path: [
            [from.x, from.y],
            [to.x, to.y],
          ],
          button: 'left',
        });
      },
      scroll: (direction, amount) => {
        if (direction !== 'up' && direction !== 'down') throw new CuaFleetError('INVALID_ARGUMENT');
        return action(`scroll_${direction}`, { clicks: integer(amount, 1, 1000) });
      },
      type: (text) => {
        if (typeof text !== 'string') throw new CuaFleetError('INVALID_ARGUMENT');
        return action('type_text', { text });
      },
      press: (key) => {
        if (Array.isArray(key)) {
          if (!key.length || key.length > 8) throw new CuaFleetError('INVALID_KEY');
          return action('hotkey', { keys: key.map(keyName) });
        }
        return action('press_key', { key: keyName(key) });
      },
      getScreenSize: async () => {
        const { size } = await this.#command('get_screen_size', {});
        const result = size as { width?: number; height?: number } | undefined;
        return {
          width: integer(result?.width ?? NaN, 1),
          height: integer(result?.height ?? NaN, 1),
        };
      },
      getCursorPosition: async () => {
        const { position } = await this.#command('get_cursor_position', {});
        const result = position as { x?: number; y?: number } | undefined;
        return point(result?.x ?? NaN, result?.y ?? NaN);
      },
    };
  }

  get status(): ProviderStatus {
    return this.#status;
  }
  protected createSession(): FleetSession {
    return new CloudFleetSession(this.#options);
  }
  isReady(): boolean {
    return this.#status === 'running' && !this.#closing;
  }
  getInstructions(): string {
    return 'Control an isolated Linux desktop. Coordinates are display pixels. This session has no persistent pause/resume, shell tool, or live viewer.';
  }
  async snapshot(): Promise<void> {
    /* Mastra's no-checkpoint contract. */
  }

  start(): Promise<SandboxStartResult> {
    if (this.#closing) return Promise.reject(new CuaFleetError('DESTROYED'));
    if (this.#startPromise) return this.#startPromise;
    if (this.#status === 'running') return Promise.resolve({ outcome: 'connected' });
    this.#status = 'starting';
    this.#startPromise = (async () => {
      try {
        if (this.#session) {
          await this.#session.close();
          this.#session = undefined;
        }
        this.#session = this.createSession();
        await this.#session.start();
        this.#status = 'running';
        return { outcome: 'created' as const };
      } catch (error) {
        this.#status = 'error';
        try {
          await this.#session?.close();
          this.#session = undefined;
        } catch {
          throw new CuaFleetError('START_FAILED_CLEANUP_PENDING');
        }
        throw safeError(error, 'START_FAILED');
      } finally {
        this.#startPromise = undefined;
      }
    })();
    return this.#startPromise;
  }

  async stop(): Promise<void> {
    throw new CuaFleetError('PAUSE_UNSUPPORTED_USE_DESTROY');
  }

  destroy(): Promise<void> {
    this.#closing = true;
    if (this.#destroyPromise) return this.#destroyPromise;
    if (this.#status === 'destroyed') return Promise.resolve();
    this.#destroyPromise = (async () => {
      await this.#startPromise?.catch(() => {});
      await this.#queue.catch(() => {});
      this.#status = 'destroying';
      try {
        await this.#session?.close();
        this.#session = undefined;
        this.#status = 'destroyed';
      } catch (error) {
        this.#status = 'error';
        throw safeError(error, 'CLEANUP_PENDING');
      } finally {
        this.#destroyPromise = undefined;
      }
    })();
    return this.#destroyPromise;
  }

  #command(name: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    if (this.#closing) return Promise.reject(new CuaFleetError('DESTROYED'));
    const result = this.#queue.then(async () => {
      try {
        await this.start();
        if (!this.#session || this.#closing) throw new CuaFleetError('DESTROYED');
        return await this.#session.command(name, params);
      } catch (error) {
        throw safeError(error, 'COMMAND_FAILED');
      }
    });
    this.#queue = result.catch(() => {});
    return result;
  }
}
