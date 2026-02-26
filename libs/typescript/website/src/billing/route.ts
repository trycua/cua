/**
 * Billing API route handler for Next.js App Router.
 *
 * Usage in your Next.js app:
 *   // app/api/[orgSlug]/billing/route.ts
 *   export { GET } from '@trycua/website/billing/route';
 *
 * Or adapt the handler logic into your own route:
 *   import { createBillingPortalSession } from '@trycua/website/billing';
 */

import { NextResponse } from 'next/server';
import type Stripe from 'stripe';
import { createBillingPortalSession } from './createBillingPortalSession';
import type { OrgBillingInfo } from './types';

interface BillingRouteConfig {
  /** Initialized Stripe client instance */
  stripe: Stripe;
  /** Function to look up org billing info from your database */
  getOrgBilling: (orgSlug: string) => Promise<OrgBillingInfo | null>;
  /** Base URL for constructing the return URL (e.g., 'https://cua.ai') */
  baseUrl: string;
}

/**
 * Creates a GET handler for the billing API route.
 *
 * Returns:
 * - 200 with `{ url }` when billing portal session is created successfully
 * - 200 with `{ needsSetup, reason }` when the customer needs to set up billing
 * - 404 when the org is not found
 * - 500 with `{ error, message }` on unexpected errors
 */
export function createBillingRouteHandler(config: BillingRouteConfig) {
  return async function GET(
    _request: Request,
    { params }: { params: Promise<{ orgSlug: string }> }
  ) {
    const { orgSlug } = await params;

    try {
      const orgBilling = await config.getOrgBilling(orgSlug);

      if (!orgBilling) {
        return NextResponse.json(
          { error: 'not_found', message: 'Organization not found' },
          { status: 404 }
        );
      }

      if (!orgBilling.billingEnabled) {
        return NextResponse.json(
          { error: 'not_enabled', message: 'Billing is not enabled for this organization' },
          { status: 403 }
        );
      }

      const returnUrl = `${config.baseUrl}/${orgSlug}/billing`;

      const result = await createBillingPortalSession(
        config.stripe,
        orgBilling,
        returnUrl
      );

      if ('error' in result) {
        return NextResponse.json(result, { status: 500 });
      }

      return NextResponse.json(result);
    } catch (error) {
      console.error(`Billing route error for org ${orgSlug}:`, error);
      return NextResponse.json(
        { error: 'internal_error', message: 'An unexpected error occurred' },
        { status: 500 }
      );
    }
  };
}
