/**
 * This module provides the core telemetry functionality for Cua libraries.
 *
 * It provides a low-overhead way to collect anonymous usage data.
 */

export { PostHogTelemetryClient as Telemetry } from './clients';
export {
  CI_ENV_VARS,
  isCI,
  isTelemetryEnabledFromEnv,
  machineTelemetrySetting,
  type PostHogFactory,
  type PostHogLike,
  type TelemetryClientOptions,
} from './clients';
