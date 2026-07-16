package client

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

const defaultTimeout = 30 * time.Second

type Config struct {
	Endpoint     string
	AccessToken  string
	ClientID     string
	ClientSecret string
	TokenURL     string
	HTTPClient   *http.Client
}

type Client struct {
	endpoint     string
	accessToken  string
	clientID     string
	clientSecret string
	tokenURL     string
	httpClient   *http.Client

	mu          sync.Mutex
	cachedToken string
	tokenExpiry time.Time
}

type APIError struct {
	StatusCode int
	Method     string
	URL        string
	Body       string
}

func (e *APIError) Error() string {
	return fmt.Sprintf("cyclops API %s %s returned %d: %s", e.Method, e.URL, e.StatusCode, e.Body)
}

func IsNotFound(err error) bool {
	var apiErr *APIError
	return errors.As(err, &apiErr) && apiErr.StatusCode == http.StatusNotFound
}

func IsConflict(err error) bool {
	var apiErr *APIError
	return errors.As(err, &apiErr) && apiErr.StatusCode == http.StatusConflict
}

func New(cfg Config) (*Client, error) {
	endpoint := strings.TrimRight(strings.TrimSpace(cfg.Endpoint), "/")
	if endpoint == "" {
		return nil, errors.New("endpoint is required")
	}
	if _, err := url.ParseRequestURI(endpoint); err != nil {
		return nil, fmt.Errorf("invalid endpoint: %w", err)
	}
	if cfg.AccessToken == "" && (cfg.ClientID == "" || cfg.ClientSecret == "" || cfg.TokenURL == "") {
		return nil, errors.New("set access_token or client_id, client_secret, and token_url")
	}
	httpClient := cfg.HTTPClient
	if httpClient == nil {
		httpClient = &http.Client{Timeout: defaultTimeout}
	}
	return &Client{
		endpoint: endpoint, accessToken: cfg.AccessToken, clientID: cfg.ClientID,
		clientSecret: cfg.ClientSecret, tokenURL: cfg.TokenURL, httpClient: httpClient,
	}, nil
}

func (c *Client) token(ctx context.Context) (string, error) {
	if c.accessToken != "" {
		return c.accessToken, nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.cachedToken != "" && time.Now().Add(30*time.Second).Before(c.tokenExpiry) {
		return c.cachedToken, nil
	}
	form := url.Values{"grant_type": {"client_credentials"}}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.tokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.SetBasicAuth(c.clientID, c.clientSecret)
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("request OAuth token: %w", err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("read OAuth token response: %w", err)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", &APIError{StatusCode: resp.StatusCode, Method: http.MethodPost, URL: c.tokenURL, Body: compactBody(body)}
	}
	var token struct {
		AccessToken string `json:"access_token"`
		ExpiresIn   int64  `json:"expires_in"`
	}
	if err := json.Unmarshal(body, &token); err != nil {
		return "", fmt.Errorf("decode OAuth token response: %w", err)
	}
	if token.AccessToken == "" {
		return "", errors.New("OAuth token response did not contain access_token")
	}
	c.cachedToken = token.AccessToken
	if token.ExpiresIn <= 0 {
		token.ExpiresIn = 300
	}
	c.tokenExpiry = time.Now().Add(time.Duration(token.ExpiresIn) * time.Second)
	return c.cachedToken, nil
}

func (c *Client) do(ctx context.Context, method, path, contentType string, requestBody, responseBody any) error {
	var body io.Reader
	if requestBody != nil {
		encoded, err := json.Marshal(requestBody)
		if err != nil {
			return fmt.Errorf("encode request: %w", err)
		}
		body = bytes.NewReader(encoded)
	}
	token, err := c.token(ctx)
	if err != nil {
		return err
	}
	requestURL := c.endpoint + path
	req, err := http.NewRequestWithContext(ctx, method, requestURL, body)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")
	if requestBody != nil {
		if contentType == "" {
			contentType = "application/json"
		}
		req.Header.Set("Content-Type", contentType)
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("request Cyclops API: %w", err)
	}
	defer resp.Body.Close()
	responseBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("read Cyclops API response: %w", err)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return &APIError{StatusCode: resp.StatusCode, Method: method, URL: requestURL, Body: compactBody(responseBytes)}
	}
	if responseBody != nil && len(responseBytes) > 0 {
		if err := json.Unmarshal(responseBytes, responseBody); err != nil {
			return fmt.Errorf("decode Cyclops API response: %w", err)
		}
	}
	return nil
}

func compactBody(body []byte) string {
	const limit = 2048
	text := strings.TrimSpace(string(body))
	if len(text) > limit {
		return text[:limit] + "..."
	}
	return text
}

func escape(value string) string { return url.PathEscape(value) }
