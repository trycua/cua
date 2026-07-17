package keycloak

import (
	"context"
	"fmt"
	"time"

	"github.com/Nerzal/gocloak/v13"

	"cyclops-cs-backend/metrics"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

// WorkloadClientPrefix is the client-id prefix for the per-tenant
// service-account clients minted in the workloads realm for OSGym pool VM
// OIDC federation. One client per tenant (keyed by the owner's Keycloak
// subject), so every pool the tenant owns shares the same identity.
const WorkloadClientPrefix = "tenant-"

// EnsureTenantWorkloadClient idempotently ensures a confidential
// service-account client exists in this Admin's realm (expected to be the
// workloads realm) for the tenant identified by ownerSub, and returns its
// client_id + client_secret.
//
// The client's client_credentials access tokens carry two hardcoded claims:
//
//   - tenant = tenantClaim (e.g. "user-<sub>", the Capsule tenant that owns
//     the pool) — what a customer keys their OIDC trust policy off.
//   - aud    = audience (e.g. "sts.amazonaws.com") — required by AWS STS for
//     AssumeRoleWithWebIdentity; the realm's IAM OIDC provider already lists
//     this audience, so per-tenant clients need NO terraform/OIDC-provider
//     change.
//
// Idempotent: a second call returns the existing client and its current
// secret (the secret is therefore stable across calls, which lets callers
// treat a Secret-already-exists conflict as success). A lost create race is
// recovered by re-looking-up the client.
//
// See docs/decisions/2026-06-25-osgym-pool-workload-oidc.md.
func (a *Admin) EnsureTenantWorkloadClient(
	ctx context.Context, ownerSub, tenantClaim, audience string,
) (clientID, clientSecret string, err error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.ensure_tenant_workload_client",
		trace.WithAttributes(
			attribute.String("keycloak.key_type", "workload"),
			attribute.String("keycloak.realm", a.realm),
			attribute.String("tenant", tenantClaim),
		))
	defer span.End()

	start := time.Now()
	defer func() {
		metrics.RecordKeycloakRequest("EnsureTenantWorkloadClient", time.Since(start), err)
	}()

	tok, err := a.token(ctx)
	if err != nil {
		return "", "", err
	}

	clientID = WorkloadClientPrefix + ownerSub

	// Idempotent fast path: return the existing client's secret.
	if cid, secret, found, lookErr := a.lookupClientSecret(ctx, tok, clientID); lookErr != nil {
		span.RecordError(lookErr)
		span.SetStatus(codes.Error, "client lookup failed")
		return "", "", lookErr
	} else if found {
		return cid, secret, nil
	}

	saEnabled := true
	publicClient := false
	stdFlow := false
	implicitFlow := false
	directAccess := false

	c := gocloak.Client{
		ClientID:                  gocloak.StringP(clientID),
		Name:                      gocloak.StringP(fmt.Sprintf("cua tenant %s (pool VM OIDC)", tenantClaim)),
		Description:               gocloak.StringP(fmt.Sprintf("Per-tenant workload OIDC client for %s, managed by cyclops-cs-backend", tenantClaim)),
		Enabled:                   gocloak.BoolP(true),
		PublicClient:              &publicClient,
		ServiceAccountsEnabled:    &saEnabled,
		StandardFlowEnabled:       &stdFlow,
		ImplicitFlowEnabled:       &implicitFlow,
		DirectAccessGrantsEnabled: &directAccess,
		Attributes: &map[string]string{
			"tenant":     tenantClaim,
			"owner_sub":  ownerSub,
			"managed_by": "cyclops-cs-backend",
			"key_type":   "workload",
		},
		ProtocolMappers: &[]gocloak.ProtocolMapperRepresentation{
			{
				// The trust anchor: a stable per-tenant claim. We key off a
				// custom claim rather than `sub` because in client_credentials
				// grants Keycloak sets `sub` to the service-account's internal
				// UUID (mirrors the workloads-realm opencode/openclaw clients).
				Name:           gocloak.StringP("tenant"),
				Protocol:       gocloak.StringP("openid-connect"),
				ProtocolMapper: gocloak.StringP("oidc-hardcoded-claim-mapper"),
				Config: &map[string]string{
					"claim.name":           "tenant",
					"claim.value":          tenantClaim,
					"jsonType.label":       "String",
					"access.token.claim":   "true",
					"id.token.claim":       "false",
					"userinfo.token.claim": "false",
				},
			},
			{
				// aud = audience (sts.amazonaws.com). Keycloak defaults `aud`
				// to the client_id; this mapper overrides it so AWS STS — and
				// the realm's existing IAM OIDC provider audience list —
				// accept the token without a per-client provider update.
				Name:           gocloak.StringP("workload-audience"),
				Protocol:       gocloak.StringP("openid-connect"),
				ProtocolMapper: gocloak.StringP("oidc-audience-mapper"),
				Config: &map[string]string{
					"included.custom.audience": audience,
					"access.token.claim":       "true",
					"id.token.claim":           "false",
				},
			},
		},
	}

	kcUUID, err := a.client.CreateClient(ctx, tok, a.realm, c)
	if err != nil {
		// Likely a create race (another replica created it concurrently) or
		// a pre-existing client our initial lookup missed — recover by
		// re-looking-up and returning the existing secret.
		if cid, secret, found, lookErr := a.lookupClientSecret(ctx, tok, clientID); lookErr == nil && found {
			return cid, secret, nil
		}
		span.RecordError(err)
		span.SetStatus(codes.Error, "create client failed")
		return "", "", fmt.Errorf("create client: %w", err)
	}

	cs, err := a.client.GetClientSecret(ctx, tok, a.realm, kcUUID)
	if err != nil {
		return "", "", fmt.Errorf("read client secret: %w", err)
	}
	if cs.Value == nil {
		return "", "", fmt.Errorf("client secret missing in keycloak response")
	}
	return clientID, *cs.Value, nil
}

// lookupClientSecret returns (clientID, secret, found, err) for a client by
// its clientId. found=false (nil err) means no such client.
func (a *Admin) lookupClientSecret(
	ctx context.Context, tok, clientID string,
) (string, string, bool, error) {
	clients, err := a.client.GetClients(ctx, tok, a.realm, gocloak.GetClientsParams{
		ClientID: gocloak.StringP(clientID),
	})
	if err != nil {
		return "", "", false, fmt.Errorf("lookup client: %w", err)
	}
	for _, c := range clients {
		if derefStr(c.ClientID) != clientID {
			continue
		}
		cs, secErr := a.client.GetClientSecret(ctx, tok, a.realm, derefStr(c.ID))
		if secErr != nil {
			return "", "", false, fmt.Errorf("read client secret: %w", secErr)
		}
		if cs.Value == nil {
			return "", "", false, fmt.Errorf("client secret missing in keycloak response")
		}
		return clientID, *cs.Value, true, nil
	}
	return "", "", false, nil
}
