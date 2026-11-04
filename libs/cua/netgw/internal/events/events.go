// Package events writes one JSON line per refused connection or stripped
// DNS answer, for cua's blocked-connection reporting.
//
// A line holds the reason id, the protocol and the destination address and
// port. It never holds a name the guest chose (a DNS question), so the
// event stream cannot carry data out of the guest.
package events

import (
	"encoding/json"
	"io"
	"net/netip"
	"sync"
	"time"
)

// Event is one line.
type Event struct {
	Time   string `json:"t"`
	Kind   string `json:"event"`
	Reason string `json:"reason"`
	Proto  string `json:"proto,omitempty"`
	Dst    string `json:"dst"`
	// Repeats counts the same events dropped by the rate limit since the
	// last line for this key.
	Repeats int `json:"repeats,omitempty"`
}

// Kinds.
const (
	Blocked     = "blocked"
	DNSStripped = "dns_stripped"
)

// Sink rate-limits and writes events. The zero value drops everything.
type Sink struct {
	mu     sync.Mutex
	w      io.Writer
	seen   map[string]*entry
	window time.Duration
	// at most burst lines per second in total
	burst   int
	second  time.Time
	written int
	now     func() time.Time
}

type entry struct {
	at      time.Time
	dropped int
}

// New writes to w: one line per (kind, reason, proto, destination) per
// window, and at most burst lines a second.
func New(w io.Writer, window time.Duration, burst int) *Sink {
	return &Sink{w: w, seen: map[string]*entry{}, window: window, burst: burst, now: time.Now}
}

// Emit reports one event.
func (s *Sink) Emit(kind, reason, proto string, dst netip.AddrPort) {
	if s == nil || s.w == nil {
		return
	}
	d := dst.String()
	if dst.Port() == 0 {
		d = dst.Addr().String()
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.now()
	key := kind + "|" + reason + "|" + proto + "|" + d
	e := s.seen[key]
	if e != nil && now.Sub(e.at) < s.window {
		e.dropped++
		return
	}
	if now.Sub(s.second) >= time.Second {
		s.second = now
		s.written = 0
	}
	if s.written >= s.burst {
		if e != nil {
			e.dropped++
		}
		return
	}
	s.written++
	repeats := 0
	if e != nil {
		repeats = e.dropped
	}
	s.seen[key] = &entry{at: now}
	if len(s.seen) > 4096 {
		for k, v := range s.seen {
			if now.Sub(v.at) >= s.window {
				delete(s.seen, k)
			}
		}
	}
	line, _ := json.Marshal(Event{
		Time:    now.UTC().Format(time.RFC3339Nano),
		Kind:    kind,
		Reason:  reason,
		Proto:   proto,
		Dst:     d,
		Repeats: repeats,
	})
	_, _ = s.w.Write(append(line, '\n'))
}
