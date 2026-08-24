package handlers

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"cyclops-cs-backend/auth"

	"github.com/aws/aws-sdk-go-v2/aws"
	awsv4 "github.com/aws/aws-sdk-go-v2/aws/signer/v4"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	s3types "github.com/aws/aws-sdk-go-v2/service/s3/types"
)

var imageDigest = regexp.MustCompile(`^sha256:[0-9a-f]{64}$`)

type ImageObjectStore interface {
	Exists(ctx context.Context, key string, size int64, digest string) (bool, error)
	PresignPut(ctx context.Context, key string, size int64, digest string, expires time.Duration) (string, http.Header, error)
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
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 1<<20))
	if err != nil || !utf8.Valid(body) {
		writeErr(w, http.StatusBadRequest, "invalid image upload request")
		return
	}
	var request ImageUploadRequest
	decoder := json.NewDecoder(strings.NewReader(string(body)))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid image upload request")
		return
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
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
	if h.isPerKeyPrincipal(user) {
		if user.Namespace == "" || user.Namespace != request.Namespace {
			writeErr(w, http.StatusForbidden, "namespace access denied")
			return
		}
	} else if isGitHubPrincipal(user) {
		if !namespaceAllowed(user, request.Namespace) {
			writeErr(w, http.StatusForbidden, "namespace access denied")
			return
		}
	} else if !namespaceAllowed(user, request.Namespace) {
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
		key, reference := imageObjectNames(request.Namespace, file.Digest)
		exists, err := h.ImageObjects.Exists(r.Context(), key, file.SizeBytes, file.Digest)
		if err != nil {
			writeErr(w, http.StatusBadGateway, "image object lookup failed")
			return
		}
		instruction := ImageUploadInstruction{Digest: file.Digest, SizeBytes: file.SizeBytes, Reference: reference}
		if !exists {
			url, headers, err := h.ImageObjects.PresignPut(r.Context(), key, file.SizeBytes, file.Digest, h.ImageUploads.URLLifetime)
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
		if err := validateImageUploadFileName(file.Name); err != nil {
			return err
		}
	}
	return nil
}

func (h Handlers) isPerKeyPrincipal(user *auth.User) bool {
	return h.AuthCfg.KeyClientPfx != "" && strings.HasPrefix(user.AZP, h.AuthCfg.KeyClientPfx)
}

func validateImageUploadFileName(name string) error {
	if name == "" || len(name) > 255 || strings.TrimSpace(name) == "" || name == "." || name == ".." ||
		strings.ContainsAny(name, `/\`) {
		return errors.New("file name must be a bounded base name")
	}
	for _, r := range name {
		if unicode.IsControl(r) {
			return errors.New("file name must not contain control characters")
		}
	}
	return nil
}

func imageObjectNames(namespace, digest string) (key, reference string) {
	tenantHash := sha256.Sum256([]byte(namespace))
	digestHash := sha256.Sum256([]byte(digest))
	tenantLabel := "tenant-" + hex.EncodeToString(tenantHash[:16])
	digestToken := base64.RawURLEncoding.EncodeToString(digestHash[:])
	return "tenants/" + namespace + "/images/" + digestToken,
		"uploads/" + tenantLabel + "/" + digestToken
}

func imageDigestChecksum(digest string) (string, error) {
	raw, err := hex.DecodeString(strings.TrimPrefix(digest, "sha256:"))
	if err != nil || len(raw) != sha256.Size {
		return "", fmt.Errorf("invalid sha256 digest")
	}
	return base64.StdEncoding.EncodeToString(raw), nil
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
	presigner := s3.NewPresignClient(client, func(options *s3.PresignOptions) {
		options.Presigner = checksumHeaderPresigner{signer: awsv4.NewSigner()}
	})
	return &s3ImageObjectStore{client: client, presigner: presigner, bucket: bucket}
}

type checksumHeaderPresigner struct {
	signer *awsv4.Signer
}

func (p checksumHeaderPresigner) PresignHTTP(
	ctx context.Context,
	credentials aws.Credentials,
	request *http.Request,
	payloadHash string,
	service string,
	region string,
	signingTime time.Time,
	options ...func(*awsv4.SignerOptions),
) (string, http.Header, error) {
	options = append(options, func(signerOptions *awsv4.SignerOptions) {
		signerOptions.DisableHeaderHoisting = true
		signerOptions.DisableURIPathEscaping = true
	})
	return p.signer.PresignHTTP(ctx, credentials, request, payloadHash, service, region, signingTime, options...)
}

func (s *s3ImageObjectStore) Exists(ctx context.Context, key string, size int64, digest string) (bool, error) {
	checksum, err := imageDigestChecksum(digest)
	if err != nil {
		return false, err
	}
	object, err := s.client.HeadObject(ctx, &s3.HeadObjectInput{
		Bucket:       aws.String(s.bucket),
		Key:          aws.String(key),
		ChecksumMode: s3types.ChecksumModeEnabled,
	})
	if err != nil {
		var notFound *s3types.NotFound
		if errors.As(err, &notFound) {
			return false, nil
		}
		return false, err
	}
	if actual := aws.ToInt64(object.ContentLength); actual != size {
		return false, fmt.Errorf("stored image size mismatch: got %d, want %d", actual, size)
	}
	if actual := aws.ToString(object.ChecksumSHA256); actual != checksum {
		return false, fmt.Errorf("stored image checksum mismatch")
	}
	return true, nil
}

func (s *s3ImageObjectStore) PresignPut(ctx context.Context, key string, size int64, digest string, expires time.Duration) (string, http.Header, error) {
	checksum, err := imageDigestChecksum(digest)
	if err != nil {
		return "", nil, err
	}
	request, err := s.presigner.PresignPutObject(ctx, &s3.PutObjectInput{
		Bucket:         aws.String(s.bucket),
		Key:            aws.String(key),
		ContentLength:  aws.Int64(size),
		ChecksumSHA256: aws.String(checksum),
	}, s3.WithPresignExpires(expires))
	if err != nil {
		return "", nil, err
	}
	return request.URL, request.SignedHeader, nil
}

var _ ImageObjectStore = (*s3ImageObjectStore)(nil)
