package featureflagadmin

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

var (
	ErrLeaseUnavailable = errors.New("featureflagadmin: lease unavailable")
	ErrLeaseNotFound    = errors.New("featureflagadmin: pre-created lease not found")

	leaseLifecycleGuardsMu sync.Mutex
	leaseLifecycleGuards   = map[string]*lifecycleGuard{}
)

type lifecycleGuard struct {
	token chan struct{}
	refs  int
}

func acquireLifecycleGuard(key string) *lifecycleGuard {
	leaseLifecycleGuardsMu.Lock()
	defer leaseLifecycleGuardsMu.Unlock()
	guard := leaseLifecycleGuards[key]
	if guard == nil {
		guard = &lifecycleGuard{token: make(chan struct{}, 1)}
		guard.token <- struct{}{}
		leaseLifecycleGuards[key] = guard
	}
	guard.refs++
	return guard
}

func releaseLifecycleGuard(key string, guard *lifecycleGuard) {
	leaseLifecycleGuardsMu.Lock()
	defer leaseLifecycleGuardsMu.Unlock()
	guard.refs--
	if guard.refs == 0 && leaseLifecycleGuards[key] == guard {
		delete(leaseLifecycleGuards, key)
	}
}

type KubernetesLeaseLock struct {
	apiBaseURL     string
	namespace      string
	name           string
	holderIdentity string
	leaseDuration  time.Duration
	retryInterval  time.Duration
	now            func() time.Time
	client         *http.Client
	lifecycleKey   string
	waitForRetry   func(context.Context, time.Duration) error
	renewalTicks   func(time.Duration) (<-chan time.Time, func())
	renewed        func()
}

type leaseLockHooks struct {
	waitForRetry func(context.Context, time.Duration) error
	renewalTicks func(time.Duration) (<-chan time.Time, func())
	renewed      func()
}

func NewKubernetesLeaseLock(apiBaseURL, namespace, name, holderIdentity string, leaseDuration, retryInterval time.Duration, now func() time.Time) *KubernetesLeaseLock {
	return newKubernetesLeaseLock(apiBaseURL, namespace, name, holderIdentity, leaseDuration, retryInterval, now, leaseLockHooks{})
}

func newKubernetesLeaseLock(apiBaseURL, namespace, name, holderIdentity string, leaseDuration, retryInterval time.Duration, now func() time.Time, hooks leaseLockHooks) *KubernetesLeaseLock {
	if holderIdentity == "" {
		holderIdentity, _ = os.Hostname()
	}
	if leaseDuration <= 0 {
		leaseDuration = 30 * time.Second
	}
	if retryInterval <= 0 {
		retryInterval = 100 * time.Millisecond
	}
	if now == nil {
		now = time.Now
	}
	lifecycleKey := strings.TrimRight(apiBaseURL, "/") + "\x00" + namespace + "\x00" + name
	lock := &KubernetesLeaseLock{
		apiBaseURL: strings.TrimRight(apiBaseURL, "/"), namespace: namespace, name: name, holderIdentity: holderIdentity,
		leaseDuration: leaseDuration, retryInterval: retryInterval, now: now, client: &http.Client{}, lifecycleKey: lifecycleKey,
	}
	lock.waitForRetry = hooks.waitForRetry
	if lock.waitForRetry == nil {
		lock.waitForRetry = waitForLeaseRetry
	}
	lock.renewalTicks = hooks.renewalTicks
	if lock.renewalTicks == nil {
		lock.renewalTicks = renewalTicker
	}
	lock.renewed = hooks.renewed
	return lock
}

func (l *KubernetesLeaseLock) WithLock(ctx context.Context, callback func(context.Context) error) error {
	// The Kubernetes holder identity is shared by every goroutine in this pod,
	// so serialize the full lifecycle locally as well as across replicas.
	guard := acquireLifecycleGuard(l.lifecycleKey)
	defer releaseLifecycleGuard(l.lifecycleKey, guard)
	select {
	case <-ctx.Done():
		return errors.Join(ErrLeaseUnavailable, ctx.Err())
	case <-guard.token:
		defer func() { guard.token <- struct{}{} }()
	}

	for {
		lease, err := l.get(ctx)
		if err != nil {
			if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled) {
				return errors.Join(ErrLeaseUnavailable, err)
			}
			return err
		}
		if !l.available(lease) {
			if err := l.waitForRetry(ctx, l.retryInterval); err != nil {
				return errors.Join(ErrLeaseUnavailable, err)
			}
			continue
		}

		updated, conflict, err := l.put(ctx, l.claim(lease))
		if err != nil {
			return err
		}
		if conflict {
			continue
		}

		callbackCtx, cancelCallback := context.WithCancel(ctx)
		renewalCtx, cancelRenewal := context.WithCancel(context.Background())
		renewalDone := make(chan renewalResult, 1)
		go func() {
			lease, err := l.renew(renewalCtx, updated)
			renewalDone <- renewalResult{lease: lease, err: err}
		}()
		callbackDone := make(chan error, 1)
		go func() { callbackDone <- callback(callbackCtx) }()

		var callbackErr error
		var renewal renewalResult
		var renewalErr error
		select {
		case callbackErr = <-callbackDone:
			cancelRenewal()
			renewal = <-renewalDone
			renewalErr = renewal.err
		case renewal = <-renewalDone:
			renewalErr = renewal.err
			if renewalErr != nil {
				cancelCallback()
			}
			callbackErr = <-callbackDone
		}
		cancelCallback()
		cancelRenewal()

		releaseCtx, releaseCancel := context.WithTimeout(context.Background(), l.releaseTimeout())
		l.release(releaseCtx, renewal.lease)
		releaseCancel()
		if renewalErr != nil {
			return errors.Join(ErrLeaseUnavailable, renewalErr, callbackErr)
		}
		return errors.Join(callbackErr, renewalErr)
	}
}

func (l *KubernetesLeaseLock) renewalRequestTimeout() time.Duration {
	timeout := l.leaseDuration / 4
	if timeout > time.Second {
		timeout = time.Second
	}
	if timeout < 100*time.Millisecond {
		timeout = 100 * time.Millisecond
	}
	return timeout
}

func (l *KubernetesLeaseLock) releaseTimeout() time.Duration {
	timeout := l.renewalRequestTimeout()
	if timeout < l.retryInterval {
		timeout = l.retryInterval
	}
	return timeout
}

func (l *KubernetesLeaseLock) claim(lease leaseObject) leaseObject {
	lease.APIVersion = "coordination.k8s.io/v1"
	lease.Kind = "Lease"
	lease.Spec.HolderIdentity = l.holderIdentity
	lease.Spec.LeaseDurationSeconds = int32(l.leaseDuration / time.Second)
	if lease.Spec.LeaseDurationSeconds < 1 {
		lease.Spec.LeaseDurationSeconds = 1
	}
	now := l.now().UTC().Format(time.RFC3339Nano)
	lease.Spec.AcquireTime = &now
	lease.Spec.RenewTime = &now
	return lease
}

func (l *KubernetesLeaseLock) renew(ctx context.Context, lease leaseObject) (leaseObject, error) {
	interval := l.leaseDuration / 2
	if interval <= 0 {
		interval = time.Millisecond
	}
	ticks, stop := l.renewalTicks(interval)
	defer stop()
	for {
		select {
		case <-ctx.Done():
			return lease, nil
		case <-ticks:
			renewCtx, cancel := context.WithTimeout(ctx, l.renewalRequestTimeout())
			lease.Spec.RenewTime = stringPtr(l.now().UTC().Format(time.RFC3339Nano))
			updated, conflict, err := l.put(renewCtx, lease)
			cancel()
			if err != nil {
				if ctx.Err() != nil {
					return lease, nil
				}
				return lease, err
			}
			if conflict {
				readCtx, readCancel := context.WithTimeout(ctx, l.renewalRequestTimeout())
				current, err := l.get(readCtx)
				readCancel()
				if err != nil {
					if ctx.Err() != nil {
						return lease, nil
					}
					return lease, err
				}
				if current.Spec.HolderIdentity != l.holderIdentity {
					return lease, ErrLeaseUnavailable
				}
				lease = current
				continue
			}
			lease = updated
			if l.renewed != nil {
				l.renewed()
			}
		}
	}
}

type renewalResult struct {
	lease leaseObject
	err   error
}

func stringPtr(value string) *string { return &value }

func (l *KubernetesLeaseLock) available(lease leaseObject) bool {
	if lease.Spec.HolderIdentity == "" {
		return true
	}
	if lease.Spec.HolderIdentity == l.holderIdentity {
		return true
	}
	if lease.Spec.RenewTime == nil {
		return true
	}
	renewedAt, err := time.Parse(time.RFC3339Nano, *lease.Spec.RenewTime)
	if err != nil {
		return false
	}
	duration := time.Duration(lease.Spec.LeaseDurationSeconds) * time.Second
	if duration <= 0 {
		duration = l.leaseDuration
	}
	return !l.now().Before(renewedAt.Add(duration))
}

func (l *KubernetesLeaseLock) release(ctx context.Context, lease leaseObject) {
	lease.Spec.HolderIdentity = ""
	lease.Spec.AcquireTime = nil
	lease.Spec.RenewTime = nil
	_, _, _ = l.put(ctx, lease)
}

func (l *KubernetesLeaseLock) get(ctx context.Context) (leaseObject, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, l.leaseURL(), nil)
	if err != nil {
		return leaseObject{}, fmt.Errorf("build lease GET: %w", err)
	}
	response, err := l.client.Do(request)
	if err != nil {
		return leaseObject{}, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotFound {
		return leaseObject{}, ErrLeaseNotFound
	}
	if response.StatusCode != http.StatusOK {
		return leaseObject{}, fmt.Errorf("lease GET: unexpected HTTP status %d", response.StatusCode)
	}
	var lease leaseObject
	if err := json.NewDecoder(response.Body).Decode(&lease); err != nil {
		return leaseObject{}, fmt.Errorf("decode lease GET: %w", err)
	}
	return lease, nil
}

func (l *KubernetesLeaseLock) put(ctx context.Context, lease leaseObject) (leaseObject, bool, error) {
	body, err := json.Marshal(lease)
	if err != nil {
		return leaseObject{}, false, fmt.Errorf("marshal lease PUT: %w", err)
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPut, l.leaseURL(), bytes.NewReader(body))
	if err != nil {
		return leaseObject{}, false, fmt.Errorf("build lease PUT: %w", err)
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := l.client.Do(request)
	if err != nil {
		return leaseObject{}, false, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusConflict {
		return leaseObject{}, true, nil
	}
	if response.StatusCode == http.StatusNotFound {
		return leaseObject{}, false, ErrLeaseNotFound
	}
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return leaseObject{}, false, fmt.Errorf("lease PUT: unexpected HTTP status %d", response.StatusCode)
	}
	var updated leaseObject
	if err := json.NewDecoder(response.Body).Decode(&updated); err != nil {
		return leaseObject{}, false, fmt.Errorf("decode lease PUT: %w", err)
	}
	return updated, false, nil
}

func waitForLeaseRetry(ctx context.Context, interval time.Duration) error {
	timer := time.NewTimer(interval)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

func (l *KubernetesLeaseLock) leaseURL() string {
	return l.apiBaseURL + "/apis/coordination.k8s.io/v1/namespaces/" + l.namespace + "/leases/" + l.name
}

type leaseObject struct {
	APIVersion string        `json:"apiVersion,omitempty"`
	Kind       string        `json:"kind,omitempty"`
	Metadata   leaseMetadata `json:"metadata"`
	Spec       leaseSpec     `json:"spec"`
}

type leaseMetadata struct {
	Name            string `json:"name,omitempty"`
	Namespace       string `json:"namespace,omitempty"`
	ResourceVersion string `json:"resourceVersion,omitempty"`
}

type leaseSpec struct {
	HolderIdentity       string  `json:"holderIdentity,omitempty"`
	LeaseDurationSeconds int32   `json:"leaseDurationSeconds,omitempty"`
	AcquireTime          *string `json:"acquireTime,omitempty"`
	RenewTime            *string `json:"renewTime,omitempty"`
}

func renewalTicker(interval time.Duration) (<-chan time.Time, func()) {
	ticker := time.NewTicker(interval)
	return ticker.C, ticker.Stop
}
