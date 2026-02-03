import posthog from 'posthog-js';
import { PostHogProvider, usePostHog } from 'posthog-js/react';
import { useCallback, useEffect, type ReactNode } from 'react';
import type { Model } from '../types';

// PostHog configuration (public keys, safe for client-side)
const POSTHOG_KEY = 'phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14';
const POSTHOG_HOST = 'https://eu.i.posthog.com';

let isInitialized = false;

/**
 * Initialize PostHog for telemetry.
 * Safe to call multiple times - will only initialize once.
 */
function initPostHog() {
  if (isInitialized || typeof window === 'undefined') return;

  try {
    posthog.init(POSTHOG_KEY, {
      api_host: POSTHOG_HOST,
      autocapture: false,
      capture_pageview: false,
      capture_pageleave: false,
      disable_session_recording: true,
      persistence: 'localStorage',
    });
    isInitialized = true;
  } catch (error) {
    console.warn('[Playground Telemetry] Failed to initialize PostHog:', error);
  }
}

/**
 * Telemetry provider props
 */
export interface TelemetryProviderProps {
  children: ReactNode;
  /** Set to true to disable all telemetry */
  disabled?: boolean;
}

/**
 * Provides telemetry context to the playground.
 * Initializes PostHog on mount unless disabled.
 */
export function TelemetryProvider({ children, disabled }: TelemetryProviderProps) {
  useEffect(() => {
    if (!disabled) {
      initPostHog();
    }
  }, [disabled]);

  if (disabled) {
    return <>{children}</>;
  }

  return <PostHogProvider client={posthog}>{children}</PostHogProvider>;
}

// Check if we're in development mode (safe for browser environments)
const isDev = typeof window !== 'undefined' && window.location?.hostname === 'localhost';

/**
 * Telemetry hook for the playground.
 *
 * Events tracked:
 * - playground_viewed: Page load (for DAU/WAU/MAU)
 * - playground_message_sent: User sends a message
 * - playground_trajectory_completed: Agent completes a trajectory
 * - playground_trajectory_failed: Agent request fails
 * - playground_trajectory_stopped: User manually stops generation
 * - playground_example_prompt_selected: User clicks an example prompt
 * - playground_trajectory_exported: User exports a trajectory
 * - playground_trajectory_replayed: User replays a trajectory
 */
export function usePlaygroundTelemetry() {
  const posthogClient = usePostHog();

  // Helper to capture events with dev logging
  const capture = useCallback(
    (event: string, properties?: Record<string, unknown>) => {
      if (isDev) {
        console.log(`[Telemetry] ${event}`, properties ?? {});
      }
      posthogClient?.capture(event, properties);
    },
    [posthogClient]
  );

  /**
   * Track playground page view - called once on mount.
   * This is the primary event for DAU/WAU/MAU tracking.
   */
  const trackPlaygroundViewed = useCallback(() => {
    capture('playground_viewed');
  }, [capture]);

  /**
   * Track when user sends a message.
   * Includes model info and message content.
   */
  const trackMessageSent = useCallback(
    (params: {
      model: Model | undefined;
      isFirstMessage: boolean;
      sandboxType: 'vm' | 'custom';
      message: string;
    }) => {
      capture('playground_message_sent', {
        model_id: params.model?.id,
        model_name: params.model?.name,
        is_first_message: params.isFirstMessage,
        sandbox_type: params.sandboxType,
        message: params.message,
      });
    },
    [capture]
  );

  /**
   * Track when an agent trajectory completes successfully.
   * Includes iteration count to understand task complexity.
   */
  const trackTrajectoryCompleted = useCallback(
    (params: { model: Model | undefined; iterationCount: number; durationMs: number }) => {
      capture('playground_trajectory_completed', {
        model_id: params.model?.id,
        model_name: params.model?.name,
        iteration_count: params.iterationCount,
        duration_ms: params.durationMs,
      });
    },
    [capture]
  );

  /**
   * Track when an agent request fails.
   */
  const trackTrajectoryFailed = useCallback(
    (params: { model: Model | undefined; errorType: string }) => {
      capture('playground_trajectory_failed', {
        model_id: params.model?.id,
        model_name: params.model?.name,
        error_type: params.errorType,
      });
    },
    [capture]
  );

  /**
   * Track when user manually stops generation.
   */
  const trackTrajectoryStopped = useCallback(
    (params: { model: Model | undefined }) => {
      capture('playground_trajectory_stopped', {
        model_id: params.model?.id,
        model_name: params.model?.name,
      });
    },
    [capture]
  );

  /**
   * Track when user selects an example prompt.
   */
  const trackExamplePromptSelected = useCallback(
    (params: { promptId: string; promptTitle: string }) => {
      capture('playground_example_prompt_selected', {
        prompt_id: params.promptId,
        prompt_title: params.promptTitle,
      });
    },
    [capture]
  );

  /**
   * Track when user exports a trajectory.
   */
  const trackTrajectoryExported = useCallback(
    (params: { runCount: number }) => {
      capture('playground_trajectory_exported', {
        run_count: params.runCount,
      });
    },
    [capture]
  );

  /**
   * Track when user replays a trajectory.
   */
  const trackTrajectoryReplayed = useCallback(
    (params: { runIndex: number }) => {
      capture('playground_trajectory_replayed', {
        run_index: params.runIndex,
      });
    },
    [capture]
  );

  return {
    trackPlaygroundViewed,
    trackMessageSent,
    trackTrajectoryCompleted,
    trackTrajectoryFailed,
    trackTrajectoryStopped,
    trackExamplePromptSelected,
    trackTrajectoryExported,
    trackTrajectoryReplayed,
  };
}
