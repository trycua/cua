// Copyright 2026 Cua AI, Inc.
// SPDX-License-Identifier: MIT

package gateway

// Which names the gateway's resolver answered with which addresses, for as
// long as each answer lives: a rule by name applies to the addresses its
// name resolved to here, never to an address the guest found elsewhere.
// (The pattern of binding allowed names to the addresses a resolver the
// platform controls returned follows perplexityai/SPACE, Apache-2.0; no
// code is taken from it.)

import (
	"net/netip"
	"sync"
	"time"
)

const (
	// The life of a binding, whatever the answer's TTL says.
	minBindTTL = 60 * time.Second
	maxBindTTL = 300 * time.Second
	// Limits, so a guest can't grow the table without bound.
	maxBoundAddrs   = 4096
	maxNamesPerAddr = 32
)

type bindings struct {
	mu  sync.Mutex
	now func() time.Time
	m   map[netip.Addr]map[string]time.Time
}

func newBindings() *bindings {
	return &bindings{now: time.Now, m: map[netip.Addr]map[string]time.Time{}}
}

// add binds name to ip for ttl (clamped to 60..300 s).
func (b *bindings) add(ip netip.Addr, name string, ttl time.Duration) {
	ttl = min(max(ttl, minBindTTL), maxBindTTL)
	b.mu.Lock()
	defer b.mu.Unlock()
	now := b.now()
	names := b.m[ip]
	if names == nil {
		if len(b.m) >= maxBoundAddrs {
			b.pruneLocked(now)
			if len(b.m) >= maxBoundAddrs {
				return
			}
		}
		names = map[string]time.Time{}
		b.m[ip] = names
	}
	if _, ok := names[name]; !ok && len(names) >= maxNamesPerAddr {
		return
	}
	names[name] = now.Add(ttl)
}

// names are the live names bound to ip.
func (b *bindings) names(ip netip.Addr) []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	now := b.now()
	var out []string
	for name, until := range b.m[ip] {
		if until.After(now) {
			out = append(out, name)
		}
	}
	return out
}

func (b *bindings) pruneLocked(now time.Time) {
	for ip, names := range b.m {
		for name, until := range names {
			if !until.After(now) {
				delete(names, name)
			}
		}
		if len(names) == 0 {
			delete(b.m, ip)
		}
	}
}
