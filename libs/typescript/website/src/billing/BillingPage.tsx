'use client';

import { useEffect, useState } from 'react';
import type { BillingResponse } from './types';

interface BillingPageProps {
  orgSlug: string;
  /** Base path for API calls (defaults to '') */
  apiBasePath?: string;
  /**
   * Initial result from server-side billing check.
   * When provided, skips the client-side API fetch and renders immediately.
   * This avoids a redundant round-trip and prevents failures when the
   * API route returns different results than the server-side check.
   */
  initialResult?: BillingResponse;
  /** Custom component to render when billing setup is needed */
  renderSetupNeeded?: (props: { orgSlug: string; reason: string }) => React.ReactNode;
  /** Custom component to render on error */
  renderError?: (props: { message: string; onRetry: () => void }) => React.ReactNode;
}

/**
 * Billing page component that fetches the billing portal URL and either
 * redirects to Stripe or shows appropriate fallback UI.
 *
 * Handles the case where the Stripe customer doesn't exist by showing
 * a setup prompt instead of crashing.
 */
export function BillingPage({
  orgSlug,
  apiBasePath = '',
  initialResult,
  renderSetupNeeded,
  renderError,
}: BillingPageProps) {
  const [state, setState] = useState<
    | { status: 'loading' }
    | { status: 'redirecting'; url: string }
    | { status: 'needs_setup'; reason: string }
    | { status: 'error'; message: string }
  >(() => {
    if (initialResult) {
      return billingResponseToState(initialResult);
    }
    return { status: 'loading' };
  });

  const fetchBilling = async () => {
    setState({ status: 'loading' });
    try {
      const response = await fetch(`${apiBasePath}/api/${orgSlug}/billing`);
      if (!response.ok) {
        const text = await response.text();
        let message = 'An error occurred while loading billing information.';
        try {
          const data = JSON.parse(text);
          if (data.message) message = data.message;
        } catch {
          // Response was not JSON
        }
        setState({ status: 'error', message });
        return;
      }

      const data: BillingResponse = await response.json();

      if ('url' in data) {
        setState({ status: 'redirecting', url: data.url });
        window.location.href = data.url;
        return;
      }

      if ('needsSetup' in data && data.needsSetup) {
        setState({ status: 'needs_setup', reason: data.reason });
        return;
      }

      if ('error' in data) {
        setState({ status: 'error', message: data.message });
        return;
      }

      setState({ status: 'error', message: 'Unexpected response from billing service' });
    } catch {
      setState({
        status: 'error',
        message: 'Unable to connect to the billing service. Please try again.',
      });
    }
  };

  useEffect(() => {
    // Skip fetching if we already have a result from the server
    if (initialResult) return;
    fetchBilling();
  }, [orgSlug]);

  if (state.status === 'loading' || state.status === 'redirecting') {
    return (
      <div className="flex min-h-[400px] items-center justify-center">
        <div className="flex flex-col items-center gap-4">
          <div className="h-8 w-8 animate-spin rounded-full border-4 border-neutral-200 border-t-neutral-900 dark:border-neutral-700 dark:border-t-white" />
          <p className="text-sm text-neutral-600 dark:text-neutral-400">
            {state.status === 'redirecting'
              ? 'Redirecting to billing portal...'
              : 'Loading billing information...'}
          </p>
        </div>
      </div>
    );
  }

  if (state.status === 'needs_setup') {
    if (renderSetupNeeded) {
      return <>{renderSetupNeeded({ orgSlug, reason: state.reason })}</>;
    }

    return (
      <div className="flex min-h-[400px] items-center justify-center">
        <div className="mx-auto max-w-md rounded-lg border border-neutral-200 bg-white p-8 text-center shadow-sm dark:border-neutral-700 dark:bg-neutral-800">
          <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-amber-100 dark:bg-amber-900/30">
            <svg
              className="h-6 w-6 text-amber-600 dark:text-amber-400"
              fill="none"
              viewBox="0 0 24 24"
              strokeWidth={1.5}
              stroke="currentColor"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M2.25 8.25h19.5M2.25 9h19.5m-16.5 5.25h6m-6 2.25h3m-3.75 3h15a2.25 2.25 0 002.25-2.25V6.75A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25v10.5A2.25 2.25 0 004.5 19.5z"
              />
            </svg>
          </div>
          <h2 className="mb-2 text-lg font-semibold text-neutral-900 dark:text-white">
            Set Up Billing
          </h2>
          <p className="mb-6 text-sm text-neutral-600 dark:text-neutral-400">
            {state.reason === 'customer_not_found'
              ? 'Your billing account needs to be reconnected. This can happen when switching between environments.'
              : 'Usage-based billing is enabled for your organization. Set up a payment method to get started.'}
          </p>
          <a
            href={`/${orgSlug}/settings`}
            className="inline-flex items-center gap-2 rounded-md bg-neutral-900 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-neutral-800 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-100"
          >
            Go to Settings
          </a>
        </div>
      </div>
    );
  }

  if (state.status === 'error') {
    if (renderError) {
      return <>{renderError({ message: state.message, onRetry: fetchBilling })}</>;
    }

    return (
      <div className="flex min-h-[400px] items-center justify-center">
        <div className="mx-auto max-w-md rounded-lg border border-red-200 bg-white p-8 text-center shadow-sm dark:border-red-800 dark:bg-neutral-800">
          <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-red-100 dark:bg-red-900/30">
            <svg
              className="h-6 w-6 text-red-600 dark:text-red-400"
              fill="none"
              viewBox="0 0 24 24"
              strokeWidth={1.5}
              stroke="currentColor"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z"
              />
            </svg>
          </div>
          <h2 className="mb-2 text-lg font-semibold text-neutral-900 dark:text-white">
            Billing Unavailable
          </h2>
          <p className="mb-6 text-sm text-neutral-600 dark:text-neutral-400">{state.message}</p>
          <button
            type="button"
            onClick={fetchBilling}
            className="inline-flex items-center gap-2 rounded-md bg-neutral-900 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-neutral-800 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-100"
          >
            Try Again
          </button>
        </div>
      </div>
    );
  }

  return null;
}

function billingResponseToState(result: BillingResponse) {
  if ('url' in result) {
    return { status: 'redirecting' as const, url: result.url };
  }
  if ('needsSetup' in result && result.needsSetup) {
    return { status: 'needs_setup' as const, reason: result.reason };
  }
  if ('error' in result) {
    return { status: 'error' as const, message: result.message };
  }
  return { status: 'error' as const, message: 'Unexpected response from billing service' };
}
