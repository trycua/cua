package main

import (
	"context"
	"fmt"
	"sync"

	"github.com/trycua/cloud/pkg/featureflags"
)

const featureFlagManagementModeEphemeral = "ephemeral"

func featureFlagManagementStoreFromEnv(getenv func(string) string) (featureflags.ManagementStore, bool, error) {
	mode := getenv("FEATURE_FLAG_MANAGEMENT_MODE")
	switch mode {
	case "":
		store, err := featureflags.NewManagementStore()
		return store, false, err
	case featureFlagManagementModeEphemeral:
		store, err := featureflags.NewEphemeralManagementStore(getenv("FEATURE_FLAG_MANAGEMENT_SEED"))
		return store, true, err
	default:
		return nil, false, fmt.Errorf("unsupported feature flag management mode %q", mode)
	}
}

type processLocalMutationLock struct {
	once  sync.Once
	token chan struct{}
}

func (l *processLocalMutationLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	l.once.Do(func() {
		l.token = make(chan struct{}, 1)
		l.token <- struct{}{}
	})
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-l.token:
	}
	defer func() { l.token <- struct{}{} }()
	if err := ctx.Err(); err != nil {
		return err
	}
	return callback(ctx)
}
