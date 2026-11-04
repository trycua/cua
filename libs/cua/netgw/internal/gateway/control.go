package gateway

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"net/netip"
	"strings"
	"time"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
)

// ServeControl answers on ln until it closes, one request line per
// connection:
//
//   - "probe <guest address>:<port>": "ok" when the gateway can open a TCP
//     connection to that port in the guest (readiness of a published port),
//     "no <why>" otherwise. Only addresses in the virtual network.
//   - "lease <mac>": "ok <address>" when the gateway's DHCP has leased an
//     address to that MAC address (how a host finds its guest's address),
//     "no <why>" otherwise.
func (g *Gateway) ServeControl(ln net.Listener) {
	for {
		c, err := ln.Accept()
		if err != nil {
			return
		}
		go func() {
			defer c.Close()
			_ = c.SetDeadline(time.Now().Add(10 * time.Second))
			line, err := bufio.NewReader(c).ReadString('\n')
			if err != nil {
				return
			}
			fmt.Fprintln(c, g.probe(strings.TrimSpace(line)))
		}()
	}
}

func (g *Gateway) probe(line string) string {
	if mac, ok := strings.CutPrefix(line, "lease "); ok {
		return g.lease(strings.TrimSpace(mac))
	}
	arg, ok := strings.CutPrefix(line, "probe ")
	if !ok {
		return "no unknown request"
	}
	ap, err := netip.ParseAddrPort(strings.TrimSpace(arg))
	if err != nil || !ap.Addr().Is4() || !g.network.Subnet.Contains(ap.Addr()) {
		return "no not an address and port in the virtual network"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	conn, err := gonet.DialContextTCP(ctx, g.stack, tcpip.FullAddress{
		NIC:  1,
		Addr: tcpip.AddrFrom4(ap.Addr().As4()),
		Port: ap.Port(),
	}, ipv4.ProtocolNumber)
	if err != nil {
		return "no " + err.Error()
	}
	conn.Close()
	return "ok"
}

func (g *Gateway) lease(mac string) string {
	hw, err := net.ParseMAC(mac)
	if err != nil {
		return "no not a MAC address"
	}
	// The address the guest uses: one the gateway's DHCP leased to it
	// that the guest has also answered for (the gateway's neighbor table
	// has it), so a lease nobody took up yet is not reported.
	leased := map[string]bool{}
	for ip, m := range g.pool.Leases() {
		if l, err := net.ParseMAC(m); err == nil && l.String() == hw.String() {
			leased[ip] = true
		}
	}
	neighbors, tcpErr := g.stack.Neighbors(1, ipv4.ProtocolNumber)
	if tcpErr != nil {
		return "no " + tcpErr.String()
	}
	for _, n := range neighbors {
		ip := toAddr(n.Addr).String()
		if net.HardwareAddr(n.LinkAddr).String() == hw.String() && leased[ip] {
			return "ok " + ip
		}
	}
	return "no lease in use"
}
