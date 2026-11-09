import { createContext, useContext, useCallback, type ReactNode } from 'react';
import type { Model } from '../types';

/** Coarse classification of a failed trajectory (sent instead of the message). */
export type TrajectoryErrorType =
  | 'agent_error'
  | 'timeout'
  | 'network'
  | 'http_error'
  | 'auth'
  | 'rate_limit'
  | 'parse_error'
  | 'unknown';

/** Map an error to a {@link TrajectoryErrorType}; the message itself is never sent. */
export function classifyTrajectoryError(error: unknown, timedOut = false): TrajectoryErrorType {
  if (timedOut) return 'timeout';
  if (!(error instanceof Error)) return 'unknown';
  const text = `${error.name} ${error.message}`.toLowerCase();
  if (text.includes('timeout') || text.includes('timed out')) return 'timeout';
  if (
    text.includes('401') ||
    text.includes('403') ||
    text.includes('unauthorized') ||
    text.includes('forbidden')
  )
    return 'auth';
  if (text.includes('429') || text.includes('rate limit')) return 'rate_limit';
  if (text.includes('http') || /\b[45]\d\d\b/.test(text)) return 'http_error';
  if (text.includes('json') || text.includes('parse') || error.name === 'SyntaxError')
    return 'parse_error';
  if (
    text.includes('network') ||
    text.includes('fetch') ||
    text.includes('connect') ||
    error.name === 'TypeError'
  )
    return 'network';
  return 'unknown';
}

/**
 * Telemetry function types
 */
export interface TelemetryFunctions {
  trackPlaygroundViewed: () => void;
  trackMessageSent: (params: {
    model: Model | undefined;
    isFirstMessage: boolean;
    sandboxType: 'vm' | 'custom';
  }) => void;
  trackTrajectoryCompleted: (params: {
    model: Model | undefined;
    iterationCount: number;
    durationMs: number;
  }) => void;
  /** errorType is a fixed classification, never a raw error message. */
  trackTrajectoryFailed: (params: {
    model: Model | undefined;
    errorType: TrajectoryErrorType;
  }) => void;
  trackTrajectoryStopped: (params: { model: Model | undefined }) => void;
  trackExamplePromptSelected: (params: { promptId: string }) => void;
  trackTrajectoryExported: (params: { runCount: number }) => void;
  trackTrajectoryReplayed: (params: { runIndex: number }) => void;
}

/**
 * Telemetry context - allows consumers to provide their own telemetry implementation
 */
const TelemetryContext = createContext<TelemetryFunctions | null>(null);

/**
 * Telemetry provider props
 */
export interface TelemetryProviderProps {
  children: ReactNode;
  /** Custom telemetry functions - if not provided, uses no-op */
  telemetry?: TelemetryFunctions;
}

/**
 * Telemetry provider that allows consumers to inject their own tracking implementation.
 * If no telemetry prop is provided, uses no-op functions.
 */
export function TelemetryProvider({ children, telemetry }: TelemetryProviderProps) {
  return (
    <TelemetryContext.Provider value={telemetry ?? null}>{children}</TelemetryContext.Provider>
  );
}

/**
 * Hook to access telemetry functions.
 * Returns no-op functions if no TelemetryProvider with custom telemetry is present.
 */
export function usePlaygroundTelemetry(): TelemetryFunctions {
  const context = useContext(TelemetryContext);
  const noop = useCallback(() => {}, []);

  // If context is provided, use it; otherwise return no-op functions
  if (context) {
    return context;
  }

  return {
    trackPlaygroundViewed: noop,
    trackMessageSent: noop as TelemetryFunctions['trackMessageSent'],
    trackTrajectoryCompleted: noop as TelemetryFunctions['trackTrajectoryCompleted'],
    trackTrajectoryFailed: noop as TelemetryFunctions['trackTrajectoryFailed'],
    trackTrajectoryStopped: noop as TelemetryFunctions['trackTrajectoryStopped'],
    trackExamplePromptSelected: noop as TelemetryFunctions['trackExamplePromptSelected'],
    trackTrajectoryExported: noop as TelemetryFunctions['trackTrajectoryExported'],
    trackTrajectoryReplayed: noop as TelemetryFunctions['trackTrajectoryReplayed'],
  };
}
