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
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/signedurls"

	"github.com/google/uuid"
)

type CreateSignedServiceURLRequest struct {
	Claim          string `json:"claim" binding:"required"`
	Sandbox        string `json:"sandbox" binding:"required"`
	Service        string `json:"service" binding:"required"`
	LogicalService string `json:"logicalService" binding:"required"`
	// Label is optional and limited to 120 UTF-8 bytes. OpenAPI maxLength is a
	// character upper bound; backend byte validation remains authoritative.
	Label            *string `json:"label" maxLength:"120"`
	ExpiresInSeconds uint32  `json:"expiresInSeconds" binding:"required" minimum:"60" maximum:"86400"`
}

type SignedServiceURLResponse struct {
	ID             string  `json:"id"`
	Namespace      string  `json:"namespace"`
	Claim          string  `json:"claim"`
	Sandbox        string  `json:"sandbox"`
	Service        string  `json:"service"`
	LogicalService string  `json:"logicalService"`
	Label          *string `json:"label"`
	URL            string  `json:"url"`
	CreatedAt      string  `json:"createdAt"`
	ExpiresAt      string  `json:"expiresAt"`
	RevokedAt      *string `json:"revokedAt"`
}

// CreateSignedServiceURL godoc
//
//	@Summary		Create a signed URL for a sandbox service
//	@Description	Creates a temporary bearer link for one logical sandbox service.
//	@Tags		signed-service-urls
//	@Accept		json
//	@Produce	json
//	@Param		namespace	path		string		true	"K8s namespace"
//	@Param		body		body		CreateSignedServiceURLRequest	true	"Signed service URL details"
//	@Success		201		{object}	SignedServiceURLResponse
//	@Failure		400,401,404,500,503	{object}	ErrorResponse
//	@Security	BearerAuth
//	@Router		/api/signed-service-urls/{namespace} [post]
func (h Handlers) CreateSignedServiceURL(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	service := h.signedServiceURLService()
	if service == nil {
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
		return
	}

	var request CreateSignedServiceURLRequest
	if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid request body")
		return
	}
	namespace := r.PathValue("namespace")
	if !validSignedServiceURLIdentifier(namespace) || !validSignedServiceURLIdentifier(request.Claim) || !validSignedServiceURLIdentifier(request.Sandbox) || !validSignedServiceURLIdentifier(request.LogicalService) {
		writeErr(w, http.StatusBadRequest, "invalid signed service URL request")
		return
	}
	expectedServiceName := request.Sandbox + "-" + request.LogicalService
	if !validSignedServiceURLIdentifier(expectedServiceName) || request.Service != expectedServiceName {
		writeErr(w, http.StatusBadRequest, "invalid signed service URL request")
		return
	}
	validationSubject := signedServiceURLValidationSubject(user, h.AuthCfg.KeyClientPfx)
	serviceName, found, err := h.authorizedSignedService(r.Context(), namespace, request.Claim, request.Sandbox, request.LogicalService, validationSubject)
	if err != nil {
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
		return
	}
	if !found || request.Service != serviceName {
		writeErr(w, http.StatusNotFound, "sandbox service not found")
		return
	}
	exists, err := h.signedServiceExistsFor(r.Context(), namespace, serviceName, validationSubject)
	if err != nil {
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
		return
	}
	if !exists {
		writeErr(w, http.StatusNotFound, "sandbox service not found")
		return
	}
	record, err := service.Create(r.Context(), signedurls.CreateInput{Namespace: namespace, ClaimName: request.Claim, SandboxName: request.Sandbox, ServiceName: serviceName, LogicalService: request.LogicalService, Label: request.Label, CreatorSub: user.ID, ExpiresIn: time.Duration(request.ExpiresInSeconds) * time.Second})
	if err != nil {
		writeSignedServiceURLError(w, err, "create")
		return
	}
	writeJSON(w, http.StatusCreated, signedServiceURLResponse(record))
}

const signedServiceURLServiceCheckTimeout = 5 * time.Second

func signedServiceURLValidationSubject(user *auth.User, keyClientPfx string) string {
	if strings.HasPrefix(user.AZP, keyClientPrefix(keyClientPfx)) && user.Namespace != "" {
		return ""
	}
	return user.ID
}

func keyClientPrefix(value string) string {
	if value == "" {
		return "key-"
	}
	return value
}

func (h Handlers) authorizedSignedService(ctx context.Context, namespace, claim, sandbox, logicalService, subject string) (string, bool, error) {
	if h.signedServiceURLs != nil && h.SignedServiceURLProvider == nil {
		return sandbox + "-" + logicalService, true, nil
	}
	claimResponse, err := h.k8sImpersonate(ctx, http.MethodGet, "/apis/osgym.cua.ai/v1alpha1/namespaces/"+url.PathEscape(namespace)+"/osgymsandboxclaims/"+url.PathEscape(claim), nil, subject)
	if err != nil {
		return "", false, err
	}
	defer claimResponse.Body.Close()
	if claimResponse.StatusCode == http.StatusNotFound {
		return "", false, nil
	}
	if claimResponse.StatusCode != http.StatusOK {
		return "", false, fmt.Errorf("signed service claim lookup returned HTTP %d", claimResponse.StatusCode)
	}
	var claimPayload struct {
		Metadata struct {
			Name string `json:"name"`
		} `json:"metadata"`
		Spec struct {
			SandboxTemplateRef struct {
				Name string `json:"name"`
			} `json:"sandboxTemplateRef"`
		} `json:"spec"`
		Status struct {
			Phase   string `json:"phase"`
			Sandbox struct {
				Name string `json:"name"`
			} `json:"sandbox"`
		} `json:"status"`
	}
	if err := json.NewDecoder(io.LimitReader(claimResponse.Body, k8sResponseBodyLimit)).Decode(&claimPayload); err != nil {
		return "", false, err
	}
	if claimPayload.Metadata.Name != claim || claimPayload.Status.Phase != "Bound" || claimPayload.Status.Sandbox.Name != sandbox || !validSignedServiceURLIdentifier(claimPayload.Spec.SandboxTemplateRef.Name) {
		return "", false, nil
	}
	templateResponse, err := h.k8sImpersonate(ctx, http.MethodGet, "/apis/osgym.cua.ai/v1alpha1/namespaces/"+url.PathEscape(namespace)+"/osgymsandboxtemplates/"+url.PathEscape(claimPayload.Spec.SandboxTemplateRef.Name), nil, subject)
	if err != nil {
		return "", false, err
	}
	defer templateResponse.Body.Close()
	if templateResponse.StatusCode == http.StatusNotFound {
		return "", false, nil
	}
	if templateResponse.StatusCode != http.StatusOK {
		return "", false, fmt.Errorf("signed service template lookup returned HTTP %d", templateResponse.StatusCode)
	}
	var templatePayload struct {
		Spec struct {
			VMTemplate struct {
				Services []struct {
					Name string `json:"name"`
				} `json:"services"`
			} `json:"vmTemplate"`
		} `json:"spec"`
	}
	if err := json.NewDecoder(io.LimitReader(templateResponse.Body, k8sResponseBodyLimit)).Decode(&templatePayload); err != nil {
		return "", false, err
	}
	for _, service := range templatePayload.Spec.VMTemplate.Services {
		if service.Name == logicalService {
			serviceName := sandbox + "-" + logicalService
			return serviceName, validSignedServiceURLIdentifier(serviceName), nil
		}
	}
	return "", false, nil
}

func validSignedServiceURLIdentifier(value string) bool {
	return len(value) <= 63 && dnsLabel.MatchString(value)
}

func (h Handlers) signedServiceExistsFor(ctx context.Context, namespace, serviceName, subject string) (bool, error) {
	if h.signedServiceExists != nil {
		return h.signedServiceExists(ctx, namespace, serviceName, subject)
	}
	if h.SignedServiceURLs == nil && h.SignedServiceURLProvider == nil && !h.checkSignedServiceExists {
		return true, nil
	}
	ctx, cancel := context.WithTimeout(ctx, signedServiceURLServiceCheckTimeout)
	defer cancel()
	response, err := h.k8sImpersonate(ctx, http.MethodGet, "/api/v1/namespaces/"+url.PathEscape(namespace)+"/services/"+url.PathEscape(serviceName), nil, subject)
	if err != nil {
		return false, fmt.Errorf("check signed service URL service: %w", err)
	}
	defer response.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, k8sResponseBodyLimit))
	switch response.StatusCode {
	case http.StatusOK:
		return true, nil
	case http.StatusNotFound:
		return false, nil
	default:
		return false, fmt.Errorf("check signed service URL service: unexpected Kubernetes status %d", response.StatusCode)
	}
}

// ListSignedServiceURLs godoc
//
//	@Summary		List signed URLs for a sandbox claim
//	@Tags		signed-service-urls
//	@Produce	json
//	@Param		namespace	path		string	true	"K8s namespace"
//	@Param		claim		query		string	true	"Claim name"
//	@Success		200		{array}	SignedServiceURLResponse
//	@Failure		400,401,500,503	{object}	ErrorResponse
//	@Security	BearerAuth
//	@Router		/api/signed-service-urls/{namespace} [get]
func (h Handlers) ListSignedServiceURLs(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	service := h.signedServiceURLService()
	if service == nil {
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
		return
	}
	claims := r.URL.Query()["claim"]
	if len(claims) != 1 || !validSignedServiceURLIdentifier(claims[0]) {
		writeErr(w, http.StatusBadRequest, "invalid claim")
		return
	}
	records, err := service.List(r.Context(), r.PathValue("namespace"), claims[0])
	if err != nil {
		writeSignedServiceURLError(w, err, "list")
		return
	}
	response := make([]SignedServiceURLResponse, 0, len(records))
	for _, record := range records {
		response = append(response, signedServiceURLResponse(record))
	}
	writeJSON(w, http.StatusOK, response)
}

// RevokeSignedServiceURL godoc
//
//	@Summary		Revoke a signed URL
//	@Tags		signed-service-urls
//	@Param		namespace	path		string	true	"K8s namespace"
//	@Param		id			path		string	true	"Signed URL ID"
//	@Success		204
//	@Failure		400,401,404,500,503	{object}	ErrorResponse
//	@Security	BearerAuth
//	@Router		/api/signed-service-urls/{namespace}/{id} [delete]
func (h Handlers) RevokeSignedServiceURL(w http.ResponseWriter, r *http.Request) {
	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "missing user")
		return
	}
	service := h.signedServiceURLService()
	if service == nil {
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
		return
	}
	id, err := uuid.Parse(r.PathValue("id"))
	if err != nil {
		writeErr(w, http.StatusBadRequest, "invalid signed service URL ID")
		return
	}
	if _, err := service.Revoke(r.Context(), r.PathValue("namespace"), id); err != nil {
		writeSignedServiceURLError(w, err, "revoke")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func signedServiceURLResponse(record signedurls.Record) SignedServiceURLResponse {
	response := SignedServiceURLResponse{ID: record.ID.String(), Namespace: record.Namespace, Claim: record.ClaimName, Sandbox: record.SandboxName, Service: record.ServiceName, LogicalService: record.LogicalService, Label: record.Label, URL: record.URL, CreatedAt: record.CreatedAt.UTC().Format(time.RFC3339), ExpiresAt: record.ExpiresAt.UTC().Format(time.RFC3339)}
	if record.RevokedAt != nil {
		revokedAt := record.RevokedAt.UTC().Format(time.RFC3339)
		response.RevokedAt = &revokedAt
	}
	return response
}

func writeSignedServiceURLError(w http.ResponseWriter, err error, operation string) {
	switch {
	case errors.Is(err, signedurls.ErrUnavailable):
		writeErr(w, http.StatusServiceUnavailable, "signed service URLs are unavailable")
	case errors.Is(err, signedurls.ErrInvalidInput):
		writeErr(w, http.StatusBadRequest, "invalid signed service URL request")
	case errors.Is(err, signedurls.ErrNotFound):
		writeErr(w, http.StatusNotFound, "signed service URL not found")
	default:
		writeErr(w, http.StatusInternalServerError, "failed to "+operation+" signed service URL")
	}
}
