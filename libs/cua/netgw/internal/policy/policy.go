// Package policy decides, for every connection a guest opens, whether the
// gateway dials it.
//
// The decision has three parts, in this order (the first that answers
// wins):
//
//  1. Locked denies the gateway applies whatever the rules say: the host's
//     loopback (127.0.0.0/8, ::1), 0.0.0.0/8, the cloud metadata endpoints,
//     multicast, broadcast and the virtual network's own subnet.
//  2. The host itself: the virtual network's host address (which the
//     gateway maps to the host's loopback) and, unless BlockHostAddrs is
//     off, the host's own interface addresses. Only a rule whose network
//     is exactly that one address (a /32 or /128) may open them, so a
//     broad allow such as 0.0.0.0/0 never reaches the host.
//  3. The rules file: an ordered list, first match wins, then the default.
//
// The rules file is written by cua (crates/cua-vmm, netgw.rs) and may be
// replaced at any time; see Store.
package policy

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
	"slices"
	"strings"
)

// Proto is a transport protocol.
type Proto string

const (
	TCP Proto = "tcp"
	UDP Proto = "udp"
)

// Rule is one entry of the rules file.
type Rule struct {
	// Net is an address or a CIDR (IPv4 or IPv6).
	Net string `json:"net"`
	// Ports limits the rule to these destination ports (empty: every port).
	Ports []uint16 `json:"ports,omitempty"`
	// Proto limits the rule to one protocol ("tcp", "udp"; empty: both).
	Proto Proto `json:"proto,omitempty"`
	// Accept dials a matching connection; false refuses it.
	Accept bool `json:"accept"`
	// Reason is the id reported when this rule refuses a connection
	// ("user:deny", "default:lan", ...).
	Reason string `json:"reason,omitempty"`

	prefix netip.Prefix
}

// File is the rules file.
type File struct {
	// Version is 1.
	Version int `json:"version"`
	// Rules, in order; the first match wins.
	Rules []Rule `json:"rules"`
	// DefaultAccept is the verdict when no rule matches.
	DefaultAccept bool `json:"default_accept"`
	// DefaultReason is reported when the default refuses ("default:none").
	DefaultReason string `json:"default_reason,omitempty"`
	// BlockHostAddrs refuses the host's own interface addresses unless an
	// exact rule opens one (on unless the file says false).
	BlockHostAddrs *bool `json:"block_host_addrs,omitempty"`
}

// MaxRules is the most rules one file may hold.
const MaxRules = 4096

// Reasons the gateway reports on its own.
const (
	ReasonLoopback     = "platform:loopback"
	ReasonMetadata     = "platform:metadata"
	ReasonGateway      = "platform:gateway"
	ReasonMulticast    = "platform:multicast"
	ReasonHostLoopback = "default:host-loopback"
	ReasonHostAddress  = "default:host-address"
	ReasonUserDeny     = "user:deny"
	ReasonDefaultNone  = "default:none"
)

// Parse checks a rules file.
func Parse(data []byte) (*File, error) {
	var f File
	dec := json.NewDecoder(strings.NewReader(string(data)))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&f); err != nil {
		return nil, fmt.Errorf("rules: %w", err)
	}
	if f.Version != 1 {
		return nil, fmt.Errorf("rules: version %d (this gateway reads version 1)", f.Version)
	}
	if len(f.Rules) > MaxRules {
		return nil, fmt.Errorf("rules: %d rules, at most %d", len(f.Rules), MaxRules)
	}
	for i := range f.Rules {
		r := &f.Rules[i]
		p, err := parseNet(r.Net)
		if err != nil {
			return nil, fmt.Errorf("rules[%d]: %w", i, err)
		}
		r.prefix = p
		switch r.Proto {
		case "", TCP, UDP:
		default:
			return nil, fmt.Errorf("rules[%d]: proto %q (tcp, udp or none)", i, r.Proto)
		}
		if r.Reason == "" && !r.Accept {
			r.Reason = ReasonUserDeny
		}
	}
	if f.DefaultReason == "" {
		f.DefaultReason = ReasonDefaultNone
	}
	return &f, nil
}

// Load reads and checks a rules file.
func Load(path string) (*File, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return Parse(data)
}

func parseNet(s string) (netip.Prefix, error) {
	s = strings.TrimSpace(s)
	if strings.Contains(s, "/") {
		p, err := netip.ParsePrefix(s)
		if err != nil {
			return netip.Prefix{}, fmt.Errorf("net %q: %w", s, err)
		}
		return normalizePrefix(p.Masked()), nil
	}
	a, err := netip.ParseAddr(s)
	if err != nil {
		return netip.Prefix{}, fmt.Errorf("net %q: %w", s, err)
	}
	a = a.Unmap()
	return netip.PrefixFrom(a, a.BitLen()), nil
}

// normalizePrefix turns ::ffff:a.b.c.d/96+n into a.b.c.d/n, so mapped
// forms are judged as IPv4.
func normalizePrefix(p netip.Prefix) netip.Prefix {
	if p.Addr().Is4In6() && p.Bits() >= 96 {
		return netip.PrefixFrom(p.Addr().Unmap(), p.Bits()-96)
	}
	return p
}

// Normalize judges IPv4-mapped (::ffff:0:0/96) and NAT64 (64:ff9b::/96)
// addresses as the IPv4 address they carry.
func Normalize(a netip.Addr) netip.Addr {
	a = a.Unmap()
	if a.Is6() {
		b := a.As16()
		if b[0] == 0x00 && b[1] == 0x64 && b[2] == 0xff && b[3] == 0x9b &&
			b[4]|b[5]|b[6]|b[7]|b[8]|b[9]|b[10]|b[11] == 0 {
			return netip.AddrFrom4([4]byte{b[12], b[13], b[14], b[15]})
		}
	}
	return a
}

var (
	loopbackNets = []netip.Prefix{
		netip.MustParsePrefix("127.0.0.0/8"),
		netip.MustParsePrefix("0.0.0.0/8"),
		netip.MustParsePrefix("::1/128"),
		netip.MustParsePrefix("::/128"),
	}
	metadataNets = []netip.Prefix{
		netip.MustParsePrefix("169.254.169.254/32"),
		netip.MustParsePrefix("169.254.170.2/32"),
		netip.MustParsePrefix("fd00:ec2::254/128"),
		netip.MustParsePrefix("168.63.129.16/32"),
		netip.MustParsePrefix("100.100.100.200/32"),
	}
	multicastNets = []netip.Prefix{
		netip.MustParsePrefix("224.0.0.0/4"),
		netip.MustParsePrefix("240.0.0.0/4"),
		netip.MustParsePrefix("ff00::/8"),
	}
)

func inAny(nets []netip.Prefix, a netip.Addr) bool {
	for _, n := range nets {
		if n.Contains(a) {
			return true
		}
	}
	return false
}

// Network is what the gateway knows about its own network.
type Network struct {
	// Subnet is the virtual network the guest is on.
	Subnet netip.Prefix
	// HostIP is the address in Subnet the gateway maps to the host's
	// loopback (invalid: none).
	HostIP netip.Addr
	// HostAddrs returns the host's own interface addresses (nil: none).
	HostAddrs func() []netip.Addr
}

// Verdict is the answer for one connection.
type Verdict struct {
	Accept bool
	// Reason is set when Accept is false.
	Reason string
	// ToHostLoopback: dial the host's loopback instead (the destination was
	// HostIP).
	ToHostLoopback bool
}

// Decide answers whether the guest may open proto to dst:port.
func (f *File) Decide(n *Network, proto Proto, dst netip.Addr, port uint16) Verdict {
	dst = Normalize(dst)
	switch {
	case inAny(loopbackNets, dst):
		return Verdict{Reason: ReasonLoopback}
	case inAny(metadataNets, dst):
		return Verdict{Reason: ReasonMetadata}
	case inAny(multicastNets, dst) || dst == netip.AddrFrom4([4]byte{255, 255, 255, 255}):
		return Verdict{Reason: ReasonMulticast}
	}
	if n.HostIP.IsValid() && dst == n.HostIP {
		if r := f.exact(dst, proto, port); r != nil {
			if r.Accept {
				return Verdict{Accept: true, ToHostLoopback: true}
			}
			return Verdict{Reason: r.Reason}
		}
		return Verdict{Reason: ReasonHostLoopback}
	}
	if n.Subnet.IsValid() && n.Subnet.Contains(dst) {
		return Verdict{Reason: ReasonGateway}
	}
	if f.blockHostAddrs() && n.HostAddrs != nil && slices.Contains(n.HostAddrs(), dst) {
		if r := f.exact(dst, proto, port); r != nil {
			if r.Accept {
				return Verdict{Accept: true}
			}
			return Verdict{Reason: r.Reason}
		}
		return Verdict{Reason: ReasonHostAddress}
	}
	for i := range f.Rules {
		r := &f.Rules[i]
		if r.matches(dst, proto, port) {
			if r.Accept {
				return Verdict{Accept: true}
			}
			return Verdict{Reason: r.Reason}
		}
	}
	if f.DefaultAccept {
		return Verdict{Accept: true}
	}
	return Verdict{Reason: f.DefaultReason}
}

// StripAnswer reports whether a DNS answer of dst should be removed (and
// why): the address is refused on every port, so a name that resolves to
// it (a rebinding answer such as 127.0.0.1 or 10.0.0.1) never reaches the
// guest. Rules limited to ports or one protocol do not count.
func (f *File) StripAnswer(n *Network, dst netip.Addr) (bool, string) {
	dst = Normalize(dst)
	switch {
	case inAny(loopbackNets, dst):
		return true, ReasonLoopback
	case inAny(metadataNets, dst):
		return true, ReasonMetadata
	case inAny(multicastNets, dst):
		return true, ReasonMulticast
	case n.HostIP.IsValid() && dst == n.HostIP:
		return true, ReasonHostLoopback
	case n.Subnet.IsValid() && n.Subnet.Contains(dst):
		return true, ReasonGateway
	}
	if f.blockHostAddrs() && n.HostAddrs != nil && slices.Contains(n.HostAddrs(), dst) {
		return true, ReasonHostAddress
	}
	for i := range f.Rules {
		r := &f.Rules[i]
		if len(r.Ports) > 0 || r.Proto != "" || !r.prefix.Contains(dst) {
			continue
		}
		if r.Accept {
			return false, ""
		}
		return true, r.Reason
	}
	if f.DefaultAccept {
		return false, ""
	}
	return true, f.DefaultReason
}

func (f *File) blockHostAddrs() bool {
	return f.BlockHostAddrs == nil || *f.BlockHostAddrs
}

// exact is the first rule that names exactly dst (a /32 or /128) and
// matches proto and port.
func (f *File) exact(dst netip.Addr, proto Proto, port uint16) *Rule {
	for i := range f.Rules {
		r := &f.Rules[i]
		if r.prefix.Bits() == dst.BitLen() && r.prefix.Addr() == dst && r.matches(dst, proto, port) {
			return r
		}
	}
	return nil
}

func (r *Rule) matches(dst netip.Addr, proto Proto, port uint16) bool {
	if !r.prefix.Contains(dst) {
		return false
	}
	if r.Proto != "" && r.Proto != proto {
		return false
	}
	return len(r.Ports) == 0 || slices.Contains(r.Ports, port)
}
