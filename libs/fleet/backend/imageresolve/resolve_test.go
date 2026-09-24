package imageresolve

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func digestOf(body string) string {
	sum := sha256.Sum256([]byte(body))
	return "sha256:" + hex.EncodeToString(sum[:])
}

func TestParseReference(t *testing.T) {
	digest := "sha256:" + strings.Repeat("a", 64)
	for _, testCase := range []struct {
		in   string
		want string
	}{
		{"python:3.12-slim", "docker.io/library/python:3.12-slim"},
		{"python", "docker.io/library/python:latest"},
		{"acme/app:1", "docker.io/acme/app:1"},
		{"index.docker.io/library/redis:7", "docker.io/library/redis:7"},
		{"ghcr.io/trycua/linux:24.04", "ghcr.io/trycua/linux:24.04"},
		{"ghcr.io/trycua/linux@" + digest, "ghcr.io/trycua/linux@" + digest},
		{"ghcr.io/trycua/linux:24.04@" + digest, "ghcr.io/trycua/linux:24.04@" + digest},
		{"localhost:5000/app:1", "localhost:5000/app:1"},
		{"GHCR.io/trycua/linux:24.04", "ghcr.io/trycua/linux:24.04"},
	} {
		got, err := ParseReference(testCase.in)
		if err != nil || got.String() != testCase.want {
			t.Errorf("ParseReference(%q) = %q, %v; want %q", testCase.in, got.String(), err, testCase.want)
		}
	}
	for _, bad := range []string{"", "a b", "ghcr.io/Trycua/linux:1", "ghcr.io/x@sha256:short", "ghcr.io/x:bad/tag", strings.Repeat("a", 1100)} {
		if _, err := ParseReference(bad); !errors.Is(err, ErrInvalidReference) {
			t.Errorf("ParseReference(%q) err = %v; want ErrInvalidReference", bad, err)
		}
	}
}

func TestVariantRefMapsCanonicalImages(t *testing.T) {
	for _, testCase := range []struct {
		in, runtime, want string
		wantErr           bool
	}{
		{"ghcr.io/trycua/linux:24.04", "kubevirt", "ghcr.io/trycua/linux:24.04-disk", false},
		{"ghcr.io/trycua/linux:24.04-disk", "kubevirt", "ghcr.io/trycua/linux:24.04-disk", false},
		{"ghcr.io/trycua/linux:24.04-disk", "gvisor", "ghcr.io/trycua/linux:24.04", false},
		{"ghcr.io/trycua/linux:24.04", "gvisor", "ghcr.io/trycua/linux:24.04", false},
		{"ghcr.io/trycua/linux:24.04", "", "ghcr.io/trycua/linux:24.04", false},
		{"ghcr.io/trycua/windows:2022-disk", "kubevirt", "ghcr.io/trycua/windows:2022-disk", false},
		{"ghcr.io/trycua/windows:2022-disk", "gvisor", "", true},
		{"python:3.12", "kubevirt", "docker.io/library/python:3.12", false},
		{"ghcr.io/trycua/linux:24.04", "docker", "", true},
	} {
		ref, err := ParseReference(testCase.in)
		if err != nil {
			t.Fatal(err)
		}
		got, err := VariantRef(ref, testCase.runtime)
		if testCase.wantErr {
			if !errors.Is(err, ErrUnsupported) {
				t.Errorf("VariantRef(%q, %q) err = %v; want ErrUnsupported", testCase.in, testCase.runtime, err)
			}
			continue
		}
		if err != nil || got.String() != testCase.want {
			t.Errorf("VariantRef(%q, %q) = %q, %v; want %q", testCase.in, testCase.runtime, got.String(), err, testCase.want)
		}
	}
}

// fakeRegistry is a distribution-spec registry that demands the anonymous
// bearer flow, like ghcr.io and Docker Hub.
type fakeRegistry struct {
	server    *httptest.Server
	manifests map[string]string // "<repo>:<ref>" -> body
	gets      atomic.Int32
	nonGets   atomic.Int32
}

func newFakeRegistry(t *testing.T) *fakeRegistry {
	registry := &fakeRegistry{manifests: map[string]string{}}
	registry.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			registry.nonGets.Add(1)
		}
		if r.URL.Path == "/token" {
			if !strings.HasPrefix(r.URL.Query().Get("scope"), "repository:") {
				http.Error(w, "scope", http.StatusBadRequest)
				return
			}
			fmt.Fprint(w, `{"token":"anon"}`)
			return
		}
		if r.Header.Get("Authorization") != "Bearer anon" {
			w.Header().Set("WWW-Authenticate", fmt.Sprintf(`Bearer realm="%s/token",service="fake",scope="repository:x:pull"`, registry.server.URL))
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		registry.gets.Add(1)
		path := strings.TrimPrefix(r.URL.Path, "/v2/")
		repository, reference, ok := strings.Cut(path, "/manifests/")
		if !ok {
			http.NotFound(w, r)
			return
		}
		body, ok := registry.manifests[repository+":"+reference]
		if !ok {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Docker-Content-Digest", digestOf(body))
		fmt.Fprint(w, body)
	}))
	t.Cleanup(registry.server.Close)
	return registry
}

func (registry *fakeRegistry) resolver() *Resolver {
	host := strings.TrimPrefix(registry.server.URL, "http://")
	resolver := &Resolver{
		Registries: map[string]string{"ghcr.io": host, "docker.io": host},
		TokenHosts: map[string]bool{host: true},
		Scheme:     "http",
		CacheTTL:   time.Minute,
	}
	resolver.Client = &http.Client{Timeout: 5 * time.Second, CheckRedirect: resolver.checkRedirect}
	return resolver
}

const linuxIndex = `{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","platform":{"os":"linux","architecture":"arm64"}},{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:2222222222222222222222222222222222222222222222222222222222222222","platform":{"os":"linux","architecture":"amd64"}}]}`

const diskIndex = `{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","annotations":{"ai.cua.image.variant":"containerdisk"},"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:3333333333333333333333333333333333333333333333333333333333333333","platform":{"os":"linux","architecture":"amd64"}}]}`

const plainManifest = `{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"digest":"sha256:4444444444444444444444444444444444444444444444444444444444444444"}}`

func TestResolvePinsCanonicalVariantsThroughTheAnonymousTokenFlow(t *testing.T) {
	registry := newFakeRegistry(t)
	registry.manifests["trycua/linux:24.04"] = linuxIndex
	registry.manifests["trycua/linux:24.04-disk"] = diskIndex
	resolver := registry.resolver()

	rootfs, err := resolver.Resolve(context.Background(), "ghcr.io/trycua/linux:24.04", "gvisor")
	if err != nil {
		t.Fatal(err)
	}
	if rootfs.Variant != VariantRootfs || rootfs.VariantSource != "default" ||
		rootfs.PinnedRef != "ghcr.io/trycua/linux@"+digestOf(linuxIndex) ||
		rootfs.PlatformDigest == nil || *rootfs.PlatformDigest != "sha256:"+strings.Repeat("2", 64) ||
		rootfs.MediaType != mediaTypeOCIIndex {
		t.Fatalf("rootfs = %+v", rootfs)
	}

	disk, err := resolver.Resolve(context.Background(), "ghcr.io/trycua/linux:24.04", "kubevirt")
	if err != nil {
		t.Fatal(err)
	}
	if disk.Ref != "ghcr.io/trycua/linux:24.04" || disk.ResolvedRef != "ghcr.io/trycua/linux:24.04-disk" ||
		disk.Variant != VariantContainerDisk || disk.VariantSource != "annotation" ||
		disk.Digest != digestOf(diskIndex) {
		t.Fatalf("disk = %+v", disk)
	}

	// Cached: a second resolve does not hit the registry.
	before := registry.gets.Load()
	if _, err := resolver.Resolve(context.Background(), "ghcr.io/trycua/linux:24.04", "kubevirt"); err != nil {
		t.Fatal(err)
	}
	if registry.gets.Load() != before {
		t.Fatal("cached resolve reached the registry")
	}
	// Read-only: the resolver only ever issues GETs (manifest and token).
	if registry.nonGets.Load() != 0 {
		t.Fatalf("resolver sent %d non-GET requests", registry.nonGets.Load())
	}
}

func TestResolveShortRefsAndDigestRefs(t *testing.T) {
	registry := newFakeRegistry(t)
	registry.manifests["library/python:3.12-slim"] = plainManifest
	registry.manifests["library/python:"+digestOf(plainManifest)] = plainManifest
	resolver := registry.resolver()

	result, err := resolver.Resolve(context.Background(), "python:3.12-slim", "")
	if err != nil {
		t.Fatal(err)
	}
	if result.PinnedRef != "docker.io/library/python@"+digestOf(plainManifest) || result.PlatformDigest != nil || result.Variant != VariantRootfs {
		t.Fatalf("result = %+v", result)
	}
	pinned, err := resolver.Resolve(context.Background(), result.PinnedRef, "")
	if err != nil || pinned.Digest != result.Digest || pinned.ResolvedRef != result.PinnedRef {
		t.Fatalf("pinned = %+v, %v", pinned, err)
	}
}

func TestResolveErrors(t *testing.T) {
	registry := newFakeRegistry(t)
	resolver := registry.resolver()
	ctx := context.Background()

	if _, err := resolver.Resolve(ctx, "ghcr.io/trycua/missing:1", ""); !errors.Is(err, ErrNotFound) {
		t.Errorf("missing: %v", err)
	}
	if _, err := resolver.Resolve(ctx, "evil.internal/app:1", ""); !errors.Is(err, ErrRegistryNotAllowed) {
		t.Errorf("unlisted registry: %v", err)
	}
	if _, err := resolver.Resolve(ctx, "169.254.169.254/latest:1", ""); !errors.Is(err, ErrRegistryNotAllowed) {
		t.Errorf("metadata host: %v", err)
	}
	if _, err := resolver.Resolve(ctx, "ghcr.io/trycua/linux:24.04", "docker"); !errors.Is(err, ErrUnsupported) {
		t.Errorf("runtime: %v", err)
	}
	if _, err := resolver.Resolve(ctx, "not a ref", ""); !errors.Is(err, ErrInvalidReference) {
		t.Errorf("invalid: %v", err)
	}
}

func TestResolveRefusesTokenRealmsOffTheAllowlist(t *testing.T) {
	var realmHits atomic.Int32
	realm := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		realmHits.Add(1)
		fmt.Fprint(w, `{"token":"x"}`)
	}))
	defer realm.Close()
	registry := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("WWW-Authenticate", fmt.Sprintf(`Bearer realm="%s/token",service="x"`, realm.URL))
		w.WriteHeader(http.StatusUnauthorized)
	}))
	defer registry.Close()
	host := strings.TrimPrefix(registry.URL, "http://")
	resolver := &Resolver{Registries: map[string]string{"ghcr.io": host}, TokenHosts: map[string]bool{host: true}, Scheme: "http"}

	if _, err := resolver.Resolve(context.Background(), "ghcr.io/a/b:1", ""); !errors.Is(err, ErrRegistryNotAllowed) {
		t.Fatalf("err = %v", err)
	}
	if realmHits.Load() != 0 {
		t.Fatal("resolver called a token realm outside the allowlist")
	}
}

func TestResolveRefusesRedirectsOffTheAllowlist(t *testing.T) {
	var elsewhereHits atomic.Int32
	elsewhere := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		elsewhereHits.Add(1)
	}))
	defer elsewhere.Close()
	registry := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, elsewhere.URL+"/v2/x", http.StatusTemporaryRedirect)
	}))
	defer registry.Close()
	host := strings.TrimPrefix(registry.URL, "http://")
	resolver := &Resolver{Registries: map[string]string{"ghcr.io": host}, TokenHosts: map[string]bool{}, Scheme: "http"}
	resolver.Client = &http.Client{CheckRedirect: resolver.checkRedirect}

	if _, err := resolver.Resolve(context.Background(), "ghcr.io/a/b:1", ""); !errors.Is(err, ErrRegistryNotAllowed) {
		t.Fatalf("err = %v", err)
	}
	if elsewhereHits.Load() != 0 {
		t.Fatal("resolver followed a redirect off the allowlist")
	}
}

func TestResolveRejectsDigestMismatchAndOversizedManifests(t *testing.T) {
	registry := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/big") {
			w.Write([]byte(strings.Repeat(" ", maxManifestBytes+1)))
			return
		}
		w.Header().Set("Docker-Content-Digest", "sha256:"+strings.Repeat("0", 64))
		fmt.Fprint(w, plainManifest)
	}))
	defer registry.Close()
	host := strings.TrimPrefix(registry.URL, "http://")
	resolver := &Resolver{Registries: map[string]string{"ghcr.io": host}, TokenHosts: map[string]bool{}, Scheme: "http"}

	if _, err := resolver.Resolve(context.Background(), "ghcr.io/a/b:1", ""); !errors.Is(err, ErrUpstream) {
		t.Fatalf("mismatch err = %v", err)
	}
	if _, err := resolver.Resolve(context.Background(), "ghcr.io/a/b:big", ""); !errors.Is(err, ErrUpstream) {
		t.Fatalf("oversized err = %v", err)
	}
}

func TestDefaultRegistriesAreHTTPSHostsOnly(t *testing.T) {
	resolver := NewResolver()
	if resolver.scheme() != "https" {
		t.Fatal("production scheme must be https")
	}
	for name, host := range DefaultRegistries {
		if _, err := url.Parse("https://" + host); err != nil || strings.ContainsAny(host, "/@") {
			t.Errorf("registry %s host %q is not a bare host", name, host)
		}
	}
}
