package auth

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func imagePolicyResponse(t *testing.T, method, path, body string, user *User) (*httptest.ResponseRecorder, bool) {
	t.Helper()
	contentType := "application/json"
	if method == http.MethodPatch {
		contentType = "application/merge-patch+json"
	}
	return imagePolicyResponseWithContentTypes(t, method, path, body, user, contentType)
}

func imagePolicyResponseWithContentTypes(t *testing.T, method, path, body string, user *User, contentTypes ...string) (*httptest.ResponseRecorder, bool) {
	t.Helper()
	reached := false
	mux := http.NewServeMux()
	mux.Handle("/api/k8s/{path...}", RouteContext("/api/k8s/{path...}")(PolicyMiddleware(K8sRoutePolicy(), WithPipeline())(http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		reached = true
		forwarded, err := io.ReadAll(request.Body)
		if err != nil || string(forwarded) != body {
			t.Errorf("forwarded body = %q, %v; want %q", forwarded, err, body)
		}
		w.WriteHeader(http.StatusNoContent)
	}))))
	request := httptest.NewRequest(method, "/api/k8s/"+path, strings.NewReader(body))
	for _, contentType := range contentTypes {
		request.Header.Add("Content-Type", contentType)
	}
	request = request.WithContext(context.WithValue(request.Context(), UserKey, user))
	response := httptest.NewRecorder()
	mux.ServeHTTP(response, request)
	return response, reached
}

func TestImageTenantCRUD(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false)
	installCountingFacts(t, "tenant-a")
	for _, principal := range []struct {
		name string
		user *User
	}{
		{"interactive", &User{ID: "owner-a", AZP: "cyclops-cs-spa"}},
		{"cli", &User{ID: "owner-a", AZP: "cua-cli"}},
		{"desktop", &User{ID: "owner-a", AZP: "cua-desktop"}},
		{"user-key", &User{ID: "owner-a", AZP: "ukey-example", PrincipalType: PrincipalTypeUserKey}},
		{"github", &User{ID: "owner-a", PrincipalType: PrincipalTypeGitHubOIDC, AllowedNamespaces: []string{"tenant-a"}}},
	} {
		for _, operation := range []struct{ name, method, suffix, body string }{
			{"create", "POST", "", `{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{"name":"image-a","namespace":"tenant-a"},"spec":{}}`},
			{"get", "GET", "/image-a", ""},
			{"list", "GET", "?limit=50&labelSelector=app%3Dbuilder", ""},
			{"watch", "GET", "?watch=true&resourceVersion=123", ""},
			{"update", "PATCH", "/image-a", `{"metadata":{"resourceVersion":"123"},"spec":{}}`},
			{"delete", "DELETE", "/image-a", ""},
		} {
			for _, namespace := range []string{"tenant-a", "tenant-b"} {
				t.Run(principal.name+"/"+operation.name+"/"+namespace, func(t *testing.T) {
					response, reached := imagePolicyResponse(t, operation.method, "apis/images.cua.ai/v1alpha1/namespaces/"+namespace+"/images"+operation.suffix, operation.body, principal.user)
					want := http.StatusForbidden
					if namespace == "tenant-a" {
						want = http.StatusNoContent
					}
					if response.Code != want || reached != (want == http.StatusNoContent) {
						t.Fatalf("status=%d reached=%v, want %d; %s", response.Code, reached, want, response.Body.String())
					}
				})
			}
		}
	}
}

func TestImageMutationBillingAdmission(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, true)
	installCountingFacts(t, "tenant-a")
	stripe := &countingAdmissionFacts{cacheKey: StripeCardsFactProvider}
	installAdmissionFacts(t, StripeCardsFactProvider, stripe)
	installAdmissionFacts(t, CurrentYearFactProvider, &countingAdmissionFacts{cacheKey: CurrentYearFactProvider, facts: FactSet{"current_year": 2026}})
	installAdmissionFacts(t, CurrentMonthFactProvider, &countingAdmissionFacts{cacheKey: CurrentMonthFactProvider, facts: FactSet{"current_month": 9}})
	for _, principal := range []struct {
		name string
		user *User
	}{
		{"user", &User{ID: "owner-a", AZP: "cua-cli"}},
		{"github", &User{ID: "owner-a", PrincipalType: PrincipalTypeGitHubOIDC, AllowedNamespaces: []string{"tenant-a"}}},
	} {
		for _, hasCard := range []bool{false, true} {
			for _, operation := range []struct{ name, method, suffix, body string }{
				{"create", "POST", "", `{"metadata":{"name":"image-a"},"spec":{}}`},
				{"replace", "PUT", "/image-a", `{"metadata":{"name":"image-a","resourceVersion":"123"},"spec":{}}`},
				{"patch-spec", "PATCH", "/image-a", `{"spec":{"recipe":{"steps":["new-build"]}}}`},
				{"patch-metadata", "PATCH", "/image-a", `{"metadata":{"resourceVersion":"123"}}`},
				{"patch-create", "PATCH", "/new-image?fieldManager=kopf", `{"metadata":{"name":"new-image"},"spec":{}}`},
				{"get", "GET", "/image-a", ""},
				{"list", "GET", "", ""},
				{"delete", "DELETE", "/image-a", ""},
			} {
				cardState := "no-card"
				if hasCard {
					cardState = "card"
				}
				t.Run(principal.name+"/"+cardState+"/"+operation.name, func(t *testing.T) {
					stripe.facts = FactSet{"cards": []map[string]any{}}
					if hasCard {
						stripe.facts = FactSet{"cards": []map[string]any{{"exp_year": 2027, "exp_month": 1}}}
					}
					before := stripe.loads.Load()
					response, reached := imagePolicyResponse(t, operation.method, "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images"+operation.suffix, operation.body, principal.user)
					billed := operation.method == "POST" || operation.method == "PATCH"
					wantStatus, wantLoads := http.StatusNoContent, int64(0)
					if billed {
						wantLoads = 1
					}
					if operation.method == "PUT" || (billed && !hasCard) {
						wantStatus = http.StatusForbidden
					}
					if response.Code != wantStatus || reached != (wantStatus == http.StatusNoContent) || stripe.loads.Load()-before != wantLoads {
						t.Fatalf("status=%d reached=%v billing loads=%d, want %d loads=%d; %s", response.Code, reached, stripe.loads.Load()-before, wantStatus, wantLoads, response.Body.String())
					}
				})
			}
		}
	}
}

func TestImagePatchPreservesControllerMetadata(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false, "admin")
	installCountingFacts(t, "tenant-a")
	user := &User{ID: "admin", AZP: "cua-cli"}
	const collection = "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images"
	for _, body := range []string{
		`{"metadata":{"finalizers":[]}}`,
		`{"metadata":{"finalizers":null}}`,
		`{"metadata":{"finalizers":["attacker.example/finalizer"]}}`,
		`{"metadata":{"ownerReferences":[]}}`,
		`{"metadata":{"ownerReferences":null}}`,
		`{"metadata":{"managedFields":[]}}`,
		`{"metadata":{"managedFields":null}}`,
		`{"metadata":{"deletionTimestamp":null}}`,
		`{"metadata":{"uid":"forged"}}`,
		`{"metadata":{"generation":123}}`,
		`{"metadata":{"annotations":{"images.cua.ai/review":"caller-controlled"}}}`,
		`{"metadata":{"annotations":null}}`,
		`{"metadata":{"labels":null}}`,
		`{"metadata":null}`,
	} {
		for _, method := range []string{"POST", "PATCH"} {
			t.Run(method+"/"+body, func(t *testing.T) {
				path := collection
				if method == "PATCH" {
					path += "/image-a"
				}
				response, reached := imagePolicyResponse(t, method, path, body, user)
				if response.Code != http.StatusForbidden || reached {
					t.Fatalf("status=%d reached=%v; %s", response.Code, reached, response.Body.String())
				}
			})
		}
	}
	for _, body := range []string{
		`{"spec":{}}`,
		`{"metadata":{"name":"image-a","resourceVersion":"123"},"spec":{}}`,
	} {
		response, reached := imagePolicyResponse(t, "PUT", collection+"/image-a", body, user)
		if response.Code != http.StatusForbidden || reached {
			t.Errorf("PUT %s: status=%d reached=%v", body, response.Code, reached)
		}
	}
	response, reached := imagePolicyResponse(t, "PATCH", collection+"/image-a?fieldManager=kopf", `{"metadata":{"name":"image-a","namespace":"tenant-a","resourceVersion":"123"},"spec":{}}`, user)
	if response.Code != http.StatusNoContent || !reached {
		t.Fatalf("safe merge patch: status=%d reached=%v; %s", response.Code, reached, response.Body.String())
	}
}

func TestImagePatchRequiresExactMergePatchMediaType(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false, "admin")
	installCountingFacts(t, "tenant-a")
	for _, contentTypes := range [][]string{
		nil,
		{""},
		{"application/json"},
		{"application/json-patch+json"},
		{"application/strategic-merge-patch+json"},
		{"application/apply-patch+yaml"},
		{"application/apply-patch+cbor"},
		{"application/merge-patch+json; charset=utf-8"},
		{"application/merge-patch+json", "application/apply-patch+yaml"},
		{"application/apply-patch+yaml", "application/merge-patch+json"},
		{"application/merge-patch+json, application/apply-patch+yaml"},
	} {
		t.Run(strings.Join(contentTypes, ","), func(t *testing.T) {
			response, reached := imagePolicyResponseWithContentTypes(t, "PATCH", "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images/image-a?fieldManager=kopf&force=true", `{"apiVersion":"images.cua.ai/v1alpha1","kind":"Image","metadata":{"name":"image-a"},"spec":{}}`, &User{ID: "admin", AZP: "cua-cli"}, contentTypes...)
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status=%d reached=%v; %s", response.Code, reached, response.Body.String())
			}
		})
	}
}

func TestImagePolicyBoundaries(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, false, "admin")
	facts := installCountingFacts(t, "tenant-a")
	const collection = "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images"
	for _, testCase := range []struct{ name, method, path, body string }{
		{"cluster-list", "GET", "apis/images.cua.ai/v1alpha1/images", ""},
		{"wrong-version", "GET", "apis/images.cua.ai/v1/namespaces/tenant-a/images", ""},
		{"wrong-group", "GET", "apis/images.cua.ai.evil/v1alpha1/namespaces/tenant-a/images", ""},
		{"wrong-resource", "GET", collection + "builds", ""},
		{"delete-collection", "DELETE", collection, ""},
		{"patch-collection", "PATCH", collection, `{}`},
		{"put-collection", "PUT", collection, `{}`},
		{"post-item", "POST", collection + "/image-a", `{}`},
		{"status-read", "GET", collection + "/image-a/status", ""},
		{"status-patch", "PATCH", collection + "/image-a/status", `{"status":{}}`},
		{"status-put", "PUT", collection + "/image-a/status", `{"status":{}}`},
		{"encoded-status", "PATCH", collection + "/image-a%2Fstatus", `{}`},
		{"encoded-parent", "GET", collection + "/%2e%2e", ""},
		{"encoded-dot", "GET", collection + "/%2e", ""},
		{"trailing-slash", "GET", collection + "/", ""},
		{"encoded-cross-namespace", "GET", "apis/images.cua.ai/v1alpha1/namespaces/tenant-b%2Fimages/images", ""},
		{"finalizers", "PATCH", collection + "/image-a/finalizers", `{}`},
		{"create-status", "POST", collection, `{"spec":{},"status":{"phase":"Ready"}}`},
		{"patch-status", "PATCH", collection + "/image-a", `{"status":{"phase":"Ready"}}`},
		{"put-status", "PUT", collection + "/image-a", `{"spec":{},"status":{}}`},
		{"namespace-mismatch", "PATCH", collection + "/image-a", `{"metadata":{"namespace":"tenant-b"}}`},
		{"wrong-kind", "POST", collection, `{"kind":"Pod","spec":{}}`},
		{"wrong-api-version", "POST", collection, `{"apiVersion":"v1","spec":{}}`},
		{"unknown-root", "PATCH", collection + "/image-a", `{"data":{}}`},
		{"invalid-spec-shape", "PATCH", collection + "/image-a", `{"spec":[]}`},
		{"malformed-json", "POST", collection, `{`},
		{"json-patch-status", "PATCH", collection + "/image-a", `[{"op":"add","path":"/status","value":{}}]`},
		{"secrets", "GET", "api/v1/namespaces/tenant-a/secrets", ""},
		{"jobs", "POST", "apis/batch/v1/namespaces/tenant-a/jobs", `{}`},
		{"pvc", "POST", "api/v1/namespaces/tenant-a/persistentvolumeclaims", `{}`},
		{"snapshot", "DELETE", "apis/snapshot.storage.k8s.io/v1/namespaces/tenant-a/volumesnapshots/snapshot-a", ""},
	} {
		t.Run(testCase.name, func(t *testing.T) {
			response, reached := imagePolicyResponse(t, testCase.method, testCase.path, testCase.body, &User{ID: "admin", AZP: "cyclops-cs-spa"})
			if response.Code != http.StatusForbidden || reached {
				t.Fatalf("status=%d reached=%v; %s", response.Code, reached, response.Body.String())
			}
		})
	}
	for _, user := range []*User{nil, {AZP: "cyclops-cs-spa"}, {ID: "owner-a", AZP: "key-tenant-a", Namespace: "tenant-a"}, {ID: "owner-a", AZP: "unrelated"}} {
		response, reached := imagePolicyResponse(t, "GET", collection, "", user)
		if response.Code != http.StatusForbidden || reached {
			t.Fatalf("principal %+v: status=%d reached=%v", user, response.Code, reached)
		}
	}
	facts.unreachable = "tenant-a"
	response, reached := imagePolicyResponse(t, "GET", collection, "", &User{ID: "owner-a", AZP: "cua-cli"})
	if response.Code < 500 || reached {
		t.Fatalf("unavailable ownership: status=%d reached=%v", response.Code, reached)
	}
}

func TestImageCreationRequiresBillingAdmission(t *testing.T) {
	t.Cleanup(resetFlagsCache)
	setCardAdmissionFlags(t, true)
	installCountingFacts(t, "tenant-a")
	stripe := &countingAdmissionFacts{cacheKey: StripeCardsFactProvider, facts: FactSet{"cards": []map[string]any{}}}
	installAdmissionFacts(t, StripeCardsFactProvider, stripe)
	installAdmissionFacts(t, CurrentYearFactProvider, &countingAdmissionFacts{cacheKey: CurrentYearFactProvider, facts: FactSet{"current_year": 2026}})
	installAdmissionFacts(t, CurrentMonthFactProvider, &countingAdmissionFacts{cacheKey: CurrentMonthFactProvider, facts: FactSet{"current_month": 9}})
	const path = "apis/images.cua.ai/v1alpha1/namespaces/tenant-a/images"
	user := &User{ID: "owner-a", AZP: "cua-cli"}
	response, reached := imagePolicyResponse(t, "POST", path, `{"spec":{}}`, user)
	if response.Code != http.StatusForbidden || reached || stripe.loads.Load() != 1 {
		t.Fatalf("without card: status=%d reached=%v stripe loads=%d", response.Code, reached, stripe.loads.Load())
	}
	stripe.facts = FactSet{"cards": []map[string]any{{"exp_year": 2027, "exp_month": 1}}}
	response, reached = imagePolicyResponse(t, "POST", path, `{"spec":{}}`, user)
	if response.Code != http.StatusNoContent || !reached {
		t.Fatalf("with card: status=%d reached=%v; %s", response.Code, reached, response.Body.String())
	}
}
