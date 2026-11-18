package gateway

// The guest's resolver, on the gateway address (and any more DNS addresses
// of the configuration), port 53 (UDP and TCP).
//
// Questions are answered from the host: A and AAAA through the host's own
// resolver (so the host's VPN and split DNS apply), other types forwarded
// to the host's name servers. Before an answer goes back, every address in
// it (A, AAAA, and the address hints of SVCB/HTTPS records) that the rules
// refuse on every port is removed, so a name that resolves to 127.0.0.1,
// 10.0.0.1 or a metadata address (DNS rebinding) never reaches the guest.
// A reverse lookup of such an address gets NXDOMAIN, and so do the cloud
// metadata names.
//
// Names are never logged: the events carry the reason and the removed
// address only.
//
// Two kinds of names are answered here and never leave the gateway:
//
//   - `host`: the machine that runs the Space, at the host address (only
//     the ports its rules open answer there);
//   - `<space>.<tree>`: a member of the VM's own tree, at its tree address
//     (from the peers file). A name of another tree is not known here.

import (
	"context"
	"errors"
	"net"
	"net/netip"
	"strings"
	"time"

	"github.com/miekg/dns"
	log "github.com/sirupsen/logrus"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"

	"github.com/trycua/cua/libs/cua/netgw/internal/events"
)

// answerTTL is the TTL of answers made from the host's resolver.
const answerTTL = 60

// Names that always get NXDOMAIN (cloud metadata).
var blockedNames = map[string]bool{
	"metadata.google.internal.":   true,
	"metadata.":                   true,
	"instance-data.":              true,
	"instance-data.ec2.internal.": true,
}

type resolver struct {
	g           *Gateway
	nameservers []string
	client      *dns.Client
	lookup      func(ctx context.Context, network, host string) ([]netip.Addr, error)
}

func newResolver(g *Gateway) (*resolver, error) {
	r := &resolver{
		g:      g,
		client: &dns.Client{Timeout: 5 * time.Second},
		lookup: net.DefaultResolver.LookupNetIP,
	}
	r.nameservers = g.cfg.DNSUpstream
	if len(r.nameservers) == 0 {
		if conf, err := dns.ClientConfigFromFile("/etc/resolv.conf"); err == nil {
			for _, s := range conf.Servers {
				r.nameservers = append(r.nameservers, net.JoinHostPort(s, conf.Port))
			}
		}
	}
	mux := dns.HandlerFunc(r.serve)
	for _, ip := range append([]string{g.cfg.GatewayIP}, g.cfg.DNSIPs...) {
		addr := tcpip.FullAddress{
			NIC:  1,
			Addr: tcpip.AddrFrom4(netip.MustParseAddr(ip).As4()),
			Port: 53,
		}
		udpConn, err := gonet.DialUDP(g.stack, &addr, nil, ipv4.ProtocolNumber)
		if err != nil {
			return nil, err
		}
		tcpLn, err := gonet.ListenTCP(g.stack, addr, ipv4.ProtocolNumber)
		if err != nil {
			return nil, err
		}
		go func() {
			if err := (&dns.Server{PacketConn: udpConn, Handler: mux}).ActivateAndServe(); err != nil {
				log.Errorf("dns (udp): %v", err)
			}
		}()
		go func() {
			if err := (&dns.Server{Listener: tcpLn, Handler: mux}).ActivateAndServe(); err != nil {
				log.Errorf("dns (tcp): %v", err)
			}
		}()
	}
	return r, nil
}

func (r *resolver) serve(w dns.ResponseWriter, req *dns.Msg) {
	m := r.answer(req)
	size := dns.MinMsgSize
	if _, ok := w.RemoteAddr().(*net.TCPAddr); ok {
		size = dns.MaxMsgSize
	}
	if e := req.IsEdns0(); e != nil {
		size = int(e.UDPSize())
	}
	m.Truncate(size)
	if err := w.WriteMsg(m); err != nil {
		log.Debugf("dns reply: %v", err)
	}
}

// answer resolves req and filters the reply.
func (r *resolver) answer(req *dns.Msg) *dns.Msg {
	m := new(dns.Msg)
	m.SetReply(req)
	m.RecursionAvailable = true
	if len(req.Question) != 1 {
		m.Rcode = dns.RcodeFormatError
		return m
	}
	q := req.Question[0]
	name := strings.ToLower(q.Name)
	if blockedNames[name] {
		m.Rcode = dns.RcodeNameError
		return m
	}
	if ip, ok := r.localName(name); ok {
		if q.Qclass == dns.ClassINET && q.Qtype == dns.TypeA && ip.IsValid() {
			hdr := dns.RR_Header{Name: q.Name, Rrtype: dns.TypeA, Class: dns.ClassINET, Ttl: 5}
			m.Answer = append(m.Answer, &dns.A{Hdr: hdr, A: net.IP(ip.AsSlice())})
		} else if !ip.IsValid() {
			m.Rcode = dns.RcodeNameError
		}
		// Any other type: the name exists with no such record.
		return m
	}
	rules := r.g.rules.Current()
	if q.Qtype != dns.TypePTR && !rules.DNSAllowed(name) {
		// A denied name, or no network: the question goes no further.
		m.Rcode = dns.RcodeRefused
		return m
	}
	if q.Qtype == dns.TypePTR {
		if ip, ok := reverseAddr(name); ok {
			if strip, _ := r.strip(ip); strip {
				m.Rcode = dns.RcodeNameError
				return m
			}
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	switch {
	case q.Qclass == dns.ClassINET && (q.Qtype == dns.TypeA || q.Qtype == dns.TypeAAAA):
		network := "ip4"
		if q.Qtype == dns.TypeAAAA {
			network = "ip6"
		}
		ips, err := r.lookup(ctx, network, q.Name)
		if err != nil {
			var de *net.DNSError
			if errors.As(err, &de) && de.IsNotFound {
				// No such name, or no address of this family.
				if len(ips) == 0 && r.exists(ctx, q.Name) {
					break
				}
				m.Rcode = dns.RcodeNameError
			} else {
				m.Rcode = dns.RcodeServerFailure
			}
			break
		}
		for _, ip := range ips {
			hdr := dns.RR_Header{Name: q.Name, Class: dns.ClassINET, Ttl: answerTTL}
			if q.Qtype == dns.TypeA && ip.Unmap().Is4() {
				hdr.Rrtype = dns.TypeA
				m.Answer = append(m.Answer, &dns.A{Hdr: hdr, A: net.IP(ip.Unmap().AsSlice())})
			} else if q.Qtype == dns.TypeAAAA && ip.Is6() && !ip.Is4In6() {
				hdr.Rrtype = dns.TypeAAAA
				m.Answer = append(m.Answer, &dns.AAAA{Hdr: hdr, AAAA: net.IP(ip.AsSlice())})
			}
		}
	default:
		if len(r.nameservers) == 0 {
			m.Rcode = dns.RcodeServerFailure
			return m
		}
		got := r.forward(req)
		if got == nil {
			m.Rcode = dns.RcodeServerFailure
			return m
		}
		got.Id = req.Id
		m = got
	}
	r.filter(m)
	// The addresses an allowed name resolved to stay reachable for as long
	// as the answer lives.
	if rules.DomainVerdict(name) == 1 {
		for _, rr := range m.Answer {
			switch a := rr.(type) {
			case *dns.A:
				if ip, ok := netip.AddrFromSlice(a.A); ok {
					r.g.bound.add(ip.Unmap(), name, time.Duration(a.Hdr.Ttl)*time.Second)
				}
			case *dns.AAAA:
				if ip, ok := netip.AddrFromSlice(a.AAAA); ok {
					r.g.bound.add(ip, name, time.Duration(a.Hdr.Ttl)*time.Second)
				}
			}
		}
	}
	return m
}

// HostName is the name a Space uses for the machine that runs it.
const HostName = "host."

// localName answers the names the gateway owns: `host` (an invalid
// address when this layout has no host address: NXDOMAIN) and the members
// of the VM's tree.
func (r *resolver) localName(name string) (netip.Addr, bool) {
	if name == HostName {
		if r.g.cfg.HostIP == "" {
			return netip.Addr{}, true
		}
		ip, err := netip.ParseAddr(r.g.cfg.HostIP)
		if err != nil {
			return netip.Addr{}, true
		}
		return ip, true
	}
	if r.g.tree != nil && strings.Count(strings.TrimSuffix(name, "."), ".") == 1 {
		if ip, ok := r.g.tree.Lookup(name); ok {
			return ip, true
		}
	}
	return netip.Addr{}, false
}

// exists reports whether name has an address of either family (so a
// missing AAAA is NODATA, not NXDOMAIN).
func (r *resolver) exists(ctx context.Context, name string) bool {
	ips, _ := r.lookup(ctx, "ip", name)
	return len(ips) > 0
}

// forward sends req to the host's name servers in turn.
func (r *resolver) forward(req *dns.Msg) *dns.Msg {
	for _, ns := range r.nameservers {
		resp, _, err := r.client.Exchange(req, ns)
		if err == nil && resp != nil && resp.Truncated {
			tcp := &dns.Client{Net: "tcp", Timeout: r.client.Timeout}
			resp, _, err = tcp.Exchange(req, ns)
		}
		if err == nil && resp != nil {
			return resp
		}
		log.Debugf("dns upstream %s: %v", ns, err)
	}
	return nil
}

// strip asks the rules whether an answer of ip is removed.
func (r *resolver) strip(ip netip.Addr) (bool, string) {
	return r.g.rules.Current().StripAnswer(&r.g.network, ip)
}

// filter removes the refused addresses from every section of m.
func (r *resolver) filter(m *dns.Msg) {
	m.Answer = r.filterRRs(m.Answer)
	m.Ns = r.filterRRs(m.Ns)
	m.Extra = r.filterRRs(m.Extra)
}

func (r *resolver) filterRRs(rrs []dns.RR) []dns.RR {
	out := rrs[:0]
	for _, rr := range rrs {
		keep := true
		switch v := rr.(type) {
		case *dns.A:
			keep = r.keep(v.A)
		case *dns.AAAA:
			keep = r.keep(v.AAAA)
		case *dns.SVCB:
			v.Value = r.filterHints(v.Value)
		case *dns.HTTPS:
			v.Value = r.filterHints(v.Value)
		}
		if keep {
			out = append(out, rr)
		}
	}
	return out
}

func (r *resolver) filterHints(kvs []dns.SVCBKeyValue) []dns.SVCBKeyValue {
	out := kvs[:0]
	for _, kv := range kvs {
		switch h := kv.(type) {
		case *dns.SVCBIPv4Hint:
			h.Hint = r.keepIPs(h.Hint)
			if len(h.Hint) == 0 {
				continue
			}
		case *dns.SVCBIPv6Hint:
			h.Hint = r.keepIPs(h.Hint)
			if len(h.Hint) == 0 {
				continue
			}
		}
		out = append(out, kv)
	}
	return out
}

func (r *resolver) keepIPs(ips []net.IP) []net.IP {
	out := ips[:0]
	for _, ip := range ips {
		if r.keep(ip) {
			out = append(out, ip)
		}
	}
	return out
}

func (r *resolver) keep(ip net.IP) bool {
	a, ok := netip.AddrFromSlice(ip)
	if !ok {
		return false
	}
	if strip, reason := r.strip(a); strip {
		r.g.events.Emit(events.DNSStripped, reason, "", netip.AddrPortFrom(a.Unmap(), 0))
		return false
	}
	return true
}

// reverseAddr parses a reverse-lookup name (in-addr.arpa, ip6.arpa).
func reverseAddr(name string) (netip.Addr, bool) {
	name = strings.TrimSuffix(name, ".")
	if s, ok := strings.CutSuffix(name, ".in-addr.arpa"); ok {
		parts := strings.Split(s, ".")
		if len(parts) != 4 {
			return netip.Addr{}, false
		}
		for i, j := 0, 3; i < j; i, j = i+1, j-1 {
			parts[i], parts[j] = parts[j], parts[i]
		}
		a, err := netip.ParseAddr(strings.Join(parts, "."))
		return a, err == nil
	}
	if s, ok := strings.CutSuffix(name, ".ip6.arpa"); ok {
		nibbles := strings.Split(s, ".")
		if len(nibbles) != 32 {
			return netip.Addr{}, false
		}
		var b strings.Builder
		for i := 31; i >= 0; i-- {
			b.WriteString(nibbles[i])
			if i%4 == 0 && i != 0 {
				b.WriteByte(':')
			}
		}
		a, err := netip.ParseAddr(b.String())
		return a, err == nil
	}
	return netip.Addr{}, false
}
