/**
 * Next.js Instrumentation Entry Point
 *
 * This file is automatically loaded by Next.js to set up instrumentation.
 * It initializes OpenTelemetry for the Four Golden Signals monitoring.
 *
 * @see https://nextjs.org/docs/app/building-your-application/optimizing/instrumentation
 */

export async function register() {
  // Only run on server-side
  if (process.env.NEXT_RUNTIME === 'nodejs') {
    // Dynamically import to avoid issues with edge runtime
    const { initializeOtel } = await import('./lib/otel/instrumentation');
    const { initializeMetrics } = await import('./lib/otel');

    // Initialize OpenTelemetry SDK
    initializeOtel();

    // Initialize Four Golden Signals metrics
    initializeMetrics();

    console.log('[OTEL] OpenTelemetry instrumentation registered for cua-docs');
  }
}
