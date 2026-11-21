package gateway

import (
	"net"
	"testing"

	"github.com/containers/gvisor-tap-vsock/pkg/tap"
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"github.com/insomniacslk/dhcp/dhcpv4"
)

func discover(t *testing.T) *dhcpv4.DHCPv4 {
	mac, _ := net.ParseMAC("52:54:00:12:34:56")
	m, err := dhcpv4.NewDiscovery(mac)
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func pool(t *testing.T) *tap.IPPool {
	p, err := tap.NewIPPool("10.0.2.0/24")
	if err != nil {
		t.Fatal(err)
	}
	if err := p.Reserve("10.0.2.2", "5a:94:ef:e4:0c:dd"); err != nil {
		t.Fatal(err)
	}
	return p
}

// Windows refuses an offer with an empty DNS search list (option 119): it logs
// error 0xD and keeps a 169.254 address. Without search domains the option is
// not sent at all.
func TestOfferHasNoEmptySearchList(t *testing.T) {
	cfg := &types.Configuration{MTU: 1500, Subnet: "10.0.2.0/24", GatewayIP: "10.0.2.2"}
	reply := dhcpReply(cfg, pool(t), discover(t))
	if reply == nil {
		t.Fatal("no offer")
	}
	if reply.MessageType() != dhcpv4.MessageTypeOffer {
		t.Fatalf("message type %v", reply.MessageType())
	}
	if _, ok := reply.Options[uint8(dhcpv4.OptionDNSDomainSearchList)]; ok {
		t.Fatal("an empty DNS search list was sent")
	}
	if !reply.YourIPAddr.Equal(net.ParseIP("10.0.2.3")) && reply.YourIPAddr.IsUnspecified() {
		t.Fatalf("no address offered: %v", reply.YourIPAddr)
	}
}

func TestOfferKeepsRealSearchDomains(t *testing.T) {
	cfg := &types.Configuration{MTU: 1500, Subnet: "10.0.2.0/24", GatewayIP: "10.0.2.2", DNSSearchDomains: []string{"corp.example"}}
	reply := dhcpReply(cfg, pool(t), discover(t))
	if reply == nil {
		t.Fatal("no offer")
	}
	if _, ok := reply.Options[uint8(dhcpv4.OptionDNSDomainSearchList)]; !ok {
		t.Fatal("the search domains were dropped")
	}
}
