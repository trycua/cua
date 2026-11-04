// Copyright 2026 Cua AI, Inc.
// SPDX-License-Identifier: MIT

package gateway

import (
	"net/netip"
	"testing"
	"time"
)

func TestBindingsLiveForTheClampedTTL(t *testing.T) {
	b := newBindings()
	now := time.Unix(1000, 0)
	b.now = func() time.Time { return now }
	ip := netip.MustParseAddr("140.82.112.3")
	b.add(ip, "github.com.", time.Second) // clamped up to 60 s
	if got := b.names(ip); len(got) != 1 || got[0] != "github.com." {
		t.Fatalf("names = %v", got)
	}
	now = now.Add(59 * time.Second)
	if len(b.names(ip)) != 1 {
		t.Fatal("still bound before 60 s")
	}
	now = now.Add(2 * time.Second)
	if len(b.names(ip)) != 0 {
		t.Fatal("expired after 60 s")
	}
	b.add(ip, "a.example.", time.Hour) // clamped down to 300 s
	now = now.Add(301 * time.Second)
	if len(b.names(ip)) != 0 {
		t.Fatal("expired after 300 s")
	}
}
