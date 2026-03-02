import { createContext, useContext, useCallback, type ReactNode } from 'react';
import type { Model } from '../types';

/**
 * Telemetry function types
 */
export interface TelemetryFunctions {
  trackPlaygroundViewed: () => void;
  trackMessageSent: (params: {
    model: Model | undefined;
    isFirstMessage: boolean;
    sandboxType: 'vm' | 'custom';
    message: string;
  }) => void;
  trackTrajectoryCompleted: (params: {
    model: Model | undefined;
    iterationCount: number;
    durationMs: number;
  }) => void;
  trackTrajectoryFailed: (params: { model: Model | undefined; errorType: string }) => void;
  trackTrajectoryStopped: (params: { model: Model | undefined }) => void;
  trackExamplePromptSelected: (params: { promptId: string; promptTitle: string }) => void;
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
