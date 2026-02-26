import type Stripe from 'stripe';
import type { BillingResponse, OrgBillingInfo } from './types';

/**
 * Creates a Stripe billing portal session with proper error handling.
 *
 * Handles the case where the Stripe customer ID stored in the database
 * no longer exists in Stripe (e.g., test/live mode mismatch, deleted customer).
 * Instead of crashing, returns a structured response indicating that billing
 * setup is needed.
 */
export async function createBillingPortalSession(
  stripe: Stripe,
  orgBilling: OrgBillingInfo,
  returnUrl: string
): Promise<BillingResponse> {
  if (!orgBilling.stripeCustomerId) {
    return {
      needsSetup: true,
      reason: 'no_customer_id',
    };
  }

  try {
    const session = await stripe.billingPortal.sessions.create({
      customer: orgBilling.stripeCustomerId,
      return_url: returnUrl,
    });

    return { url: session.url };
  } catch (error: unknown) {
    if (
      isStripeError(error) &&
      error.code === 'resource_missing'
    ) {
      console.warn(
        `Stripe customer ${orgBilling.stripeCustomerId} not found for org ${orgBilling.orgSlug}. ` +
          'The customer may have been deleted or the Stripe environment may have changed.'
      );
      return {
        needsSetup: true,
        reason: 'customer_not_found',
      };
    }

    console.error('Failed to create billing portal session:', error);
    return {
      error: 'billing_error',
      message:
        isStripeError(error)
          ? error.message
          : 'An unexpected error occurred while loading the billing portal.',
    };
  }
}

function isStripeError(
  error: unknown
): error is Stripe.errors.StripeError {
  return (
    typeof error === 'object' &&
    error !== null &&
    'type' in error &&
    typeof (error as Record<string, unknown>).type === 'string' &&
    (error as Record<string, unknown>).type === 'StripeInvalidRequestError'
  );
}
