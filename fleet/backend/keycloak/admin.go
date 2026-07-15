// Package keycloak — gocloak admin wrapper used by /api/keys and
// /api/user-keys to CRUD per-key service-account clients in the
// cyclops-cs realm.
package keycloak

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/Nerzal/gocloak/v13"
	"github.com/google/uuid"

	"cyclops-cs-backend/identity"
	"cyclops-cs-backend/metrics"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"
)

type Admin struct {
	client           *gocloak.GoCloak
	baseURL          string
	realm            string
	clientID         string
	clientSecret     string
	keyClientPfx     string // pool/gateway key client-id prefix ("key-")
	userKeyClientPfx string // per-user key client-id prefix ("ukey-")
}

func adminTracer() trace.Tracer {
	return otel.Tracer("cyclops-cs-backend/keycloak")
}

func NewAdmin(baseURL, realm, clientID, clientSecret, keyClientPfx, userKeyClientPfx string) *Admin {
	return &Admin{
		client:           gocloak.NewClient(baseURL),
		baseURL:          baseURL,
		realm:            realm,
		clientID:         clientID,
		clientSecret:     clientSecret,
		keyClientPfx:     keyClientPfx,
		userKeyClientPfx: userKeyClientPfx,
	}
}

func (a *Admin) token(ctx context.Context) (string, error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.admin_login")
	defer span.End()

	start := time.Now()
	t, err := a.client.LoginClient(ctx, a.clientID, a.clientSecret, a.realm)
	metrics.RecordKeycloakRequest("AdminLogin", time.Since(start), err)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, "admin login failed")
		return "", fmt.Errorf("admin login: %w", err)
	}
	return t.AccessToken, nil
}

type KeyClient struct {
	ID        string `json:"id"`
	ClientID  string `json:"client_id"`
	Name      string `json:"name"`
	Namespace string `json:"namespace"`
	OwnerSub  string `json:"owner_sub"`
}

// CreateKeyClient creates a confidential service-account client whose
// access tokens carry a hardcoded `namespace` claim. Returns the
// client_id and the freshly generated client_secret.
func (a *Admin) CreateKeyClient(ctx context.Context, name, namespace, ownerSub string) (clientID, clientSecret, kcUUID string, err error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.create_key_client", trace.WithAttributes(
		attribute.String("keycloak.key_type", "pool"),
		attribute.String("key.name", name),
		attribute.String("k8s.namespace", namespace),
	))
	defer span.End()

	start := time.Now()
	defer func() { metrics.RecordKeycloakRequest("CreateKeyClient", time.Since(start), err) }()

	tok, err := a.token(ctx)
	if err != nil {
		return "", "", "", err
	}

	clientID = a.keyClientPfx + shortUUID()
	desc := fmt.Sprintf("cyclops-cs API key %q for owner %s", name, ownerSub)
	saEnabled := true
	publicClient := false
	stdFlow := false
	implicitFlow := false
	directAccess := false

	c := gocloak.Client{
		ClientID:                  gocloak.StringP(clientID),
		Name:                      gocloak.StringP(name),
		Description:               gocloak.StringP(desc),
		Enabled:                   gocloak.BoolP(true),
		PublicClient:              &publicClient,
		ServiceAccountsEnabled:    &saEnabled,
		StandardFlowEnabled:       &stdFlow,
		ImplicitFlowEnabled:       &implicitFlow,
		DirectAccessGrantsEnabled: &directAccess,
		Attributes: &map[string]string{
			"namespace":  namespace,
			"owner_sub":  ownerSub,
			"key_name":   name,
			"managed_by": "cyclops-cs-backend",
		},
		ProtocolMappers: &[]gocloak.ProtocolMapperRepresentation{
			{
				Name:           gocloak.StringP("namespace"),
				Protocol:       gocloak.StringP("openid-connect"),
				ProtocolMapper: gocloak.StringP("oidc-hardcoded-claim-mapper"),
				Config: &map[string]string{
					"claim.name":           "namespace",
					"claim.value":          namespace,
					"jsonType.label":       "String",
					"access.token.claim":   "true",
					"id.token.claim":       "false",
					"userinfo.token.claim": "false",
				},
			},
		},
	}

	kcUUID, err = a.client.CreateClient(ctx, tok, a.realm, c)
	if err != nil {
		return "", "", "", fmt.Errorf("create client: %w", err)
	}
	cs, err := a.client.GetClientSecret(ctx, tok, a.realm, kcUUID)
	if err != nil {
		_ = a.client.DeleteClient(ctx, tok, a.realm, kcUUID)
		return "", "", "", fmt.Errorf("read client secret: %w", err)
	}
	if cs.Value == nil {
		_ = a.client.DeleteClient(ctx, tok, a.realm, kcUUID)
		return "", "", "", fmt.Errorf("client secret missing in keycloak response")
	}
	return clientID, *cs.Value, kcUUID, nil
}

// ListKeyClients returns every client managed by cyclops-cs-backend that
// belongs to ownerSub.
func (a *Admin) ListKeyClients(ctx context.Context, ownerSub string) ([]KeyClient, error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.list_key_clients", trace.WithAttributes(
		attribute.String("keycloak.key_type", "pool"),
	))
	defer span.End()

	start := time.Now()
	var err error
	defer func() { metrics.RecordKeycloakRequest("ListKeyClients", time.Since(start), err) }()

	var tok string
	tok, err = a.token(ctx)
	if err != nil {
		return nil, err
	}
	out := []KeyClient{}
	first := 0
	const max = 100
	for {
		params := gocloak.GetClientsParams{
			First: &first,
			Max:   gocloak.IntP(max),
		}
		clients, listErr := a.client.GetClients(ctx, tok, a.realm, params)
		if listErr != nil {
			err = fmt.Errorf("list clients: %w", listErr)
			return nil, err
		}
		for _, c := range clients {
			attrs := derefMap(c.Attributes)
			if attrs["managed_by"] != "cyclops-cs-backend" || attrs["owner_sub"] != ownerSub {
				continue
			}
			out = append(out, KeyClient{
				ID:        derefStr(c.ID),
				ClientID:  derefStr(c.ClientID),
				Name:      derefStr(c.Name),
				Namespace: attrs["namespace"],
				OwnerSub:  attrs["owner_sub"],
			})
		}
		if len(clients) < max {
			break
		}
		first += max
	}
	return out, nil
}

// DeleteKeyClient deletes by Keycloak UUID, but only if owner_sub matches.
func (a *Admin) DeleteKeyClient(ctx context.Context, kcUUID, ownerSub string) error {
	ctx, span := adminTracer().Start(ctx, "keycloak.delete_key_client", trace.WithAttributes(
		attribute.String("keycloak.key_type", "pool"),
		attribute.String("keycloak.client_uuid", kcUUID),
	))
	defer span.End()

	start := time.Now()
	var err error
	defer func() { metrics.RecordKeycloakRequest("DeleteKeyClient", time.Since(start), err) }()

	var tok string
	tok, err = a.token(ctx)
	if err != nil {
		return err
	}
	c, err := a.client.GetClient(ctx, tok, a.realm, kcUUID)
	if err != nil {
		return fmt.Errorf("get client: %w", err)
	}
	attrs := derefMap(c.Attributes)
	if attrs["managed_by"] != "cyclops-cs-backend" {
		return fmt.Errorf("not a cyclops-cs-managed client")
	}
	if attrs["owner_sub"] != ownerSub {
		return fmt.Errorf("forbidden")
	}
	if err := a.client.DeleteClient(ctx, tok, a.realm, kcUUID); err != nil {
		return fmt.Errorf("delete client: %w", err)
	}
	return nil
}

// UserKeyClient is a per-user API key Keycloak client.
type UserKeyClient struct {
	ID       string   `json:"id"`
	ClientID string   `json:"client_id"`
	Name     string   `json:"name"`
	Scope    []string `json:"scope"`
	OwnerSub string   `json:"owner_sub"`
}

// CreateUserKeyClient creates a confidential service-account client whose
// access tokens carry hardcoded `user_sub` and `user_groups` claims.
// Returns the client_id, freshly generated client_secret, and token URL.
func (a *Admin) CreateUserKeyClient(ctx context.Context, name, ownerSub string, scope []string) (clientID, clientSecret, tokenURL string, err error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.create_user_key_client", trace.WithAttributes(
		attribute.String("keycloak.key_type", "user"),
		attribute.String("key.name", name),
	))
	defer span.End()

	start := time.Now()
	defer func() { metrics.RecordKeycloakRequest("CreateUserKeyClient", time.Since(start), err) }()

	tok, err := a.token(ctx)
	if err != nil {
		return "", "", "", err
	}

	clientID = a.userKeyClientPfx + shortUUID()
	desc := fmt.Sprintf("cyclops-cs user API key %q for owner %s", name, ownerSub)
	saEnabled := true
	publicClient := false
	stdFlow := false
	implicitFlow := false
	directAccess := false

	scopeStr := strings.Join(scope, ",")

	c := gocloak.Client{
		ClientID:                  gocloak.StringP(clientID),
		Name:                      gocloak.StringP(name),
		Description:               gocloak.StringP(desc),
		Enabled:                   gocloak.BoolP(true),
		PublicClient:              &publicClient,
		ServiceAccountsEnabled:    &saEnabled,
		StandardFlowEnabled:       &stdFlow,
		ImplicitFlowEnabled:       &implicitFlow,
		DirectAccessGrantsEnabled: &directAccess,
		Attributes: &map[string]string{
			"owner_sub":  ownerSub,
			"key_name":   name,
			"managed_by": "cyclops-cs-backend",
			"key_type":   "user",
			"scope":      scopeStr,
		},
		ProtocolMappers: &[]gocloak.ProtocolMapperRepresentation{
			{
				Name:           gocloak.StringP("user_sub"),
				Protocol:       gocloak.StringP("openid-connect"),
				ProtocolMapper: gocloak.StringP("oidc-hardcoded-claim-mapper"),
				Config: &map[string]string{
					"claim.name":           "user_sub",
					"claim.value":          ownerSub,
					"jsonType.label":       "String",
					"access.token.claim":   "true",
					"id.token.claim":       "false",
					"userinfo.token.claim": "false",
				},
			},
			{
				Name:           gocloak.StringP("user_groups"),
				Protocol:       gocloak.StringP("openid-connect"),
				ProtocolMapper: gocloak.StringP("oidc-hardcoded-claim-mapper"),
				Config: &map[string]string{
					"claim.name":           "user_groups",
					"claim.value":          identity.ImpersonateGroup(ctx, ownerSub),
					"jsonType.label":       "String",
					"access.token.claim":   "true",
					"id.token.claim":       "false",
					"userinfo.token.claim": "false",
				},
			},
		},
	}

	kcUUID, err := a.client.CreateClient(ctx, tok, a.realm, c)
	if err != nil {
		return "", "", "", fmt.Errorf("create client: %w", err)
	}
	cs, err := a.client.GetClientSecret(ctx, tok, a.realm, kcUUID)
	if err != nil {
		_ = a.client.DeleteClient(ctx, tok, a.realm, kcUUID)
		return "", "", "", fmt.Errorf("read client secret: %w", err)
	}
	if cs.Value == nil {
		_ = a.client.DeleteClient(ctx, tok, a.realm, kcUUID)
		return "", "", "", fmt.Errorf("client secret missing in keycloak response")
	}

	tokenURL = a.tokenURL()
	return clientID, *cs.Value, tokenURL, nil
}

// ListUserKeyClients returns every user-key client owned by ownerSub.
func (a *Admin) ListUserKeyClients(ctx context.Context, ownerSub string) ([]UserKeyClient, error) {
	ctx, span := adminTracer().Start(ctx, "keycloak.list_user_key_clients", trace.WithAttributes(
		attribute.String("keycloak.key_type", "user"),
	))
	defer span.End()

	start := time.Now()
	var err error
	defer func() { metrics.RecordKeycloakRequest("ListUserKeyClients", time.Since(start), err) }()

	var tok string
	tok, err = a.token(ctx)
	if err != nil {
		return nil, err
	}
	out := []UserKeyClient{}
	first := 0
	const max = 100
	for {
		params := gocloak.GetClientsParams{
			First: &first,
			Max:   gocloak.IntP(max),
		}
		clients, listErr := a.client.GetClients(ctx, tok, a.realm, params)
		if listErr != nil {
			err = fmt.Errorf("list clients: %w", listErr)
			return nil, err
		}
		for _, c := range clients {
			attrs := derefMap(c.Attributes)
			if attrs["managed_by"] != "cyclops-cs-backend" || attrs["key_type"] != "user" || attrs["owner_sub"] != ownerSub {
				continue
			}
			var scope []string
			if s := attrs["scope"]; s != "" {
				scope = strings.Split(s, ",")
			}
			out = append(out, UserKeyClient{
				ID:       derefStr(c.ID),
				ClientID: derefStr(c.ClientID),
				Name:     derefStr(c.Name),
				Scope:    scope,
				OwnerSub: attrs["owner_sub"],
			})
		}
		if len(clients) < max {
			break
		}
		first += max
	}
	return out, nil
}

// DeleteUserKeyClient deletes a user-key client by Keycloak UUID,
// but only if owner_sub matches and the client is a user key.
func (a *Admin) DeleteUserKeyClient(ctx context.Context, ownerSub, kcUUID string) error {
	ctx, span := adminTracer().Start(ctx, "keycloak.delete_user_key_client", trace.WithAttributes(
		attribute.String("keycloak.key_type", "user"),
		attribute.String("keycloak.client_uuid", kcUUID),
	))
	defer span.End()

	start := time.Now()
	var err error
	defer func() { metrics.RecordKeycloakRequest("DeleteUserKeyClient", time.Since(start), err) }()

	var tok string
	tok, err = a.token(ctx)
	if err != nil {
		return err
	}
	c, err := a.client.GetClient(ctx, tok, a.realm, kcUUID)
	if err != nil {
		return fmt.Errorf("get client: %w", err)
	}
	attrs := derefMap(c.Attributes)
	if attrs["managed_by"] != "cyclops-cs-backend" {
		return fmt.Errorf("not a cyclops-cs-managed client")
	}
	if attrs["key_type"] != "user" {
		return fmt.Errorf("not a user key client")
	}
	if attrs["owner_sub"] != ownerSub {
		return fmt.Errorf("forbidden")
	}
	if err := a.client.DeleteClient(ctx, tok, a.realm, kcUUID); err != nil {
		return fmt.Errorf("delete client: %w", err)
	}
	return nil
}

// tokenURL returns the Keycloak token endpoint for the realm.
func (a *Admin) tokenURL() string {
	return fmt.Sprintf("%s/realms/%s/protocol/openid-connect/token", a.baseURL, a.realm)
}

func shortUUID() string {
	return strings.ReplaceAll(uuid.NewString(), "-", "")[:12]
}

func derefStr(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func derefMap(m *map[string]string) map[string]string {
	if m == nil {
		return map[string]string{}
	}
	return *m
}
