package provider

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/provider/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/hashicorp/terraform-plugin-framework/types/basetypes"
	"github.com/trycua/cloud/cyclops-cs/sdk-bindings/go-uniffi/fleet_sdk"
)

const (
	sdkPollIntervalMs uint64 = 5000
	sdkPollLimit      uint32 = 100
)

type fleetsProvider struct{ version string }

type providerModel struct {
	Endpoint     types.String `tfsdk:"endpoint"`
	AccessToken  types.String `tfsdk:"access_token"`
	ClientID     types.String `tfsdk:"client_id"`
	ClientSecret types.String `tfsdk:"client_secret"`
	TokenURL     types.String `tfsdk:"token_url"`
}

func New(version string) func() provider.Provider {
	return func() provider.Provider { return &fleetsProvider{version: version} }
}

func (p *fleetsProvider) Metadata(_ context.Context, _ provider.MetadataRequest, resp *provider.MetadataResponse) {
	resp.TypeName = "fleets"
	resp.Version = p.version
}

func (p *fleetsProvider) Schema(_ context.Context, _ provider.SchemaRequest, resp *provider.SchemaResponse) {
	resp.Schema = schema.Schema{
		Description: "Manage Cua Fleet computer-use pools.",
		Attributes: map[string]schema.Attribute{
			"endpoint":      schema.StringAttribute{Required: true, Description: "Cyclops base URL, for example https://cyclops.example.com."},
			"access_token":  schema.StringAttribute{Optional: true, Sensitive: true, Description: "Bearer token. May also be set with CYCLOPS_ACCESS_TOKEN."},
			"client_id":     schema.StringAttribute{Optional: true, Description: "Cyclops user-key client ID. May also be set with CYCLOPS_CLIENT_ID."},
			"client_secret": schema.StringAttribute{Optional: true, Sensitive: true, Description: "Cyclops user-key client secret. May also be set with CYCLOPS_CLIENT_SECRET."},
			"token_url":     schema.StringAttribute{Optional: true, Description: "OAuth token endpoint returned with the Cyclops user key. May also be set with CYCLOPS_TOKEN_URL."},
		},
	}
}

func (p *fleetsProvider) Configure(ctx context.Context, req provider.ConfigureRequest, resp *provider.ConfigureResponse) {
	var data providerModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &data)...)
	if resp.Diagnostics.HasError() {
		return
	}
	value := func(config types.String, env string) string {
		if !config.IsNull() && !config.IsUnknown() && config.ValueString() != "" {
			return config.ValueString()
		}
		return os.Getenv(env)
	}
	apiClient, err := newCyclopsClient(
		value(data.Endpoint, "CYCLOPS_ENDPOINT"),
		value(data.AccessToken, "CYCLOPS_ACCESS_TOKEN"),
		value(data.ClientID, "CYCLOPS_CLIENT_ID"),
		value(data.ClientSecret, "CYCLOPS_CLIENT_SECRET"),
		value(data.TokenURL, "CYCLOPS_TOKEN_URL"),
	)
	if err != nil {
		resp.Diagnostics.AddError("Invalid Fleets provider configuration", err.Error())
		return
	}
	resp.DataSourceData = apiClient
	resp.ResourceData = apiClient
}

func (p *fleetsProvider) Resources(_ context.Context) []func() resource.Resource {
	return []func() resource.Resource{NewPoolResource, NewLegacyPoolResource}
}

func (p *fleetsProvider) DataSources(_ context.Context) []func() datasource.DataSource { return nil }

func newCyclopsClient(endpoint, accessToken, clientID, clientSecret, tokenURL string) (*fleet_sdk.CyclopsClient, error) {
	endpoint = strings.TrimRight(strings.TrimSpace(endpoint), "/")
	if endpoint == "" {
		return nil, fmt.Errorf("endpoint is required")
	}
	client := sdkHTTPClient{client: &http.Client{Timeout: 30 * time.Second}}
	if accessToken != "" {
		return fleet_sdk.CyclopsClientConnectWithAccessToken(fleet_sdk.CyclopsTokenProviderConfiguration{
			BaseUrl: endpoint, PoolPollIntervalMs: sdkPollIntervalMs, PoolPollLimit: sdkPollLimit,
			ClaimPollIntervalMs: sdkPollIntervalMs, ClaimPollLimit: sdkPollLimit,
		}, accessToken, client)
	}
	if clientID == "" || clientSecret == "" || tokenURL == "" {
		return nil, fmt.Errorf("set access_token or client_id, client_secret, and token_url")
	}
	return fleet_sdk.CyclopsClientConnect(fleet_sdk.CyclopsConfiguration{
		BaseUrl: endpoint, TokenUrl: tokenURL, Credentials: fleet_sdk.NewCyclopsCredentials(clientID, clientSecret),
		PoolPollIntervalMs: sdkPollIntervalMs, PoolPollLimit: sdkPollLimit,
		ClaimPollIntervalMs: sdkPollIntervalMs, ClaimPollLimit: sdkPollLimit,
	}, client)
}

type sdkHTTPClient struct{ client *http.Client }

func (c sdkHTTPClient) Execute(request fleet_sdk.HttpRequest) (fleet_sdk.HttpResponse, error) {
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

func objectValue(ctx context.Context, object types.Object, target any, diagnostics *diag.Diagnostics) bool {
	if object.IsNull() || object.IsUnknown() {
		return false
	}
	diagnostics.Append(object.As(ctx, target, basetypes.ObjectAsOptions{})...)
	return !diagnostics.HasError()
}

var _ provider.Provider = (*fleetsProvider)(nil)
