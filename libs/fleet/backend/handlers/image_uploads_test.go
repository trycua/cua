package handlers

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"
)

const imageUploadDigest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

type fakeImageObjectStore struct {
	exists         bool
	existsErr      error
	presignErr     error
	existsCalls    []imageObjectCall
	presignCalls   []imageObjectCall
	presignedURL   string
	presignedHeads http.Header
}

type imageObjectCall struct {
	key     string
	size    int64
	digest  string
	expires time.Duration
}

func (s *fakeImageObjectStore) Exists(_ context.Context, key string, size int64, digest string) (bool, error) {
	s.existsCalls = append(s.existsCalls, imageObjectCall{key: key, size: size, digest: digest})
	return s.exists, s.existsErr
}

func (s *fakeImageObjectStore) PresignPut(_ context.Context, key string, size int64, digest string, expires time.Duration) (string, http.Header, error) {
	s.presignCalls = append(s.presignCalls, imageObjectCall{key: key, size: size, digest: digest, expires: expires})
	return s.presignedURL, s.presignedHeads.Clone(), s.presignErr
}

func TestPresignImageUploadsSignsMissingFilesWithStableOpaqueReferences(t *testing.T) {
	store := &fakeImageObjectStore{
		presignedURL:   "https://uploads.example.test/signed",
		presignedHeads: http.Header{"Content-Length": {"12"}, "X-Amz-Meta-Test": {"yes"}},
	}
	h := imageUploadHandlers(store)
	user := &auth.User{ID: "user-123", AllowedNamespaces: []string{"workers"}}

	first := presignImageUploads(t, h, user, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})
	second := presignImageUploads(t, h, user, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})

	if got, want := len(first.Files), 1; got != want {
		t.Fatalf("files = %d, want %d", got, want)
	}
	file := first.Files[0]
	if file.Digest != imageUploadDigest || file.SizeBytes != 12 {
		t.Fatalf("file = %#v, want digest and size echoed", file)
	}
	if file.Reference == "" || file.Reference != second.Files[0].Reference {
		t.Fatalf("reference = %q then %q, want stable non-empty reference", file.Reference, second.Files[0].Reference)
	}
	if file.Upload == nil || file.Upload.Method != http.MethodPut || file.Upload.URL != store.presignedURL {
		t.Fatalf("upload = %#v, want PUT %q", file.Upload, store.presignedURL)
	}
	if got := file.Upload.Headers["X-Amz-Meta-Test"]; got != "yes" {
		t.Fatalf("upload headers = %#v, want signed header", file.Upload.Headers)
	}
	if got, want := len(store.existsCalls), 2; got != want {
		t.Fatalf("Exists calls = %d, want %d", got, want)
	}
	if got, want := len(store.presignCalls), 2; got != want {
		t.Fatalf("PresignPut calls = %d, want %d", got, want)
	}
	if store.existsCalls[0].key == file.Reference || store.presignCalls[0].key == file.Reference {
		t.Fatalf("store key leaked as client reference: %q", file.Reference)
	}
	if got, want := store.presignCalls[0].expires, 15*time.Minute; got != want {
		t.Fatalf("presign expiry = %s, want %s", got, want)
	}
	if got, want := store.existsCalls[0].digest, imageUploadDigest; got != want {
		t.Fatalf("Exists digest = %q, want %q", got, want)
	}
	if got, want := store.presignCalls[0].digest, imageUploadDigest; got != want {
		t.Fatalf("PresignPut digest = %q, want %q", got, want)
	}
}

func TestPresignImageUploadsOmitsUploadForExistingFiles(t *testing.T) {
	store := &fakeImageObjectStore{exists: true}
	response := presignImageUploads(t, imageUploadHandlers(store), &auth.User{
		ID: "user-123", AllowedNamespaces: []string{"workers"},
	}, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})

	if response.Files[0].Upload != nil {
		t.Fatalf("upload = %#v, want omitted for an existing object", response.Files[0].Upload)
	}
	if got := len(store.presignCalls); got != 0 {
		t.Fatalf("PresignPut calls = %d, want 0", got)
	}
}

func TestPresignImageUploadsRejectsUnauthorizedNamespaceBeforeStoreCalls(t *testing.T) {
	store := &fakeImageObjectStore{}
	w := httptest.NewRecorder()
	r := jsonRequest(t, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})
	r = withUser(r, &auth.User{ID: "user-123"})
	imageUploadHandlers(store).PresignImageUploads(w, r)

	if got, want := w.Code, http.StatusForbidden; got != want {
		t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
	}
	assertImageObjectStoreUnused(t, store)
}

func TestPresignImageUploadsDeniesGitHubNamespaceWithoutRBACProbe(t *testing.T) {
	resetOwnershipCache()
	fakeK8s := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fakeK8s.server.Close()
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	store := &fakeImageObjectStore{}
	w := httptest.NewRecorder()
	r := withUser(jsonRequest(t, validImageUploadRequest("worker-rootfs")), &auth.User{
		ID: "github-subject", PrincipalType: auth.PrincipalTypeGitHubOIDC,
		AllowedNamespaces: []string{"other"},
	})

	imageUploadHandlers(store).PresignImageUploads(w, r)

	if got, want := w.Code, http.StatusForbidden; got != want {
		t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
	}
	if len(fakeK8s.requests) != 0 {
		t.Fatalf("RBAC probes = %d, want 0", len(fakeK8s.requests))
	}
	assertImageObjectStoreUnused(t, store)
}

func TestPresignImageUploadsAllowsNormalUserThroughRBACFallback(t *testing.T) {
	resetOwnershipCache()
	fakeK8s := newFakeK8s(http.StatusOK, `{"items":[]}`)
	defer fakeK8s.server.Close()
	overrideK8sClient(fakeK8s.server.Client(), fakeK8s.server.URL, "fake-sa-token")
	store := &fakeImageObjectStore{exists: true}

	presignImageUploads(t, imageUploadHandlers(store), &auth.User{ID: "user-123"}, validImageUploadRequest("worker-rootfs"))

	if got, want := len(fakeK8s.requests), 1; got != want {
		t.Fatalf("RBAC probes = %d, want %d", got, want)
	}
}

func TestPresignImageUploadsAllowsPerKeyPrincipalInClaimedNamespace(t *testing.T) {
	store := &fakeImageObjectStore{exists: true}
	response := presignImageUploads(t, imageUploadHandlers(store), &auth.User{
		ID: "service-account-key-one", AZP: "key-one", Namespace: "workers",
	}, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})

	if len(response.Files) != 1 {
		t.Fatalf("files = %d, want 1", len(response.Files))
	}
}

func TestPresignImageUploadsDeniesPerKeyPrincipalOutsideClaimedNamespace(t *testing.T) {
	store := &fakeImageObjectStore{}
	w := httptest.NewRecorder()
	r := withUser(jsonRequest(t, ImageUploadRequest{
		Namespace: "other",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	}), &auth.User{
		ID: "service-account-key-one", AZP: "key-one", Namespace: "workers",
		AllowedNamespaces: []string{"other"},
	})
	imageUploadHandlers(store).PresignImageUploads(w, r)

	if got, want := w.Code, http.StatusForbidden; got != want {
		t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
	}
	assertImageObjectStoreUnused(t, store)
}

func TestPresignImageUploadsUsesStableTenantIdentityAcrossCredentials(t *testing.T) {
	firstStore := &fakeImageObjectStore{exists: true}
	first := presignImageUploads(t, imageUploadHandlers(firstStore), &auth.User{
		ID: "service-account-key-one", AZP: "key-one", Namespace: "workers",
	}, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})
	secondStore := &fakeImageObjectStore{exists: true}
	second := presignImageUploads(t, imageUploadHandlers(secondStore), &auth.User{
		ID: "service-account-key-two", AZP: "key-two", Namespace: "workers",
	}, ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs"}},
	})

	if got, want := second.Files[0].Reference, first.Files[0].Reference; got != want {
		t.Fatalf("reference = %q, want %q", got, want)
	}
	if got, want := secondStore.existsCalls[0].key, firstStore.existsCalls[0].key; got != want {
		t.Fatalf("store key = %q, want %q", got, want)
	}
}

func TestPresignImageUploadsRejectsInvalidRequestsBeforeStoreCalls(t *testing.T) {
	tests := []struct {
		name    string
		request ImageUploadRequest
	}{
		{
			name: "malformed digest",
			request: ImageUploadRequest{Namespace: "workers", Files: []ImageUploadFileRequest{{
				Digest: "sha256:not-a-digest", SizeBytes: 12, Name: "worker-rootfs",
			}}},
		},
		{name: "zero files", request: ImageUploadRequest{Namespace: "workers"}},
		{
			name: "too many files",
			request: ImageUploadRequest{Namespace: "workers", Files: []ImageUploadFileRequest{
				{Digest: imageUploadDigest, SizeBytes: 12, Name: "one"},
				{Digest: imageUploadDigest, SizeBytes: 12, Name: "two"},
			}},
		},
		{
			name: "file exceeds configured size",
			request: ImageUploadRequest{Namespace: "workers", Files: []ImageUploadFileRequest{{
				Digest: imageUploadDigest, SizeBytes: 101, Name: "worker-rootfs",
			}}},
		},
		{
			name: "invalid namespace",
			request: ImageUploadRequest{Namespace: "Workers", Files: []ImageUploadFileRequest{{
				Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs",
			}}},
		},
		{
			name: "namespace exceeds DNS label length",
			request: ImageUploadRequest{Namespace: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Files: []ImageUploadFileRequest{{
				Digest: imageUploadDigest, SizeBytes: 12, Name: "worker-rootfs",
			}}},
		},
		{name: "empty file name", request: validImageUploadRequest("")},
		{name: "slash in file name", request: validImageUploadRequest("dir/file")},
		{name: "backslash in file name", request: validImageUploadRequest(`dir\file`)},
		{name: "dot file name", request: validImageUploadRequest(".")},
		{name: "dot dot file name", request: validImageUploadRequest("..")},
		{name: "control in file name", request: validImageUploadRequest("worker\nrootfs")},
		{name: "oversized file name", request: validImageUploadRequest(strings.Repeat("a", 256))},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			store := &fakeImageObjectStore{existsErr: errors.New("must not be called")}
			w := httptest.NewRecorder()
			r := withUser(jsonRequest(t, test.request), &auth.User{
				ID: "user-123", AllowedNamespaces: []string{"workers"},
			})
			imageUploadHandlers(store).PresignImageUploads(w, r)

			if got, want := w.Code, http.StatusBadRequest; got != want {
				t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
			}
			assertImageObjectStoreUnused(t, store)
		})
	}
}

func TestPresignImageUploadsRejectsInvalidUTF8BeforeStoreCalls(t *testing.T) {
	store := &fakeImageObjectStore{existsErr: errors.New("must not be called")}
	body := append([]byte(`{"namespace":"workers","files":[{"digest":"`+imageUploadDigest+`","sizeBytes":12,"name":"`), 0xff)
	body = append(body, []byte(`"}]}`)...)
	r := httptest.NewRequest(http.MethodPost, "/api/image-uploads/presign", bytes.NewReader(body))
	r.Header.Set("Content-Type", "application/json")
	r = withUser(r, &auth.User{ID: "user-123", AllowedNamespaces: []string{"workers"}})
	w := httptest.NewRecorder()

	imageUploadHandlers(store).PresignImageUploads(w, r)

	if got, want := w.Code, http.StatusBadRequest; got != want {
		t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
	}
	assertImageObjectStoreUnused(t, store)
}

func TestS3ImageObjectStorePresignRequiresClaimedSHA256(t *testing.T) {
	store := testS3ImageObjectStore(t, http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		t.Fatal("presigning must not send an HTTP request")
	}))

	url, headers, err := store.PresignPut(context.Background(), "tenants/workers/images/test", 12, imageUploadDigest, 15*time.Minute)
	if err != nil {
		t.Fatalf("PresignPut() error = %v", err)
	}
	if got, want := headers.Get("X-Amz-Checksum-Sha256"), imageUploadChecksum(t); got != want {
		t.Fatalf("checksum header = %q, want %q; headers = %#v; url = %s", got, want, headers, url)
	}
}

func TestS3ImageObjectStoreExistsRequiresExactSizeAndChecksum(t *testing.T) {
	tests := []struct {
		name       string
		size       int64
		checksum   string
		wantExists bool
		wantErr    bool
	}{
		{name: "matching object", size: 12, checksum: imageUploadChecksum(t), wantExists: true},
		{name: "poisoned checksum", size: 12, checksum: base64.StdEncoding.EncodeToString(make([]byte, 32)), wantErr: true},
		{name: "wrong size", size: 13, checksum: imageUploadChecksum(t), wantErr: true},
		{name: "missing checksum", size: 12, wantErr: true},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			var checksumMode string
			store := testS3ImageObjectStore(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				checksumMode = r.Header.Get("X-Amz-Checksum-Mode")
				w.Header().Set("Content-Length", fmt.Sprint(test.size))
				if test.checksum != "" {
					w.Header().Set("X-Amz-Checksum-Sha256", test.checksum)
				}
				w.WriteHeader(http.StatusOK)
			}))

			exists, err := store.Exists(context.Background(), "tenants/workers/images/test", 12, imageUploadDigest)
			if (err != nil) != test.wantErr {
				t.Fatalf("Exists() error = %v, wantErr %t", err, test.wantErr)
			}
			if exists != test.wantExists {
				t.Fatalf("Exists() = %t, want %t", exists, test.wantExists)
			}
			if got, want := checksumMode, "ENABLED"; got != want {
				t.Fatalf("checksum mode = %q, want %q", got, want)
			}
		})
	}
}

func testS3ImageObjectStore(t *testing.T, handler http.Handler) ImageObjectStore {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	cfg := aws.Config{
		Region:       "us-east-1",
		BaseEndpoint: aws.String(server.URL),
		Credentials:  credentials.NewStaticCredentialsProvider("access", "secret", ""),
		HTTPClient:   server.Client(),
	}
	client := s3.NewFromConfig(cfg, func(options *s3.Options) { options.UsePathStyle = true })
	return NewS3ImageObjectStore(client, "image-uploads")
}

func imageUploadChecksum(t *testing.T) string {
	t.Helper()
	raw, err := hex.DecodeString(strings.TrimPrefix(imageUploadDigest, "sha256:"))
	if err != nil {
		t.Fatalf("decode digest: %v", err)
	}
	return base64.StdEncoding.EncodeToString(raw)
}

func imageUploadHandlers(store ImageObjectStore) Handlers {
	return Handlers{
		AuthCfg: config.AuthConfiguration{KeyClientPfx: "key-"},
		ImageUploads: config.ImageUploadConfiguration{
			MaxFileBytes:       100,
			MaxFilesPerRequest: 1,
			URLLifetime:        15 * time.Minute,
		},
		ImageObjects: store,
	}
}

func validImageUploadRequest(name string) ImageUploadRequest {
	return ImageUploadRequest{
		Namespace: "workers",
		Files:     []ImageUploadFileRequest{{Digest: imageUploadDigest, SizeBytes: 12, Name: name}},
	}
}

func presignImageUploads(t *testing.T, h Handlers, user *auth.User, request ImageUploadRequest) ImageUploadResponse {
	t.Helper()
	w := httptest.NewRecorder()
	h.PresignImageUploads(w, withUser(jsonRequest(t, request), user))
	if got, want := w.Code, http.StatusOK; got != want {
		t.Fatalf("status = %d, want %d; body = %s", got, want, w.Body.String())
	}
	var response ImageUploadResponse
	if err := json.NewDecoder(w.Body).Decode(&response); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	return response
}

func jsonRequest(t *testing.T, body ImageUploadRequest) *http.Request {
	t.Helper()
	encoded, err := json.Marshal(body)
	if err != nil {
		t.Fatalf("marshal request: %v", err)
	}
	r := httptest.NewRequest(http.MethodPost, "/api/image-uploads/presign", bytes.NewReader(encoded))
	r.Header.Set("Content-Type", "application/json")
	return r
}

func assertImageObjectStoreUnused(t *testing.T, store *fakeImageObjectStore) {
	t.Helper()
	if len(store.existsCalls) != 0 || len(store.presignCalls) != 0 {
		t.Fatalf("store calls = exists:%d presign:%d, want none", len(store.existsCalls), len(store.presignCalls))
	}
}
