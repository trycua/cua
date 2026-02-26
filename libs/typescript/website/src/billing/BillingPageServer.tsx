import { redirect } from 'next/navigation';
import type Stripe from 'stripe';
import { createBillingPortalSession } from './createBillingPortalSession';
import type { OrgBillingInfo } from './types';
import { BillingPage } from './BillingPage';

interface BillingPageServerProps {
  orgSlug: string;
  stripe: Stripe;
  orgBilling: OrgBillingInfo | null;
  baseUrl: string;
}

/**
 * Server component for the billing page.
 *
 * Usage in your Next.js App Router page:
 *
 *   // app/[orgSlug]/billing/page.tsx
 *   export default async function BillingPageRoute({ params }) {
 *     const { orgSlug } = await params;
 *     const orgBilling = await getOrgBilling(orgSlug);
 *     return (
 *       <BillingPageServer
 *         orgSlug={orgSlug}
 *         stripe={stripe}
 *         orgBilling={orgBilling}
 *         baseUrl={process.env.NEXT_PUBLIC_BASE_URL!}
 *       />
 *     );
 *   }
 */
export async function BillingPageServer({
  orgSlug,
  stripe,
  orgBilling,
  baseUrl,
}: BillingPageServerProps) {
  if (!orgBilling || !orgBilling.billingEnabled) {
    redirect(`/${orgSlug}`);
  }

  const returnUrl = `${baseUrl}/${orgSlug}/billing`;
  const result = await createBillingPortalSession(stripe, orgBilling, returnUrl);

  // If we got a portal URL, redirect to Stripe
  if ('url' in result) {
    redirect(result.url);
  }

  // For needsSetup or error cases, pass the result directly to the client
  // component so it renders immediately without a redundant API fetch.
  // This is critical for new orgs that may not have a Stripe customer yet.
  return <BillingPage orgSlug={orgSlug} initialResult={result} />;
}
