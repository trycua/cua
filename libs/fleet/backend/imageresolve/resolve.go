// Package imageresolve pins public registry image refs to digests for
// GET /api/images/resolve, so SDKs can run a digest-pinned image (and hash a
// pool spec by digest) without registry access of their own.
//
// It is deliberately small: an anonymous OCI distribution client (one
// manifest GET plus the anonymous bearer-token dance) over an allowlist of
// public registries. The allowlist is the SSRF boundary: the gateway never
// dials a host a caller names unless it is one of the public registries (or
// their token realms), and it never follows a redirect off that list.
// Private refs are the SDK's job (it holds the credentials); here they fail
// with ErrNotFound/ErrUnauthorized.
//
// Canonical cua images (ghcr.io/trycua/linux, ghcr.io/trycua/windows) map to
// their runtime variant first: runtime kubevirt resolves the containerDisk
// sibling tag (`24.04` -> `24.04-disk`), gvisor/macos the rootfs tag.
package imageresolve

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"sync"
	"time"
)

const (
	mediaTypeOCIIndex      = "application/vnd.oci.image.index.v1+json"
	mediaTypeOCIManifest   = "application/vnd.oci.image.manifest.v1+json"
	mediaTypeDockerList    = "application/vnd.docker.distribution.manifest.list.v2+json"
	mediaTypeDockerV2      = "application/vnd.docker.distribution.manifest.v2+json"
	maxManifestBytes       = 4 << 20
	maxTokenBytes          = 64 << 10
	defaultCacheTTL        = time.Minute
	maxCacheEntries        = 1024
	maxReferenceBytes      = 1024
	variantAnnotation      = "ai.cua.image.variant"
	canonicalDiskTagSuffix = "-disk"

	VariantRootfs        = "rootfs"
	VariantContainerDisk = "containerdisk"
)

var (
	ErrInvalidReference   = errors.New("invalid image reference")
	ErrRegistryNotAllowed = errors.New("registry is not on the public registry allowlist")
	ErrNotFound           = errors.New("image not found")
	ErrUnauthorized       = errors.New("image requires credentials (resolve private images client-side)")
	ErrUnsupported        = errors.New("unsupported image or runtime")
	ErrUpstream           = errors.New("registry request failed")
)

// DefaultRegistries are the public registries the gateway resolves against,
// keyed by the registry name refs use, valued by the API host to dial.
var DefaultRegistries = map[string]string{
	"ghcr.io":           "ghcr.io",
	"docker.io":         "registry-1.docker.io",
	"public.ecr.aws":    "public.ecr.aws",
	"quay.io":           "quay.io",
	"gcr.io":            "gcr.io",
	"mcr.microsoft.com": "mcr.microsoft.com",
	"registry.k8s.io":   "registry.k8s.io",
	"nvcr.io":           "nvcr.io",
}

// DefaultTokenHosts are the anonymous token realms those registries name.
var DefaultTokenHosts = map[string]bool{
	"ghcr.io":           true,
	"auth.docker.io":    true,
	"public.ecr.aws":    true,
	"quay.io":           true,
	"gcr.io":            true,
	"mcr.microsoft.com": true,
	"registry.k8s.io":   true,
	"nvcr.io":           true,
}

// canonicalRepositories lists the canonical cua image repositories and
// whether each has a rootfs (docker/gVisor) variant.
var canonicalRepositories = map[string]bool{
	"ghcr.io/trycua/linux":   true,
	"ghcr.io/trycua/windows": false,
}

var (
	tagPattern    = regexp.MustCompile(`^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$`)
	digestPattern = regexp.MustCompile(`^sha256:[0-9a-f]{64}$`)
	pathComponent = regexp.MustCompile(`^[a-z0-9]+(?:(?:[._]|__|-+)[a-z0-9]+)*$`)
)

// Reference is a parsed image ref.
type Reference struct {
	Registry   string
	Repository string
	Tag        string
	Digest     string
}

// Name is registry/repository.
func (r Reference) Name() string { return r.Registry + "/" + r.Repository }

// String is the canonical full ref.
func (r Reference) String() string {
	s := r.Name()
	if r.Tag != "" {
		s += ":" + r.Tag
	}
	if r.Digest != "" {
		s += "@" + r.Digest
	}
	return s
}

// ParseReference parses a docker-style ref. Short refs resolve to docker.io
// (`python:3.12` -> `docker.io/library/python:3.12`), matching docker and the
// cua SDK's resolver.
func ParseReference(raw string) (Reference, error) {
	if raw == "" || len(raw) > maxReferenceBytes || strings.ContainsAny(raw, " \t\r\n") {
		return Reference{}, ErrInvalidReference
	}
	var ref Reference
	rest := raw
	if at := strings.Index(rest, "@"); at >= 0 {
		ref.Digest = rest[at+1:]
		rest = rest[:at]
		if !digestPattern.MatchString(ref.Digest) {
			return Reference{}, fmt.Errorf("%w: digest must be sha256:<64 hex>", ErrInvalidReference)
		}
	}
	if colon := strings.LastIndex(rest, ":"); colon > strings.LastIndex(rest, "/") {
		ref.Tag = rest[colon+1:]
		rest = rest[:colon]
		if !tagPattern.MatchString(ref.Tag) {
			return Reference{}, fmt.Errorf("%w: bad tag", ErrInvalidReference)
		}
	}
	parts := strings.SplitN(rest, "/", 2)
	if len(parts) == 2 && (strings.ContainsAny(parts[0], ".:") || parts[0] == "localhost") {
		ref.Registry, ref.Repository = strings.ToLower(parts[0]), parts[1]
	} else {
		ref.Registry, ref.Repository = "docker.io", rest
	}
	if ref.Registry == "index.docker.io" || ref.Registry == "registry-1.docker.io" {
		ref.Registry = "docker.io"
	}
	if ref.Registry == "docker.io" && !strings.Contains(ref.Repository, "/") {
		ref.Repository = "library/" + ref.Repository
	}
	for _, component := range strings.Split(ref.Repository, "/") {
		if !pathComponent.MatchString(component) {
			return Reference{}, fmt.Errorf("%w: bad repository", ErrInvalidReference)
		}
	}
	if ref.Tag == "" && ref.Digest == "" {
		ref.Tag = "latest"
	}
	return ref, nil
}

// Result is the JSON body of GET /api/images/resolve.
type Result struct {
	Ref            string  `json:"ref"`
	ResolvedRef    string  `json:"resolvedRef"`
	PinnedRef      string  `json:"pinnedRef"`
	Digest         string  `json:"digest"`
	Variant        string  `json:"variant"`
	VariantSource  string  `json:"variantSource"`
	PlatformDigest *string `json:"platformDigest"`
	MediaType      string  `json:"mediaType"`
}

// Resolver resolves refs against the public registry allowlist.
type Resolver struct {
	Client *http.Client
	// Registries maps a ref's registry name to the host to dial.
	Registries map[string]string
	// TokenHosts are the token realm hosts the resolver may call.
	TokenHosts map[string]bool
	// Scheme is "https" in production; tests point it at httptest servers.
	Scheme   string
	CacheTTL time.Duration
	Now      func() time.Time

	mu    sync.Mutex
	cache map[string]cacheEntry
}

type cacheEntry struct {
	result  Result
	expires time.Time
}

// NewResolver returns the production resolver.
func NewResolver() *Resolver {
	r := &Resolver{
		Registries: DefaultRegistries,
		TokenHosts: DefaultTokenHosts,
		Scheme:     "https",
		CacheTTL:   defaultCacheTTL,
	}
	r.Client = &http.Client{Timeout: 15 * time.Second, CheckRedirect: r.checkRedirect}
	return r
}

func (r *Resolver) allowedHost(host string) bool {
	for _, apiHost := range r.Registries {
		if apiHost == host {
			return true
		}
	}
	return r.TokenHosts[host]
}

func (r *Resolver) checkRedirect(request *http.Request, via []*http.Request) error {
	if len(via) >= 5 {
		return errors.New("too many redirects")
	}
	if request.URL.Scheme != r.scheme() || !r.allowedHost(request.URL.Host) {
		return fmt.Errorf("%w: redirect to %s", ErrRegistryNotAllowed, request.URL.Host)
	}
	return nil
}

func (r *Resolver) scheme() string {
	if r.Scheme == "" {
		return "https"
	}
	return r.Scheme
}

func (r *Resolver) now() time.Time {
	if r.Now != nil {
		return r.Now()
	}
	return time.Now()
}

// VariantRef maps a canonical cua ref to the variant a runtime runs. Other
// refs are returned unchanged.
func VariantRef(ref Reference, runtime string) (Reference, error) {
	hasRootfs, canonical := canonicalRepositories[ref.Name()]
	if !canonical || ref.Tag == "" || runtime == "" {
		return ref, nil
	}
	isDisk := strings.HasSuffix(ref.Tag, canonicalDiskTagSuffix)
	switch runtime {
	case "kubevirt":
		if !isDisk {
			ref.Tag += canonicalDiskTagSuffix
			ref.Digest = ""
		}
	case "gvisor", "macos":
		if !hasRootfs {
			return Reference{}, fmt.Errorf("%w: %s is a KubeVirt containerDisk image; use runtime kubevirt", ErrUnsupported, ref.Name())
		}
		if isDisk {
			ref.Tag = strings.TrimSuffix(ref.Tag, canonicalDiskTagSuffix)
			ref.Digest = ""
		}
	default:
		return Reference{}, fmt.Errorf("%w: runtime must be kubevirt, gvisor or macos", ErrUnsupported)
	}
	return ref, nil
}

// Resolve pins raw (optionally mapped to runtime's variant) to a digest.
func (r *Resolver) Resolve(ctx context.Context, raw, runtime string) (Result, error) {
	if runtime != "" && runtime != "kubevirt" && runtime != "gvisor" && runtime != "macos" {
		return Result{}, fmt.Errorf("%w: runtime must be kubevirt, gvisor or macos", ErrUnsupported)
	}
	ref, err := ParseReference(raw)
	if err != nil {
		return Result{}, err
	}
	ref, err = VariantRef(ref, runtime)
	if err != nil {
		return Result{}, err
	}
	if _, ok := r.Registries[ref.Registry]; !ok {
		return Result{}, fmt.Errorf("%w: %s", ErrRegistryNotAllowed, ref.Registry)
	}

	key := ref.String()
	if cached, ok := r.cached(key); ok {
		cached.Ref = raw
		return cached, nil
	}
	result, err := r.resolve(ctx, ref)
	if err != nil {
		return Result{}, err
	}
	r.store(key, result)
	result.Ref = raw
	return result, nil
}

func (r *Resolver) cached(key string) (Result, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry, ok := r.cache[key]
	if !ok || r.now().After(entry.expires) {
		return Result{}, false
	}
	return entry.result, true
}

func (r *Resolver) store(key string, result Result) {
	ttl := r.CacheTTL
	if ttl <= 0 {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.cache == nil || len(r.cache) >= maxCacheEntries {
		r.cache = map[string]cacheEntry{}
	}
	r.cache[key] = cacheEntry{result: result, expires: r.now().Add(ttl)}
}

type manifest struct {
	MediaType   string            `json:"mediaType"`
	Annotations map[string]string `json:"annotations"`
	Manifests   []struct {
		MediaType string `json:"mediaType"`
		Digest    string `json:"digest"`
		Platform  *struct {
			OS           string `json:"os"`
			Architecture string `json:"architecture"`
		} `json:"platform"`
		Annotations map[string]string `json:"annotations"`
	} `json:"manifests"`
}

func (r *Resolver) resolve(ctx context.Context, ref Reference) (Result, error) {
	reference := ref.Digest
	if reference == "" {
		reference = ref.Tag
	}
	host := r.Registries[ref.Registry]
	manifestURL := fmt.Sprintf("%s://%s/v2/%s/manifests/%s", r.scheme(), host, ref.Repository, reference)
	body, headers, err := r.getManifest(ctx, manifestURL, ref.Repository)
	if err != nil {
		return Result{}, err
	}

	sum := sha256.Sum256(body)
	digest := "sha256:" + hex.EncodeToString(sum[:])
	if header := headers.Get("Docker-Content-Digest"); header != "" && header != digest {
		return Result{}, fmt.Errorf("%w: registry digest header does not match the manifest", ErrUpstream)
	}
	if ref.Digest != "" && ref.Digest != digest {
		return Result{}, fmt.Errorf("%w: manifest does not match the requested digest", ErrUpstream)
	}

	var parsed manifest
	if err := json.Unmarshal(body, &parsed); err != nil {
		return Result{}, fmt.Errorf("%w: manifest is not JSON: %w", ErrUpstream, err)
	}
	mediaType := parsed.MediaType
	if mediaType == "" {
		mediaType = strings.TrimSpace(strings.Split(headers.Get("Content-Type"), ";")[0])
	}

	result := Result{
		ResolvedRef: Reference{Registry: ref.Registry, Repository: ref.Repository, Tag: ref.Tag}.String(),
		PinnedRef:   ref.Name() + "@" + digest,
		Digest:      digest,
		MediaType:   mediaType,
	}
	if ref.Tag == "" {
		result.ResolvedRef = result.PinnedRef
	}

	variantAnnotationValue := parsed.Annotations[variantAnnotation]
	if mediaType == mediaTypeOCIIndex || mediaType == mediaTypeDockerList {
		for _, child := range parsed.Manifests {
			if child.Platform != nil && child.Platform.OS == "linux" && child.Platform.Architecture == "amd64" {
				platformDigest := child.Digest
				result.PlatformDigest = &platformDigest
				if variantAnnotationValue == "" {
					variantAnnotationValue = child.Annotations[variantAnnotation]
				}
				break
			}
		}
	}
	switch {
	case variantAnnotationValue == VariantContainerDisk || variantAnnotationValue == VariantRootfs:
		result.Variant, result.VariantSource = variantAnnotationValue, "annotation"
	case strings.HasSuffix(ref.Tag, canonicalDiskTagSuffix):
		result.Variant, result.VariantSource = VariantContainerDisk, "tag"
	default:
		result.Variant, result.VariantSource = VariantRootfs, "default"
	}
	return result, nil
}

var acceptManifests = strings.Join([]string{mediaTypeOCIIndex, mediaTypeOCIManifest, mediaTypeDockerList, mediaTypeDockerV2}, ", ")

func (r *Resolver) getManifest(ctx context.Context, manifestURL, repository string) ([]byte, http.Header, error) {
	token := ""
	for attempt := 0; attempt < 2; attempt++ {
		request, err := http.NewRequestWithContext(ctx, http.MethodGet, manifestURL, nil)
		if err != nil {
			return nil, nil, fmt.Errorf("%w: %w", ErrUpstream, err)
		}
		request.Header.Set("Accept", acceptManifests)
		if token != "" {
			request.Header.Set("Authorization", "Bearer "+token)
		}
		response, err := r.client().Do(request)
		if err != nil {
			if errors.Is(err, ErrRegistryNotAllowed) {
				return nil, nil, err
			}
			return nil, nil, fmt.Errorf("%w: %w", ErrUpstream, err)
		}
		body, readErr := readBounded(response.Body, maxManifestBytes)
		response.Body.Close()
		switch {
		case response.StatusCode == http.StatusOK:
			if readErr != nil {
				return nil, nil, fmt.Errorf("%w: %w", ErrUpstream, readErr)
			}
			return body, response.Header, nil
		case response.StatusCode == http.StatusUnauthorized && token == "":
			token, err = r.anonymousToken(ctx, response.Header.Get("WWW-Authenticate"), repository)
			if err != nil {
				return nil, nil, err
			}
		case response.StatusCode == http.StatusUnauthorized || response.StatusCode == http.StatusForbidden:
			return nil, nil, ErrUnauthorized
		case response.StatusCode == http.StatusNotFound:
			return nil, nil, ErrNotFound
		default:
			return nil, nil, fmt.Errorf("%w: registry returned %d", ErrUpstream, response.StatusCode)
		}
	}
	return nil, nil, ErrUnauthorized
}

func (r *Resolver) client() *http.Client {
	if r.Client != nil {
		return r.Client
	}
	return &http.Client{Timeout: 15 * time.Second, CheckRedirect: r.checkRedirect}
}

var challengeParam = regexp.MustCompile(`(\w+)="([^"]*)"`)

// anonymousToken performs the distribution-spec anonymous bearer flow.
func (r *Resolver) anonymousToken(ctx context.Context, challenge, repository string) (string, error) {
	if !strings.HasPrefix(strings.ToLower(challenge), "bearer ") {
		return "", ErrUnauthorized
	}
	params := map[string]string{}
	for _, match := range challengeParam.FindAllStringSubmatch(challenge, -1) {
		params[strings.ToLower(match[1])] = match[2]
	}
	realm, err := url.Parse(params["realm"])
	if err != nil {
		return "", fmt.Errorf("%w: token realm %q: %w", ErrRegistryNotAllowed, params["realm"], err)
	}
	if realm.Scheme != r.scheme() || !r.TokenHosts[realm.Host] || realm.User != nil {
		return "", fmt.Errorf("%w: token realm %q", ErrRegistryNotAllowed, params["realm"])
	}
	query := realm.Query()
	if service := params["service"]; service != "" {
		query.Set("service", service)
	}
	query.Set("scope", "repository:"+repository+":pull")
	realm.RawQuery = query.Encode()

	request, err := http.NewRequestWithContext(ctx, http.MethodGet, realm.String(), nil)
	if err != nil {
		return "", fmt.Errorf("%w: %w", ErrUpstream, err)
	}
	response, err := r.client().Do(request)
	if err != nil {
		return "", fmt.Errorf("%w: %w", ErrUpstream, err)
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusUnauthorized || response.StatusCode == http.StatusForbidden {
		return "", ErrUnauthorized
	}
	if response.StatusCode != http.StatusOK {
		return "", fmt.Errorf("%w: token endpoint returned %d", ErrUpstream, response.StatusCode)
	}
	body, err := readBounded(response.Body, maxTokenBytes)
	if err != nil {
		return "", fmt.Errorf("%w: %w", ErrUpstream, err)
	}
	var tokens struct {
		Token       string `json:"token"`
		AccessToken string `json:"access_token"`
	}
	if err := json.Unmarshal(body, &tokens); err != nil {
		return "", fmt.Errorf("%w: token response is not JSON: %w", ErrUpstream, err)
	}
	if tokens.Token != "" {
		return tokens.Token, nil
	}
	if tokens.AccessToken != "" {
		return tokens.AccessToken, nil
	}
	return "", ErrUnauthorized
}

func readBounded(reader io.Reader, limit int64) ([]byte, error) {
	body, err := io.ReadAll(io.LimitReader(reader, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(body)) > limit {
		return nil, fmt.Errorf("response exceeds %d bytes", limit)
	}
	return body, nil
}
