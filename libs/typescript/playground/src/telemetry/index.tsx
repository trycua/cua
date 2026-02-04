import { useCallback, type ReactNode } from 'react';
import type { Model } from '../types';

/**
 * Telemetry provider props
 */
export interface TelemetryProviderProps {
  children: ReactNode;
}

/**
 * No-op telemetry provider.
 * Consumers should implement their own telemetry using the callback props
 * on PlaygroundContent (onPlaygroundViewed, onExamplePromptSelected, etc.)
 */
export function TelemetryProvider({ children }: TelemetryProviderProps) {
  return <>{children}</>;
}

/**
 * Telemetry hook interface - consumers can implement their own tracking.
 * This is a no-op implementation for backwards compatibility.
 */
export function usePlaygroundTelemetry() {
  const noop = useCallback(() => {}, []);

  return {
    trackPlaygroundViewed: noop,
    trackMessageSent: noop as (params: {
      model: Model | undefined;
      isFirstMessage: boolean;
      sandboxType: 'vm' | 'custom';
      message: string;
    }) => void,
    trackTrajectoryCompleted: noop as (params: {
      model: Model | undefined;
      iterationCount: number;
      durationMs: number;
    }) => void,
    trackTrajectoryFailed: noop as (params: {
      model: Model | undefined;
      errorType: string;
    }) => void,
    trackTrajectoryStopped: noop as (params: { model: Model | undefined }) => void,
    trackExamplePromptSelected: noop as (params: { promptId: string; promptTitle: string }) => void,
    trackTrajectoryExported: noop as (params: { runCount: number }) => void,
    trackTrajectoryReplayed: noop as (params: { runIndex: number }) => void,
  };
}
