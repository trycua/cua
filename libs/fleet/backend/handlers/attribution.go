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

type AttributionFactsReader interface {
	ReadBoundClaimForSandbox(context.Context, string, string) (BoundClaim, error)
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

func (reader fleetAttributionFactsReader) ReadBoundClaimForSandbox(ctx context.Context, namespace, sandbox string) (BoundClaim, error) {
	subject, err := reader.userSubject(ctx)
	if err != nil {
		return BoundClaim{}, err
	}
	sandboxPath := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxes/%s", url.PathEscape(namespace), url.PathEscape(sandbox))
	response, err := reader.handlers.k8sImpersonate(ctx, http.MethodGet, sandboxPath, nil, subject)
	if err != nil {
		return BoundClaim{}, err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return BoundClaim{}, fmt.Errorf("sandbox lookup returned HTTP %d", response.StatusCode)
	}
	var sandboxPayload struct {
		Metadata struct {
			OwnerReferences []struct {
				APIVersion string `json:"apiVersion"`
				Kind       string `json:"kind"`
				Name       string `json:"name"`
				UID        string `json:"uid"`
				Controller *bool  `json:"controller"`
			} `json:"ownerReferences"`
		} `json:"metadata"`
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 1<<20)).Decode(&sandboxPayload); err != nil {
		return BoundClaim{}, err
	}
	var ownerName, ownerUID string
	for _, owner := range sandboxPayload.Metadata.OwnerReferences {
		if owner.APIVersion != "osgym.cua.ai/v1alpha1" || owner.Kind != "OSGymSandboxClaim" || owner.Controller == nil || !*owner.Controller {
			continue
		}
		if ownerName != "" {
			return BoundClaim{}, fmt.Errorf("sandbox has multiple claim controller owners")
		}
		ownerName, ownerUID = owner.Name, owner.UID
	}
	if ownerName == "" || ownerUID == "" {
		return BoundClaim{Sandbox: sandbox}, nil
	}

	claimPath := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxclaims/%s", url.PathEscape(namespace), url.PathEscape(ownerName))
	response, err = reader.handlers.k8sImpersonate(ctx, http.MethodGet, claimPath, nil, subject)
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
			UID  string `json:"uid"`
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
	return BoundClaim{
		Claim:        payload.Metadata.Name,
		Sandbox:      payload.Status.Sandbox.Name,
		Bound:        payload.Status.Phase == "Bound",
		OwnerMatched: payload.Metadata.Name == ownerName && payload.Metadata.UID == ownerUID,
	}, nil
}

func (reader fleetAttributionFactsReader) PoolExists(ctx context.Context, namespace, pool string) (bool, error) {
	subject, err := reader.userSubject(ctx)
	if err != nil {
		return false, err
	}
	if pool == "" {
		return false, fmt.Errorf("missing pool")
	}
	// Fleet SDK pools are native OSGymSandboxWarmPool resources. Keep the
	// qualification lookup aligned with the resource the SDK actually creates.
	path := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxwarmpools/%s", url.PathEscape(namespace), url.PathEscape(pool))
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
	if status < http.StatusOK || status >= http.StatusMultipleChoices {
		return QualificationResult{Reason: "non_2xx"}
	}
	if isProbePath(r.PathValue("path")) {
		return QualificationResult{Reason: "probe_request"}
	}
	ns, service := r.PathValue("namespace"), r.PathValue("service")
	sandbox, ok := strings.CutSuffix(service, "-server")
	if !ok || sandbox == "" {
		return QualificationResult{Reason: "service_mismatch"}
	}
	bound, err := reader.ReadBoundClaimForSandbox(ctx, ns, sandbox)
	if err != nil {
		return QualificationResult{Reason: "binding_lookup_failed"}
	}
	if bound.Claim == "" {
		return QualificationResult{Reason: "claim_missing"}
	}
	if !bound.OwnerMatched {
		return QualificationResult{Reason: "claim_mismatch"}
	}
	if !bound.Bound {
		return QualificationResult{Reason: "claim_not_bound"}
	}
	if bound.Sandbox == "" {
		return QualificationResult{Reason: "sandbox_missing"}
	}
	if bound.Sandbox != sandbox || service != bound.Sandbox+"-server" {
		return QualificationResult{Reason: "service_mismatch"}
	}
	pool, err := reader.PoolExists(ctx, ns, ns)
	if err != nil {
		return QualificationResult{Reason: "pool_lookup_failed"}
	}
	if !pool {
		return QualificationResult{Reason: "pool_missing"}
	}
	if !QualifiesFleetAttribution(AttributionFacts{AuthenticatedNamespace: true, Claim: bound, Service: service, Namespace: ns, Route: r.Pattern, Path: r.PathValue("path"), Method: r.Method, NamespacePoolExists: pool, UpstreamStatus: status, Upgrade: upgrade}) {
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

// BoundClaim is the persisted sandbox-to-claim relationship derived by the backend.
type BoundClaim struct {
	Claim, Sandbox string
	Bound          bool
	OwnerMatched   bool
}

// AttributionFacts contains already-proven facts; qualification performs no I/O.
type AttributionFacts struct {
	AuthenticatedNamespace                  bool
	Claim                                   BoundClaim
	Service, Namespace, Route, Path, Method string
	NamespacePoolExists                     bool
	UpstreamStatus                          int
	Upgrade                                 bool
}

// QualifiesFleetAttribution is a pure Phase 1 predicate, not an activation claim.
func QualifiesFleetAttribution(f AttributionFacts) bool {
	if !f.AuthenticatedNamespace || !f.Claim.Bound || !f.Claim.OwnerMatched || f.Claim.Claim == "" || f.Claim.Sandbox == "" || f.Service != f.Claim.Sandbox+"-server" || !f.NamespacePoolExists || f.UpstreamStatus < 200 || f.UpstreamStatus >= 300 || f.Upgrade || f.Method == "HEAD" || f.Method == "OPTIONS" {
		return false
	}
	if f.Route == "/api/k8s" || f.Route == "/api/orch" || !strings.HasPrefix(f.Route, "/api/svc/") {
		return false
	}
	if isProbePath(f.Path) {
		return false
	}
	return true
}

func isProbePath(path string) bool {
	probes := map[string]struct{}{"health": {}, "healthz": {}, "ready": {}, "readiness": {}, "readyz": {}, "live": {}, "liveness": {}, "livez": {}, "metrics": {}}
	for _, segment := range strings.Split(strings.ToLower(strings.Trim(path, "/")), "/") {
		if _, excluded := probes[segment]; excluded {
			return true
		}
	}
	return false
}
