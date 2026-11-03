//go:build !linux

package relay

import (
	"errors"
	"net"
	"net/netip"
)

// originalDst: the tree relay runs in a Linux VM's network namespace only
// (elsewhere the network gateway carries the tree itself).
func originalDst(net.Conn) (netip.AddrPort, error) {
	return netip.AddrPort{}, errors.New("the tree relay runs on Linux only")
}
