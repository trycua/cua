package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"time"

	cyclopssdk "github.com/trycua/cloud/cyclops-cs/sdk-bindings/go"
)

func runAgent(sdk *cyclopssdk.SDK, sandbox cyclopssdk.Sandbox) (any, error) {
	mcp, err := sdk.ServiceClient(sandbox, "mcp")
	if err != nil {
		return nil, err
	}
	body := []byte(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cyclops-sdk-go-example","version":"0.1.0"}}}`)
	deadline := time.Now().Add(5 * time.Minute)
	for {
		request, err := mcp.NewRequest("POST", "/mcp", bytes.NewReader(body))
		if err != nil {
			return nil, err
		}
		request.Header.Set("accept", "application/json, text/event-stream")
		request.Header.Set("content-type", "application/json")
		response, err := mcp.Do(request)
		if err != nil {
			return nil, err
		}
		responseBody, readErr := io.ReadAll(response.Body)
		response.Body.Close()
		if readErr != nil {
			return nil, readErr
		}
		if response.StatusCode >= 200 && response.StatusCode < 300 {
			return map[string]any{"namespace": sandbox.Namespace, "claim": sandbox.Claim, "sandbox": sandbox.Sandbox, "mcp_status": response.StatusCode}, nil
		}
		transient := response.StatusCode == 502 || response.StatusCode == 503 || response.StatusCode == 504
		if !transient || time.Now().After(deadline) {
			return nil, fmt.Errorf("MCP initialize failed with HTTP %d: %s", response.StatusCode, responseBody)
		}
		time.Sleep(5 * time.Second)
	}
}

func main() {
	namespace := env("CYCLOPS_NAMESPACE", fmt.Sprintf("sdk-example-%08x", uint32(time.Now().UnixNano())))
	sdk, err := cyclopssdk.Connect(cyclopssdk.Configuration{
		BaseURL: env("CUA_BASE_URL", "https://run.cua.ai"),
		OAuth: cyclopssdk.OAuthConfiguration{
			TokenURL: env("CUA_TOKEN_URL", "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"),
			ClientID: required("CUA_CLIENT_ID"), ClientSecret: required("CUA_CLIENT_SECRET"),
		},
	})
	must(err)
	defer func() { must(sdk.Close()) }()
	cpuCores := uint32(4)
	memory := "4Gi"
	imagePullSecret := required("CUA_IMAGE_PULL_SECRET")
	protocol := cyclopssdk.PoolSpecServiceProtocolTCP
	pool, err := sdk.CreatePool(cyclopssdk.CreatePoolRequest{
		Namespace: namespace,
		Spec: cyclopssdk.PoolSpec{
			Replicas: 1,
			Services: []cyclopssdk.PoolSpecService{{
				Name: "mcp", TargetPort: 3000, Protocol: &protocol,
			}},
			Template: cyclopssdk.PoolTemplate{
				ContainerDiskImage: required("CUA_IMAGE"), ImagePullSecret: &imagePullSecret, CpuCores: &cpuCores, Memory: &memory,
			},
		},
	})
	must(err)
	defer func() { must(sdk.DeletePool(pool)) }()
	claim, err := sdk.CreateClaim(cyclopssdk.CreateClaimRequest{Pool: pool})
	must(err)
	defer func() { must(sdk.DeleteClaim(claim)) }()
	sandbox, err := sdk.WaitClaim(claim)
	must(err)
	result, err := runAgent(sdk, sandbox)
	must(err)
	output, err := json.Marshal(result)
	must(err)
	fmt.Println(string(output))
}

func env(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}
func required(name string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	panic(name + " is required")
}
func must(err error) {
	if err != nil && !errors.Is(err, os.ErrProcessDone) {
		panic(err)
	}
}
