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

export class PostHogTelemetryClient {
  private config: {
    enabled: boolean;
    sampleRate: number;
    posthog: { apiKey: string; host: string };
  };
  private installationId: string;
  private initialized = false;
  private queuedEvents: {
    name: string;
    properties: Record<string, unknown>;
    timestamp: number;
  }[] = [];
  private startTime: number; // seconds
  private posthogClient?: PostHog;
  private counters: Record<string, number> = {};

  private logger = pino({ name: 'core.telemetry' });

  constructor() {
    // set up config
    this.config = {
      enabled: true,
      sampleRate: TELEMETRY_SAMPLE_RATE,
      posthog: { apiKey: PUBLIC_POSTHOG_API_KEY, host: PUBLIC_POSTHOG_HOST },
    };
    // Check CUA_TELEMETRY_ENABLED environment variable (defaults to enabled)
    const telemetryEnabled = ['1', 'true', 'yes', 'on'].includes(
      process.env.CUA_TELEMETRY_ENABLED?.toLowerCase() || 'true'
    );

    this.config.enabled = telemetryEnabled;
    this.config.sampleRate = Number.parseFloat(
      process.env.CUA_TELEMETRY_SAMPLE_RATE || String(TELEMETRY_SAMPLE_RATE)
    );
    // init client
    this.installationId = this._getOrCreateInstallationId();
    this.startTime = Date.now() / 1000; // Convert to seconds

    // Log telemetry status on startup
    if (this.config.enabled) {
      this.logger.info(`Telemetry enabled (sampling at ${this.config.sampleRate}%)`);
      // Initialize PostHog client if config is available
      this._initializePosthog();
    } else {
      this.logger.info('Telemetry disabled');
    }
  }

  /**
   * Get or create a random installation ID.
   * This ID is not tied to any personal information.
   */
  private _getOrCreateInstallationId(): string {
    const homeDir = os.homedir();
    const idFile = path.join(homeDir, '.cua', 'installation_id');

    try {
      if (fs.existsSync(idFile)) {
        return fs.readFileSync(idFile, 'utf-8').trim();
      }
    } catch (error) {
      this.logger.debug(`Failed to read installation ID: ${error}`);
    }

    // Create new ID if not exists
    const newId = uuidv4();
    try {
      const dir = path.dirname(idFile);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }
      fs.writeFileSync(idFile, newId);
      return newId;
    } catch (error) {
      this.logger.debug(`Failed to write installation ID: ${error}`);
    }

    // Fallback to in-memory ID if file operations fail
    return newId;
  }

  /**
   * Initialize the PostHog client with configuration.
   */
  private _initializePosthog(): boolean {
    if (this.initialized) {
      return true;
    }

    try {
      this.posthogClient = new PostHog(this.config.posthog.apiKey, {
        host: this.config.posthog.host,
        flushAt: 20, // Number of events to batch before sending
        flushInterval: 30000, // Send events every 30 seconds
      });
      this.initialized = true;
      this.logger.debug('PostHog client initialized successfully');

      // Process any queued events
      this._processQueuedEvents();
      return true;
    } catch (error) {
      this.logger.error(`Failed to initialize PostHog client: ${error}`);
      return false;
    }
  }

  /**
   * Process any events that were queued before initialization.
   */
  private _processQueuedEvents(): void {
    if (!this.posthogClient || this.queuedEvents.length === 0) {
      return;
    }

    for (const event of this.queuedEvents) {
      this._captureEvent(event.name, event.properties);
    }
    this.queuedEvents = [];
  }

  /**
   * Capture an event with PostHog.
   */
  private _captureEvent(eventName: string, properties?: Record<string, unknown>): void {
    if (!this.posthogClient) {
      return;
    }

    try {
      // Add standard properties
      const eventProperties = {
        ...properties,
        version: process.env.npm_package_version || 'unknown',
        platform: process.platform,
        node_version: process.version,
        is_ci: this._isCI,
      };

      this.posthogClient.capture({
        distinctId: this.installationId,
        event: eventName,
        properties: eventProperties,
      });
    } catch (error) {
      this.logger.debug(`Failed to capture event: ${error}`);
    }
  }

  private get _isCI(): boolean {
    /**
     * Detect if running in CI environment.
     */
    return !!(
      process.env.CI ||
      process.env.CONTINUOUS_INTEGRATION ||
      process.env.GITHUB_ACTIONS ||
      process.env.GITLAB_CI ||
      process.env.CIRCLECI ||
      process.env.TRAVIS ||
      process.env.JENKINS_URL
    );
  }

  increment(counterName: string, value = 1) {
    /**
     * Increment a named counter.
     */
    if (!this.config.enabled) {
      return;
    }

    if (!(counterName in this.counters)) {
      this.counters[counterName] = 0;
    }
    this.counters[counterName] += value;
  }

  recordEvent(eventName: string, properties?: Record<string, unknown>): void {
    /**
     * Record an event with optional properties.
     */
    if (!this.config.enabled) {
      return;
    }

    // Increment counter for this event type
    const counterKey = `event:${eventName}`;
    this.increment(counterKey);

    // Apply sampling
    if (Math.random() * 100 > this.config.sampleRate) {
      return;
    }

    const event = {
      name: eventName,
      properties: properties || {},
      timestamp: Date.now() / 1000,
    };

    if (this.initialized && this.posthogClient) {
      this._captureEvent(eventName, properties);
    } else {
      // Queue event if not initialized
      this.queuedEvents.push(event);
      // Try to initialize again
      if (this.config.enabled && !this.initialized) {
        this._initializePosthog();
      }
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
      // Send counter data as a single event
      if (Object.keys(this.counters).length > 0) {
        this._captureEvent('telemetry_counters', {
          counters: { ...this.counters },
          duration: Date.now() / 1000 - this.startTime,
        });
      }

      await this.posthogClient.flush();
      this.logger.debug('Telemetry flushed successfully');

      // Clear counters after sending
      this.counters = {};
      return true;
    } catch (error) {
      this.logger.debug(`Failed to flush telemetry: ${error}`);
      return false;
    }
  }

  enable(): void {
    /**
     * Enable telemetry collection.
     */
    this.config.enabled = true;
    this.logger.info('Telemetry enabled');
    if (!this.initialized) {
      this._initializePosthog();
    }
  }

  async disable(): Promise<void> {
    /**
     * Disable telemetry collection.
     */
    this.config.enabled = false;
    await this.posthogClient?.disable();
    this.logger.info('Telemetry disabled');
  }

  get enabled(): boolean {
    /**
     * Check if telemetry is enabled.
     */
    return this.config.enabled;
  }

  async shutdown(): Promise<void> {
    /**
     * Shutdown the telemetry client and flush any pending events.
     */
    if (this.posthogClient) {
      await this.flush();
      await this.posthogClient.shutdown();
      this.initialized = false;
      this.posthogClient = undefined;
    }
  }
}
