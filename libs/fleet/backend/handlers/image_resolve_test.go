package handlers

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/imageresolve"
)

type fakeImageResolver struct {
	ref, runtime string
	result       imageresolve.Result
	err          error
}

func (f *fakeImageResolver) Resolve(_ context.Context, ref, runtime string) (imageresolve.Result, error) {
	f.ref, f.runtime = ref, runtime
	return f.result, f.err
}

func resolveRequest(query string) *http.Request {
	return withUser(httptest.NewRequest(http.MethodGet, "/api/images/resolve"+query, nil), &auth.User{ID: "user"})
}

func TestResolveImageReturnsThePin(t *testing.T) {
	resolver := &fakeImageResolver{result: imageresolve.Result{
		Ref: "ghcr.io/trycua/linux:24.04", ResolvedRef: "ghcr.io/trycua/linux:24.04-disk",
		PinnedRef: "ghcr.io/trycua/linux@sha256:abc", Digest: "sha256:abc",
		Variant: "containerdisk", VariantSource: "tag", MediaType: "application/vnd.oci.image.index.v1+json",
	}}
	w := httptest.NewRecorder()
	Handlers{ImageResolver: resolver}.ResolveImage(w, resolveRequest("?ref=ghcr.io%2Ftrycua%2Flinux%3A24.04&runtime=kubevirt"))

	if w.Code != http.StatusOK || resolver.ref != "ghcr.io/trycua/linux:24.04" || resolver.runtime != "kubevirt" {
		t.Fatalf("status=%d resolver=%+v", w.Code, resolver)
	}
	if w.Header().Get("Cache-Control") != "private, no-store" {
		t.Fatalf("cache-control = %q", w.Header().Get("Cache-Control"))
	}
	var body map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"ref", "resolvedRef", "pinnedRef", "digest", "variant", "variantSource", "platformDigest", "mediaType"} {
		if _, ok := body[key]; !ok {
			t.Errorf("response is missing %q: %s", key, w.Body.String())
		}
	}
}

func TestResolveImageMapsErrors(t *testing.T) {
	for _, testCase := range []struct {
		err  error
		want int
	}{
		{imageresolve.ErrInvalidReference, http.StatusBadRequest},
		{fmt.Errorf("%w: runtime", imageresolve.ErrUnsupported), http.StatusBadRequest},
		{imageresolve.ErrNotFound, http.StatusNotFound},
		{imageresolve.ErrUnauthorized, http.StatusUnprocessableEntity},
		{imageresolve.ErrRegistryNotAllowed, http.StatusUnprocessableEntity},
		{fmt.Errorf("%w: 500", imageresolve.ErrUpstream), http.StatusBadGateway},
	} {
		w := httptest.NewRecorder()
		Handlers{ImageResolver: &fakeImageResolver{err: testCase.err}}.ResolveImage(w, resolveRequest("?ref=x"))
		if w.Code != testCase.want {
			t.Errorf("%v: status = %d, want %d", testCase.err, w.Code, testCase.want)
		}
	}

	w := httptest.NewRecorder()
	Handlers{ImageResolver: &fakeImageResolver{}}.ResolveImage(w, resolveRequest(""))
	if w.Code != http.StatusBadRequest {
		t.Errorf("missing ref: status = %d", w.Code)
	}
	w = httptest.NewRecorder()
	Handlers{}.ResolveImage(w, resolveRequest("?ref=x"))
	if w.Code != http.StatusServiceUnavailable {
		t.Errorf("unconfigured: status = %d", w.Code)
	}
}
