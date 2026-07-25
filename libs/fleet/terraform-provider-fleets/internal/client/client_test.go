package client

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"reflect"
	"sync"
	"testing"
)

func TestPoolLifecycle(t *testing.T) {
	t.Parallel()

	var mu sync.Mutex
	var requests []string
	var created Pool
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		requests = append(requests, r.Method+" "+r.URL.Path)
		mu.Unlock()

		if r.URL.Path == "/token" {
			if id, secret, ok := r.BasicAuth(); !ok || id != "client" || secret != "secret" {
				t.Fatalf("unexpected OAuth credentials: %q %q %v", id, secret, ok)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "token", "expires_in": 300})
			return
		}
		if got := r.Header.Get("Authorization"); got != "Bearer token" {
			t.Fatalf("unexpected authorization header %q", got)
		}

		switch r.Method + " " + r.URL.Path {
		case "POST /api/namespaces":
			w.WriteHeader(http.StatusCreated)
		case "POST /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools":
			if err := json.NewDecoder(r.Body).Decode(&created); err != nil {
				t.Fatal(err)
			}
			w.WriteHeader(http.StatusCreated)
		case "GET /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo":
			created.Metadata.Namespace = "demo"
			created.Status = PoolStatus{Phase: "Ready", AvailableCount: 2}
			_ = json.NewEncoder(w).Encode(created)
		case "PATCH /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo":
			if got := r.Header.Get("Content-Type"); got != "application/merge-patch+json" {
				t.Fatalf("unexpected patch content type %q", got)
			}
			w.WriteHeader(http.StatusOK)
		case "DELETE /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo",
			"DELETE /api/namespaces/demo":
			w.WriteHeader(http.StatusNoContent)
		default:
			http.Error(w, "unexpected request", http.StatusNotFound)
		}
	}))
	defer server.Close()

	api, err := New(Config{Endpoint: server.URL, ClientID: "client", ClientSecret: "secret", TokenURL: server.URL + "/token"})
	if err != nil {
		t.Fatal(err)
	}
	pool := Pool{Metadata: PoolMetadata{Name: "demo"}, Spec: PoolSpec{
		Replicas: 2,
		Template: PoolTemplate{ContainerDiskImage: "example/image:latest", ImagePullSecret: "ecr-credentials", CPUCores: 4, Memory: "8Gi"},
		Services: []Service{{Name: "ssh", TargetPort: 22, Protocol: "TCP"}},
	}}
	ctx := context.Background()
	if err := api.CreatePool(ctx, pool); err != nil {
		t.Fatal(err)
	}
	if created.APIVersion != "cua.ai/v1" || created.Kind != "OSGymWorkspacePool" {
		t.Fatalf("unexpected resource type: %#v", created)
	}
	if !reflect.DeepEqual(created.Metadata.Labels, map[string]string{"cua.ai/pool": "demo"}) {
		t.Fatalf("unexpected labels: %#v", created.Metadata.Labels)
	}
	got, err := api.GetPool(ctx, "demo", "demo")
	if err != nil {
		t.Fatal(err)
	}
	if got.Status.Phase != "Ready" || got.Status.AvailableCount != 2 {
		t.Fatalf("unexpected status: %#v", got.Status)
	}
	if err := api.UpdatePool(ctx, "demo", "demo", pool.Spec); err != nil {
		t.Fatal(err)
	}
	if err := api.DeletePool(ctx, "demo", "demo"); err != nil {
		t.Fatal(err)
	}

	mu.Lock()
	defer mu.Unlock()
	want := []string{
		"POST /token",
		"POST /api/namespaces",
		"POST /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools",
		"GET /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo",
		"PATCH /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo",
		"DELETE /api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo",
		"DELETE /api/namespaces/demo",
	}
	if !reflect.DeepEqual(requests, want) {
		t.Fatalf("requests mismatch\n got: %#v\nwant: %#v", requests, want)
	}
}

func TestCreatePoolReusesExistingNamespace(t *testing.T) {
	t.Parallel()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/namespaces" {
			http.Error(w, `{"error":"already exists"}`, http.StatusConflict)
			return
		}
		w.WriteHeader(http.StatusCreated)
	}))
	defer server.Close()

	api, err := New(Config{Endpoint: server.URL, AccessToken: "token"})
	if err != nil {
		t.Fatal(err)
	}
	if err := api.CreatePool(context.Background(), Pool{Metadata: PoolMetadata{Name: "demo"}}); err != nil {
		t.Fatal(err)
	}
}
