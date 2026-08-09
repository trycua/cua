package githubtrust

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"time"

	"github.com/google/uuid"
	"github.com/redis/go-redis/v9"
)

const (
	keyPrefix        = "ghtrust:policy:"
	ownerIndexPrefix = "ghtrust:owner:"
	repoIndexPrefix  = "ghtrust:repo:"
)

type Store interface {
	List(ctx context.Context, ownerSub string) ([]*Policy, error)
	Create(ctx context.Context, policy *Policy) error
	Get(ctx context.Context, ownerSub, id string) (*Policy, error)
	Update(ctx context.Context, policy *Policy) error
	Delete(ctx context.Context, ownerSub, id string) (bool, error)
	ResolveByRepository(ctx context.Context, repository string) ([]*Policy, error)
}

type redisStore struct{ c *redis.Client }

func policyKey(id string) string   { return keyPrefix + id }
func ownerKey(owner string) string { return ownerIndexPrefix + owner }
func repoKey(repo string) string   { return repoIndexPrefix + repo }

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
		slog.Warn("githubtrust: redis ping failed at startup; will retry on demand", "err", err)
	}
	return &redisStore{c: c}, nil
}

func (s *redisStore) List(ctx context.Context, ownerSub string) ([]*Policy, error) {
	ids, err := s.c.SMembers(ctx, ownerKey(ownerSub)).Result()
	if err != nil {
		return nil, fmt.Errorf("smembers owner index: %w", err)
	}
	out := make([]*Policy, 0, len(ids))
	for _, id := range ids {
		fields, err := s.c.HGetAll(ctx, policyKey(id)).Result()
		if err != nil {
			return nil, fmt.Errorf("hgetall %s: %w", id, err)
		}
		if len(fields) == 0 {
			s.c.SRem(ctx, ownerKey(ownerSub), id)
			continue
		}
		policy, err := hydrate(fields)
		if err != nil {
			return nil, err
		}
		if policy.OwnerSub != ownerSub {
			continue
		}
		out = append(out, policy)
	}
	sortByUpdatedDesc(out)
	return out, nil
}

func (s *redisStore) Create(ctx context.Context, policy *Policy) error {
	now := time.Now().UTC()
	if policy.ID == "" {
		policy.ID = uuid.NewString()
	}
	if policy.CreatedAt.IsZero() {
		policy.CreatedAt = now
	}
	policy.UpdatedAt = now
	return s.save(ctx, nil, policy)
}

func (s *redisStore) Get(ctx context.Context, ownerSub, id string) (*Policy, error) {
	fields, err := s.c.HGetAll(ctx, policyKey(id)).Result()
	if err != nil {
		return nil, fmt.Errorf("hgetall: %w", err)
	}
	if len(fields) == 0 {
		return nil, nil
	}
	policy, err := hydrate(fields)
	if err != nil {
		return nil, err
	}
	if policy.OwnerSub != ownerSub {
		return nil, nil
	}
	return policy, nil
}

func (s *redisStore) Update(ctx context.Context, policy *Policy) error {
	current, err := s.Get(ctx, policy.OwnerSub, policy.ID)
	if err != nil {
		return err
	}
	if current == nil {
		return redis.Nil
	}
	policy.CreatedAt = current.CreatedAt
	policy.UpdatedAt = time.Now().UTC()
	return s.save(ctx, current, policy)
}

func (s *redisStore) Delete(ctx context.Context, ownerSub, id string) (bool, error) {
	current, err := s.Get(ctx, ownerSub, id)
	if err != nil {
		return false, err
	}
	if current == nil {
		return false, nil
	}
	pipe := s.c.TxPipeline()
	pipe.Del(ctx, policyKey(id))
	pipe.SRem(ctx, ownerKey(ownerSub), id)
	pipe.SRem(ctx, repoKey(current.Repository), id)
	if _, err := pipe.Exec(ctx); err != nil {
		return false, fmt.Errorf("delete policy: %w", err)
	}
	return true, nil
}

func (s *redisStore) ResolveByRepository(ctx context.Context, repository string) ([]*Policy, error) {
	ids, err := s.c.SMembers(ctx, repoKey(repository)).Result()
	if err != nil {
		return nil, fmt.Errorf("smembers repo index: %w", err)
	}
	out := make([]*Policy, 0, len(ids))
	for _, id := range ids {
		fields, err := s.c.HGetAll(ctx, policyKey(id)).Result()
		if err != nil {
			return nil, fmt.Errorf("hgetall %s: %w", id, err)
		}
		if len(fields) == 0 {
			s.c.SRem(ctx, repoKey(repository), id)
			continue
		}
		policy, err := hydrate(fields)
		if err != nil {
			return nil, err
		}
		if policy.Repository != repository {
			continue
		}
		out = append(out, policy)
	}
	sortByUpdatedDesc(out)
	return out, nil
}

func (s *redisStore) save(ctx context.Context, previous, policy *Policy) error {
	allowed, err := json.Marshal(policy.AllowedNamespaces)
	if err != nil {
		return fmt.Errorf("marshal allowed_namespaces: %w", err)
	}
	pipe := s.c.TxPipeline()
	pipe.HSet(ctx, policyKey(policy.ID), map[string]any{
		"id":                 policy.ID,
		"owner_sub":          policy.OwnerSub,
		"name":               policy.Name,
		"repository":         policy.Repository,
		"allowed_namespaces": string(allowed),
		"enabled":            policy.Enabled,
		"created_at":         policy.CreatedAt.UTC().Format(time.RFC3339Nano),
		"updated_at":         policy.UpdatedAt.UTC().Format(time.RFC3339Nano),
	})
	pipe.SAdd(ctx, ownerKey(policy.OwnerSub), policy.ID)
	pipe.SAdd(ctx, repoKey(policy.Repository), policy.ID)
	if previous != nil && previous.Repository != "" && previous.Repository != policy.Repository {
		pipe.SRem(ctx, repoKey(previous.Repository), policy.ID)
	}
	if _, err := pipe.Exec(ctx); err != nil {
		return fmt.Errorf("save policy: %w", err)
	}
	return nil
}

func hydrate(fields map[string]string) (*Policy, error) {
	policy := &Policy{
		ID:         fields["id"],
		OwnerSub:   fields["owner_sub"],
		Name:       fields["name"],
		Repository: fields["repository"],
	}
	if v := fields["allowed_namespaces"]; v != "" {
		if err := json.Unmarshal([]byte(v), &policy.AllowedNamespaces); err != nil {
			return nil, fmt.Errorf("decode allowed_namespaces: %w", err)
		}
	}
	if v := fields["enabled"]; v == "1" || v == "true" {
		policy.Enabled = true
	}
	if v := fields["created_at"]; v != "" {
		if parsed, err := time.Parse(time.RFC3339Nano, v); err == nil {
			policy.CreatedAt = parsed
		}
	}
	if v := fields["updated_at"]; v != "" {
		if parsed, err := time.Parse(time.RFC3339Nano, v); err == nil {
			policy.UpdatedAt = parsed
		}
	}
	return policy, nil
}
