// Package templates provides Redis-backed, per-user pool templates for
// cyclops-cs.
//
// A template is a saved OSGymWorkspacePool config the user can reuse to
// stamp out new pools without re-entering every field. It is the
// persistent, per-user evolution of the SPA's existing in-memory
// "Duplicate pool" action.
//
// Each template is stored exactly as requested — four fields:
//
//	user        the owning user (Keycloak sub)
//	created_at  when the template was first saved (RFC3339Nano)
//	name        the template's name (unique per user)
//	config      the opaque pool-config JSON the SPA sends to createPool
//
// The HTTP layer scopes every call to the caller's sub, and the Redis
// key layout (see store.go) namespaces every key by user, so one user
// can never see or clobber another's templates.
package templates

import (
	"encoding/json"
	"time"
)

// Template is a single saved pool config owned by one user. Config is
// kept opaque (json.RawMessage) so the backend never has to track the
// evolving OSGymWorkspacePool spec — the SPA owns that shape.
type Template struct {
	User      string          `json:"user"`
	Name      string          `json:"name"`
	CreatedAt time.Time       `json:"createdAt"`
	Config    json.RawMessage `json:"config"`
}
