package handlers

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/productanalytics"
)

const FleetClaimHeader = "X-Cua-Fleet-Claim"

type AttributionFactsReader interface {
	ReadBoundClaim(context.Context, string, string) (BoundClaim, error)
	PoolExists(context.Context, string, string) (bool, error)
}

func (h Handlers) FleetAttributionQualifier() productanalytics.SvcQualifier {
	qualification := h.FleetAttributionQualification()
	return func(ctx context.Context, r *http.Request, status int) bool {
		return qualification(ctx, r, status).Qualifies
	}
}

func (h Handlers) FleetAttributionQualification() productanalytics.SvcQualification {
	reader := fleetAttributionFactsReader{handlers: h}
	return func(ctx context.Context, r *http.Request, status int) productanalytics.SvcQualificationResult {
		result := QualifySvcRequestResult(ctx, r, status, reader)
		return productanalytics.SvcQualificationResult{Qualifies: result.Qualifies, Reason: result.Reason}
	}
}

type fleetAttributionFactsReader struct{ handlers Handlers }

func (reader fleetAttributionFactsReader) userSubject(ctx context.Context) (string, error) {
	user := auth.GetUser(ctx)
	if user == nil || user.ID == "" {
		return "", fmt.Errorf("missing authenticated user")
	}
	return user.ID, nil
}

func (reader fleetAttributionFactsReader) ReadBoundClaim(ctx context.Context, namespace, claim string) (BoundClaim, error) {
	subject, err := reader.userSubject(ctx)
	if err != nil {
		return BoundClaim{}, err
	}
	path := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxclaims/%s", url.PathEscape(namespace), url.PathEscape(claim))
	response, err := reader.handlers.k8sImpersonate(ctx, http.MethodGet, path, nil, subject)
	if err != nil {
		return BoundClaim{}, err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return BoundClaim{}, fmt.Errorf("claim lookup returned HTTP %d", response.StatusCode)
	}
	var payload struct {
		Metadata struct {
			Name string `json:"name"`
		} `json:"metadata"`
		Status struct {
			Phase   string `json:"phase"`
			Sandbox struct {
				Name string `json:"name"`
			} `json:"sandbox"`
		} `json:"status"`
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 1<<20)).Decode(&payload); err != nil {
		return BoundClaim{}, err
	}
	return BoundClaim{Claim: payload.Metadata.Name, Sandbox: payload.Status.Sandbox.Name, Bound: payload.Status.Phase == "Bound"}, nil
}

func (reader fleetAttributionFactsReader) PoolExists(ctx context.Context, namespace, pool string) (bool, error) {
	subject, err := reader.userSubject(ctx)
	if err != nil {
		return false, err
	}
	if pool == "" {
		return false, fmt.Errorf("missing pool")
	}
	path := fmt.Sprintf("/apis/cua.ai/v1/namespaces/%s/osgymworkspacepools/%s", url.PathEscape(namespace), url.PathEscape(pool))
	response, err := reader.handlers.k8sImpersonate(ctx, http.MethodGet, path, nil, subject)
	if err != nil {
		return false, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotFound {
		return false, nil
	}
	if response.StatusCode != http.StatusOK {
		return false, fmt.Errorf("pool lookup returned HTTP %d", response.StatusCode)
	}
	return true, nil
}

func QualifySvcRequest(ctx context.Context, r *http.Request, status int, reader AttributionFactsReader) bool {
	return QualifySvcRequestResult(ctx, r, status, reader).Qualifies
}

type QualificationResult struct {
	Qualifies bool
	Reason    string
}

func QualifySvcRequestResult(ctx context.Context, r *http.Request, status int, reader AttributionFactsReader) QualificationResult {
	upgrade := isUpgradeRequest(r)
	if reader == nil {
		return QualificationResult{Reason: "facts_unavailable"}
	}
	if r.Pattern == "" || !strings.HasPrefix(r.Pattern, "/api/svc/") {
		return QualificationResult{Reason: "not_svc_route"}
	}
	if r.Method == "HEAD" || r.Method == "OPTIONS" {
		return QualificationResult{Reason: "invalid_method"}
	}
	if upgrade {
		return QualificationResult{Reason: "upgrade_request"}
	}
	claim := r.Header.Get(FleetClaimHeader)
	if len(r.Header.Values(FleetClaimHeader)) > 1 {
		return QualificationResult{Reason: "multiple_claims"}
	}
	if claim == "" {
		return QualificationResult{Reason: "missing_claim"}
	}
	if len(claim) > 128 || !validAttributionClaim(claim) {
		return QualificationResult{Reason: "invalid_claim"}
	}
	ns, service := r.PathValue("namespace"), r.PathValue("service")
	bound, err := reader.ReadBoundClaim(ctx, ns, claim)
	if err != nil {
		return QualificationResult{Reason: "claim_lookup_failed"}
	}
	if bound.Claim != claim {
		return QualificationResult{Reason: "claim_mismatch"}
	}
	if !bound.Bound {
		return QualificationResult{Reason: "claim_not_bound"}
	}
	if bound.Sandbox == "" {
		return QualificationResult{Reason: "sandbox_missing"}
	}
	if service != bound.Sandbox+"-server" {
		return QualificationResult{Reason: "service_mismatch"}
	}
	pool, err := reader.PoolExists(ctx, ns, ns)
	if err != nil {
		return QualificationResult{Reason: "pool_lookup_failed"}
	}
	if !pool {
		return QualificationResult{Reason: "pool_missing"}
	}
	if status < 200 || status >= 300 {
		return QualificationResult{Reason: "non_2xx"}
	}
	if !QualifiesFleetAttribution(AttributionFacts{AuthenticatedNamespace: true, SDKClaim: claim, Claim: bound, Service: service, Namespace: ns, Route: r.Pattern, Path: r.PathValue("path"), Method: r.Method, NamespacePoolExists: pool, UpstreamStatus: status, Upgrade: upgrade}) {
		return QualificationResult{Reason: "probe_request"}
	}
	return QualificationResult{Qualifies: true}
}

func isUpgradeRequest(r *http.Request) bool {
	if r.Header.Get("Upgrade") != "" {
		return true
	}
	for _, value := range r.Header.Values("Connection") {
		for _, token := range strings.Split(value, ",") {
			if strings.EqualFold(strings.TrimSpace(token), "upgrade") {
				return true
			}
		}
	}
	return false
}

func validAttributionClaim(value string) bool {
	for _, b := range []byte(value) {
		if !(b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9' || strings.ContainsRune("._~-", rune(b))) {
			return false
		}
	}
	return true
}

// BoundClaim is the exact claim correlation supplied by the SDK.
type BoundClaim struct {
	Claim, Sandbox string
	Bound          bool
}

// AttributionFacts contains already-proven facts; qualification performs no I/O.
type AttributionFacts struct {
	AuthenticatedNamespace                  bool
	SDKClaim                                string
	Claim                                   BoundClaim
	Service, Namespace, Route, Path, Method string
	NamespacePoolExists                     bool
	UpstreamStatus                          int
	Upgrade                                 bool
}

// QualifiesFleetAttribution is a pure Phase 1 predicate, not an activation claim.
func QualifiesFleetAttribution(f AttributionFacts) bool {
	if !f.AuthenticatedNamespace || f.SDKClaim == "" || !f.Claim.Bound || f.Claim.Claim == "" || f.Claim.Claim != f.SDKClaim || f.Claim.Sandbox == "" || f.Service != f.Claim.Sandbox+"-server" || !f.NamespacePoolExists || f.UpstreamStatus < 200 || f.UpstreamStatus >= 300 || f.Upgrade || f.Method == "HEAD" || f.Method == "OPTIONS" {
		return false
	}
	if f.Route == "/api/k8s" || f.Route == "/api/orch" || !strings.HasPrefix(f.Route, "/api/svc/") {
		return false
	}
	probes := map[string]struct{}{"health": {}, "healthz": {}, "ready": {}, "readiness": {}, "readyz": {}, "live": {}, "liveness": {}, "livez": {}, "metrics": {}}
	for _, segment := range strings.Split(strings.ToLower(strings.Trim(f.Path, "/")), "/") {
		if _, excluded := probes[segment]; excluded {
			return false
		}
	}
	return true
}
