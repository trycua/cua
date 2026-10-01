/**
 * Telemetry client using PostHog for collecting anonymous usage data.
 */

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { pino } from 'pino';
import { PostHog } from 'posthog-node';
import { v4 as uuidv4 } from 'uuid';

// Controls how frequently telemetry will be sent (percentage)
export const TELEMETRY_SAMPLE_RATE = 100; // 100% sampling rate

// Public PostHog config for anonymous telemetry
// These values are intentionally public and meant for anonymous telemetry only
// https://posthog.com/docs/product-analytics/troubleshooting#is-it-ok-for-my-api-key-to-be-exposed-and-public
export const PUBLIC_POSTHOG_API_KEY = 'phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14';
export const PUBLIC_POSTHOG_HOST = 'https://eu.i.posthog.com';

const FALSY = new Set(['0', 'false', 'no', 'off']);
const TRUTHY = new Set(['1', 'true', 'yes', 'on']);

export const CI_ENV_VARS = [
  'CI',
  'GITHUB_ACTIONS',
  'GITLAB_CI',
  'BUILDKITE',
  'CIRCLECI',
  'JENKINS_URL',
  'TF_BUILD',
  'CONTINUOUS_INTEGRATION',
] as const;

type Env = Record<string, string | undefined>;

const norm = (v: string | undefined) => (v ?? '').trim().toLowerCase();

/** True when the process looks like it runs in CI. */
export function isCI(env: Env = process.env): boolean {
  return CI_ENV_VARS.some((name) => {
    const v = norm(env[name]);
    return v !== '' && !FALSY.has(v);
  });
}

/**
 * Shared Cua telemetry enablement rules.
 *
 * Off when DO_NOT_TRACK is non-empty and not "0", CUA_TELEMETRY is
 * 0/false/no/off, legacy CUA_TELEMETRY_ENABLED is 0/false/no/off, or legacy
 * CUA_TELEMETRY_DISABLED is truthy. Then the machine setting
 * (`[telemetry] enabled` in `$CUA_HOME/config.toml`, default
 * `~/.cua/config.toml`, written by `cua telemetry off` and the Cua Spaces
 * app) decides; CUA_TELEMETRY=1 wins over it. Off by default in CI unless
 * CUA_TELEMETRY is explicitly on. Otherwise on.
 *
 * `homeDir` is the user's home when `env` has no HOME (the process's own
 * home directory for the client); without either no config file is read.
 */
export function isTelemetryEnabledFromEnv(env: Env = process.env, homeDir?: string): boolean {
  const dnt = norm(env.DO_NOT_TRACK);
  if (dnt !== '' && dnt !== '0') return false;
  const cua = norm(env.CUA_TELEMETRY);
  if (FALSY.has(cua)) return false;
  if (FALSY.has(norm(env.CUA_TELEMETRY_ENABLED))) return false;
  if (TRUTHY.has(norm(env.CUA_TELEMETRY_DISABLED))) return false;
  if (TRUTHY.has(cua)) return true;
  const machine = machineTelemetrySetting(env, homeDir);
  if (machine !== undefined) return machine;
  if (isCI(env)) return false;
  return true;
}

/**
 * The machine-wide switch, `[telemetry] enabled` in `$CUA_HOME/config.toml`
 * (default `~/.cua/config.toml`), if set. Reads the `[telemetry]` table form
 * the Cua CLI writes and a root `telemetry.enabled` key.
 */
export function machineTelemetrySetting(
  env: Env = process.env,
  homeDir?: string
): boolean | undefined {
  const cuaHome = (env.CUA_HOME ?? '').trim();
  const userHome = (env.HOME ?? env.USERPROFILE ?? homeDir ?? '').trim();
  const dir = cuaHome !== '' ? cuaHome : userHome !== '' ? path.join(userHome, '.cua') : '';
  if (dir === '') return undefined;
  let text: string;
  try {
    text = fs.readFileSync(path.join(dir, 'config.toml'), 'utf-8');
  } catch {
    return undefined;
  }
  let table = '';
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const header = /^\[\s*([^\]]*?)\s*\]\s*(#.*)?$/.exec(line);
    if (header) {
      table = header[1];
      continue;
    }
    const key = table === 'telemetry' ? 'enabled' : table === '' ? 'telemetry.enabled' : null;
    if (key === null) continue;
    const m = /^([A-Za-z0-9_."]+)\s*=\s*(.*?)\s*(#.*)?$/.exec(line);
    if (!m || m[1].replace(/"/g, '').replace(/\s+/g, '') !== key) continue;
    const value = norm(m[2].replace(/^(["'])(.*)\1$/, '$2'));
    if (FALSY.has(value) || value === 'disable' || value === 'disabled') return false;
    if (TRUTHY.has(value) || value === 'enable' || value === 'enabled') return true;
    if (/^-?\d+$/.test(value)) return Number(value) !== 0;
    return undefined;
  }
  return undefined;
}

/** Minimal surface of the posthog-node client used here (injectable for tests). */
export interface PostHogLike {
  capture(message: {
    distinctId: string;
    event: string;
    properties?: Record<string, unknown>;
    disableGeoip?: boolean;
  }): void;
  flush(): Promise<unknown>;
  shutdown(): Promise<unknown>;
  disable?(): Promise<unknown> | unknown;
}

export type PostHogFactory = (
  apiKey: string,
  options: { host: string; flushAt: number; flushInterval: number; disableGeoip: boolean }
) => PostHogLike;

export interface TelemetryClientOptions {
  /** Replace the PostHog client (tests). */
  clientFactory?: PostHogFactory;
  /** Override the environment (tests). Defaults to process.env. */
  env?: Env;
  /** Override the home directory (tests). Defaults to os.homedir(). */
  homeDir?: string;
}

const defaultFactory: PostHogFactory = (apiKey, options) =>
  new PostHog(apiKey, options) as unknown as PostHogLike;

export class PostHogTelemetryClient {
  private config: {
    enabled: boolean;
    sampleRate: number;
    posthog: { apiKey: string; host: string };
  };
  private installationId?: string;
  private initialized = false;
  private queuedEvents: {
    name: string;
    properties: Record<string, unknown>;
    timestamp: number;
  }[] = [];
  private startTime: number; // seconds
  private posthogClient?: PostHogLike;
  private counters: Record<string, number> = {};
  private readonly clientFactory: PostHogFactory;
  private readonly homeDir: string;

  private logger = pino({ name: 'core.telemetry' });

  constructor(options: TelemetryClientOptions = {}) {
    const env = options.env ?? process.env;
    this.clientFactory = options.clientFactory ?? defaultFactory;
    this.homeDir = options.homeDir ?? os.homedir();
    this.config = {
      enabled: isTelemetryEnabledFromEnv(env, this.homeDir),
      sampleRate: Number.parseFloat(env.CUA_TELEMETRY_SAMPLE_RATE || String(TELEMETRY_SAMPLE_RATE)),
      posthog: { apiKey: PUBLIC_POSTHOG_API_KEY, host: PUBLIC_POSTHOG_HOST },
    };
    this.startTime = Date.now() / 1000;

    if (this.config.enabled) {
      this.logger.debug(`Telemetry enabled (sampling at ${this.config.sampleRate}%)`);
      this._initializePosthog();
    } else {
      this.logger.debug('Telemetry disabled');
    }
  }

  /**
   * Get or create a random installation id in ~/.config/cua/installation_id
   * (shared with the Python SDK). Only called while telemetry is enabled.
   * The id is a random UUID, not derived from any personal information.
   */
  private _getOrCreateInstallationId(): string {
    const idFile = path.join(this.homeDir, '.config', 'cua', 'installation_id');
    const legacyFile = path.join(this.homeDir, '.cua', 'installation_id');

    for (const file of [idFile, legacyFile]) {
      try {
        if (fs.existsSync(file)) {
          const stored = fs.readFileSync(file, 'utf-8').trim();
          if (stored) return stored;
        }
      } catch {
        this.logger.debug('Failed to read installation id');
      }
    }

    const newId = uuidv4();
    try {
      fs.mkdirSync(path.dirname(idFile), { recursive: true });
      fs.writeFileSync(idFile, newId);
    } catch {
      this.logger.debug('Failed to write installation id');
    }
    return newId;
  }

  private _initializePosthog(): boolean {
    if (this.initialized) {
      return true;
    }
    if (!this.config.enabled) {
      return false;
    }

    try {
      if (!this.installationId) {
        this.installationId = this._getOrCreateInstallationId();
      }
      this.posthogClient = this.clientFactory(this.config.posthog.apiKey, {
        host: this.config.posthog.host,
        flushAt: 20,
        flushInterval: 30000,
        disableGeoip: true,
      });
      this.initialized = true;
      this.logger.debug('PostHog client initialized');
      this._processQueuedEvents();
      return true;
    } catch (error) {
      this.logger.debug(
        `Failed to initialize PostHog client: ${(error as Error)?.name ?? 'Error'}`
      );
      return false;
    }
  }

  private _processQueuedEvents(): void {
    if (!this.posthogClient || this.queuedEvents.length === 0) {
      return;
    }
    for (const event of this.queuedEvents) {
      this._captureEvent(event.name, event.properties);
    }
    this.queuedEvents = [];
  }

  private _captureEvent(eventName: string, properties?: Record<string, unknown>): void {
    if (!this.posthogClient || !this.installationId) {
      return;
    }

    try {
      const eventProperties = {
        ...properties,
        version: process.env.npm_package_version || 'unknown',
        platform: process.platform,
        node_version: process.versions.node.split('.')[0],
        is_ci: isCI(),
        $process_person_profile: false,
        $geoip_disable: true,
      };

      this.posthogClient.capture({
        distinctId: this.installationId,
        event: eventName,
        properties: eventProperties,
        disableGeoip: true,
      });
    } catch (error) {
      this.logger.debug(`Failed to capture event: ${(error as Error)?.name ?? 'Error'}`);
    }
  }

  increment(counterName: string, value = 1) {
    if (!this.config.enabled) {
      return;
    }
    if (!(counterName in this.counters)) {
      this.counters[counterName] = 0;
    }
    this.counters[counterName] += value;
  }

  recordEvent(eventName: string, properties?: Record<string, unknown>): void {
    if (!this.config.enabled) {
      return;
    }

    this.increment(`event:${eventName}`);

    if (Math.random() * 100 > this.config.sampleRate) {
      return;
    }

    if (this.initialized && this.posthogClient) {
      this._captureEvent(eventName, properties);
    } else {
      this.queuedEvents.push({
        name: eventName,
        properties: properties || {},
        timestamp: Date.now() / 1000,
      });
      this._initializePosthog();
    }
  }

  /**
   * Flush any pending events to PostHog.
   */
  async flush(): Promise<boolean> {
    if (!this.config.enabled || !this.posthogClient) {
      return false;
    }

    try {
      if (Object.keys(this.counters).length > 0) {
        this._captureEvent('telemetry_counters', {
          counters: { ...this.counters },
          duration: Date.now() / 1000 - this.startTime,
        });
      }
      await this.posthogClient.flush();
      this.counters = {};
      return true;
    } catch (error) {
      this.logger.debug(`Failed to flush telemetry: ${(error as Error)?.name ?? 'Error'}`);
      return false;
    }
  }

  /**
   * Enable telemetry collection for this process. Explicit opt-outs in the
   * environment (DO_NOT_TRACK, CUA_TELEMETRY=0, ...) still win.
   */
  enable(): void {
    if (
      !isTelemetryEnabledFromEnv(
        {
          ...process.env,
          CUA_TELEMETRY: process.env.CUA_TELEMETRY || '1',
        },
        this.homeDir
      )
    ) {
      return;
    }
    this.config.enabled = true;
    if (!this.initialized) {
      this._initializePosthog();
    }
  }

  async disable(): Promise<void> {
    this.config.enabled = false;
    await this.posthogClient?.disable?.();
  }

  get enabled(): boolean {
    return this.config.enabled;
  }

  async shutdown(): Promise<void> {
    if (this.posthogClient) {
      await this.flush();
      await this.posthogClient.shutdown();
      this.initialized = false;
      this.posthogClient = undefined;
    }
  }
}
