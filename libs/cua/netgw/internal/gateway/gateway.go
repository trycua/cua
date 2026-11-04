// Copyright The gvisor-tap-vsock Authors.
// Copyright Cua AI, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may
// not use this file except in compliance with the License. You may obtain a
// copy of the License at http://www.apache.org/licenses/LICENSE-2.0 (also in
// libs/cua/netgw/LICENSES/Apache-2.0-gvisor-tap-vsock.txt).
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
// WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//
// MODIFIED: adapted from gvisor-tap-vsock v0.9.0,
// pkg/virtualnetwork/virtualnetwork.go and pkg/virtualnetwork/services.go.
// Changes by Cua: the stack's TCP and UDP handlers are the policy-checked
// forwarders of this package (no NAT table, no ICMP forwarder, no HTTP
// API; port forwards come from the configuration only), its DNS service is this package's filtering
// resolver, and DHCP is optional.

// Package gateway is the Cua network gateway: a user-mode network (gVisor's
// netstack, through gvisor-tap-vsock's switch, link and DHCP) for one VM,
// where every connection the guest opens is checked against the rules
// (policy package) before the gateway dials it from the host.
package gateway

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"sync"
	"time"

	"github.com/containers/gvisor-tap-vsock/pkg/services/dhcp"
	"github.com/containers/gvisor-tap-vsock/pkg/services/forwarder"
	"github.com/containers/gvisor-tap-vsock/pkg/tap"
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	log "github.com/sirupsen/logrus"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/network/arp"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"

	"github.com/trycua/cua/libs/cua/netgw/internal/config"
	"github.com/trycua/cua/libs/cua/netgw/internal/events"
	"github.com/trycua/cua/libs/cua/netgw/internal/policy"
	"github.com/trycua/cua/libs/cua/netgw/internal/tree"
)

// Gateway is one VM's network.
type Gateway struct {
	cfg     *config.Config
	stack   *stack.Stack
	sw      *tap.Switch
	rules   *policy.Store
	events  *events.Sink
	network policy.Network
	conns   *registry
	dns     *resolver
	// The names the resolver answered for each address ([bindings]).
	bound *bindings
	ports   *forwarder.PortsForwarder
	pool    *tap.IPPool
	// The VM's tree (nil: none): its running members, by tree address.
	tree *tree.Peers

	// HostLoopback is where a connection to the host address goes when a
	// rule allows it (127.0.0.1; tests change it).
	HostLoopback netip.Addr

	dialTimeout time.Duration
}

// New builds the network of cfg. The rules are checked on every new
// connection; a change of rules also closes the open connections they no
// longer allow.
func New(cfg *config.Config, rules *policy.Store, sink *events.Sink) (*Gateway, error) {
	subnet := netip.MustParsePrefix(cfg.Subnet)
	pool, err := tap.NewIPPool(cfg.Subnet)
	if err != nil {
		return nil, fmt.Errorf("subnet: %w", err)
	}
	if err := pool.Reserve(cfg.GatewayIP, cfg.GatewayMAC); err != nil {
		return nil, fmt.Errorf("gateway address: %w", err)
	}
	for ip, mac := range cfg.StaticLeases {
		if err := pool.Reserve(ip, mac); err != nil {
			return nil, fmt.Errorf("static lease %s: %w", ip, err)
		}
	}
	var virtualIPs []string
	var hostIP netip.Addr
	if cfg.HostIP != "" {
		hostIP = netip.MustParseAddr(cfg.HostIP)
		if cfg.HostIP != cfg.GatewayIP {
			virtualIPs = append(virtualIPs, cfg.HostIP)
			// Never leased: it stands for the host.
			if err := pool.Reserve(cfg.HostIP, "reserved:host"); err != nil {
				return nil, fmt.Errorf("host address: %w", err)
			}
		}
	}
	// More resolver addresses: the stack's own, answered for in ARP, never
	// leased.
	for _, ip := range cfg.DNSIPs {
		virtualIPs = append(virtualIPs, ip)
		if err := pool.Reserve(ip, "reserved:dns"); err != nil {
			return nil, fmt.Errorf("dns address %s: %w", ip, err)
		}
	}
	link, err := tap.NewLinkEndpoint(false, uint32(cfg.MTU), cfg.GatewayMAC, cfg.GatewayIP, virtualIPs)
	if err != nil {
		return nil, fmt.Errorf("link: %w", err)
	}
	sw := tap.NewSwitch(false)
	link.Connect(sw)
	sw.Connect(link)

	s := stack.New(stack.Options{
		NetworkProtocols:   []stack.NetworkProtocolFactory{ipv4.NewProtocol, arp.NewProtocol},
		TransportProtocols: []stack.TransportProtocolFactory{tcp.NewProtocol, udp.NewProtocol},
	})
	if err := s.CreateNIC(1, link); err != nil {
		return nil, errors.New(err.String())
	}
	for _, ip := range append([]string{cfg.GatewayIP}, cfg.DNSIPs...) {
		if err := s.AddProtocolAddress(1, tcpip.ProtocolAddress{
			Protocol:          ipv4.ProtocolNumber,
			AddressWithPrefix: tcpip.AddrFrom4(netip.MustParseAddr(ip).As4()).WithPrefix(),
		}, stack.AddressProperties{}); err != nil {
			return nil, errors.New(err.String())
		}
	}
	// Spoofing and promiscuous mode: the stack answers for every
	// destination, so the forwarders see each connection the guest opens.
	s.SetSpoofing(1, true)
	s.SetPromiscuousMode(1, true)
	sub, err := tcpip.NewSubnet(
		tcpip.AddrFrom4(subnet.Masked().Addr().As4()),
		tcpip.MaskFromBytes(net.CIDRMask(subnet.Bits(), 32)),
	)
	if err != nil {
		return nil, fmt.Errorf("subnet: %w", err)
	}
	s.SetRouteTable([]tcpip.Route{{Destination: sub, NIC: 1}})

	g := &Gateway{
		cfg:    cfg,
		stack:  s,
		sw:     sw,
		rules:  rules,
		events: sink,
		network: policy.Network{
			Subnet:    subnet.Masked(),
			HostIP:    hostIP,
			HostAddrs: hostAddrs(),
		},
		conns:        newRegistry(),
		bound:        newBindings(),
		pool:         pool,
		HostLoopback: netip.MustParseAddr("127.0.0.1"),
		dialTimeout:  time.Duration(cfg.TCPConnectTimeoutSecs) * time.Second,
	}
	if cfg.Tree != nil {
		g.tree = tree.Open(cfg.Tree.Peers)
	}
	s.SetTransportProtocolHandler(tcp.ProtocolNumber, g.tcpForwarder().HandlePacket)
	s.SetTransportProtocolHandler(udp.ProtocolNumber, g.udpForwarder().HandlePacket)
	rules.OnChange(g.recheck)

	if g.dns, err = newResolver(g); err != nil {
		return nil, fmt.Errorf("dns: %w", err)
	}
	if len(cfg.Forwards) > 0 {
		g.ports = forwarder.NewPortsForwarder(s)
		for _, f := range cfg.Forwards {
			if err := g.ports.Expose(types.TCP, f.Host, f.Guest); err != nil {
				return nil, fmt.Errorf("forward %s: %w", f.Host, err)
			}
		}
	}
	if cfg.DHCP {
		srv, err := dhcp.New(&types.Configuration{
			MTU:               cfg.MTU,
			Subnet:            cfg.Subnet,
			GatewayIP:         cfg.GatewayIP,
			GatewayMacAddress: cfg.GatewayMAC,
			DNSSearchDomains:  cfg.SearchDomains,
			DHCPStaticLeases:  cfg.StaticLeases,
		}, s, pool)
		if err != nil {
			return nil, fmt.Errorf("dhcp: %w", err)
		}
		go func() {
			if err := srv.Serve(); err != nil {
				log.Errorf("dhcp: %v", err)
			}
		}()
	}
	return g, nil
}

// ServeTree takes the tree's connections on the configured ingress socket
// and follows the peers file, until ctx ends. Nothing without a tree.
func (g *Gateway) ServeTree(ctx context.Context) error {
	if g.cfg.Tree == nil || g.tree == nil {
		return nil
	}
	ln, err := tree.Listen(g.cfg.Tree.Ingress)
	if err != nil {
		return fmt.Errorf("tree ingress %s: %w", g.cfg.Tree.Ingress, err)
	}
	guest := netip.MustParseAddr(g.cfg.Tree.GuestIP)
	go func() {
		<-ctx.Done()
		ln.Close()
		os.Remove(g.cfg.Tree.Ingress)
	}()
	go g.tree.Watch(time.Second, ctx.Done(), func(err error) {
		log.Errorf("tree peers not reloaded (the previous ones stay): %v", err)
	})
	go tree.Serve(ln, g.tree, func(ctx context.Context, port uint16) (net.Conn, error) {
		return gonet.DialContextTCP(ctx, g.stack, tcpip.FullAddress{
			NIC:  1,
			Addr: tcpip.AddrFrom4(guest.As4()),
			Port: port,
		}, ipv4.ProtocolNumber)
	}, g.dialTimeout)
	return nil
}

// treeSocket is the ingress of tree member dst, when it is one.
func (g *Gateway) treeSocket(dst netip.Addr) (string, bool) {
	if g.tree == nil {
		return "", false
	}
	return g.tree.Socket(dst)
}

// Close stops the published ports.
func (g *Gateway) Close() error {
	if g.ports != nil {
		return g.ports.Close()
	}
	return nil
}

// AcceptVfkit serves a Virtualization.framework file-handle device.
func (g *Gateway) AcceptVfkit(ctx context.Context, conn net.Conn) error {
	return g.sw.Accept(ctx, conn, types.VfkitProtocol)
}

// AcceptQemu serves a QEMU -netdev stream connection.
func (g *Gateway) AcceptQemu(ctx context.Context, conn net.Conn) error {
	return g.sw.Accept(ctx, conn, types.QemuProtocol)
}

// decide is the rules' answer, with an event when they refuse.
func (g *Gateway) decide(proto policy.Proto, dst netip.Addr, port uint16) policy.Verdict {
	v := g.rules.Current().DecideNamed(&g.network, proto, dst, port, g.bound.names(dst))
	if !v.Accept {
		g.events.Emit(events.Blocked, v.Reason, string(proto), netip.AddrPortFrom(dst, port))
		log.Debugf("refused %s %s: %s", proto, netip.AddrPortFrom(dst, port), v.Reason)
	}
	return v
}

// recheck closes the open connections the new rules refuse.
func (g *Gateway) recheck(f *policy.File) {
	for _, c := range g.conns.snapshot() {
		v := f.DecideNamed(&g.network, c.proto, c.dst, c.port, g.bound.names(c.dst))
		if !v.Accept {
			g.events.Emit(events.Blocked, v.Reason, string(c.proto), netip.AddrPortFrom(c.dst, c.port))
			c.close()
		}
	}
}

// hostAddrs lists the host's interface addresses, refreshed at most every
// two seconds (they change when the host changes networks).
func hostAddrs() func() []netip.Addr {
	var (
		mu   sync.Mutex
		at   time.Time
		last []netip.Addr
	)
	return func() []netip.Addr {
		mu.Lock()
		defer mu.Unlock()
		if time.Since(at) < 2*time.Second && last != nil {
			return last
		}
		addrs, err := net.InterfaceAddrs()
		if err != nil {
			log.Warnf("host addresses: %v", err)
			return last
		}
		out := make([]netip.Addr, 0, len(addrs))
		for _, a := range addrs {
			if p, err := netip.ParsePrefix(a.String()); err == nil {
				out = append(out, p.Addr().Unmap().WithZone(""))
			}
		}
		last, at = out, time.Now()
		return last
	}
}

func toAddr(a tcpip.Address) netip.Addr {
	ip, _ := netip.AddrFromSlice(a.AsSlice())
	return ip.Unmap()
}
