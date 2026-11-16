//go:build linux

package relay

import (
	"errors"
	"net"
	"net/netip"
	"syscall"
	"unsafe"
)

// soOriginalDst is SO_ORIGINAL_DST (linux/netfilter_ipv4.h).
const soOriginalDst = 80

// originalDst is where the guest sent a connection the namespace
// redirected to the relay (conntrack's original destination).
func originalDst(c net.Conn) (netip.AddrPort, error) {
	tc, ok := c.(*net.TCPConn)
	if !ok {
		return netip.AddrPort{}, errors.New("not a TCP connection")
	}
	raw, err := tc.SyscallConn()
	if err != nil {
		return netip.AddrPort{}, err
	}
	var sa syscall.RawSockaddrInet4
	var sockErr error
	err = raw.Control(func(fd uintptr) {
		size := uint32(unsafe.Sizeof(sa))
		_, _, errno := syscall.Syscall6(syscall.SYS_GETSOCKOPT, fd,
			syscall.IPPROTO_IP, soOriginalDst,
			uintptr(unsafe.Pointer(&sa)), uintptr(unsafe.Pointer(&size)), 0)
		if errno != 0 {
			sockErr = errno
		}
	})
	if err != nil {
		return netip.AddrPort{}, err
	}
	if sockErr != nil {
		return netip.AddrPort{}, sockErr
	}
	// The port is in network byte order in memory, whatever the host's.
	pb := (*[2]byte)(unsafe.Pointer(&sa.Port))
	port := uint16(pb[0])<<8 | uint16(pb[1])
	return netip.AddrPortFrom(netip.AddrFrom4(sa.Addr), port), nil
}
