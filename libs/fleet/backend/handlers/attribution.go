package handlers

import (
	"context"
	"encoding/json"
	"errors"
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
		return productanalytics.SvcQualificationResult{
			Qualifies: result.Qualifies, Reason: result.Reason,
			LookupStage: result.LookupStage, LookupErrorClass: result.LookupErrorClass,
		}
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
		return BoundClaim{}, errors.Join(err, attributionLookupError{stage: "sandbox", class: "missing_identity"})
	}
	sandboxPath := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxes/%s", url.PathEscape(namespace), url.PathEscape(sandbox))
	response, err := reader.handlers.k8sImpersonate(ctx, http.MethodGet, sandboxPath, nil, subject)
	if err != nil {
		return BoundClaim{}, errors.Join(err, classifyAttributionLookupError("sandbox", err, "request_failed"))
	}
	if response.StatusCode != http.StatusOK {
		defer closeAttributionResponse(response.Body)
		return BoundClaim{}, attributionLookupStatusError("sandbox", response.StatusCode)
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
	if err := decodeAttributionResponse(response.Body, &sandboxPayload); err != nil {
		return BoundClaim{}, errors.Join(err, classifyAttributionLookupError("sandbox", err, "invalid_response"))
	}
	var ownerName, ownerUID string
	for _, owner := range sandboxPayload.Metadata.OwnerReferences {
		if owner.APIVersion != "osgym.cua.ai/v1alpha1" || owner.Kind != "OSGymSandboxClaim" || owner.Controller == nil || !*owner.Controller {
			continue
		}
		if ownerName != "" {
			return BoundClaim{}, attributionLookupError{stage: "sandbox", class: "invalid_binding"}
		}
		ownerName, ownerUID = owner.Name, owner.UID
	}
	if ownerName == "" || ownerUID == "" {
		return BoundClaim{Sandbox: sandbox}, nil
	}

	claimPath := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxclaims/%s", url.PathEscape(namespace), url.PathEscape(ownerName))
	response, err = reader.handlers.k8sImpersonate(ctx, http.MethodGet, claimPath, nil, subject)
	if err != nil {
		return BoundClaim{}, errors.Join(err, classifyAttributionLookupError("claim", err, "request_failed"))
	}
	if response.StatusCode != http.StatusOK {
		defer closeAttributionResponse(response.Body)
		return BoundClaim{}, attributionLookupStatusError("claim", response.StatusCode)
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
	if err := decodeAttributionResponse(response.Body, &payload); err != nil {
		return BoundClaim{}, errors.Join(err, classifyAttributionLookupError("claim", err, "invalid_response"))
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
		return false, errors.Join(err, attributionLookupError{stage: "pool", class: "missing_identity"})
	}
	if pool == "" {
		return false, attributionLookupError{stage: "pool", class: "invalid_request"}
	}
	// Fleet SDK pools are native OSGymSandboxWarmPool resources. Keep the
	// qualification lookup aligned with the resource the SDK actually creates.
	path := fmt.Sprintf("/apis/osgym.cua.ai/v1alpha1/namespaces/%s/osgymsandboxwarmpools/%s", url.PathEscape(namespace), url.PathEscape(pool))
	response, err := reader.handlers.k8sImpersonate(ctx, http.MethodGet, path, nil, subject)
	if err != nil {
		return false, errors.Join(err, classifyAttributionLookupError("pool", err, "request_failed"))
	}
	defer closeAttributionResponse(response.Body)
	if response.StatusCode == http.StatusNotFound {
		return false, nil
	}
	if response.StatusCode != http.StatusOK {
		return false, attributionLookupStatusError("pool", response.StatusCode)
	}
	return true, nil
}

// decodeAttributionResponse releases each response before the next fact read.
// Keeping a partially read sandbox response open through the claim lookup, or
// closing an unread pool response, prevents HTTP/1 connection reuse.
func decodeAttributionResponse(body io.ReadCloser, payload any) error {
	defer closeAttributionResponse(body)
	return json.NewDecoder(io.LimitReader(body, 1<<20)).Decode(payload)
}

func closeAttributionResponse(body io.ReadCloser) {
	// Consume only a bounded remainder so small Kubernetes responses reach EOF
	// and their connections can be reused. The existing request context also
	// bounds a slow body by the shared qualification deadline. Cleanup errors
	// do not replace a decoded result or an authoritative HTTP status.
	_, _ = io.Copy(io.Discard, io.LimitReader(body, k8sResponseBodyLimit))
	_ = body.Close()
}

func QualifySvcRequest(ctx context.Context, r *http.Request, status int, reader AttributionFactsReader) bool {
	return QualifySvcRequestResult(ctx, r, status, reader).Qualifies
}

type QualificationResult struct {
	Qualifies        bool
	Reason           string
	LookupStage      string
	LookupErrorClass string
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
		return attributionLookupRejection("binding_lookup_failed", "unknown", err)
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
		return attributionLookupRejection("pool_lookup_failed", "pool", err)
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
