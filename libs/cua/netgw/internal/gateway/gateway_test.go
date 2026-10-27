package gateway

// End to end without a VM: a guest network stack (gVisor's netstack) on the
// other end of a QEMU-protocol connection opens connections through the
// gateway, as a guest kernel would.

import (
	"bufio"
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/miekg/dns"
	"gvisor.dev/gvisor/pkg/buffer"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/link/channel"
	"gvisor.dev/gvisor/pkg/tcpip/link/ethernet"
	"gvisor.dev/gvisor/pkg/tcpip/network/arp"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"

	"github.com/trycua/cua/libs/cua/netgw/internal/config"
	"github.com/trycua/cua/libs/cua/netgw/internal/events"
	"github.com/trycua/cua/libs/cua/netgw/internal/policy"
)

const (
	guestIP  = "172.16.0.2"
	gwIP     = "172.16.0.1"
	hostIP   = "172.16.0.6"
	guestMAC = "5a:94:ef:e4:0c:ee"
)

type harness struct {
	t      *testing.T
	gw     *Gateway
	guest  *stack.Stack
	rules  string
	events *syncBuffer
	store  *policy.Store
}

type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

func newHarness(t *testing.T, rules string) *harness {
	t.Helper()
	dir := t.TempDir()
	h := &harness{t: t, rules: filepath.Join(dir, "rules.json"), events: &syncBuffer{}}
	h.writeRules(rules)
	cfg := &config.Config{
		Subnet:     "172.16.0.0/29",
		GatewayIP:  gwIP,
		GatewayMAC: "5a:94:ef:e4:0c:dd",
		HostIP:     hostIP,
		Listen:     config.Listen{Qemu: "unix://unused"},
		Rules:      h.rules,
	}
	if err := cfg.Check(); err != nil {
		t.Fatal(err)
	}
	cfg.TCPConnectTimeoutSecs = 3
	store, err := policy.Open(h.rules)
	if err != nil {
		t.Fatal(err)
	}
	h.store = store
	gw, err := New(cfg, store, events.New(h.events, time.Millisecond, 1000))
	if err != nil {
		t.Fatal(err)
	}
	h.gw = gw

	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	gwSide, guestSide := net.Pipe()
	go func() { _ = gw.AcceptQemu(ctx, gwSide) }()
	h.guest = guestStack(t, ctx, guestSide)
	return h
}

func (h *harness) writeRules(s string) {
	h.t.Helper()
	tmp := h.rules + ".tmp"
	if err := os.WriteFile(tmp, []byte(s), 0o600); err != nil {
		h.t.Fatal(err)
	}
	if err := os.Rename(tmp, h.rules); err != nil {
		h.t.Fatal(err)
	}
}

// guestStack is a guest at 172.16.0.2/29 behind the gateway, speaking
// QEMU's stream protocol (a 32-bit length, then an Ethernet frame).
func guestStack(t *testing.T, ctx context.Context, conn net.Conn) *stack.Stack {
	mac, _ := net.ParseMAC(guestMAC)
	ch := channel.New(256, 1500, tcpip.LinkAddress(mac))
	s := stack.New(stack.Options{
		NetworkProtocols:   []stack.NetworkProtocolFactory{ipv4.NewProtocol, arp.NewProtocol},
		TransportProtocols: []stack.TransportProtocolFactory{tcp.NewProtocol, udp.NewProtocol},
	})
	if err := s.CreateNIC(1, ethernet.New(ch)); err != nil {
		t.Fatal(err)
	}
	if err := s.AddProtocolAddress(1, tcpip.ProtocolAddress{
		Protocol:          ipv4.ProtocolNumber,
		AddressWithPrefix: tcpip.AddrFrom4(netip.MustParseAddr(guestIP).As4()).WithPrefix(),
	}, stack.AddressProperties{}); err != nil {
		t.Fatal(err)
	}
	s.SetRouteTable([]tcpip.Route{{
		Destination: header.IPv4EmptySubnet,
		Gateway:     tcpip.AddrFrom4(netip.MustParseAddr(gwIP).As4()),
		NIC:         1,
	}})
	go func() {
		for {
			pkt := ch.ReadContext(ctx)
			if pkt == nil {
				return
			}
			frame := pkt.ToView().AsSlice()
			var n [4]byte
			binary.BigEndian.PutUint32(n[:], uint32(len(frame)))
			_, err := conn.Write(append(n[:], frame...))
			pkt.DecRef()
			if err != nil {
				return
			}
		}
	}()
	go func() {
		r := bufio.NewReader(conn)
		for {
			var n [4]byte
			if _, err := io.ReadFull(r, n[:]); err != nil {
				return
			}
			frame := make([]byte, binary.BigEndian.Uint32(n[:]))
			if _, err := io.ReadFull(r, frame); err != nil {
				return
			}
			pkt := stack.NewPacketBuffer(stack.PacketBufferOptions{Payload: buffer.MakeWithData(frame)})
			ch.InjectInbound(0, pkt)
			pkt.DecRef()
		}
	}()
	t.Cleanup(func() { conn.Close(); s.Close() })
	return s
}

func (h *harness) dialTCP(dst string, port uint16) (net.Conn, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return gonet.DialContextTCP(ctx, h.guest, tcpip.FullAddress{
		NIC:  1,
		Addr: tcpip.AddrFrom4(netip.MustParseAddr(dst).As4()),
		Port: port,
	}, ipv4.ProtocolNumber)
}

func (h *harness) dialUDP(dst string, port uint16) *gonet.UDPConn {
	c, err := gonet.DialUDP(h.guest, nil, &tcpip.FullAddress{
		NIC:  1,
		Addr: tcpip.AddrFrom4(netip.MustParseAddr(dst).As4()),
		Port: port,
	}, ipv4.ProtocolNumber)
	if err != nil {
		h.t.Fatal(err)
	}
	return c
}

// echo listens on addr and echoes each line.
func echo(t *testing.T, addr string) uint16 {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() { _, _ = io.Copy(c, c); c.Close() }()
		}
	}()
	return uint16(ln.Addr().(*net.TCPAddr).Port)
}

func roundTrip(t *testing.T, c net.Conn) {
	t.Helper()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write([]byte("ping\n")); err != nil {
		t.Fatal(err)
	}
	line, err := bufio.NewReader(c).ReadString('\n')
	if err != nil || line != "ping\n" {
		t.Fatalf("echo: %q %v", line, err)
	}
}

func refused(t *testing.T, err error) {
	t.Helper()
	if err == nil {
		t.Fatal("connected")
	}
	if !strings.Contains(err.Error(), "refused") {
		t.Fatalf("not refused (a reset): %v", err)
	}
}

func (h *harness) waitEvent(kind, reason, dst string) {
	h.t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		for _, line := range strings.Split(h.events.String(), "\n") {
			var e events.Event
			if json.Unmarshal([]byte(line), &e) == nil && e.Kind == kind && e.Reason == reason && e.Dst == dst {
				return
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	h.t.Fatalf("no %s %s %s event in:\n%s", kind, reason, dst, h.events.String())
}

const openRules = `{"version":1,"default_accept":true,"rules":[{"net":"0.0.0.0/0","accept":true}]}`

func TestHostLoopbackIsRefusedEvenUnderAllowAll(t *testing.T) {
	port := echo(t, "127.0.0.1:0")
	h := newHarness(t, openRules)
	// (A guest kernel does not send 127.0.0.0/8 out; the rules refuse it
	// anyway, see the policy tests.)
	_, err := h.dialTCP(hostIP, port)
	refused(t, err)
	h.waitEvent(events.Blocked, policy.ReasonHostLoopback, hostIP+":"+itoa(port))
	_, err = h.dialTCP("169.254.169.254", 80)
	refused(t, err)
	h.waitEvent(events.Blocked, policy.ReasonMetadata, "169.254.169.254:80")
	_, err = h.dialTCP(gwIP, 22)
	refused(t, err)
}

func TestTheHostsOwnAddressIsRefused(t *testing.T) {
	var lan netip.Addr
	for _, a := range hostAddrs()() {
		if a.Is4() && !a.IsLoopback() && !a.IsLinkLocalUnicast() {
			lan = a
			break
		}
	}
	if !lan.IsValid() {
		t.Skip("no IPv4 interface address on this host")
	}
	port := echo(t, net.JoinHostPort(lan.String(), "0"))
	h := newHarness(t, openRules)
	_, err := h.dialTCP(lan.String(), port)
	refused(t, err)
	h.waitEvent(events.Blocked, policy.ReasonHostAddress, lan.String()+":"+itoa(port))
}

func TestListedLocalhostPortsReachTheHostLoopback(t *testing.T) {
	port := echo(t, "127.0.0.1:0")
	other := echo(t, "127.0.0.1:0")
	h := newHarness(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"`+hostIP+`/32","ports":[`+itoa(port)+`],"proto":"tcp","accept":true}]}`)
	c, err := h.dialTCP(hostIP, port)
	if err != nil {
		t.Fatal(err)
	}
	roundTrip(t, c)
	_, err = h.dialTCP(hostIP, other)
	refused(t, err)

	// A change of rules closes the connection they no longer allow.
	h.writeRules(openRules)
	if changed, err := h.store.Reload(); !changed || err != nil {
		t.Fatalf("reload: %v %v", changed, err)
	}
	_ = c.SetReadDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Read(make([]byte, 1)); err == nil {
		t.Fatal("the connection stayed open")
	}
	_, err = h.dialTCP(hostIP, port)
	refused(t, err)
}

func TestUserDenyAndDefaultDrop(t *testing.T) {
	h := newHarness(t, `{"version":1,"default_accept":false,"rules":[
		{"net":"10.0.0.0/8","accept":false,"reason":"default:lan"}]}`)
	_, err := h.dialTCP("10.1.2.3", 80)
	refused(t, err)
	h.waitEvent(events.Blocked, "default:lan", "10.1.2.3:80")
	_, err = h.dialTCP("192.0.2.10", 443)
	refused(t, err)
	h.waitEvent(events.Blocked, policy.ReasonDefaultNone, "192.0.2.10:443")
}

func TestUDPFollowsTheRules(t *testing.T) {
	pc, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer pc.Close()
	go func() {
		buf := make([]byte, 1500)
		for {
			n, from, err := pc.ReadFrom(buf)
			if err != nil {
				return
			}
			_, _ = pc.WriteTo(buf[:n], from)
		}
	}()
	port := uint16(pc.LocalAddr().(*net.UDPAddr).Port)
	h := newHarness(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"`+hostIP+`","ports":[`+itoa(port)+`],"proto":"udp","accept":true}]}`)

	c := h.dialUDP(hostIP, port)
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write([]byte("hi")); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 16)
	n, err := c.Read(buf)
	if err != nil || string(buf[:n]) != "hi" {
		t.Fatalf("udp echo: %q %v", buf[:n], err)
	}

	// Another host port: dropped.
	other, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()
	otherPort := uint16(other.LocalAddr().(*net.UDPAddr).Port)
	d := h.dialUDP(hostIP, otherPort)
	defer d.Close()
	_, _ = d.Write([]byte("hi"))
	_ = other.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
	if _, _, err := other.ReadFrom(buf); err == nil {
		t.Fatal("udp to an unlisted host port arrived")
	}
	h.waitEvent(events.Blocked, policy.ReasonHostLoopback, hostIP+":"+itoa(otherPort))
}

func TestDNSStripsRebindingAnswers(t *testing.T) {
	h := newHarness(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"10.0.0.0/8","accept":false,"reason":"default:lan"}]}`)
	h.gw.dns.lookup = func(_ context.Context, network, host string) ([]netip.Addr, error) {
		switch host {
		case "rebind.example.":
			return []netip.Addr{
				netip.MustParseAddr("127.0.0.1"),
				netip.MustParseAddr("10.0.0.7"),
				netip.MustParseAddr("93.184.216.34"),
			}, nil
		case "private.example.":
			return []netip.Addr{netip.MustParseAddr("10.0.0.8")}, nil
		}
		return nil, &net.DNSError{Err: "no such host", Name: host, IsNotFound: true}
	}
	ask := func(name string, qtype uint16) *dns.Msg {
		t.Helper()
		c := h.dialUDP(gwIP, 53)
		defer c.Close()
		_ = c.SetDeadline(time.Now().Add(5 * time.Second))
		q := new(dns.Msg)
		q.SetQuestion(name, qtype)
		wire, _ := q.Pack()
		if _, err := c.Write(wire); err != nil {
			t.Fatal(err)
		}
		buf := make([]byte, 4096)
		n, err := c.Read(buf)
		if err != nil {
			t.Fatal(err)
		}
		m := new(dns.Msg)
		if err := m.Unpack(buf[:n]); err != nil {
			t.Fatal(err)
		}
		return m
	}
	m := ask("rebind.example.", dns.TypeA)
	if len(m.Answer) != 1 || m.Answer[0].(*dns.A).A.String() != "93.184.216.34" {
		t.Fatalf("answers: %v", m.Answer)
	}
	h.waitEvent(events.DNSStripped, policy.ReasonLoopback, "127.0.0.1")
	h.waitEvent(events.DNSStripped, "default:lan", "10.0.0.7")
	if m := ask("private.example.", dns.TypeA); m.Rcode != dns.RcodeSuccess || len(m.Answer) != 0 {
		t.Fatalf("private answer kept: %v", m)
	}
	if m := ask("metadata.google.internal.", dns.TypeA); m.Rcode != dns.RcodeNameError {
		t.Fatalf("metadata name: %v", m)
	}
	if m := ask("7.0.0.10.in-addr.arpa.", dns.TypePTR); m.Rcode != dns.RcodeNameError {
		t.Fatalf("reverse lookup of a refused address: %v", m)
	}
	if m := ask("missing.example.", dns.TypeA); m.Rcode != dns.RcodeNameError {
		t.Fatalf("missing name: %v", m)
	}
	// No name the guest chose is ever in the events.
	if s := h.events.String(); strings.Contains(s, "example") || strings.Contains(s, "metadata") {
		t.Fatalf("a name leaked into the events: %s", s)
	}
}

func itoa(p uint16) string { return strconv.Itoa(int(p)) }
