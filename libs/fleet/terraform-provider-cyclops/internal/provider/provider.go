package provider

import (
	"context"
	"os"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/provider/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"
	"github.com/hashicorp/terraform-plugin-framework/types/basetypes"
	"github.com/trycua/terraform-provider-cyclops/internal/client"
)

type cyclopsProvider struct{ version string }

type providerModel struct {
	Endpoint     types.String `tfsdk:"endpoint"`
	AccessToken  types.String `tfsdk:"access_token"`
	ClientID     types.String `tfsdk:"client_id"`
	ClientSecret types.String `tfsdk:"client_secret"`
	TokenURL     types.String `tfsdk:"token_url"`
}

func New(version string) func() provider.Provider {
	return func() provider.Provider { return &cyclopsProvider{version: version} }
}

func (p *cyclopsProvider) Metadata(_ context.Context, _ provider.MetadataRequest, resp *provider.MetadataResponse) {
	resp.TypeName = "cyclops"
	resp.Version = p.version
}

func (p *cyclopsProvider) Schema(_ context.Context, _ provider.SchemaRequest, resp *provider.SchemaResponse) {
	resp.Schema = schema.Schema{
		Description: "Manage Cyclops computer-use pools.",
		Attributes: map[string]schema.Attribute{
			"endpoint":      schema.StringAttribute{Required: true, Description: "Cyclops base URL, for example https://cyclops.example.com."},
			"access_token":  schema.StringAttribute{Optional: true, Sensitive: true, Description: "Bearer token. May also be set with CYCLOPS_ACCESS_TOKEN."},
			"client_id":     schema.StringAttribute{Optional: true, Description: "Cyclops user-key client ID. May also be set with CYCLOPS_CLIENT_ID."},
			"client_secret": schema.StringAttribute{Optional: true, Sensitive: true, Description: "Cyclops user-key client secret. May also be set with CYCLOPS_CLIENT_SECRET."},
			"token_url":     schema.StringAttribute{Optional: true, Description: "OAuth token endpoint returned with the Cyclops user key. May also be set with CYCLOPS_TOKEN_URL."},
		},
	}
}

func (p *cyclopsProvider) Configure(ctx context.Context, req provider.ConfigureRequest, resp *provider.ConfigureResponse) {
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
	apiClient, err := client.New(client.Config{
		Endpoint: value(data.Endpoint, "CYCLOPS_ENDPOINT"), AccessToken: value(data.AccessToken, "CYCLOPS_ACCESS_TOKEN"),
		ClientID: value(data.ClientID, "CYCLOPS_CLIENT_ID"), ClientSecret: value(data.ClientSecret, "CYCLOPS_CLIENT_SECRET"),
		TokenURL: value(data.TokenURL, "CYCLOPS_TOKEN_URL"),
	})
	if err != nil {
		resp.Diagnostics.AddError("Invalid Cyclops provider configuration", err.Error())
		return
	}
	resp.DataSourceData = apiClient
	resp.ResourceData = apiClient
}

func (p *cyclopsProvider) Resources(_ context.Context) []func() resource.Resource {
	return []func() resource.Resource{NewPoolResource}
}

func (p *cyclopsProvider) DataSources(_ context.Context) []func() datasource.DataSource { return nil }

var _ provider.Provider = (*cyclopsProvider)(nil)

func objectValue(ctx context.Context, object types.Object, target any, diagnostics *diag.Diagnostics) bool {
	if object.IsNull() || object.IsUnknown() {
		return false
	}
	diagnostics.Append(object.As(ctx, target, basetypes.ObjectAsOptions{})...)
	return !diagnostics.HasError()
}
