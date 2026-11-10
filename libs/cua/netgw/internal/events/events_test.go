package events

import (
	"bytes"
	"encoding/json"
	"net/netip"
	"os"
	"strings"
	"testing"
	"time"
)

func TestOneLinePerDestinationPerWindow(t *testing.T) {
	var buf bytes.Buffer
	s := New(&buf, 10*time.Second, 3)
	now := time.Unix(1000, 0)
	s.now = func() time.Time { return now }
	dst := netip.MustParseAddrPort("10.0.0.1:22")
	for range 5 {
		s.Emit(Blocked, "default:lan", "tcp", dst)
	}
	s.Emit(Blocked, "default:lan", "tcp", netip.MustParseAddrPort("10.0.0.2:22"))
	s.Emit(DNSStripped, "default:lan", "", netip.AddrPortFrom(netip.MustParseAddr("10.0.0.3"), 0))
	s.Emit(Blocked, "default:lan", "tcp", netip.MustParseAddrPort("10.0.0.4:22")) // over the burst
	lines := strings.Split(strings.TrimSpace(buf.String()), "\n")
	if len(lines) != 3 {
		t.Fatalf("%d lines: %q", len(lines), lines)
	}
	var e Event
	if err := json.Unmarshal([]byte(lines[2]), &e); err != nil {
		t.Fatal(err)
	}
	if e.Kind != DNSStripped || e.Dst != "10.0.0.3" || e.Proto != "" {
		t.Fatalf("%+v", e)
	}
	now = now.Add(11 * time.Second)
	buf.Reset()
	s.Emit(Blocked, "default:lan", "tcp", dst)
	if err := json.Unmarshal(bytes.TrimSpace(buf.Bytes()), &e); err != nil {
		t.Fatal(err)
	}
	if e.Repeats != 4 || e.Dst != "10.0.0.1:22" || e.Reason != "default:lan" {
		t.Fatalf("%+v", e)
	}
}

func TestNilSinkDropsEverything(t *testing.T) {
	var s *Sink
	s.Emit(Blocked, "x", "tcp", netip.MustParseAddrPort("10.0.0.1:1"))
}

func TestTheFileRotates(t *testing.T) {
	path := t.TempDir() + "/events.jsonl"
	f, err := OpenFile(path, 100)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	line := []byte(strings.Repeat("x", 59) + "\n")
	for range 3 {
		if _, err := f.Write(line); err != nil {
			t.Fatal(err)
		}
	}
	cur, _ := os.ReadFile(path)
	old, _ := os.ReadFile(path + ".1")
	if len(cur) != 60 || len(old) != 60 {
		t.Fatalf("current %d bytes, previous %d", len(cur), len(old))
	}
}
