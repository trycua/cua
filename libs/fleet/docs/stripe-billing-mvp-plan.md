# Stripe fleet card setup MVP plan

## Scope

- Stripe-hosted card setup in Checkout `setup` mode.
- Fleet-owned Customer metadata derived from the authenticated Cyclops subject.
- Signed SetupIntent webhook default-card configuration.
- Sanitized saved-card status for the authenticated fleet owner.
- Billing Portal card replacement and removal.

## API shape

- `GET /api/billing/summary` returns the authenticated fleet owner's sanitized saved-card status.
- `POST /api/billing/setup-session` accepts an empty request body and returns a Stripe-hosted card setup URL.
- `POST /api/billing/portal-session` accepts an empty request body and returns the authenticated fleet owner's Billing Portal URL.
- `POST /api/billing/webhook` verifies the bounded raw Stripe body and configures the fleet's default card from signed SetupIntent events.

## Security properties

- The browser never receives a Stripe secret key or submits card details to Cyclops.
- The backend derives fleet ownership from the authenticated request context and controlled Customer metadata.
- Setup and portal sessions use only the Customer owned by the authenticated subject.
- Webhook verification uses the unparsed raw body and rejects missing, invalid, or oversized signatures.
