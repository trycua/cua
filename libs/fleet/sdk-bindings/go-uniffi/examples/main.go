//go:build ignore

package main

import (
	"bytes"
	"fmt"
	"io"
	"net/http"
	"os"

	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
)

const (
	serviceName = "mcp"
	servicePath = "/health"
)

type netHTTPClient struct{ client *http.Client }

func (c netHTTPClient) Execute(request cyclops_sdk.HttpRequest) (cyclops_sdk.HttpResponse, error) {
	var body io.Reader
	if request.Body != nil {
		body = bytes.NewReader(*request.Body)
	}
	nativeRequest, err := http.NewRequest(request.Method, request.Url, body)
	if err != nil {
		return cyclops_sdk.HttpResponse{}, err
	}
	for _, header := range request.Headers {
		nativeRequest.Header.Add(header.Name, header.Value)
	}
	nativeResponse, err := c.client.Do(nativeRequest)
	if err != nil {
		return cyclops_sdk.HttpResponse{}, err
	}
	defer nativeResponse.Body.Close()
	responseBody, err := io.ReadAll(nativeResponse.Body)
	if err != nil {
		return cyclops_sdk.HttpResponse{}, err
	}
	headers := make([]cyclops_sdk.HttpHeader, 0, len(nativeResponse.Header))
	for name, values := range nativeResponse.Header {
		for _, value := range values {
			headers = append(headers, cyclops_sdk.HttpHeader{Name: name, Value: value})
		}
	}
	return cyclops_sdk.HttpResponse{Status: uint16(nativeResponse.StatusCode), Headers: headers, Body: responseBody}, nil
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintf(os.Stderr, "Lifecycle failed: %v\n", err)
		os.Exit(1)
	}
}

func run() error {
	baseURL := requiredEnv("CYCLOPS_BASE_URL")
	tokenURL := requiredEnv("CYCLOPS_TOKEN_URL")
	clientID := requiredEnv("CYCLOPS_CLIENT_ID")
	clientSecret := requiredEnv("CYCLOPS_CLIENT_SECRET")
	namespace := requiredEnv("CYCLOPS_NAMESPACE")
	image := requiredEnv("CYCLOPS_IMAGE")
	if baseURL == "" || tokenURL == "" || clientID == "" || clientSecret == "" || namespace == "" || image == "" {
		return fmt.Errorf("set all required CYCLOPS_* environment variables; see README.md")
	}

	credentials := cyclops_sdk.NewCyclopsCredentials(clientID, clientSecret)
	client, err := cyclops_sdk.CyclopsClientConnect(cyclops_sdk.CyclopsConfiguration{
		BaseUrl: baseURL, TokenUrl: tokenURL, Credentials: credentials,
		PoolPollIntervalMs: 5000, PoolPollLimit: 100,
		ClaimPollIntervalMs: 5000, ClaimPollLimit: 120,
	}, netHTTPClient{client: &http.Client{}})
	if err != nil {
		return fmt.Errorf("connect client: %w", err)
	}
	defer client.Destroy()

	var pool *cyclops_sdk.Pool
	var claim *cyclops_sdk.Claim
	defer func() {
		if claim != nil {
			fmt.Println("[cleanup] Deleting claim...")
			if err := client.DeleteClaim(*claim); err != nil {
				fmt.Fprintf(os.Stderr, "cleanup claim failed: %v\n", err)
			}
		}
		if pool != nil {
			fmt.Println("[cleanup] Deleting pool...")
			if err := client.DeletePool(*pool); err != nil {
				fmt.Fprintf(os.Stderr, "cleanup pool failed: %v\n", err)
			}
		}
	}()

	fmt.Println("[1/5] Creating pool...")
	cpuCores := uint32(4)
	memory := "4Gi"
	poolSpec := cyclops_sdk_schema.PoolSpec{Replicas: 1, Template: cyclops_sdk_schema.PoolTemplate{ContainerDiskImage: image, CpuCores: &cpuCores, Memory: &memory}}
	if imagePullSecret := os.Getenv("CYCLOPS_IMAGE_PULL_SECRET"); imagePullSecret != "" {
		poolSpec.Template.ImagePullSecret = &imagePullSecret
	}
	services := []cyclops_sdk_schema.SandboxService{{Name: serviceName, TargetPort: 3000}}
	poolSpec.Services = &services
	createdPool, err := client.CreatePool(cyclops_sdk.CreatePoolRequest{Namespace: namespace, Spec: poolSpec})
	if err != nil { return fmt.Errorf("create pool: %w", err) }
	pool = &createdPool
	fmt.Printf("Pool: %+v\n", *pool)

	fmt.Println("[2/5] Creating claim...")
	createdClaim, err := client.CreateClaim(cyclops_sdk.CreateClaimRequest{Pool: *pool})
	if err != nil { return fmt.Errorf("create claim: %w", err) }
	claim = &createdClaim
	fmt.Printf("Claim: %+v\n", *claim)

	fmt.Println("[3/5] Waiting for claim to bind a sandbox...")
	sandbox, err := client.WaitClaim(*claim)
	if err != nil { return fmt.Errorf("wait for claim: %w", err) }
	fmt.Printf("Sandbox: %+v\n", sandbox)

	fmt.Println("[4/5] Calling the sandbox service...")
	response, err := client.ServiceRequest(sandbox, serviceName, servicePath, cyclops_sdk.HttpRequest{Method: http.MethodGet, Url: "https://ignored.invalid" + servicePath, Headers: []cyclops_sdk.HttpHeader{}})
	if err != nil { return fmt.Errorf("service request: %w", err) }
	fmt.Printf("Service response: status=%d body=%q\n", response.Status, response.Body)
	fmt.Println("[5/5] Lifecycle completed; cleanup will now run.")
	return nil
}

func requiredEnv(name string) string {
	value := os.Getenv(name)
	if value == "" { fmt.Fprintf(os.Stderr, "missing required environment variable: %s\n", name) }
	return value
}
