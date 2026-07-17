package templates

// Redis-backed persistence for pool templates.
//
// Schema:
//
//	pooltpl:<user>:<name>   HASH   user, name, created_at (RFC3339Nano), config
//	pooltpl:index:<user>    SET    template names owned by <user>
//
// The per-user index SET is the listing index — List() reads it and
// HGETALLs each member, so we never SCAN the whole keyspace (and never
// risk returning another user's templates). The `pooltpl:` prefix keeps
// this keyspace disjoint from the batch driver's `batch:*` keys when both
// share one Redis instance.
//
// Unlike the batch Store, writes here are synchronous: a template save is
// a deliberate, low-frequency user action (clicking "Save as template"),
// not a hot per-request mutation, so there's no CUA-535 latency concern to
// engineer around.

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"time"

	"github.com/redis/go-redis/v9"
)

const (
	keyPrefix   = "pooltpl:"
	indexPrefix = "pooltpl:index:"
)

// Store persists per-user pool templates. Every method takes the owning
// user so the storage layer enforces the same isolation the HTTP layer
// does — there is no way to address a template without naming its owner.
type Store interface {
	Save(ctx context.Context, t *Template) error
	Get(ctx context.Context, user, name string) (*Template, error)
	List(ctx context.Context, user string) ([]*Template, error)
	// Delete reports whether a template existed, so the handler can
	// distinguish 404 from a successful no-op.
	Delete(ctx context.Context, user, name string) (bool, error)
}

func tplKey(user, name string) string { return keyPrefix + user + ":" + name }
func indexKey(user string) string     { return indexPrefix + user }

type redisStore struct{ c *redis.Client }

// New connects to Redis and returns a Store. An empty url returns
// (nil, nil): the caller treats a nil Store as "templates disabled" and
// the handlers reply 503, rather than a no-op store silently dropping
// writes and confusing the user with an always-empty list. A malformed
// url is a genuine config error and is returned as such.
//
// The startup ping is best-effort: a transient Redis outage during a
// deploy must not permanently disable the feature, and the go-redis
// client reconnects on its own, so a failed ping is logged and the store
// is still returned. Real outages then surface as per-request 500s
// (handler errors), not as a silent always-503.
func New(ctx context.Context, url string) (Store, error) {
	if url == "" {
		return nil, nil
	}
	opts, err := redis.ParseURL(url)
	if err != nil {
		return nil, fmt.Errorf("parse redis url: %w", err)
	}
	c := redis.NewClient(opts)
	pingCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	if err := c.Ping(pingCtx).Err(); err != nil {
		slog.Warn("templates: redis ping failed at startup; will retry on demand", "err", err)
	}
	return &redisStore{c: c}, nil
}

// Save writes (or overwrites) a template and adds it to the owner's index.
// Overwriting an existing template preserves its original created_at — a
// re-save updates the config without resetting the template's age.
func (s *redisStore) Save(ctx context.Context, t *Template) error {
	key := tplKey(t.User, t.Name)

	createdAt := t.CreatedAt
	switch existing, err := s.c.HGet(ctx, key, "created_at").Result(); {
	case err == nil && existing != "":
		if parsed, perr := time.Parse(time.RFC3339Nano, existing); perr == nil {
			createdAt = parsed
		}
	case err != nil && err != redis.Nil:
		return fmt.Errorf("hget created_at: %w", err)
	}
	if createdAt.IsZero() {
		createdAt = time.Now().UTC()
	}
	t.CreatedAt = createdAt

	pipe := s.c.TxPipeline()
	pipe.HSet(ctx, key, map[string]interface{}{
		"user":       t.User,
		"name":       t.Name,
		"created_at": createdAt.UTC().Format(time.RFC3339Nano),
		"config":     string(t.Config),
	})
	pipe.SAdd(ctx, indexKey(t.User), t.Name)
	if _, err := pipe.Exec(ctx); err != nil {
		return fmt.Errorf("save template: %w", err)
	}
	return nil
}

// Get returns one template, or (nil, nil) if the user has no template by
// that name.
func (s *redisStore) Get(ctx context.Context, user, name string) (*Template, error) {
	fields, err := s.c.HGetAll(ctx, tplKey(user, name)).Result()
	if err != nil {
		return nil, fmt.Errorf("hgetall: %w", err)
	}
	if len(fields) == 0 {
		return nil, nil
	}
	return hydrate(user, name, fields), nil
}

// List returns all of the user's templates, newest first. Stale index
// entries (name present but hash gone, e.g. TTL/eviction) are self-healed
// with an SREM and skipped.
func (s *redisStore) List(ctx context.Context, user string) ([]*Template, error) {
	names, err := s.c.SMembers(ctx, indexKey(user)).Result()
	if err != nil {
		return nil, fmt.Errorf("smembers: %w", err)
	}
	out := make([]*Template, 0, len(names))
	for _, name := range names {
		fields, err := s.c.HGetAll(ctx, tplKey(user, name)).Result()
		if err != nil {
			return nil, fmt.Errorf("hgetall %s: %w", name, err)
		}
		if len(fields) == 0 {
			s.c.SRem(ctx, indexKey(user), name)
			continue
		}
		out = append(out, hydrate(user, name, fields))
	}
	sortByCreatedDesc(out)
	return out, nil
}

// Delete removes a template and its index entry. The bool reports whether
// the template existed so the handler can return 404 vs 204.
func (s *redisStore) Delete(ctx context.Context, user, name string) (bool, error) {
	pipe := s.c.TxPipeline()
	del := pipe.Del(ctx, tplKey(user, name))
	pipe.SRem(ctx, indexKey(user), name)
	if _, err := pipe.Exec(ctx); err != nil {
		return false, fmt.Errorf("delete template: %w", err)
	}
	return del.Val() > 0, nil
}

// hydrate builds a Template from a Redis HASH. user/name args are the
// addressed key components, used as a fallback if the stored fields are
// somehow missing.
func hydrate(user, name string, fields map[string]string) *Template {
	t := &Template{
		User:   fields["user"],
		Name:   fields["name"],
		Config: json.RawMessage(fields["config"]),
	}
	if t.User == "" {
		t.User = user
	}
	if t.Name == "" {
		t.Name = name
	}
	if v := fields["created_at"]; v != "" {
		if parsed, err := time.Parse(time.RFC3339Nano, v); err == nil {
			t.CreatedAt = parsed
		}
	}
	if len(t.Config) == 0 {
		t.Config = json.RawMessage("null")
	}
	return t
}

// sortByCreatedDesc orders templates newest-first. Insertion sort keeps
// the dependency surface zero (no sort import churn) and is trivially fast
// for the handful of templates a single user keeps.
func sortByCreatedDesc(ts []*Template) {
	for i := 1; i < len(ts); i++ {
		for j := i; j > 0 && ts[j].CreatedAt.After(ts[j-1].CreatedAt); j-- {
			ts[j], ts[j-1] = ts[j-1], ts[j]
		}
	}
}
