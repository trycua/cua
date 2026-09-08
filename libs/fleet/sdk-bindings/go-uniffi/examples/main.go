//go:build ignore

package main

import (
	"bytes"
	"fmt"
	"io"
	"net/http"
	"os"

	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/fleet_sdk"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/cyclops_sdk_schema"
)

const (
	serviceName = "mcp"
	servicePath = "/health"
)

type netHTTPClient struct{ client *http.Client }

func (c netHTTPClient) Execute(request fleet_sdk.HttpRequest) (fleet_sdk.HttpResponse, error) {
	var body io.Reader
	if request.Body != nil {
		body = bytes.NewReader(*request.Body)
	}
	nativeRequest, err := http.NewRequest(request.Method, request.Url, body)
	if err != nil {
		return fleet_sdk.HttpResponse{}, err
	}
	for _, header := range request.Headers {
		nativeRequest.Header.Add(header.Name, header.Value)
	}
	nativeResponse, err := c.client.Do(nativeRequest)
	if err != nil {
		return fleet_sdk.HttpResponse{}, err
	}
	defer nativeResponse.Body.Close()
	responseBody, err := io.ReadAll(nativeResponse.Body)
	if err != nil {
		return fleet_sdk.HttpResponse{}, err
	}
	headers := make([]fleet_sdk.HttpHeader, 0, len(nativeResponse.Header))
	for name, values := range nativeResponse.Header {
		for _, value := range values {
			headers = append(headers, fleet_sdk.HttpHeader{Name: name, Value: value})
		}
	}
	return fleet_sdk.HttpResponse{Status: uint16(nativeResponse.StatusCode), Headers: headers, Body: responseBody}, nil
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

	credentials := fleet_sdk.NewCyclopsCredentials(clientID, clientSecret)
	client, err := fleet_sdk.CyclopsClientConnect(fleet_sdk.CyclopsConfiguration{
		BaseUrl: baseURL, TokenUrl: tokenURL, Credentials: credentials,
		PoolPollIntervalMs: 5000, PoolPollLimit: 100,
		ClaimPollIntervalMs: 5000, ClaimPollLimit: 120,
	}, netHTTPClient{client: &http.Client{}})
	if err != nil {
		return fmt.Errorf("connect client: %w", err)
	}
	defer client.Destroy()

	var pool *fleet_sdk.Pool
	var template *fleet_sdk.Template
	var claim *fleet_sdk.Claim
	defer func() {
		if claim != nil {
			fmt.Println("[cleanup] Deleting claim...")
			if err := client.DeleteClaim(*claim); err != nil {
				fmt.Fprintf(os.Stderr, "cleanup claim failed: %v\n", err)
			}
		}
		if template != nil {
			fmt.Println("[cleanup] Deleting template...")
			if err := client.DeleteTemplate(*template); err != nil {
				fmt.Fprintf(os.Stderr, "cleanup template failed: %v\n", err)
			}
		}
		if pool != nil {
			fmt.Println("[cleanup] Deleting pool...")
			if err := client.DeletePool(*pool); err != nil {
				fmt.Fprintf(os.Stderr, "cleanup pool failed: %v\n", err)
			}
		}
	}()

	fmt.Println("[1/5] Creating pool and template...")
	cpuCores := uint32(4)
	memory := "4Gi"
	templateName := namespace + "-template"
	poolSpec := cyclops_sdk_schema.OsGymSandboxWarmPoolSpec{Replicas: 1, SandboxTemplateRef: cyclops_sdk_schema.SandboxTemplateRef{Name: templateName}}
	createdPool, err := client.CreatePool(fleet_sdk.CreatePoolRequest{Namespace: namespace, Spec: poolSpec})
	if err != nil { return fmt.Errorf("create pool: %w", err) }
	pool = &createdPool
	fmt.Printf("Pool: %+v\n", *pool)

	services := []cyclops_sdk_schema.SandboxService{{Name: serviceName, TargetPort: 3000}}
	vmTemplate := cyclops_sdk_schema.VmTemplate{ContainerDiskImage: image, CpuCores: &cpuCores, Memory: &memory, Services: &services}
	if imagePullSecret := os.Getenv("CYCLOPS_IMAGE_PULL_SECRET"); imagePullSecret != "" {
		vmTemplate.ImagePullSecret = &imagePullSecret
	}
	createdTemplate, err := client.CreateTemplate(fleet_sdk.CreateTemplateRequest{Namespace: namespace, Name: templateName, Spec: cyclops_sdk_schema.OsGymSandboxTemplateSpec{VmTemplate: vmTemplate}})
	if err != nil { return fmt.Errorf("create template: %w", err) }
	template = &createdTemplate
	fmt.Printf("Template: %+v\n", *template)

	fmt.Println("[2/5] Creating claim...")
	createdClaim, err := client.CreateClaim(fleet_sdk.CreateClaimRequest{Pool: *pool})
	if err != nil { return fmt.Errorf("create claim: %w", err) }
	claim = &createdClaim
	fmt.Printf("Claim: %+v\n", *claim)

	fmt.Println("[3/5] Waiting for claim to bind a sandbox...")
	sandbox, err := client.WaitClaim(*claim)
	if err != nil { return fmt.Errorf("wait for claim: %w", err) }
	fmt.Printf("Sandbox: %+v\n", sandbox)

	fmt.Println("[4/5] Calling the sandbox service...")
	response, err := client.ServiceRequest(sandbox, serviceName, servicePath, fleet_sdk.HttpRequest{Method: http.MethodGet, Url: "https://ignored.invalid" + servicePath, Headers: []fleet_sdk.HttpHeader{}})
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
