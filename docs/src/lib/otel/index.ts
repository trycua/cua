/**
 * OpenTelemetry instrumentation for CUA Docs
 * Exports the Four Golden Signals: Latency, Traffic, Errors, Saturation
 *
 * Exporter endpoint: otel.cua.ai
 */

import {
  Counter,
  Histogram,
  Gauge,
  metrics,
  ValueType,
} from '@opentelemetry/api';

// OTEL Configuration
const OTEL_SERVICE_NAME = 'cua-docs';
const OTEL_SERVICE_VERSION = '1.0.0';

// Metric names following OpenTelemetry semantic conventions
const METRIC_PREFIX = 'cua.docs';

/**
 * Four Golden Signals metrics for the CUA Docs service
 */
export interface GoldenSignalsMetrics {
  // LATENCY - Request duration histograms
  requestDuration: Histogram;
  copilotResponseTime: Histogram;
  toolExecutionTime: Histogram;

  // TRAFFIC - Request counters
  requestsTotal: Counter;
  copilotMessagesTotal: Counter;
  toolCallsTotal: Counter;
  activeConnections: Gauge;

  // ERRORS - Error counters
  errorsTotal: Counter;
  copilotErrorsTotal: Counter;
  toolErrorsTotal: Counter;

  // SATURATION - Resource utilization gauges
  concurrentRequests: Gauge;
  copilotQueueDepth: Gauge;
  memoryUsageBytes: Gauge;
}

let metricsInstance: GoldenSignalsMetrics | null = null;
let meterInitialized = false;

/**
 * Initialize OpenTelemetry metrics for the Four Golden Signals
 */
export function initializeMetrics(): GoldenSignalsMetrics {
  if (metricsInstance) {
    return metricsInstance;
  }

  const meter = metrics.getMeter(OTEL_SERVICE_NAME, OTEL_SERVICE_VERSION);

  metricsInstance = {
    // ============================================
    // LATENCY - Time to service requests
    // ============================================

    // Overall HTTP request duration
    requestDuration: meter.createHistogram(`${METRIC_PREFIX}.request.duration`, {
      description: 'Duration of HTTP requests in milliseconds',
      unit: 'ms',
      valueType: ValueType.DOUBLE,
    }),

    // CopilotKit response time (including LLM inference)
    copilotResponseTime: meter.createHistogram(`${METRIC_PREFIX}.copilot.response_time`, {
      description: 'Time to generate CopilotKit responses in milliseconds',
      unit: 'ms',
      valueType: ValueType.DOUBLE,
    }),

    // MCP tool execution time
    toolExecutionTime: meter.createHistogram(`${METRIC_PREFIX}.tool.execution_time`, {
      description: 'Duration of MCP tool executions in milliseconds',
      unit: 'ms',
      valueType: ValueType.DOUBLE,
    }),

    // ============================================
    // TRAFFIC - Request volume
    // ============================================

    // Total HTTP requests
    requestsTotal: meter.createCounter(`${METRIC_PREFIX}.requests.total`, {
      description: 'Total number of HTTP requests',
      valueType: ValueType.INT,
    }),

    // CopilotKit messages processed
    copilotMessagesTotal: meter.createCounter(`${METRIC_PREFIX}.copilot.messages.total`, {
      description: 'Total number of CopilotKit messages processed',
      valueType: ValueType.INT,
    }),

    // MCP tool calls
    toolCallsTotal: meter.createCounter(`${METRIC_PREFIX}.tool.calls.total`, {
      description: 'Total number of MCP tool calls',
      valueType: ValueType.INT,
    }),

    // Active WebSocket/SSE connections
    activeConnections: meter.createGauge(`${METRIC_PREFIX}.connections.active`, {
      description: 'Number of active connections',
      valueType: ValueType.INT,
    }),

    // ============================================
    // ERRORS - Failure rates
    // ============================================

    // Total errors
    errorsTotal: meter.createCounter(`${METRIC_PREFIX}.errors.total`, {
      description: 'Total number of errors',
      valueType: ValueType.INT,
    }),

    // CopilotKit-specific errors
    copilotErrorsTotal: meter.createCounter(`${METRIC_PREFIX}.copilot.errors.total`, {
      description: 'Total number of CopilotKit errors',
      valueType: ValueType.INT,
    }),

    // Tool execution errors
    toolErrorsTotal: meter.createCounter(`${METRIC_PREFIX}.tool.errors.total`, {
      description: 'Total number of tool execution errors',
      valueType: ValueType.INT,
    }),

    // ============================================
    // SATURATION - Resource utilization
    // ============================================

    // Concurrent requests being processed
    concurrentRequests: meter.createGauge(`${METRIC_PREFIX}.requests.concurrent`, {
      description: 'Number of requests currently being processed',
      valueType: ValueType.INT,
    }),

    // CopilotKit message queue depth
    copilotQueueDepth: meter.createGauge(`${METRIC_PREFIX}.copilot.queue_depth`, {
      description: 'Number of CopilotKit messages waiting to be processed',
      valueType: ValueType.INT,
    }),

    // Memory usage
    memoryUsageBytes: meter.createGauge(`${METRIC_PREFIX}.memory.usage_bytes`, {
      description: 'Current memory usage in bytes',
      unit: 'By',
      valueType: ValueType.INT,
    }),
  };

  meterInitialized = true;
  return metricsInstance;
}

/**
 * Get the metrics instance (initializes if needed)
 */
export function getMetrics(): GoldenSignalsMetrics {
  return metricsInstance || initializeMetrics();
}

// Attribute types for consistent labeling
export interface RequestAttributes {
  method: string;
  route: string;
  status_code: number;
}

export interface CopilotAttributes {
  category: string;
  question_type: string;
  response_type: string;
  has_code_snippet: boolean;
  conversation_id: string;
}

export interface ToolAttributes {
  tool_name: string;
  success: boolean;
  error_type?: string;
}

/**
 * Record a request with all four golden signals
 */
export function recordRequest(
  attributes: RequestAttributes,
  durationMs: number,
  isError: boolean = false
): void {
  const m = getMetrics();
  const labels = {
    method: attributes.method,
    route: attributes.route,
    status_code: String(attributes.status_code),
  };

  // Latency
  m.requestDuration.record(durationMs, labels);

  // Traffic
  m.requestsTotal.add(1, labels);

  // Errors
  if (isError) {
    m.errorsTotal.add(1, {
      ...labels,
      error_type: attributes.status_code >= 500 ? 'server_error' : 'client_error',
    });
  }
}

/**
 * Record a CopilotKit interaction with all four golden signals
 */
export function recordCopilotInteraction(
  attributes: CopilotAttributes,
  responseTimeMs: number,
  isError: boolean = false,
  errorType?: string
): void {
  const m = getMetrics();
  const labels = {
    category: attributes.category,
    question_type: attributes.question_type,
    response_type: attributes.response_type,
    has_code_snippet: String(attributes.has_code_snippet),
  };

  // Latency
  m.copilotResponseTime.record(responseTimeMs, labels);

  // Traffic
  m.copilotMessagesTotal.add(1, labels);

  // Errors
  if (isError) {
    m.copilotErrorsTotal.add(1, {
      ...labels,
      error_type: errorType || 'unknown',
    });
  }
}

/**
 * Record a tool execution with all four golden signals
 */
export function recordToolExecution(
  attributes: ToolAttributes,
  durationMs: number
): void {
  const m = getMetrics();
  const labels = {
    tool_name: attributes.tool_name,
    success: String(attributes.success),
  };

  // Latency
  m.toolExecutionTime.record(durationMs, labels);

  // Traffic
  m.toolCallsTotal.add(1, labels);

  // Errors
  if (!attributes.success) {
    m.toolErrorsTotal.add(1, {
      ...labels,
      error_type: attributes.error_type || 'unknown',
    });
  }
}

/**
 * Update saturation metrics
 */
export function updateSaturation(
  concurrentRequests: number,
  queueDepth: number = 0
): void {
  const m = getMetrics();
  m.concurrentRequests.record(concurrentRequests);
  m.copilotQueueDepth.record(queueDepth);

  // Record memory usage if available
  if (typeof process !== 'undefined' && process.memoryUsage) {
    const memUsage = process.memoryUsage();
    m.memoryUsageBytes.record(memUsage.heapUsed);
  }
}

/**
 * Helper to create a timed operation that records latency
 */
export async function withTiming<T>(
  operation: () => Promise<T>,
  recordFn: (durationMs: number, error?: Error) => void
): Promise<T> {
  const startTime = performance.now();
  let error: Error | undefined;

  try {
    return await operation();
  } catch (e) {
    error = e instanceof Error ? e : new Error(String(e));
    throw e;
  } finally {
    const durationMs = performance.now() - startTime;
    recordFn(durationMs, error);
  }
}

// Export types
export type { GoldenSignalsMetrics };
