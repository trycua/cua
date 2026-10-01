package featureflagadmin

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

func TestKubernetesLeaseLockAcquiresFreeLeaseWithResourceVersion(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "7"}})
	defer server.Close()
	clock := fixedClock(time.Date(2026, 8, 13, 12, 0, 0, 0, time.UTC))
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Millisecond, clock)

	calls := 0
	if err := lock.WithLock(context.Background(), func(context.Context) error { calls++; return nil }); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	if calls != 1 {
		t.Fatalf("callback calls = %d, want 1", calls)
	}
	requests := server.Requests()
	if len(requests) != 3 || requests[0].Method != http.MethodGet || requests[1].Method != http.MethodPut || requests[2].Method != http.MethodPut {
		t.Fatalf("requests = %#v", requests)
	}
	if requests[1].Lease.Metadata.ResourceVersion != "7" || requests[1].Lease.Spec.HolderIdentity != "pod-a" {
		t.Fatalf("acquire request = %#v", requests[1].Lease)
	}
	if requests[2].Lease.Spec.HolderIdentity != "" {
		t.Fatalf("release request = %#v", requests[2].Lease)
	}
}

func TestKubernetesLeaseLockTakesOverExpiredLease(t *testing.T) {
	now := time.Date(2026, 8, 13, 12, 0, 0, 0, time.UTC)
	expired := now.Add(-31 * time.Second).Format(time.RFC3339Nano)
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "3"}, Spec: leaseSpec{HolderIdentity: "old-pod", RenewTime: &expired, LeaseDurationSeconds: 30}})
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Millisecond, fixedClock(now))

	if err := lock.WithLock(context.Background(), func(context.Context) error { return nil }); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	requests := server.Requests()
	if requests[1].Lease.Spec.HolderIdentity != "pod-a" || requests[1].Lease.Metadata.ResourceVersion != "3" {
		t.Fatalf("takeover request = %#v", requests[1].Lease)
	}
}

func TestKubernetesLeaseLockWaitsForHeldLeaseUntilContextCanceled(t *testing.T) {
	now := time.Date(2026, 8, 13, 12, 0, 0, 0, time.UTC)
	renewed := now.Format(time.RFC3339Nano)
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "2"}, Spec: leaseSpec{HolderIdentity: "other-pod", RenewTime: &renewed, LeaseDurationSeconds: 60}})
	defer server.Close()
	waiting := make(chan struct{}, 1)
	lock := newKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Hour, fixedClock(now), leaseLockHooks{
		waitForRetry: func(ctx context.Context, _ time.Duration) error {
			waiting <- struct{}{}
			<-ctx.Done()
			return ctx.Err()
		},
	})
	ctx, cancel := context.WithCancel(context.Background())
	errDone := make(chan error, 1)
	calls := 0
	go func() {
		errDone <- lock.WithLock(ctx, func(context.Context) error { calls++; return nil })
	}()

	<-waiting
	cancel()
	err := <-errDone
	if !errors.Is(err, ErrLeaseUnavailable) {
		t.Fatalf("WithLock() error = %v, want ErrLeaseUnavailable", err)
	}
	if calls != 0 || server.Count(http.MethodGet) != 1 {
		t.Fatalf("callback calls=%d GETs=%d", calls, server.Count(http.MethodGet))
	}
}

func TestKubernetesLeaseLockRejectsMissingPrecreatedLease(t *testing.T) {
	server := newLeaseServer(t, leaseObject{})
	server.missing = true
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Millisecond, time.Now)

	err := lock.WithLock(context.Background(), func(context.Context) error { return nil })
	if !errors.Is(err, ErrLeaseNotFound) {
		t.Fatalf("WithLock() error = %v, want ErrLeaseNotFound", err)
	}
	if server.Count(http.MethodPost) != 0 {
		t.Fatalf("POST count = %d, want 0", server.Count(http.MethodPost))
	}
}

func TestKubernetesLeaseLockReleasesAfterCallbackContextIsCanceled(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Millisecond, time.Now)
	ctx, cancel := context.WithCancel(context.Background())

	if err := lock.WithLock(ctx, func(context.Context) error { cancel(); return context.Canceled }); !errors.Is(err, context.Canceled) {
		t.Fatalf("WithLock() error = %v, want context.Canceled", err)
	}
	requests := server.Requests()
	if len(requests) != 3 || requests[2].Lease.Spec.HolderIdentity != "" {
		t.Fatalf("requests = %#v", requests)
	}
}

func TestKubernetesLeaseLockRetriesConflictAndBestEffortReleases(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	server.conflicts = 1
	server.releaseStatus = http.StatusInternalServerError
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", 30*time.Second, time.Millisecond, time.Now)

	if err := lock.WithLock(context.Background(), func(context.Context) error { return nil }); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	if server.Count(http.MethodGet) != 2 || server.Count(http.MethodPut) != 3 {
		t.Fatalf("GETs=%d PUTs=%d", server.Count(http.MethodGet), server.Count(http.MethodPut))
	}
}

type recordedLeaseRequest struct {
	Method string
	Lease  leaseObject
}

type leaseServer struct {
	*httptest.Server
	mu                            sync.Mutex
	lease                         leaseObject
	missing                       bool
	conflicts                     int
	releaseStatus                 int
	requireCurrentResourceVersion bool
	renewalLosesOwnership         bool
	acquireCompleted              bool
	requests                      []recordedLeaseRequest
}

func newLeaseServer(t *testing.T, initial leaseObject) *leaseServer {
	t.Helper()
	server := &leaseServer{lease: initial}
	server.Server = httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		server.mu.Lock()
		defer server.mu.Unlock()
		if request.URL.Path != "/apis/coordination.k8s.io/v1/namespaces/cyclops-cs/leases/cyclops-feature-flags-writer" {
			http.NotFound(writer, request)
			return
		}
		if server.missing {
			http.NotFound(writer, request)
			return
		}
		switch request.Method {
		case http.MethodGet:
			server.requests = append(server.requests, recordedLeaseRequest{Method: request.Method})
			writeLeaseJSON(t, writer, server.lease)
		case http.MethodPut:
			var lease leaseObject
			if err := json.NewDecoder(request.Body).Decode(&lease); err != nil {
				t.Errorf("decode lease request: %v", err)
				http.Error(writer, "invalid lease request", http.StatusBadRequest)
				return
			}
			server.requests = append(server.requests, recordedLeaseRequest{Method: request.Method, Lease: lease})
			if lease.Spec.HolderIdentity != "" && server.renewalLosesOwnership && server.acquireCompleted {
				server.lease.Spec.HolderIdentity = "other-pod"
				server.lease.Metadata.ResourceVersion = incrementResourceVersion(server.lease.Metadata.ResourceVersion)
				writer.WriteHeader(http.StatusConflict)
				return
			}
			if lease.Spec.HolderIdentity != "" && server.conflicts > 0 {
				server.conflicts--
				writer.WriteHeader(http.StatusConflict)
				return
			}
			if lease.Spec.HolderIdentity == "" && server.releaseStatus != 0 {
				writer.WriteHeader(server.releaseStatus)
				return
			}
			resourceVersion := server.lease.Metadata.ResourceVersion
			if resourceVersion == "" {
				resourceVersion = "1"
			}
			if server.requireCurrentResourceVersion && lease.Metadata.ResourceVersion != resourceVersion {
				writer.WriteHeader(http.StatusConflict)
				return
			}
			lease.Metadata.ResourceVersion = incrementResourceVersion(resourceVersion)
			server.lease = lease
			if lease.Spec.HolderIdentity != "" {
				server.acquireCompleted = true
			}
			writeLeaseJSON(t, writer, lease)
		default:
			writer.WriteHeader(http.StatusMethodNotAllowed)
		}
	}))
	return server
}

func (s *leaseServer) Requests() []recordedLeaseRequest {
	s.mu.Lock()
	defer s.mu.Unlock()
	requests := make([]recordedLeaseRequest, len(s.requests))
	copy(requests, s.requests)
	return requests
}

func (s *leaseServer) Count(method string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	count := 0
	for _, request := range s.requests {
		if request.Method == method {
			count++
		}
	}
	return count
}

func writeLeaseJSON(t *testing.T, writer http.ResponseWriter, lease leaseObject) {
	t.Helper()
	writer.Header().Set("Content-Type", "application/json")
	if err := json.NewEncoder(writer).Encode(lease); err != nil {
		t.Errorf("encode lease response: %v", err)
	}
}

func fixedClock(now time.Time) func() time.Time { return func() time.Time { return now } }

func TestKubernetesLeaseLockSerializesSamePodCallbacks(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, time.Millisecond, time.Now)

	firstStarted := make(chan struct{})
	allowFirstExit := make(chan struct{})
	secondStarted := make(chan struct{})
	errors := make(chan error, 2)
	go func() {
		errors <- lock.WithLock(context.Background(), func(context.Context) error {
			close(firstStarted)
			<-allowFirstExit
			return nil
		})
	}()
	<-firstStarted
	go func() {
		errors <- lock.WithLock(context.Background(), func(context.Context) error {
			close(secondStarted)
			return nil
		})
	}()

	select {
	case <-secondStarted:
		t.Fatal("second callback entered while the first callback still held the same-pod lock")
	case <-time.After(25 * time.Millisecond):
	}
	close(allowFirstExit)
	if err := <-errors; err != nil {
		t.Fatalf("first WithLock() error = %v", err)
	}
	select {
	case <-secondStarted:
	case <-time.After(time.Second):
		t.Fatal("second callback did not enter after first callback exited")
	}
	if err := <-errors; err != nil {
		t.Fatalf("second WithLock() error = %v", err)
	}
}

func TestKubernetesLeaseLockRenewsLongCallbackBeforeTakeover(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	defer server.Close()
	owner := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, 10*time.Millisecond, time.Now)
	contender := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-b", time.Second, 10*time.Millisecond, time.Now)

	ownerStarted := make(chan struct{})
	allowOwnerExit := make(chan struct{})
	ownerDone := make(chan error, 1)
	go func() {
		ownerDone <- owner.WithLock(context.Background(), func(context.Context) error {
			close(ownerStarted)
			<-allowOwnerExit
			return nil
		})
	}()
	<-ownerStarted
	time.Sleep(1200 * time.Millisecond)

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	contenderCalls := 0
	err := contender.WithLock(ctx, func(context.Context) error { contenderCalls++; return nil })
	if !errors.Is(err, ErrLeaseUnavailable) || contenderCalls != 0 {
		t.Fatalf("contender error=%v calls=%d, want unavailable and 0 calls", err, contenderCalls)
	}
	close(allowOwnerExit)
	if err := <-ownerDone; err != nil {
		t.Fatalf("owner WithLock() error = %v", err)
	}
}

func TestKubernetesLeaseLockSerializesSeparateInstancesWithSameHolder(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	defer server.Close()
	first := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, time.Millisecond, time.Now)
	second := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, time.Millisecond, time.Now)

	firstStarted := make(chan struct{})
	allowFirstExit := make(chan struct{})
	secondStarted := make(chan struct{})
	errors := make(chan error, 2)
	go func() {
		errors <- first.WithLock(context.Background(), func(context.Context) error {
			close(firstStarted)
			<-allowFirstExit
			return nil
		})
	}()
	<-firstStarted
	go func() {
		errors <- second.WithLock(context.Background(), func(context.Context) error {
			close(secondStarted)
			return nil
		})
	}()

	select {
	case <-secondStarted:
		t.Fatal("separate same-holder lock instance entered while the first callback was active")
	case <-time.After(25 * time.Millisecond):
	}
	close(allowFirstExit)
	if err := <-errors; err != nil {
		t.Fatalf("first WithLock() error = %v", err)
	}
	select {
	case <-secondStarted:
	case <-time.After(time.Second):
		t.Fatal("second callback did not enter after first callback exited")
	}
	if err := <-errors; err != nil {
		t.Fatalf("second WithLock() error = %v", err)
	}
}

func TestKubernetesLeaseLockReleasesRenewedLeaseVersion(t *testing.T) {
	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	server.requireCurrentResourceVersion = true
	defer server.Close()
	renewalTicks := make(chan time.Time)
	renewalCompleted := make(chan struct{})
	lock := newKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, 10*time.Millisecond, time.Now, leaseLockHooks{
		renewalTicks: func(time.Duration) (<-chan time.Time, func()) { return renewalTicks, func() {} },
		renewed:      func() { close(renewalCompleted) },
	})

	if err := lock.WithLock(context.Background(), func(context.Context) error {
		renewalTicks <- time.Now()
		<-renewalCompleted
		return nil
	}); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	requests := server.Requests()
	if len(requests) < 4 || requests[len(requests)-1].Lease.Spec.HolderIdentity != "" {
		t.Fatalf("requests = %#v", requests)
	}
	server.mu.Lock()
	defer server.mu.Unlock()
	if server.lease.Spec.HolderIdentity != "" {
		t.Fatalf("lease holder = %q, want released", server.lease.Spec.HolderIdentity)
	}
}

func incrementResourceVersion(value string) string {
	if value == "1" {
		return "2"
	}
	return value + "-next"
}

func TestKubernetesLeaseLockCancelsInFlightRenewalWhenCallbackCompletes(t *testing.T) {
	var mu sync.Mutex
	lease := leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}}
	renewalStarted := make(chan struct{})
	renewalCanceled := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		switch request.Method {
		case http.MethodGet:
			writeLeaseJSON(t, writer, lease)
		case http.MethodPut:
			var updated leaseObject
			if err := json.NewDecoder(request.Body).Decode(&updated); err != nil {
				t.Errorf("decode lease: %v", err)
				http.Error(writer, "invalid lease request", http.StatusBadRequest)
				return
			}
			if updated.Spec.HolderIdentity != "" && lease.Spec.HolderIdentity != "" {
				close(renewalStarted)
				mu.Unlock()
				<-request.Context().Done()
				close(renewalCanceled)
				mu.Lock()
				return
			}
			updated.Metadata.ResourceVersion = incrementResourceVersion(lease.Metadata.ResourceVersion)
			lease = updated
			writeLeaseJSON(t, writer, updated)
		default:
			writer.WriteHeader(http.StatusMethodNotAllowed)
		}
	}))
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, 10*time.Millisecond, time.Now)

	started := time.Now()
	err := lock.WithLock(context.Background(), func(context.Context) error {
		select {
		case <-renewalStarted:
			return nil
		case <-time.After(2 * time.Second):
			return errors.New("renewal did not start")
		}
	})
	if err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	if elapsed := time.Since(started); elapsed > 1500*time.Millisecond {
		t.Fatalf("WithLock() returned after %v, want prompt shutdown", elapsed)
	}
	select {
	case <-renewalCanceled:
	case <-time.After(time.Second):
		t.Fatal("in-flight renewal request was not canceled")
	}
}

func TestKubernetesLeaseLockReleasesUnusedLifecycleGuard(t *testing.T) {
	leaseLifecycleGuardsMu.Lock()
	leaseLifecycleGuards = map[string]*lifecycleGuard{}
	leaseLifecycleGuardsMu.Unlock()

	server := newLeaseServer(t, leaseObject{Metadata: leaseMetadata{ResourceVersion: "1"}})
	defer server.Close()
	lock := NewKubernetesLeaseLock(server.URL, "cyclops-cs", "cyclops-feature-flags-writer", "pod-a", time.Second, time.Millisecond, time.Now)
	if err := lock.WithLock(context.Background(), func(context.Context) error { return nil }); err != nil {
		t.Fatalf("WithLock() error = %v", err)
	}
	leaseLifecycleGuardsMu.Lock()
	defer leaseLifecycleGuardsMu.Unlock()
	if len(leaseLifecycleGuards) != 0 {
		t.Fatalf("lifecycle guards = %#v, want empty after lock lifecycle", leaseLifecycleGuards)
	}
}
