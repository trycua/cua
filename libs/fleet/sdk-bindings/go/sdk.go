package cyclopssdk

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"sort"
	"strings"
	"time"
)

type OAuthConfiguration struct {
	TokenURL     string `json:"token_url"`
	ClientID     string `json:"client_id"`
	ClientSecret string `json:"client_secret"`
}
type Configuration struct {
	BaseURL string             `json:"base_url"`
	OAuth   OAuthConfiguration `json:"oauth"`
}
type ResourceMetadata struct {
	Namespace string            `json:"namespace"`
	Name      string            `json:"name"`
	Labels    map[string]string `json:"labels,omitempty"`
}
type Pool struct {
	APIVersion string           `json:"apiVersion"`
	Kind       string           `json:"kind"`
	Metadata   ResourceMetadata `json:"metadata"`
	Spec       map[string]any   `json:"spec"`
	Status     map[string]any   `json:"status,omitempty"`
}
type Claim struct {
	APIVersion string           `json:"apiVersion"`
	Kind       string           `json:"kind"`
	Metadata   ResourceMetadata `json:"metadata"`
	Spec       map[string]any   `json:"spec"`
	Status     map[string]any   `json:"status,omitempty"`
}
type Sandbox struct {
	Namespace string   `json:"namespace"`
	Claim     string   `json:"claim"`
	Sandbox   string   `json:"sandbox"`
	Services  []string `json:"services"`
}
type CreatePoolRequest struct {
	Namespace string   `json:"namespace"`
	Spec      PoolSpec `json:"spec"`
}
type CreateClaimRequest struct {
	Pool Pool       `json:"pool"`
	Spec *ClaimSpec `json:"spec,omitempty"`
}
type serviceRequest struct {
	Method  string      `json:"method"`
	Path    string      `json:"path"`
	Headers [][2]string `json:"headers"`
	Body    *string     `json:"body,omitempty"`
}
type serviceResponse struct {
	Status int    `json:"status"`
	Body   string `json:"body"`
}
type UnknownServiceError struct {
	Requested string
	Available []string
}

func (error *UnknownServiceError) Error() string {
	return fmt.Sprintf("unknown sandbox service %q; available services: %v", error.Requested, error.Available)
}

type SDK struct {
	configuration Configuration
	command       *exec.Cmd
	input         io.WriteCloser
	output        *bufio.Reader
	token         map[string]any
}
type ServiceClient struct {
	sdk     *SDK
	sandbox Sandbox
	service string
}

type hostCall struct {
	Kind    string `json:"kind"`
	Request *struct {
		Method  string      `json:"method"`
		URL     string      `json:"url"`
		Headers [][2]string `json:"headers"`
		Body    *string     `json:"body"`
	} `json:"request,omitempty"`
	Milliseconds int64          `json:"milliseconds,omitempty"`
	Token        map[string]any `json:"token,omitempty"`
}
type reply struct {
	OK    bool            `json:"ok"`
	Value json.RawMessage `json:"value"`
	Error string          `json:"error,omitempty"`
}

func Connect(configuration Configuration) (*SDK, error) {
	command := exec.Command(env("CYCLOPS_CORE_RUNNER", "cyclops-core-runner"))
	input, err := command.StdinPipe()
	if err != nil {
		return nil, err
	}
	output, err := command.StdoutPipe()
	if err != nil {
		return nil, err
	}
	command.Stderr = os.Stderr
	if err := command.Start(); err != nil {
		return nil, err
	}
	sdk := &SDK{configuration: configuration, command: command, input: input, output: bufio.NewReader(output)}
	if err := sdk.call(map[string]any{"op": "configure", "configuration": map[string]any{"protocol_version": 7, "base_url": configuration.BaseURL}}, nil); err != nil {
		_ = sdk.Close()
		return nil, err
	}
	return sdk, nil
}
func (sdk *SDK) CreatePool(request CreatePoolRequest) (Pool, error) {
	var value Pool
	return value, sdk.call(map[string]any{"op": "create_pool", "request": request}, &value)
}
func (sdk *SDK) ListPools(namespace string) ([]Pool, error) {
	var value []Pool
	return value, sdk.call(map[string]any{"op": "list_pools", "namespace": namespace}, &value)
}
func (sdk *SDK) GetPool(pool Pool) (Pool, error) {
	var value Pool
	return value, sdk.call(map[string]any{"op": "get_pool", "pool": pool}, &value)
}
func (sdk *SDK) UpdatePool(pool Pool) (Pool, error) {
	var value Pool
	return value, sdk.call(map[string]any{"op": "update_pool", "pool": pool}, &value)
}
func (sdk *SDK) DeletePool(pool Pool) error {
	return sdk.call(map[string]any{"op": "delete_pool", "pool": pool}, nil)
}
func (sdk *SDK) CreateClaim(request CreateClaimRequest) (Claim, error) {
	var value Claim
	return value, sdk.call(map[string]any{"op": "create_claim", "request": request}, &value)
}
func (sdk *SDK) ListClaims(namespace string) ([]Claim, error) {
	var value []Claim
	return value, sdk.call(map[string]any{"op": "list_claims", "namespace": namespace}, &value)
}
func (sdk *SDK) GetClaim(claim Claim) (Claim, error) {
	var value Claim
	return value, sdk.call(map[string]any{"op": "get_claim", "claim": claim}, &value)
}
func (sdk *SDK) UpdateClaim(claim Claim) (Claim, error) {
	var value Claim
	return value, sdk.call(map[string]any{"op": "update_claim", "claim": claim}, &value)
}
func (sdk *SDK) DeleteClaim(claim Claim) error {
	return sdk.call(map[string]any{"op": "delete_claim", "claim": claim}, nil)
}
func (sdk *SDK) WaitClaim(claim Claim) (Sandbox, error) {
	var value Sandbox
	return value, sdk.call(map[string]any{"op": "wait_claim", "claim": claim}, &value)
}
func (sdk *SDK) ServiceClient(sandbox Sandbox, service string) (*ServiceClient, error) {
	available := append([]string(nil), sandbox.Services...)
	sort.Strings(available)
	available = compactStrings(available)
	for _, candidate := range available {
		if candidate == service {
			return &ServiceClient{sdk: sdk, sandbox: sandbox, service: service}, nil
		}
	}
	return nil, &UnknownServiceError{Requested: service, Available: available}
}
func (client *ServiceClient) NewRequest(method, path string, body io.Reader) (*http.Request, error) {
	if err := validateServicePath(path); err != nil {
		return nil, err
	}
	request, err := http.NewRequest(method, "https://cyclops.invalid"+path, body)
	if err != nil {
		return nil, err
	}
	request.URL.Scheme = ""
	request.URL.Host = ""
	request.Host = ""
	return request, nil
}
func (client *ServiceClient) Do(request *http.Request) (*http.Response, error) {
	path, err := relativeRequestPath(request.URL)
	if err != nil {
		return nil, err
	}
	var body *string
	if request.Body != nil {
		data, readErr := io.ReadAll(request.Body)
		if readErr != nil {
			return nil, readErr
		}
		value := string(data)
		body = &value
	}
	headers := make([][2]string, 0, len(request.Header))
	for name, values := range request.Header {
		for _, value := range values {
			headers = append(headers, [2]string{name, value})
		}
	}
	var response serviceResponse
	err = client.sdk.call(map[string]any{
		"op":      "service_request",
		"sandbox": client.sandbox,
		"service": client.service,
		"request": serviceRequest{Method: request.Method, Path: path, Headers: headers, Body: body},
	}, &response)
	if err != nil {
		return nil, err
	}
	return &http.Response{
		StatusCode: response.Status,
		Status:     fmt.Sprintf("%d %s", response.Status, http.StatusText(response.Status)),
		Header:     make(http.Header),
		Body:       io.NopCloser(strings.NewReader(response.Body)),
		Request:    request,
	}, nil
}
func (sdk *SDK) Close() error {
	if sdk.input != nil {
		_ = sdk.input.Close()
	}
	if sdk.command != nil {
		return sdk.command.Wait()
	}
	return nil
}

func (sdk *SDK) call(request any, result any) error {
	if err := json.NewEncoder(sdk.input).Encode(request); err != nil {
		return err
	}
	for {
		line, err := sdk.output.ReadBytes('\n')
		if err != nil {
			return err
		}
		var message map[string]json.RawMessage
		if err := json.Unmarshal(line, &message); err != nil {
			return err
		}
		if kindRaw, ok := message["kind"]; ok {
			var call hostCall
			if err := json.Unmarshal(line, &call); err != nil {
				return err
			}
			value, dispatchErr := sdk.dispatch(call)
			response := map[string]any{"ok": dispatchErr == nil, "value": value}
			if dispatchErr != nil {
				response["error"] = map[string]string{"transport": dispatchErr.Error()}
			}
			if err := json.NewEncoder(sdk.input).Encode(response); err != nil {
				return err
			}
			_ = kindRaw
			continue
		}
		var response reply
		if err := json.Unmarshal(line, &response); err != nil {
			return err
		}
		if !response.OK {
			return fmt.Errorf("%s", response.Error)
		}
		if result != nil && len(response.Value) > 0 && string(response.Value) != "null" {
			return json.Unmarshal(response.Value, result)
		}
		return nil
	}
}
func (sdk *SDK) dispatch(call hostCall) (any, error) {
	switch call.Kind {
	case "http":
		body := io.Reader(nil)
		if call.Request.Body != nil {
			body = bytes.NewBufferString(*call.Request.Body)
		}
		request, err := http.NewRequest(call.Request.Method, call.Request.URL, body)
		if err != nil {
			return nil, err
		}
		for _, header := range call.Request.Headers {
			request.Header.Add(header[0], header[1])
		}
		client := &http.Client{CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
		response, err := client.Do(request)
		if err != nil {
			return nil, err
		}
		defer response.Body.Close()
		data, err := io.ReadAll(response.Body)
		return map[string]any{"status": response.StatusCode, "body": string(data)}, err
	case "sleep":
		time.Sleep(time.Duration(call.Milliseconds) * time.Millisecond)
		return nil, nil
	case "now_ms":
		return time.Now().UnixMilli(), nil
	case "acquire_oauth_credentials":
		return map[string]string{"token_url": sdk.configuration.OAuth.TokenURL, "client_id": sdk.configuration.OAuth.ClientID, "client_secret": sdk.configuration.OAuth.ClientSecret}, nil
	case "load_access_token":
		return sdk.token, nil
	case "store_access_token":
		sdk.token = call.Token
		return nil, nil
	default:
		return nil, fmt.Errorf("unknown host call %s", call.Kind)
	}
}
func validateServicePath(path string) error {
	if !strings.HasPrefix(path, "/") || strings.HasPrefix(path, "//") || strings.Contains(path, "#") {
		return fmt.Errorf("service request path must be relative and start with '/': %s", path)
	}
	return nil
}
func relativeRequestPath(value *url.URL) (string, error) {
	if value == nil || value.IsAbs() || value.Host != "" {
		return "", fmt.Errorf("service requests must use relative URLs")
	}
	path := value.EscapedPath()
	if path == "" {
		path = "/"
	}
	if value.RawQuery != "" {
		path += "?" + value.RawQuery
	}
	return path, validateServicePath(path)
}
func compactStrings(values []string) []string {
	if len(values) == 0 {
		return values
	}
	result := values[:1]
	for _, value := range values[1:] {
		if value != result[len(result)-1] {
			result = append(result, value)
		}
	}
	return result
}

func env(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}
