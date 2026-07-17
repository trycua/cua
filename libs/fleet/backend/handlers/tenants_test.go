package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
)

type roundTripperFunc func(*http.Request) (*http.Response, error)

func (f roundTripperFunc) RoundTrip(r *http.Request) (*http.Response, error) {
	return f(r)
}

func TestCreateNamespace_FirstUseCreatesPersonalTenant(t *testing.T) {
	const userSub = "test-uuid"
	desired := desiredPersonalTenant(context.Background(), userSub)
	tenantPath := capsuleTenantCollectionPath + "/" + desired.Metadata.Name
	tenantGetCount := 0
	namespacePostCount := 0

	fk := newFakeK8sHandler(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == tenantPath:
			tenantGetCount++
			if tenantGetCount == 1 {
				w.WriteHeader(http.StatusNotFound)
				_, _ = w.Write([]byte(`{"kind":"Status","reason":"NotFound"}`))
				return
			}
			if tenantGetCount == 2 {
				w.WriteHeader(http.StatusOK)
				_, _ = w.Write([]byte(pendingPersonalTenantResponse(userSub)))
				return
			}
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(existingPersonalTenantResponse(userSub)))
		case r.Method == http.MethodPost && r.URL.Path == capsuleTenantCollectionPath:
			w.WriteHeader(http.StatusCreated)
			_, _ = w.Write([]byte(existingPersonalTenantResponse(userSub)))
		case r.Method == http.MethodPost && r.URL.Path == "/api/v1/namespaces":
			namespacePostCount++
			if namespacePostCount == 1 {
				status, _ := json.Marshal(map[string]any{
					"kind":    "Status",
					"status":  "Failure",
					"code":    http.StatusBadRequest,
					"message": "admission webhook denied the request: can not assign the desired namespace to a non-owned Tenant",
				})
				w.WriteHeader(http.StatusBadRequest)
				_, _ = w.Write(status)
				return
			}
			if namespacePostCount == 2 {
				status, _ := json.Marshal(map[string]any{
					"kind":   "Status",
					"status": "Failure",
					"code":   http.StatusInternalServerError,
					"message": "admission failed: Tenant.capsule.clastix.io \"" +
						desired.Metadata.Name + "\" not found",
				})
				w.WriteHeader(http.StatusInternalServerError)
				_, _ = w.Write(status)
				return
			}
			w.WriteHeader(http.StatusCreated)
			_, _ = w.Write([]byte(nsCreatedResponse("my-workspace")))
		case r.Method == http.MethodGet && strings.HasPrefix(
			r.URL.Path, "/apis/rbac.authorization.k8s.io/v1/namespaces/",
		):
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(`{"items":[]}`))
		default:
			t.Errorf("unexpected k8s request: %s %s", r.Method, r.URL.Path)
			w.WriteHeader(http.StatusInternalServerError)
		}
	})
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	)
	r = withUser(r, &auth.User{ID: userSub})
	w := httptest.NewRecorder()

	h.CreateNamespace(w, r)

	if w.Code != http.StatusCreated {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusCreated, w.Body.String())
	}
	if len(fk.requests) != 8 {
		t.Fatalf("request count = %d, want 8", len(fk.requests))
	}
	wantSequence := []struct {
		method string
		path   string
	}{
		{http.MethodGet, tenantPath},
		{http.MethodPost, capsuleTenantCollectionPath},
		{http.MethodGet, tenantPath},
		{http.MethodGet, tenantPath},
		{http.MethodPost, "/api/v1/namespaces"},
		{http.MethodPost, "/api/v1/namespaces"},
		{http.MethodPost, "/api/v1/namespaces"},
		{http.MethodGet, "/apis/rbac.authorization.k8s.io/v1/namespaces/my-workspace/rolebindings"},
	}
	for i, want := range wantSequence {
		got := fk.requests[i]
		if got.method != want.method || got.path != want.path {
			t.Fatalf("request[%d] = %s %s, want %s %s", i, got.method, got.path, want.method, want.path)
		}
	}

	for i := 0; i < 4; i++ {
		request := fk.requests[i]
		if got := request.headers.Get("Authorization"); got != "Bearer fake-sa-token" {
			t.Fatalf("tenant request[%d] Authorization = %q", i, got)
		}
		if got := request.headers.Get("Impersonate-User"); got != "" {
			t.Fatalf("tenant request[%d] Impersonate-User = %q, want empty", i, got)
		}
		if got := request.headers.Get("Impersonate-Group"); got != "" {
			t.Fatalf("tenant request[%d] Impersonate-Group = %q, want empty", i, got)
		}
	}

	var created capsuleTenant
	if err := json.Unmarshal(fk.requests[1].body, &created); err != nil {
		t.Fatalf("decode Tenant create body: %v", err)
	}
	if created.APIVersion != "capsule.clastix.io/v1beta2" || created.Kind != "Tenant" {
		t.Fatalf("unexpected Tenant type: %s %s", created.APIVersion, created.Kind)
	}
	if err := validatePersonalTenant(created, desired); err != nil {
		t.Fatalf("Tenant create body mismatch: %v; body = %s", err, fk.requests[1].body)
	}
	wantRoles := []string{
		"admin",
		"capsule-namespace-deleter",
		"capsule-tenant-cluster-resources",
	}
	if got := created.Spec.Owners[0].ClusterRoles; !reflect.DeepEqual(got, wantRoles) {
		t.Fatalf("Tenant clusterRoles = %#v, want literal roles %#v", got, wantRoles)
	}

	namespaceRequest := fk.requests[6]
	if got := namespaceRequest.headers.Get("Impersonate-User"); got != "oidc:"+userSub {
		t.Fatalf("namespace Impersonate-User = %q, want %q", got, "oidc:"+userSub)
	}
}

func TestValidatePersonalTenant_RejectsRoleAndOwnerDrift(t *testing.T) {
	desired := desiredPersonalTenant(context.Background(), "test-uuid")
	tests := []struct {
		name   string
		mutate func(*capsuleTenant)
	}{
		{
			name: "extra role",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners[0].ClusterRoles = append(
					tenant.Spec.Owners[0].ClusterRoles, "cluster-admin",
				)
			},
		},
		{
			name: "missing role",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners[0].ClusterRoles = tenant.Spec.Owners[0].ClusterRoles[:2]
			},
		},
		{
			name: "substituted role",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners[0].ClusterRoles[1] = "cluster-admin"
			},
		},
		{
			name: "duplicate role",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners[0].ClusterRoles[1] = "admin"
			},
		},
		{
			name: "extra owner",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners = append(tenant.Spec.Owners, tenant.Spec.Owners[0])
			},
		},
		{
			name: "dynamic owner selector",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Permissions = &capsuleTenantPermissions{
					MatchOwners: []json.RawMessage{json.RawMessage(`{"matchLabels":{"team":"all"}}`)},
				}
			},
		},
		{
			name: "additional role binding",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.AdditionalRoleBindings = []json.RawMessage{
					json.RawMessage(`{"clusterRoleName":"cluster-admin"}`),
				}
			},
		},
		{
			name: "owner proxy settings",
			mutate: func(tenant *capsuleTenant) {
				tenant.Spec.Owners[0].ProxySettings = []json.RawMessage{
					json.RawMessage(`{"kind":"Nodes","operations":["List"]}`),
				}
			},
		},
		{
			name: "extra effective owner",
			mutate: func(tenant *capsuleTenant) {
				tenant.Status = &capsuleTenantStatus{Owners: []capsuleTenantOwner{
					tenant.Spec.Owners[0],
					{Kind: "Group", Name: "oidc:someone-else"},
				}}
			},
		},
		{
			name: "wrong effective owner",
			mutate: func(tenant *capsuleTenant) {
				tenant.Status = &capsuleTenantStatus{Owners: []capsuleTenantOwner{{
					Kind:         "User",
					Name:         "oidc:someone-else",
					ClusterRoles: append([]string(nil), personalTenantClusterRoles[:]...),
				}}}
			},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			existing := desiredPersonalTenant(context.Background(), "test-uuid")
			tc.mutate(&existing)
			if err := validatePersonalTenant(existing, desired); err == nil {
				t.Fatal("validatePersonalTenant() accepted mismatched Tenant")
			}
		})
	}
}

func TestValidatePersonalTenant_RejectsPrivilegeFieldsFromKubernetesJSON(t *testing.T) {
	desired := desiredPersonalTenant(context.Background(), "test-uuid")
	const base = `{
		"apiVersion":"capsule.clastix.io/v1beta2",
		"kind":"Tenant",
		"metadata":{"name":"user-test-uuid"},
		"spec":{
			"owners":[{
				"kind":"User",
				"name":"oidc:test-uuid",
				"clusterRoles":["admin","capsule-namespace-deleter","capsule-tenant-cluster-resources"]
			}]
			PRIVILEGE_FIELD
		}
	}`
	fields := []string{
		`,"permissions":{"matchOwners":[{}]}`,
		`,"additionalRoleBindings":[{"clusterRoleName":"cluster-admin"}]`,
	}
	for _, field := range fields {
		var existing capsuleTenant
		raw := strings.Replace(base, "PRIVILEGE_FIELD", field, 1)
		if err := json.Unmarshal([]byte(raw), &existing); err != nil {
			t.Fatalf("decode test Tenant: %v", err)
		}
		if err := validatePersonalTenant(existing, desired); err == nil {
			t.Fatalf("accepted Kubernetes Tenant privilege field %s", field)
		}
	}
}

func TestValidatePersonalTenant_AcceptsEquivalentRoleOrderAndEmptyDynamicOwners(t *testing.T) {
	desired := desiredPersonalTenant(context.Background(), "test-uuid")
	existing := desiredPersonalTenant(context.Background(), "test-uuid")
	existing.Spec.Owners[0].ClusterRoles = []string{
		"capsule-tenant-cluster-resources",
		"admin",
		"capsule-namespace-deleter",
	}
	existing.Spec.Permissions = &capsuleTenantPermissions{MatchOwners: []json.RawMessage{}}
	existing.Status = &capsuleTenantStatus{Owners: []capsuleTenantOwner{
		existing.Spec.Owners[0],
	}}

	if err := validatePersonalTenant(existing, desired); err != nil {
		t.Fatalf("validatePersonalTenant() rejected equivalent Tenant: %v", err)
	}
}

func TestPersonalTenantAdmissionCacheLag_OnlyMatchesExactCapsuleErrors(t *testing.T) {
	for _, resource := range []string{"Tenant.capsule.clastix.io", "tenants.capsule.clastix.io"} {
		body := []byte(`{"kind":"Status","status":"Failure","code":500,"message":"` +
			resource + ` \"user-test-uuid\" not found"}`)
		if !isPersonalTenantAdmissionCacheLag(http.StatusInternalServerError, body, "user-test-uuid") {
			t.Fatalf("expected %s cache miss to be retryable", resource)
		}
	}
	body := []byte(`{"kind":"Status","status":"Failure","code":500,"message":"Tenant.capsule.clastix.io \"user-test-uuid\" not found"}`)
	if isPersonalTenantAdmissionCacheLag(http.StatusInternalServerError, body, "user-someone-else") {
		t.Fatal("matched a different Tenant name")
	}
	if isPersonalTenantAdmissionCacheLag(http.StatusBadGateway, body, "user-test-uuid") {
		t.Fatal("matched a non-500 response")
	}
	if isPersonalTenantAdmissionCacheLag(
		http.StatusInternalServerError,
		[]byte(`{"kind":"Status","status":"Failure","code":500,"message":"another admission webhook failed"}`),
		"user-test-uuid",
	) {
		t.Fatal("matched an unrelated admission failure")
	}
	for _, malformed := range [][]byte{
		[]byte(`Tenant.capsule.clastix.io "user-test-uuid" not found`),
		[]byte(`{"message":"Tenant.capsule.clastix.io \"user-test-uuid\" not found"}`),
		[]byte(`{"kind":"Status","status":"Failure","code":500`),
	} {
		if isPersonalTenantAdmissionCacheLag(
			http.StatusInternalServerError, malformed, "user-test-uuid",
		) {
			t.Fatalf("matched malformed or non-Status body: %q", malformed)
		}
	}
	nonOwned := []byte(`{"kind":"Status","status":"Failure","code":400,"message":"admission webhook denied the request: can not assign the desired namespace to a non-owned Tenant"}`)
	if !isPersonalTenantAdmissionCacheLag(http.StatusBadRequest, nonOwned, "user-test-uuid") {
		t.Fatal("expected exact non-owned Tenant cache lag to be retryable")
	}
	if isPersonalTenantAdmissionCacheLag(http.StatusInternalServerError, nonOwned, "user-test-uuid") {
		t.Fatal("matched non-owned Tenant message with mismatched HTTP status")
	}
}

func TestK8sTenantLifecycleBypassesCapsuleProxy(t *testing.T) {
	const userSub = "test-uuid"
	direct := newFakeK8s(http.StatusOK, existingPersonalTenantResponse(userSub))
	defer direct.server.Close()
	proxy := newFakeK8s(http.StatusCreated, nsCreatedResponse("my-workspace"))
	defer proxy.server.Close()
	overrideK8sClientTargets(
		direct.server.Client(), proxy.server.URL, direct.server.URL, "fake-sa-token",
	)

	h := Handlers{}
	if _, err := h.ensurePersonalTenant(context.Background(), userSub); err != nil {
		t.Fatalf("ensurePersonalTenant() error = %v", err)
	}
	resp, err := h.k8sImpersonate(
		context.Background(), http.MethodPost, "/api/v1/namespaces", nil, userSub,
	)
	if err != nil {
		t.Fatalf("k8sImpersonate() error = %v", err)
	}
	resp.Body.Close()

	if len(direct.requests) != 1 || direct.requests[0].path != capsuleTenantCollectionPath+"/user-"+userSub {
		t.Fatalf("direct apiserver requests = %+v, want only Tenant GET", direct.requests)
	}
	if got := direct.requests[0].headers.Get("Impersonate-User"); got != "" {
		t.Fatalf("direct Tenant request Impersonate-User = %q, want empty", got)
	}
	if len(proxy.requests) != 1 || proxy.requests[0].path != "/api/v1/namespaces" {
		t.Fatalf("Capsule Proxy requests = %+v, want only Namespace POST", proxy.requests)
	}
	if got := proxy.requests[0].headers.Get("Impersonate-User"); got != "oidc:"+userSub {
		t.Fatalf("proxy Namespace request Impersonate-User = %q", got)
	}
}

func TestCreateNamespace_CancellationBoundsInformerRetryRequest(t *testing.T) {
	const userSub = "test-uuid"
	direct := newFakeK8s(http.StatusOK, existingPersonalTenantResponse(userSub))
	defer direct.server.Close()

	var posts atomic.Int32
	started := make(chan struct{}, 1)
	baseTransport := direct.server.Client().Transport
	client := &http.Client{Transport: roundTripperFunc(func(r *http.Request) (*http.Response, error) {
		if r.URL.Host == "capsule-proxy.test" {
			posts.Add(1)
			started <- struct{}{}
			<-r.Context().Done()
			return nil, r.Context().Err()
		}
		return baseTransport.RoundTrip(r)
	})}
	overrideK8sClientTargets(
		client, "http://capsule-proxy.test", direct.server.URL, "fake-sa-token",
	)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	).WithContext(ctx)
	r = withUser(r, &auth.User{ID: userSub})
	w := httptest.NewRecorder()
	done := make(chan struct{})
	go func() {
		(Handlers{}).CreateNamespace(w, r)
		close(done)
	}()

	select {
	case <-started:
		cancel()
	case <-time.After(2 * time.Second):
		cancel()
		select {
		case <-done:
		case <-time.After(time.Second):
		}
		t.Fatal("namespace POST did not start")
	}
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("handler did not stop after request cancellation")
	}

	if w.Code != http.StatusBadGateway {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadGateway, w.Body.String())
	}
	if got := posts.Load(); got != 1 {
		t.Fatalf("namespace POST count = %d, want 1", got)
	}
}

func TestEnsurePersonalTenant_AlreadyExists(t *testing.T) {
	const userSub = "test-uuid"
	fk := newFakeK8s(http.StatusOK, existingPersonalTenantResponse(userSub))
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	if _, err := (Handlers{}).ensurePersonalTenant(context.Background(), userSub); err != nil {
		t.Fatalf("ensurePersonalTenant() error = %v", err)
	}
	if len(fk.requests) != 1 {
		t.Fatalf("request count = %d, want 1 GET", len(fk.requests))
	}
	if got := fk.requests[0].method; got != http.MethodGet {
		t.Fatalf("method = %q, want GET", got)
	}
	if got := fk.requests[0].headers.Get("Impersonate-User"); got != "" {
		t.Fatalf("Impersonate-User = %q, want empty", got)
	}
}

func TestCreateNamespace_PendingTenantStatusStopsBeforeNamespace(t *testing.T) {
	const userSub = "test-uuid"
	var gets atomic.Int32
	secondGet := make(chan struct{}, 1)
	direct := newFakeK8sHandler(func(w http.ResponseWriter, _ *http.Request) {
		if gets.Add(1) == 2 {
			secondGet <- struct{}{}
		}
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(pendingPersonalTenantResponse(userSub)))
	})
	defer direct.server.Close()
	proxy := newFakeK8s(http.StatusInternalServerError, "unexpected namespace request")
	defer proxy.server.Close()
	overrideK8sClientTargets(
		direct.server.Client(), proxy.server.URL, direct.server.URL, "fake-sa-token",
	)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	).WithContext(ctx)
	r = withUser(r, &auth.User{ID: userSub})
	w := httptest.NewRecorder()
	done := make(chan struct{})
	go func() {
		(Handlers{}).CreateNamespace(w, r)
		close(done)
	}()

	select {
	case <-secondGet:
		cancel()
	case <-time.After(2 * time.Second):
		cancel()
		select {
		case <-done:
		case <-time.After(time.Second):
		}
		t.Fatal("Tenant readiness was not polled")
	}
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("handler did not stop after readiness wait cancellation")
	}

	if w.Code != http.StatusBadGateway {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadGateway, w.Body.String())
	}
	if len(proxy.requests) != 0 {
		t.Fatalf("namespace requests = %+v, want none before status.owners is ready", proxy.requests)
	}
}

func TestEnsurePersonalTenant_CreateConflictRevalidatesWinner(t *testing.T) {
	const userSub = "test-uuid"
	getCount := 0
	fk := newFakeK8sHandler(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet:
			getCount++
			if getCount == 1 {
				w.WriteHeader(http.StatusNotFound)
				return
			}
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(existingPersonalTenantResponse(userSub)))
		case http.MethodPost:
			w.WriteHeader(http.StatusConflict)
		default:
			t.Errorf("unexpected method: %s", r.Method)
			w.WriteHeader(http.StatusInternalServerError)
		}
	})
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	if _, err := (Handlers{}).ensurePersonalTenant(context.Background(), userSub); err != nil {
		t.Fatalf("ensurePersonalTenant() error = %v", err)
	}
	if len(fk.requests) != 3 {
		t.Fatalf("request count = %d, want GET, POST, GET", len(fk.requests))
	}
	if fk.requests[0].method != http.MethodGet ||
		fk.requests[1].method != http.MethodPost ||
		fk.requests[2].method != http.MethodGet {
		t.Fatalf("unexpected request sequence: %+v", fk.requests)
	}
}

func TestCreateNamespace_MismatchedTenantFailsClosed(t *testing.T) {
	existing := desiredPersonalTenant(context.Background(), "test-uuid")
	existing.Spec.Owners[0].Name = "oidc:someone-else"
	body, err := json.Marshal(existing)
	if err != nil {
		t.Fatal(err)
	}

	fk := newFakeK8s(http.StatusOK, string(body))
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	)
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()
	h.CreateNamespace(w, r)

	if w.Code != http.StatusBadGateway {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadGateway, w.Body.String())
	}
	if len(fk.requests) != 1 || fk.requests[0].method != http.MethodGet {
		t.Fatalf("requests = %+v, want only Tenant GET", fk.requests)
	}
	if strings.Contains(w.Body.String(), "someone-else") {
		t.Fatalf("response exposed Tenant details: %s", w.Body.String())
	}
}

func TestCreateNamespace_TenantBackendFailureStopsBeforeNamespace(t *testing.T) {
	fk := newFakeK8s(http.StatusInternalServerError, "backend unavailable")
	defer fk.server.Close()
	overrideK8sClient(fk.server.Client(), fk.server.URL, "fake-sa-token")

	h := Handlers{}
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	)
	r = withUser(r, &auth.User{ID: "test-uuid"})
	w := httptest.NewRecorder()
	h.CreateNamespace(w, r)

	if w.Code != http.StatusBadGateway {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusBadGateway, w.Body.String())
	}
	if len(fk.requests) != 1 || fk.requests[0].method != http.MethodGet {
		t.Fatalf("requests = %+v, want only Tenant GET", fk.requests)
	}
	if got, want := w.Body.String(), "{\"error\":\"could not ensure personal tenant\"}\n"; got != want {
		t.Fatalf("body = %q, want bounded generic error %q", got, want)
	}
}

func TestCreateNamespace_EmptySubjectIsUnauthorized(t *testing.T) {
	h := Handlers{}
	r := httptest.NewRequest(
		http.MethodPost, "/api/namespaces", strings.NewReader(`{"name":"my-workspace"}`),
	)
	r = withUser(r, &auth.User{})
	w := httptest.NewRecorder()

	h.CreateNamespace(w, r)

	if w.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d; body = %s", w.Code, http.StatusUnauthorized, w.Body.String())
	}
}
