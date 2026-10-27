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
// pkg/services/forwarder/tcp.go, udp.go and udp_proxy.go.
// Changes by Cua: every new TCP connection and UDP flow is checked against
// the rules before the gateway dials it (a refused TCP connection is
// reset, a refused UDP flow dropped, and both reported); the host address
// leads to the host's loopback only when the rules allow it (no NAT
// table); open connections are tracked so a change of rules can close
// them; the outbound connection is closed when the guest's endpoint
// cannot be made.

package gateway

import (
	"context"
	"io"
	"net"
	"net/netip"
	"strconv"
	"sync"
	"time"

	"github.com/containers/gvisor-tap-vsock/pkg/services/forwarder"
	"github.com/inetaf/tcpproxy"
	log "github.com/sirupsen/logrus"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
	"gvisor.dev/gvisor/pkg/waiter"

	"github.com/trycua/cua/libs/cua/netgw/internal/policy"
)

// Most TCP connections being dialled at once.
const tcpMaxInFlight = 128

func (g *Gateway) target(v policy.Verdict, dst netip.Addr, port uint16) string {
	if v.ToHostLoopback {
		dst = g.HostLoopback
	}
	return net.JoinHostPort(dst.String(), strconv.Itoa(int(port)))
}

func (g *Gateway) tcpForwarder() *tcp.Forwarder {
	return tcp.NewForwarder(g.stack, 0, tcpMaxInFlight, func(r *tcp.ForwarderRequest) {
		id := r.ID()
		dst, port := toAddr(id.LocalAddress), id.LocalPort
		v := g.decide(policy.TCP, dst, port)
		if !v.Accept {
			r.Complete(true)
			return
		}
		outbound, err := net.DialTimeout("tcp", g.target(v, dst, port), g.dialTimeout)
		if err != nil {
			log.Tracef("dial %s: %v", g.target(v, dst, port), err)
			r.Complete(true)
			return
		}
		var wq waiter.Queue
		ep, tcpErr := r.CreateEndpoint(&wq)
		r.Complete(false)
		if tcpErr != nil {
			outbound.Close()
			if _, ok := tcpErr.(*tcpip.ErrConnectionRefused); ok {
				log.Debugf("guest endpoint: %v", tcpErr)
			} else {
				log.Errorf("guest endpoint: %v", tcpErr)
			}
			return
		}
		guest := gonet.NewTCPConn(&wq, ep)
		var once sync.Once
		closeBoth := func() {
			once.Do(func() {
				guest.Close()
				outbound.Close()
			})
		}
		key := g.conns.add(&conn{proto: policy.TCP, dst: dst, port: port, close: closeBoth})
		defer g.conns.remove(key)
		defer closeBoth()
		remote := tcpproxy.DialProxy{
			DialContext: func(context.Context, string, string) (net.Conn, error) {
				return outbound, nil
			},
		}
		remote.HandleConn(guest)
	})
}

func (g *Gateway) udpForwarder() *udp.Forwarder {
	return udp.NewForwarder(g.stack, func(r *udp.ForwarderRequest) bool {
		id := r.ID()
		if id.LocalAddress == header.IPv4Broadcast {
			return true
		}
		dst, port := toAddr(id.LocalAddress), id.LocalPort
		v := g.decide(policy.UDP, dst, port)
		if !v.Accept {
			return true
		}
		var wq waiter.Queue
		ep, tcpErr := r.CreateEndpoint(&wq)
		if tcpErr != nil {
			if _, ok := tcpErr.(*tcpip.ErrConnectionRefused); ok {
				log.Debugf("guest endpoint: %v", tcpErr)
			} else {
				log.Errorf("guest endpoint: %v", tcpErr)
			}
			return false
		}
		target := g.target(v, dst, port)
		p, _ := forwarder.NewUDPProxy(&idleListener{underlying: gonet.NewUDPConn(&wq, ep)}, func(net.Addr) (net.Conn, error) {
			return net.Dial("udp", target)
		})
		key := g.conns.add(&conn{proto: policy.UDP, dst: dst, port: port, close: func() { _ = p.Close() }})
		go func() {
			p.Run()
			g.conns.remove(key)
			// Datagrams of this flow that arrive from here on start a new
			// request (and a new check).
			ep.Close()
		}()
		return true
	})
}

// idleListener ends a UDP flow after forwarder.UDPConnTrackTimeout without
// traffic (gvisor-tap-vsock's autoStoppingListener).
type idleListener struct {
	underlying interface {
		ReadFrom(b []byte) (int, net.Addr, error)
		WriteTo(b []byte, addr net.Addr) (int, error)
		SetReadDeadline(t time.Time) error
		io.Closer
	}
}

func (l *idleListener) ReadFrom(b []byte) (int, net.Addr, error) {
	_ = l.underlying.SetReadDeadline(time.Now().Add(forwarder.UDPConnTrackTimeout))
	return l.underlying.ReadFrom(b)
}

func (l *idleListener) WriteTo(b []byte, addr net.Addr) (int, error) {
	_ = l.underlying.SetReadDeadline(time.Now().Add(forwarder.UDPConnTrackTimeout))
	return l.underlying.WriteTo(b, addr)
}

func (l *idleListener) SetReadDeadline(t time.Time) error {
	return l.underlying.SetReadDeadline(t)
}

func (l *idleListener) Close() error {
	return l.underlying.Close()
}

// conn is one open connection or UDP flow.
type conn struct {
	proto policy.Proto
	dst   netip.Addr
	port  uint16
	close func()
}

type registry struct {
	mu   sync.Mutex
	next uint64
	open map[uint64]*conn
}

func newRegistry() *registry {
	return &registry{open: map[uint64]*conn{}}
}

func (r *registry) add(c *conn) uint64 {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.next++
	r.open[r.next] = c
	return r.next
}

func (r *registry) remove(key uint64) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.open, key)
}

func (r *registry) snapshot() []*conn {
	r.mu.Lock()
	defer r.mu.Unlock()
	out := make([]*conn, 0, len(r.open))
	for _, c := range r.open {
		out = append(out, c)
	}
	return out
}

// Open is the number of open connections and flows (tests).
func (g *Gateway) Open() int {
	return len(g.conns.snapshot())
}
