package main

import (
	"context"
	"errors"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestFeatureFlagManagementStoreFromEnvUsesExplicitEphemeralMode(t *testing.T) {
	t.Setenv("CYCLOPS_CS_ADMIN_SUBS", "original")
	values := map[string]string{
		"FEATURE_FLAG_MANAGEMENT_MODE": "ephemeral",
		"FEATURE_FLAG_MANAGEMENT_SEED": `[{"name":"/feature-flags/cyclops-cs/admin-subs","value":"[\"preview-admin\"]","tags":{"ManagedBy":"terraform"}}]`,
	}

	store, processLocal, err := featureFlagManagementStoreFromEnv(func(key string) string { return values[key] })
	if err != nil {
		t.Fatalf("featureFlagManagementStoreFromEnv() error = %v", err)
	}
	if !processLocal {
		t.Fatal("featureFlagManagementStoreFromEnv() processLocal = false, want true")
	}
	parameter, err := store.Get(context.Background(), "/feature-flags/cyclops-cs/admin-subs")
	if err != nil {
		t.Fatalf("Get() error = %v", err)
	}
	if parameter.Value != `["preview-admin"]` || parameter.Tags["ManagedBy"] != "terraform" {
		t.Fatalf("Get() = %#v", parameter)
	}
}

func TestFeatureFlagManagementStoreFromEnvRejectsUnknownMode(t *testing.T) {
	_, _, err := featureFlagManagementStoreFromEnv(func(key string) string {
		if key == "FEATURE_FLAG_MANAGEMENT_MODE" {
			return "production-ish"
		}
		return ""
	})
	if err == nil || !strings.Contains(err.Error(), "production-ish") {
		t.Fatalf("featureFlagManagementStoreFromEnv() error = %v", err)
	}
}

func TestProcessLocalMutationLockRunsCallback(t *testing.T) {
	called := false
	if err := (&processLocalMutationLock{}).WithLock(context.Background(), func(context.Context) error {
		called = true
		return nil
	}); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	if !called {
		t.Fatal("WithLock() did not run callback")
	}
}

func TestProcessLocalMutationLockSerializesCallbacks(t *testing.T) {
	lock := &processLocalMutationLock{}
	var active atomic.Int32
	var maximum atomic.Int32
	var wait sync.WaitGroup

	for range 8 {
		wait.Add(1)
		go func() {
			defer wait.Done()
			if err := lock.WithLock(context.Background(), func(context.Context) error {
				current := active.Add(1)
				if current > maximum.Load() {
					maximum.Store(current)
				}
				time.Sleep(time.Millisecond)
				active.Add(-1)
				return nil
			}); err != nil {
				t.Errorf("WithLock() error = %v", err)
			}
		}()
	}
	wait.Wait()
	if maximum.Load() != 1 {
		t.Fatalf("maximum concurrent callbacks = %d, want 1", maximum.Load())
	}
}

func TestProcessLocalMutationLockHonorsCancellationWhileQueued(t *testing.T) {
	lock := &processLocalMutationLock{}
	entered := make(chan struct{})
	release := make(chan struct{})
	firstDone := make(chan error, 1)
	go func() {
		firstDone <- lock.WithLock(context.Background(), func(context.Context) error {
			close(entered)
			<-release
			return nil
		})
	}()
	<-entered

	ctx, cancel := context.WithCancel(context.Background())
	called := false
	secondDone := make(chan error, 1)
	go func() {
		secondDone <- lock.WithLock(ctx, func(context.Context) error {
			called = true
			return nil
		})
	}()
	time.Sleep(10 * time.Millisecond)
	cancel()

	select {
	case err := <-secondDone:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("WithLock() error = %v, want context.Canceled", err)
		}
	case <-time.After(100 * time.Millisecond):
		close(release)
		t.Fatal("WithLock() did not return after context cancellation")
	}
	if called {
		t.Fatal("WithLock() ran callback after context cancellation")
	}
	close(release)
	if err := <-firstDone; err != nil {
		t.Fatalf("first WithLock() error = %v", err)
	}
}
