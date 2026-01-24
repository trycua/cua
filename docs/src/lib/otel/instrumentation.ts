/**
 * OpenTelemetry SDK Configuration for CUA Docs
 * This file sets up the OTLP exporter to otel.cua.ai
 *
 * Import this file early in your application to initialize tracing and metrics.
 */

import { NodeSDK } from '@opentelemetry/sdk-node';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { OTLPMetricExporter } from '@opentelemetry/exporter-metrics-otlp-http';
import { PeriodicExportingMetricReader } from '@opentelemetry/sdk-metrics';
import { Resource } from '@opentelemetry/resources';
import {
  ATTR_SERVICE_NAME,
  ATTR_SERVICE_VERSION,
  ATTR_DEPLOYMENT_ENVIRONMENT,
} from '@opentelemetry/semantic-conventions';
import { diag, DiagConsoleLogger, DiagLogLevel } from '@opentelemetry/api';
import { getNodeAutoInstrumentations } from '@opentelemetry/auto-instrumentations-node';

// Configuration
const OTEL_ENDPOINT = process.env.OTEL_EXPORTER_OTLP_ENDPOINT || 'https://otel.cua.ai';
const SERVICE_NAME = 'cua-docs';
const SERVICE_VERSION = process.env.npm_package_version || '1.0.0';
const ENVIRONMENT = process.env.NODE_ENV || 'development';

// Enable diagnostic logging in development
if (ENVIRONMENT === 'development') {
  diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.INFO);
}

// Resource attributes describing this service
const resource = new Resource({
  [ATTR_SERVICE_NAME]: SERVICE_NAME,
  [ATTR_SERVICE_VERSION]: SERVICE_VERSION,
  [ATTR_DEPLOYMENT_ENVIRONMENT]: ENVIRONMENT,
  'service.namespace': 'cua',
  'service.instance.id': process.env.HOSTNAME || `${SERVICE_NAME}-${Date.now()}`,
});

// OTLP Trace Exporter
const traceExporter = new OTLPTraceExporter({
  url: `${OTEL_ENDPOINT}/v1/traces`,
  headers: {
    'Content-Type': 'application/json',
  },
});

// OTLP Metrics Exporter with periodic export
const metricExporter = new OTLPMetricExporter({
  url: `${OTEL_ENDPOINT}/v1/metrics`,
  headers: {
    'Content-Type': 'application/json',
  },
});

const metricReader = new PeriodicExportingMetricReader({
  exporter: metricExporter,
  exportIntervalMillis: 15000, // Export every 15 seconds
  exportTimeoutMillis: 10000,
});

// Initialize the SDK
let sdk: NodeSDK | null = null;

export function initializeOtel(): NodeSDK {
  if (sdk) {
    return sdk;
  }

  sdk = new NodeSDK({
    resource,
    traceExporter,
    metricReader,
    instrumentations: [
      getNodeAutoInstrumentations({
        // Instrument HTTP for request tracing
        '@opentelemetry/instrumentation-http': {
          enabled: true,
          ignoreIncomingPaths: ['/health', '/ready', '/_next'],
        },
        // Instrument fetch for outbound calls
        '@opentelemetry/instrumentation-fetch': {
          enabled: true,
        },
        // Disable noisy instrumentations
        '@opentelemetry/instrumentation-fs': {
          enabled: false,
        },
        '@opentelemetry/instrumentation-dns': {
          enabled: false,
        },
      }),
    ],
  });

  // Start the SDK
  sdk.start();

  // Graceful shutdown on process exit
  const shutdown = async () => {
    try {
      await sdk?.shutdown();
      console.log('OpenTelemetry SDK shut down successfully');
    } catch (err) {
      console.error('Error shutting down OpenTelemetry SDK:', err);
    }
  };

  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);

  console.log(`OpenTelemetry initialized for ${SERVICE_NAME} -> ${OTEL_ENDPOINT}`);
  return sdk;
}

export function getOtelSdk(): NodeSDK | null {
  return sdk;
}

// Export for use in instrumentation.ts file (Next.js convention)
export { sdk };
