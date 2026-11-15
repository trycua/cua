// Package tree carries TCP connections between the VMs of one Cua tree (a
// top-level Space and the Spaces made from it) on one machine.
//
// Each VM has a tree address in Range. Its gateway (or, on Linux, the tree
// relay in its network namespace) has already judged a connection to one
// by the VM's own rules; this package then carries it to a running member
// of the same tree only:
//
//   - the peers file (written by cua: crates/cua-vmm, tree.rs) names the
//     VM's own address, whether it takes connections from its tree, and its
//     tree's other running members with their ingress sockets;
//   - Dial opens a member's ingress socket and asks for a port;
//   - Serve answers on the VM's own ingress socket: a member of its tree
//     asks for a port, and the connection is made to the guest;
//   - Lookup answers the tree's names (`<space>.<tree>`, from the peers
//     file's names) for the gateway's resolver.
//
// The wire: the dialer sends "CUATREE 1 <from address> <port>\n"; the other
// side answers "OK\n" once its guest accepted, or "NO <why>\n"; then the
// stream is the connection's.
package tree

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Range holds every VM's tree address.
var Range = netip.MustParsePrefix("198.19.0.0/16")

// Reasons reported in the events file.
const (
	// ReasonOtherTree: an address in Range that is not a running member of
	// this VM's tree.
	ReasonOtherTree = "platform:other-tree"
	// ReasonTCPOnly: anything but TCP to Range.
	ReasonTCPOnly = "tree:tcp-only"
)

// File is the peers file.
type File struct {
	Version int    `json:"version"`
	Self    string `json:"self"`
	// Ingress: the VM takes connections from its tree.
	Ingress bool `json:"ingress"`
	// Peers: tree address -> ingress socket path.
	Peers map[string]string `json:"peers"`
	// Names: `<space>.<tree>` -> tree address, for every member of the
	// tree with an address (itself included).
	Names map[string]string `json:"names,omitempty"`

	self  netip.Addr
	peers map[netip.Addr]string
	names map[string]netip.Addr
}

func parse(data []byte) (*File, error) {
	var f File
	if err := json.Unmarshal(data, &f); err != nil {
		return nil, err
	}
	if f.Version != 1 {
		return nil, fmt.Errorf("peers file version %d (want 1)", f.Version)
	}
	var err error
	if f.self, err = netip.ParseAddr(f.Self); err != nil || !Range.Contains(f.self) {
		return nil, fmt.Errorf("self %q: an address in %s", f.Self, Range)
	}
	f.peers = map[netip.Addr]string{}
	for a, sock := range f.Peers {
		ip, err := netip.ParseAddr(a)
		if err != nil || !Range.Contains(ip) || ip == f.self || sock == "" {
			return nil, fmt.Errorf("peer %q: another address in %s with a socket", a, Range)
		}
		f.peers[ip] = sock
	}
	f.names = map[string]netip.Addr{}
	for n, a := range f.Names {
		ip, err := netip.ParseAddr(a)
		if err != nil || !Range.Contains(ip) {
			return nil, fmt.Errorf("name %q: an address in %s", n, Range)
		}
		f.names[normalName(n)] = ip
	}
	return &f, nil
}

// normalName is name in lowercase without the root dot.
func normalName(name string) string {
	return strings.TrimSuffix(strings.ToLower(strings.TrimSpace(name)), ".")
}

// Peers holds the peers file in force and reloads it when it changes. No
// file (yet) means no peers and no ingress: nothing is carried.
type Peers struct {
	path    string
	current atomic.Pointer[File]
	mu      sync.Mutex
	last    []byte
}

// Open reads path (missing is fine: no peers until it appears).
func Open(path string) *Peers {
	p := &Peers{path: path}
	p.current.Store(&File{peers: map[netip.Addr]string{}, names: map[string]netip.Addr{}})
	_, _ = p.Reload()
	return p
}

// Static holds f and never reloads (tests).
func Static(f *File) *Peers {
	p := &Peers{}
	if f.peers == nil {
		g, err := parse(mustJSON(f))
		if err == nil {
			f = g
		}
	}
	p.current.Store(f)
	return p
}

func mustJSON(f *File) []byte {
	b, _ := json.Marshal(f)
	return b
}

// Reload reads the file again; a file that doesn't parse keeps the old one.
// A missing file means no peers.
func (p *Peers) Reload() (bool, error) {
	if p.path == "" {
		return false, nil
	}
	data, err := os.ReadFile(p.path)
	if errors.Is(err, os.ErrNotExist) {
		data = nil
	} else if err != nil {
		return false, err
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.last != nil && bytes.Equal(data, p.last) {
		return false, nil
	}
	if data == nil {
		p.last = []byte{}
		p.current.Store(&File{peers: map[netip.Addr]string{}, names: map[string]netip.Addr{}})
		return true, nil
	}
	f, err := parse(data)
	if err != nil {
		return false, err
	}
	p.last = data
	p.current.Store(f)
	return true, nil
}

// Watch reloads every interval until stop closes.
func (p *Peers) Watch(interval time.Duration, stop <-chan struct{}, report func(error)) {
	t := time.NewTicker(interval)
	defer t.Stop()
	var lastErr string
	for {
		select {
		case <-stop:
			return
		case <-t.C:
		}
		if _, err := p.Reload(); err != nil {
			if err.Error() != lastErr {
				lastErr = err.Error()
				report(err)
			}
		} else {
			lastErr = ""
		}
	}
}

// Self is this VM's tree address (invalid before the file exists).
func (p *Peers) Self() netip.Addr { return p.current.Load().self }

// Ingress reports whether this VM takes connections from its tree.
func (p *Peers) Ingress() bool { return p.current.Load().Ingress }

// Lookup is the tree address of the member called name (`<space>.<tree>`,
// any case, with or without the root dot).
func (p *Peers) Lookup(name string) (netip.Addr, bool) {
	a, ok := p.current.Load().names[normalName(name)]
	return a, ok
}

// Socket is the ingress socket of member addr, if it runs in this VM's tree.
func (p *Peers) Socket(addr netip.Addr) (string, bool) {
	s, ok := p.current.Load().peers[addr.Unmap()]
	return s, ok
}

// Dial opens member sock's ingress and asks for port, as from. It returns
// once the member's guest accepted.
func Dial(ctx context.Context, sock string, from netip.Addr, port uint16) (net.Conn, error) {
	var d net.Dialer
	c, err := d.DialContext(ctx, "unix", sock)
	if err != nil {
		return nil, err
	}
	if dl, ok := ctx.Deadline(); ok {
		_ = c.SetDeadline(dl)
	}
	if _, err := fmt.Fprintf(c, "CUATREE 1 %s %d\n", from, port); err != nil {
		c.Close()
		return nil, err
	}
	line, err := readLine(c)
	if err != nil {
		c.Close()
		return nil, err
	}
	if line != "OK" {
		c.Close()
		return nil, fmt.Errorf("tree member refused: %s", strings.TrimPrefix(line, "NO "))
	}
	_ = c.SetDeadline(time.Time{})
	return c, nil
}

// readLine reads one line byte by byte (nothing past it is consumed: the
// rest of the stream is the connection's).
func readLine(r io.Reader) (string, error) {
	var b []byte
	one := make([]byte, 1)
	for len(b) < 256 {
		if _, err := io.ReadFull(r, one); err != nil {
			return "", err
		}
		if one[0] == '\n' {
			return strings.TrimSpace(string(b)), nil
		}
		b = append(b, one[0])
	}
	return "", errors.New("line too long")
}

// GuestDialer connects to the guest's port (the gateway through its
// stack, the relay through the namespace or a published port).
type GuestDialer func(ctx context.Context, port uint16) (net.Conn, error)

// Serve answers member connections on ln until it closes.
func Serve(ln net.Listener, peers *Peers, dial GuestDialer, timeout time.Duration) {
	for {
		c, err := ln.Accept()
		if err != nil {
			return
		}
		go serveOne(c, peers, dial, timeout)
	}
}

// ParseRequest reads "CUATREE 1 <from> <port>".
func ParseRequest(line string) (netip.Addr, uint16, error) {
	f := strings.Fields(line)
	if len(f) != 4 || f[0] != "CUATREE" || f[1] != "1" {
		return netip.Addr{}, 0, errors.New("not a tree request")
	}
	from, err := netip.ParseAddr(f[2])
	if err != nil || !Range.Contains(from) {
		return netip.Addr{}, 0, errors.New("not a tree address")
	}
	port, err := strconv.ParseUint(f[3], 10, 16)
	if err != nil || port == 0 {
		return netip.Addr{}, 0, errors.New("not a port")
	}
	return from, uint16(port), nil
}

func serveOne(c net.Conn, peers *Peers, dial GuestDialer, timeout time.Duration) {
	_ = c.SetDeadline(time.Now().Add(timeout))
	line, err := readLine(c)
	if err != nil {
		c.Close()
		return
	}
	from, port, err := ParseRequest(line)
	if err != nil {
		fmt.Fprintf(c, "NO %v\n", err)
		c.Close()
		return
	}
	// Only a running member of this tree, and only while this VM's own
	// settings reach its tree.
	if !peers.Ingress() {
		fmt.Fprintln(c, "NO this Space takes no connections from its tree")
		c.Close()
		return
	}
	if _, ok := peers.Socket(from); !ok {
		fmt.Fprintln(c, "NO not a member of this tree")
		c.Close()
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	guest, err := dial(ctx, port)
	cancel()
	if err != nil {
		fmt.Fprintf(c, "NO %v\n", err)
		c.Close()
		return
	}
	if _, err := fmt.Fprintln(c, "OK"); err != nil {
		c.Close()
		guest.Close()
		return
	}
	_ = c.SetDeadline(time.Time{})
	Splice(c, guest)
}

// Splice copies both ways until either side ends, then closes both.
func Splice(a, b net.Conn) {
	var once sync.Once
	closeBoth := func() {
		once.Do(func() {
			a.Close()
			b.Close()
		})
	}
	done := make(chan struct{}, 2)
	cp := func(dst, src net.Conn) {
		_, _ = io.Copy(dst, src)
		if cw, ok := dst.(interface{ CloseWrite() error }); ok {
			_ = cw.CloseWrite()
		} else {
			closeBoth()
		}
		done <- struct{}{}
	}
	go cp(a, b)
	go cp(b, a)
	<-done
	<-done
	closeBoth()
}

// Listen listens on the Unix socket at path (a stale one from an earlier
// run is removed), readable by its owner only.
func Listen(path string) (net.Listener, error) {
	_ = os.Remove(path)
	ln, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	_ = os.Chmod(path, 0o600)
	return ln, nil
}
