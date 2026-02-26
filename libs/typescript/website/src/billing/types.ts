export interface BillingPortalResult {
  url: string;
}

export interface BillingSetupNeeded {
  needsSetup: true;
  reason: 'customer_not_found' | 'no_customer_id';
}

export interface BillingError {
  error: string;
  message: string;
}

export type BillingResponse = BillingPortalResult | BillingSetupNeeded | BillingError;

export interface OrgBillingInfo {
  stripeCustomerId: string | null;
  billingEnabled: boolean;
  orgSlug: string;
}
