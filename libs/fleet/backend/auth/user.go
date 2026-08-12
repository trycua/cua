package auth

// User mirrors r33drichards/grt's auth.User shape — the value placed on
// the request context after a successful TokenAuthMiddleware. Extra
// claim values relevant to cyclops-cs (`namespace`, `azp`) are surfaced
// as struct fields rather than buried in the generic Claims map.
type User struct {
	ID        string   // sub
	Name      string
	Email     string
	AZP       string   // authorized party = client_id of the token issuer
	Namespace string   // hardcoded-claim mapper on per-key clients
	Groups    []string // groups for K8s impersonation (populated from user_groups claim for ukey- tokens)
	Claims    map[string]string
}
