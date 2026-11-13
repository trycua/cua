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
// MODIFIED: adapted from gvisor-tap-vsock v0.9.0, pkg/services/dhcp/dhcp.go.
// Change by Cua: the DNS search list (option 119) is sent only when there are
// search domains. The upstream handler always sends it, empty, and Windows
// refuses such an offer (the DHCP client logs error 0xD, "invalid data", and
// keeps its 169.254 address), so no Windows guest got an address.

package gateway

import (
	"errors"
	"math"
	"net"
	"time"

	"github.com/containers/gvisor-tap-vsock/pkg/tap"
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"github.com/insomniacslk/dhcp/dhcpv4"
	"github.com/insomniacslk/dhcp/dhcpv4/server4"
	"github.com/insomniacslk/dhcp/rfc1035label"
	log "github.com/sirupsen/logrus"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
	"gvisor.dev/gvisor/pkg/waiter"
)

const dhcpServerPort = 67

// dhcpDial binds the server's UDP socket on the gateway's NIC (broadcast on).
func dhcpDial(s *stack.Stack, nic tcpip.NICID) (*gonet.UDPConn, error) {
	var wq waiter.Queue
	ep, err := s.NewEndpoint(udp.ProtocolNumber, ipv4.ProtocolNumber, &wq)
	if err != nil {
		return nil, errors.New(err.String())
	}
	ep.SocketOptions().SetBroadcast(true)
	if err := ep.Bind(tcpip.FullAddress{NIC: nic, Addr: tcpip.Address{}, Port: dhcpServerPort}); err != nil {
		ep.Close()
		return nil, errors.New(err.String())
	}
	return gonet.NewUDPConn(&wq, ep), nil
}

// dhcpReply builds the answer to a DISCOVER (an OFFER) or a REQUEST (an ACK);
// nil for any other message.
func dhcpReply(configuration *types.Configuration, ipPool *tap.IPPool, m *dhcpv4.DHCPv4) *dhcpv4.DHCPv4 {
	reply, err := dhcpv4.NewReplyFromRequest(m)
	if err != nil {
		log.Errorf("dhcp: cannot build reply from request: %v", err)
		return nil
	}
	ip, err := ipPool.GetOrAssign(m.ClientHWAddr.String())
	if err != nil {
		log.Errorf("dhcp: cannot assign ip: %v", err)
		return nil
	}
	_, parsedSubnet, err := net.ParseCIDR(configuration.Subnet)
	if err != nil {
		log.Errorf("dhcp: invalid subnet %v", err)
		return nil
	}
	reply.YourIPAddr = net.IP(ip.AsSlice())
	reply.UpdateOption(dhcpv4.OptServerIdentifier(net.ParseIP(configuration.GatewayIP)))
	reply.UpdateOption(dhcpv4.OptIPAddressLeaseTime(time.Hour))
	reply.UpdateOption(dhcpv4.Option{Code: dhcpv4.OptionSubnetMask, Value: dhcpv4.IP(parsedSubnet.Mask)})
	reply.UpdateOption(dhcpv4.Option{Code: dhcpv4.OptionRouter, Value: dhcpv4.IP(net.ParseIP(configuration.GatewayIP))})
	reply.UpdateOption(dhcpv4.Option{Code: dhcpv4.OptionDomainNameServer, Value: dhcpv4.IPs([]net.IP{net.ParseIP(configuration.GatewayIP)})})
	if mtu := configuration.MTU; mtu < 0 || mtu > math.MaxUint16 {
		log.Errorf("dhcp: invalid MTU %d", mtu)
	} else {
		reply.UpdateOption(dhcpv4.Option{Code: dhcpv4.OptionInterfaceMTU, Value: dhcpv4.Uint16(mtu)})
	}
	if len(configuration.DNSSearchDomains) > 0 {
		reply.UpdateOption(dhcpv4.Option{Code: dhcpv4.OptionDNSDomainSearchList, Value: &rfc1035label.Labels{
			Labels: configuration.DNSSearchDomains,
		}})
	}
	switch mt := m.MessageType(); mt {
	case dhcpv4.MessageTypeDiscover:
		reply.UpdateOption(dhcpv4.OptMessageType(dhcpv4.MessageTypeOffer))
	case dhcpv4.MessageTypeRequest:
		reply.UpdateOption(dhcpv4.OptMessageType(dhcpv4.MessageTypeAck))
	case dhcpv4.MessageTypeRelease:
		log.Debugf("dhcp: unhandled message type: %v", mt)
		return nil
	default:
		log.Errorf("dhcp: unhandled message type: %v", mt)
		return nil
	}
	return reply
}

// newDHCPServer serves the gateway's addresses on its NIC.
func newDHCPServer(configuration *types.Configuration, s *stack.Stack, ipPool *tap.IPPool) (*server4.Server, error) {
	ln, err := dhcpDial(s, tcpip.NICID(1))
	if err != nil {
		return nil, err
	}
	handler := func(conn net.PacketConn, peer net.Addr, m *dhcpv4.DHCPv4) {
		reply := dhcpReply(configuration, ipPool, m)
		if reply == nil {
			return
		}
		if _, err := conn.WriteTo(reply.ToBytes(), peer); err != nil {
			log.Errorf("dhcp: cannot reply to client: %v", err)
		}
	}
	return server4.NewServer("", nil, handler, server4.WithConn(ln))
}
