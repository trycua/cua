package auth

// User mirrors r33drichards/grt's auth.User shape — the value placed on
// the request context after a successful TokenAuthMiddleware. Extra
// claim values relevant to cyclops-cs (`namespace`, `azp`) are surfaced
// as struct fields rather than buried in the generic Claims map.
type User struct {
	ID                string // sub or mapped owner_sub for GitHub OIDC
	Name              string
	Email             string
	EmailVerified     bool
	AZP               string   // authorized party = client_id of the token issuer
	KeyClientPfx      string   // configured per-key client-id prefix, passed to OPA with the request
	Namespace         string   // hardcoded-claim mapper on per-key clients
	Groups            []string // groups for K8s impersonation (populated from user_groups claim for ukey- tokens)
	Claims            map[string]string
	PrincipalType     string
	Repository        string
	AllowedNamespaces []string
	PolicyIDs         []string
}

const (
	PrincipalTypeUser       = "user"
	PrincipalTypeUserKey    = "user_key"
	PrincipalTypeGitHubOIDC = "github_oidc"
)
