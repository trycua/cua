package tree

import (
	"context"
	"io"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func writePeers(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestPeersFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "tree.json")
	p := Open(path)
	if _, ok := p.Socket(netip.MustParseAddr("198.19.0.2")); ok || p.Ingress() {
		t.Fatal("no file: no peers and no ingress")
	}
	writePeers(t, path, `{"version":1,"self":"198.19.0.1","ingress":true,"peers":{"198.19.0.2":"/s/b.sock"}}`)
	if changed, err := p.Reload(); err != nil || !changed {
		t.Fatalf("reload: %v %v", changed, err)
	}
	if s, ok := p.Socket(netip.MustParseAddr("198.19.0.2")); !ok || s != "/s/b.sock" {
		t.Fatalf("peer: %q %v", s, ok)
	}
	if _, ok := p.Socket(netip.MustParseAddr("198.19.0.3")); ok {
		t.Fatal("not a member")
	}
	if p.Self() != netip.MustParseAddr("198.19.0.1") || !p.Ingress() {
		t.Fatal("self and ingress")
	}
	// A bad file keeps the peers in force.
	writePeers(t, path, `{"version":1,"self":"10.0.0.1","peers":{}}`)
	if _, err := p.Reload(); err == nil {
		t.Fatal("an address outside the range is refused")
	}
	if _, ok := p.Socket(netip.MustParseAddr("198.19.0.2")); !ok {
		t.Fatal("the previous peers stay")
	}
	// Removed (the VM stopped): no peers.
	os.Remove(path)
	if _, err := p.Reload(); err != nil {
		t.Fatal(err)
	}
	if _, ok := p.Socket(netip.MustParseAddr("198.19.0.2")); ok {
		t.Fatal("no file: no peers")
	}
}

func TestTreeNames(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "tree.json")
	writePeers(t, path, `{"version":1,"self":"198.19.0.1","ingress":true,"peers":{},
		"names":{"lead.swarm":"198.19.0.1","Worker-1.swarm":"198.19.0.2"}}`)
	p := Open(path)
	if a, ok := p.Lookup("worker-1.swarm."); !ok || a != netip.MustParseAddr("198.19.0.2") {
		t.Fatalf("a member by name: %v %v", a, ok)
	}
	if a, ok := p.Lookup("LEAD.SWARM"); !ok || a != netip.MustParseAddr("198.19.0.1") {
		t.Fatalf("itself, any case: %v %v", a, ok)
	}
	if _, ok := p.Lookup("other.tree"); ok {
		t.Fatal("a name of another tree is unknown")
	}
	writePeers(t, path, `{"version":1,"self":"198.19.0.1","peers":{},"names":{"x.y":"10.0.0.1"}}`)
	if _, err := p.Reload(); err == nil {
		t.Fatal("a name outside the range is refused")
	}
}

func TestParseRequest(t *testing.T) {
	from, port, err := ParseRequest("CUATREE 1 198.19.0.2 8080")
	if err != nil || from != netip.MustParseAddr("198.19.0.2") || port != 8080 {
		t.Fatalf("%v %v %v", from, port, err)
	}
	for _, bad := range []string{"", "CUATREE 2 198.19.0.2 80", "CUATREE 1 10.0.0.1 80", "CUATREE 1 198.19.0.2 0", "GET / HTTP/1.1"} {
		if _, _, err := ParseRequest(bad); err == nil {
			t.Errorf("%q accepted", bad)
		}
	}
}

// A member's connection reaches the guest's port; anyone else, or a VM that
// takes no tree connections, is refused.
func TestDialAndServe(t *testing.T) {
	dir := t.TempDir()
	// The guest: an echo server on a loopback port.
	guest, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer guest.Close()
	go func() {
		for {
			c, err := guest.Accept()
			if err != nil {
				return
			}
			go func() { io.Copy(c, c); c.Close() }()
		}
	}()
	guestPort := uint16(guest.Addr().(*net.TCPAddr).Port)
	var asked uint16
	dial := func(ctx context.Context, port uint16) (net.Conn, error) {
		asked = port
		var d net.Dialer
		return d.DialContext(ctx, "tcp4", guest.Addr().String())
	}
	// A short folder: a Unix socket path has a small limit.
	short, err := os.MkdirTemp("/tmp", "cuatree")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(short)
	sock := filepath.Join(short, "b.sock")
	ln, err := Listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	peersPath := filepath.Join(dir, "tree.json")
	writePeers(t, peersPath, `{"version":1,"self":"198.19.0.2","ingress":true,"peers":{"198.19.0.1":"/s/a.sock"}}`)
	peers := Open(peersPath)
	go Serve(ln, peers, dial, 5*time.Second)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	c, err := Dial(ctx, sock, netip.MustParseAddr("198.19.0.1"), guestPort)
	if err != nil {
		t.Fatalf("a member: %v", err)
	}
	if _, err := c.Write([]byte("hello")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 5)
	if _, err := io.ReadFull(c, buf); err != nil || string(buf) != "hello" {
		t.Fatalf("echo: %q %v", buf, err)
	}
	c.Close()
	if asked != guestPort {
		t.Fatalf("asked for %d, want %d", asked, guestPort)
	}

	// Not a member of the tree.
	if _, err := Dial(ctx, sock, netip.MustParseAddr("198.19.0.9"), guestPort); err == nil ||
		!strings.Contains(err.Error(), "not a member") {
		t.Fatalf("a stranger: %v", err)
	}
	// The VM takes no tree connections (its settings are `none`).
	writePeers(t, peersPath, `{"version":1,"self":"198.19.0.2","ingress":false,"peers":{"198.19.0.1":"/s/a.sock"}}`)
	if _, err := peers.Reload(); err != nil {
		t.Fatal(err)
	}
	if _, err := Dial(ctx, sock, netip.MustParseAddr("198.19.0.1"), guestPort); err == nil ||
		!strings.Contains(err.Error(), "no connections") {
		t.Fatalf("no ingress: %v", err)
	}
}
