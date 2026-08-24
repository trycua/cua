package handlers

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"cyclops-cs-backend/auth"
	"cyclops-cs-backend/config"
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
	expires time.Duration
}

func (s *fakeImageObjectStore) Exists(_ context.Context, key string, size int64) (bool, error) {
	s.existsCalls = append(s.existsCalls, imageObjectCall{key: key, size: size})
	return s.exists, s.existsErr
}

func (s *fakeImageObjectStore) PresignPut(_ context.Context, key string, size int64, expires time.Duration) (string, http.Header, error) {
	s.presignCalls = append(s.presignCalls, imageObjectCall{key: key, size: size, expires: expires})
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

func imageUploadHandlers(store ImageObjectStore) Handlers {
	return Handlers{
		ImageUploads: config.ImageUploadConfiguration{
			MaxFileBytes:       100,
			MaxFilesPerRequest: 1,
			URLLifetime:        15 * time.Minute,
		},
		ImageObjects: store,
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
