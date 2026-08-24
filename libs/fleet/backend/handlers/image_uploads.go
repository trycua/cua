package handlers

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	s3types "github.com/aws/aws-sdk-go-v2/service/s3/types"
)

var imageDigest = regexp.MustCompile(`^sha256:[0-9a-f]{64}$`)

type ImageObjectStore interface {
	Exists(ctx context.Context, key string, size int64) (bool, error)
	PresignPut(ctx context.Context, key string, size int64, expires time.Duration) (string, http.Header, error)
}

type ImageUploadFileRequest struct {
	Digest    string `json:"digest"`
	SizeBytes int64  `json:"sizeBytes"`
	Name      string `json:"name"`
}

type ImageUploadRequest struct {
	Namespace string                   `json:"namespace"`
	Files     []ImageUploadFileRequest `json:"files"`
}

type PresignedPut struct {
	Method  string            `json:"method"`
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers"`
}

type ImageUploadInstruction struct {
	Digest    string        `json:"digest"`
	SizeBytes int64         `json:"sizeBytes"`
	Reference string        `json:"reference"`
	Upload    *PresignedPut `json:"upload,omitempty"`
}

type ImageUploadResponse struct {
	Files []ImageUploadInstruction `json:"files"`
}

// PresignImageUploads returns one stable opaque reference for each requested
// digest and, only when the exact object is absent, a short-lived PUT upload.
func (h Handlers) PresignImageUploads(w http.ResponseWriter, r *http.Request) {
	var request ImageUploadRequest
	decoder := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<20))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid image upload request")
		return
	}
	if decoder.More() {
		writeErr(w, http.StatusBadRequest, "invalid image upload request")
		return
	}
	if err := h.validateImageUploadRequest(request); err != nil {
		writeErr(w, http.StatusBadRequest, err.Error())
		return
	}

	user := currentUser(r)
	if user == nil || user.ID == "" {
		writeErr(w, http.StatusUnauthorized, "authentication required")
		return
	}
	if !namespaceAllowed(user, request.Namespace) {
		allowed, err := h.userHasNamespaceRBAC(r.Context(), user.ID, request.Namespace)
		if err != nil || !allowed {
			writeErr(w, http.StatusForbidden, "namespace access denied")
			return
		}
	}
	if h.ImageObjects == nil {
		writeErr(w, http.StatusServiceUnavailable, "image uploads are unavailable")
		return
	}

	response := ImageUploadResponse{Files: make([]ImageUploadInstruction, 0, len(request.Files))}
	for _, file := range request.Files {
		key, reference := imageObjectNames(user.ID, file.Digest)
		exists, err := h.ImageObjects.Exists(r.Context(), key, file.SizeBytes)
		if err != nil {
			writeErr(w, http.StatusBadGateway, "image object lookup failed")
			return
		}
		instruction := ImageUploadInstruction{Digest: file.Digest, SizeBytes: file.SizeBytes, Reference: reference}
		if !exists {
			url, headers, err := h.ImageObjects.PresignPut(r.Context(), key, file.SizeBytes, h.ImageUploads.URLLifetime)
			if err != nil {
				writeErr(w, http.StatusBadGateway, "image upload signing failed")
				return
			}
			instruction.Upload = &PresignedPut{Method: http.MethodPut, URL: url, Headers: signedHeaders(headers)}
		}
		response.Files = append(response.Files, instruction)
	}
	writeJSON(w, http.StatusOK, response)
}

func (h Handlers) validateImageUploadRequest(request ImageUploadRequest) error {
	if len(request.Namespace) > 63 || !dnsLabel.MatchString(request.Namespace) {
		return errors.New("namespace must be a DNS label")
	}
	if len(request.Files) == 0 || len(request.Files) > h.ImageUploads.MaxFilesPerRequest {
		return errors.New("file count is outside configured bounds")
	}
	for _, file := range request.Files {
		if !imageDigest.MatchString(file.Digest) {
			return errors.New("digest must be a sha256 digest")
		}
		if file.SizeBytes <= 0 || file.SizeBytes > h.ImageUploads.MaxFileBytes {
			return errors.New("file size is outside configured bounds")
		}
	}
	return nil
}

func imageObjectNames(subject, digest string) (key, reference string) {
	tenantHash := sha256.Sum256([]byte(subject))
	digestHash := sha256.Sum256([]byte(digest))
	tenantLabel := "tenant-" + hex.EncodeToString(tenantHash[:16])
	digestToken := base64.RawURLEncoding.EncodeToString(digestHash[:])
	return "subjects/" + base64.RawURLEncoding.EncodeToString(tenantHash[:]) + "/images/" + digestToken,
		"uploads/" + tenantLabel + "/" + digestToken
}

func signedHeaders(headers http.Header) map[string]string {
	out := make(map[string]string, len(headers))
	for name, values := range headers {
		out[name] = strings.Join(values, ",")
	}
	return out
}

type s3ImageObjectStore struct {
	client    *s3.Client
	presigner *s3.PresignClient
	bucket    string
}

func NewS3ImageObjectStore(client *s3.Client, bucket string) ImageObjectStore {
	return &s3ImageObjectStore{client: client, presigner: s3.NewPresignClient(client), bucket: bucket}
}

func (s *s3ImageObjectStore) Exists(ctx context.Context, key string, size int64) (bool, error) {
	object, err := s.client.HeadObject(ctx, &s3.HeadObjectInput{Bucket: aws.String(s.bucket), Key: aws.String(key)})
	if err == nil {
		return aws.ToInt64(object.ContentLength) == size, nil
	}
	var notFound *s3types.NotFound
	if errors.As(err, &notFound) {
		return false, nil
	}
	return false, err
}

func (s *s3ImageObjectStore) PresignPut(ctx context.Context, key string, size int64, expires time.Duration) (string, http.Header, error) {
	request, err := s.presigner.PresignPutObject(ctx, &s3.PutObjectInput{
		Bucket:        aws.String(s.bucket),
		Key:           aws.String(key),
		ContentLength: aws.Int64(size),
	}, s3.WithPresignExpires(expires))
	if err != nil {
		return "", nil, err
	}
	return request.URL, request.SignedHeader, nil
}

var _ ImageObjectStore = (*s3ImageObjectStore)(nil)
