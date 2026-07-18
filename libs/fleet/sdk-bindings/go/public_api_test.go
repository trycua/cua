package cyclopssdk

import (
	"net/http"
	"testing"
)

func TestTypedResourceAPIOnly(t *testing.T) {
	var _ func(Configuration) (*SDK, error) = Connect
	var _ func(*SDK, CreatePoolRequest) (Pool, error) = (*SDK).CreatePool
	var _ func(*SDK, string) ([]Pool, error) = (*SDK).ListPools
	var _ func(*SDK, Pool) (Pool, error) = (*SDK).GetPool
	var _ func(*SDK, Pool) (Pool, error) = (*SDK).UpdatePool
	var _ func(*SDK, Pool) error = (*SDK).DeletePool
	var _ func(*SDK, CreateClaimRequest) (Claim, error) = (*SDK).CreateClaim
	var _ func(*SDK, string) ([]Claim, error) = (*SDK).ListClaims
	var _ func(*SDK, Claim) (Claim, error) = (*SDK).GetClaim
	var _ func(*SDK, Claim) (Claim, error) = (*SDK).UpdateClaim
	var _ func(*SDK, Claim) error = (*SDK).DeleteClaim
	var _ func(*SDK, Claim) (Sandbox, error) = (*SDK).WaitClaim
	var _ func(*SDK, Sandbox, string) (*ServiceClient, error) = (*SDK).ServiceClient
	var _ func(*ServiceClient, *http.Request) (*http.Response, error) = (*ServiceClient).Do
	var _ func(*SDK) error = (*SDK).Close
}

func TestUnknownServiceErrorListsAvailableNames(t *testing.T) {
	_, err := (&SDK{}).ServiceClient(Sandbox{Services: []string{"novnc", "mcp", "mcp"}}, "metrics")
	serviceErr, ok := err.(*UnknownServiceError)
	if !ok {
		t.Fatalf("error = %T, want *UnknownServiceError", err)
	}
	if serviceErr.Requested != "metrics" {
		t.Fatalf("requested = %q", serviceErr.Requested)
	}
	if got := serviceErr.Available; len(got) != 2 || got[0] != "mcp" || got[1] != "novnc" {
		t.Fatalf("available = %v", got)
	}
}

func TestServiceClientBuildsRelativeRequests(t *testing.T) {
	client := &ServiceClient{}
	request, err := client.NewRequest("POST", "/mcp?session=one", nil)
	if err != nil {
		t.Fatal(err)
	}
	if request.URL.IsAbs() || request.URL.Host != "" {
		t.Fatalf("URL = %s, want relative", request.URL)
	}
	if path, err := relativeRequestPath(request.URL); err != nil || path != "/mcp?session=one" {
		t.Fatalf("path = %q, err = %v", path, err)
	}
}
