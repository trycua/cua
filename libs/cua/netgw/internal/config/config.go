// Package config reads the gateway's configuration file (JSON, written by
// cua: crates/cua-vmm, netgw.rs).
package config

import (
	"encoding/json"
	"fmt"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"strings"
)

// Config is one gateway: one virtual network for one VM.
type Config struct {
	// Subnet of the virtual network ("172.16.0.0/29").
	Subnet string `json:"subnet"`
	// GatewayIP is the gateway's address in Subnet; it also answers DNS
	// (and DHCP when on).
	GatewayIP string `json:"gateway_ip"`
	// GatewayMAC is the gateway's MAC address.
	GatewayMAC string `json:"gateway_mac"`
	// HostIP, when set, is an address in Subnet that stands for the host:
	// a connection to it goes to the host's loopback, but only when an
	// exact rule allows it (the localhost preset's ports).
	HostIP string `json:"host_ip,omitempty"`
	// MTU of the virtual link (default 1500).
	MTU int `json:"mtu,omitempty"`
	// DHCP hands out addresses in Subnet (guests that ask, such as
	// Windows and macOS VMs).
	DHCP bool `json:"dhcp,omitempty"`
	// StaticLeases are DHCP leases: address -> MAC address.
	StaticLeases map[string]string `json:"static_leases,omitempty"`
	// SearchDomains are handed out with DHCP.
	SearchDomains []string `json:"search_domains,omitempty"`
	// Listen is where the VM's network device connects.
	Listen Listen `json:"listen"`
	// Rules is the rules file (policy package), reloaded when it changes.
	Rules string `json:"rules"`
	// Events is a file to append blocked-connection events to (JSON lines).
	Events string `json:"events,omitempty"`
	// PIDFile gets the gateway's process id.
	PIDFile string `json:"pid_file,omitempty"`
	// LogLevel: "debug", "info", "warning" (default) or "error".
	LogLevel string `json:"log_level,omitempty"`
	// DNSUpstream overrides the host's resolvers ("ip:port").
	DNSUpstream []string `json:"dns_upstream,omitempty"`
	// TCPConnectTimeoutSecs bounds an outbound dial (default 30).
	TCPConnectTimeoutSecs int `json:"tcp_connect_timeout_secs,omitempty"`
}

// Listen names the endpoints a VM's network device connects to; at least
// one is set.
type Listen struct {
	// Vfkit: unixgram://<path>, Ethernet frames as datagrams after a
	// "VFKT" greeting (Virtualization.framework's file-handle device).
	Vfkit string `json:"vfkit,omitempty"`
	// Qemu: unix://<path> or tcp://127.0.0.1:<port>, frames with a 32-bit
	// big-endian length (QEMU's -netdev stream).
	Qemu string `json:"qemu,omitempty"`
}

// Load reads path; relative file names in it are relative to its folder.
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
	abs := func(p *string) {
		if *p != "" && !filepath.IsAbs(*p) {
			*p = filepath.Join(dir, *p)
		}
	}
	abs(&c.Rules)
	abs(&c.Events)
	abs(&c.PIDFile)
	if err := c.Check(); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	return &c, nil
}

// Check validates the configuration and fills defaults.
func (c *Config) Check() error {
	sub, err := netip.ParsePrefix(c.Subnet)
	if err != nil || !sub.Addr().Is4() {
		return fmt.Errorf("subnet %q: an IPv4 CIDR", c.Subnet)
	}
	gw, err := netip.ParseAddr(c.GatewayIP)
	if err != nil || !sub.Contains(gw) {
		return fmt.Errorf("gateway_ip %q: an address in %s", c.GatewayIP, c.Subnet)
	}
	if _, err := net.ParseMAC(c.GatewayMAC); err != nil {
		return fmt.Errorf("gateway_mac %q: %w", c.GatewayMAC, err)
	}
	if c.HostIP != "" {
		h, err := netip.ParseAddr(c.HostIP)
		if err != nil || !sub.Contains(h) || h == gw {
			return fmt.Errorf("host_ip %q: another address in %s", c.HostIP, c.Subnet)
		}
	}
	for ip, mac := range c.StaticLeases {
		a, err := netip.ParseAddr(ip)
		if err != nil || !sub.Contains(a) {
			return fmt.Errorf("static lease %q: an address in %s", ip, c.Subnet)
		}
		if _, err := net.ParseMAC(mac); err != nil {
			return fmt.Errorf("static lease %q: %w", ip, err)
		}
	}
	if c.MTU == 0 {
		c.MTU = 1500
	}
	if c.MTU < 576 || c.MTU > 65535 {
		return fmt.Errorf("mtu %d: 576 to 65535", c.MTU)
	}
	if c.Listen.Vfkit == "" && c.Listen.Qemu == "" {
		return fmt.Errorf("listen: no endpoint (vfkit or qemu)")
	}
	if c.Rules == "" {
		return fmt.Errorf("rules: no rules file")
	}
	if c.TCPConnectTimeoutSecs <= 0 {
		c.TCPConnectTimeoutSecs = 30
	}
	return nil
}
