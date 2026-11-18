// Package relay is the tree relay of a Linux VM: it runs inside the VM's
// network namespace (pasta's), where the namespace redirects the guest's
// TCP to the tree range (tree.Range) to the relay's port, after the VM's
// rules judged it by its original destination (iptables `--ctorigdst`).
//
//   - Out: the relay reads the connection's original destination, and
//     carries it to that member's ingress socket when the address is a
//     running member of the VM's tree (tree.Dial); anything else in the
//     range is refused and reported.
//   - In: on the VM's own ingress socket, a member asks for a port; the
//     relay connects to the guest: directly on its address (Firecracker's
//     tap), or for QEMU's user-mode network through a port QEMU publishes
//     on the namespace's loopback, added once per guest port over the
//     relay's own QMP socket.
//
// The configuration is written by cua (crates/cua-vmm, tree.rs).
package relay

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"strings"
	"time"

	log "github.com/sirupsen/logrus"

	"github.com/trycua/cua/libs/cua/netgw/internal/events"
	"github.com/trycua/cua/libs/cua/netgw/internal/tree"
)

// Config is the relay's configuration file.
type Config struct {
	// Peers is the VM's peers file (tree package).
	Peers string `json:"peers"`
	// Ingress is the Unix socket where the tree's members connect.
	Ingress string `json:"ingress"`
	// GuestIP is the guest's address in the namespace (QEMU: the address
	// its user-mode network gives the guest).
	GuestIP string `json:"guest_ip"`
	// Listen is where redirected connections arrive ("0.0.0.0:<port>").
	Listen string `json:"listen"`
	// Events is the file refused connections are appended to.
	Events string `json:"events,omitempty"`
	// QMP, for a QEMU VM: "unix:<path>" of the relay's own monitor.
	QMP string `json:"qmp,omitempty"`
}

// Load reads path; relative file names are relative to its folder.
func Load(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var c Config
	dec := json.NewDecoder(strings.NewReader(string(data)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&c); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	dir := filepath.Dir(path)
	for _, p := range []*string{&c.Peers, &c.Events} {
		if *p != "" && !filepath.IsAbs(*p) {
			*p = filepath.Join(dir, *p)
		}
	}
	if c.Peers == "" || c.Ingress == "" || c.Listen == "" {
		return nil, fmt.Errorf("%s: peers, ingress and listen are required", path)
	}
	if _, err := netip.ParseAddr(c.GuestIP); err != nil {
		return nil, fmt.Errorf("%s: guest_ip %q: %w", path, c.GuestIP, err)
	}
	return &c, nil
}

// dialTimeout bounds a connection to a member or to the guest.
const dialTimeout = 30 * time.Second

// Run serves until ctx ends.
func Run(ctx context.Context, path string) error {
	cfg, err := Load(path)
	if err != nil {
		return err
	}
	log.SetLevel(log.WarnLevel)
	var sink *events.Sink
	if cfg.Events != "" {
		f, err := events.OpenFile(cfg.Events, 1<<20)
		if err != nil {
			return fmt.Errorf("events: %w", err)
		}
		defer f.Close()
		sink = events.New(f, 10*time.Second, 50)
	}
	peers := tree.Open(cfg.Peers)
	go peers.Watch(time.Second, ctx.Done(), func(err error) {
		log.Errorf("tree peers not reloaded (the previous ones stay): %v", err)
	})
	in, err := tree.Listen(cfg.Ingress)
	if err != nil {
		return fmt.Errorf("ingress %s: %w", cfg.Ingress, err)
	}
	defer os.Remove(cfg.Ingress)
	out, err := net.Listen("tcp4", cfg.Listen)
	if err != nil {
		in.Close()
		return fmt.Errorf("listen %s: %w", cfg.Listen, err)
	}
	go func() {
		<-ctx.Done()
		in.Close()
		out.Close()
	}()
	guest := netip.MustParseAddr(cfg.GuestIP)
	var dial tree.GuestDialer
	if cfg.QMP != "" {
		q := newPublisher(strings.TrimPrefix(cfg.QMP, "unix:"))
		dial = q.dial
	} else {
		dial = func(ctx context.Context, port uint16) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "tcp4", netip.AddrPortFrom(guest, port).String())
		}
	}
	go tree.Serve(in, peers, dial, dialTimeout)
	for {
		c, err := out.Accept()
		if err != nil {
			if ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				return nil
			}
			return err
		}
		go carry(c, peers, sink)
	}
}

// carry takes one redirected connection of the guest to its tree member.
func carry(c net.Conn, peers *tree.Peers, sink *events.Sink) {
	dst, err := originalDst(c)
	if err != nil || !tree.Range.Contains(dst.Addr()) {
		// Not a redirected tree connection (something that dialled the
		// relay's port itself): nothing to carry.
		c.Close()
		return
	}
	sock, ok := peers.Socket(dst.Addr())
	if !ok {
		sink.Emit(events.Blocked, tree.ReasonOtherTree, "tcp", dst)
		reset(c)
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), dialTimeout)
	m, err := tree.Dial(ctx, sock, peers.Self(), dst.Port())
	cancel()
	if err != nil {
		log.Debugf("tree member %s: %v", dst, err)
		reset(c)
		return
	}
	tree.Splice(c, m)
}

// reset closes c with a reset, as a refused connection would.
func reset(c net.Conn) {
	if t, ok := c.(*net.TCPConn); ok {
		_ = t.SetLinger(0)
	}
	c.Close()
}
