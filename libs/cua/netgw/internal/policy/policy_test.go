package policy

import (
	"net/netip"
	"os"
	"path/filepath"
	"testing"
)

func mustParse(t *testing.T, s string) *File {
	t.Helper()
	f, err := Parse([]byte(s))
	if err != nil {
		t.Fatal(err)
	}
	return f
}

var testNet = &Network{
	Subnet:    netip.MustParsePrefix("172.16.0.0/29"),
	HostIP:    netip.MustParseAddr("172.16.0.6"),
	HostAddrs: func() []netip.Addr { return []netip.Addr{netip.MustParseAddr("192.168.1.20")} },
}

func a(s string) netip.Addr { return netip.MustParseAddr(s) }

func TestLockedDeniesHoldWhateverTheRulesSay(t *testing.T) {
	f := mustParse(t, `{"version":1,"default_accept":true,"rules":[{"net":"0.0.0.0/0","accept":true},{"net":"::/0","accept":true}],"block_host_addrs":false}`)
	for dst, reason := range map[string]string{
		"127.0.0.1":        ReasonLoopback,
		"127.9.9.9":        ReasonLoopback,
		"0.0.0.0":          ReasonLoopback,
		"::1":              ReasonLoopback,
		"::ffff:127.0.0.1": ReasonLoopback,
		"64:ff9b::7f00:1":  ReasonLoopback,
		"169.254.169.254":  ReasonMetadata,
		"169.254.170.2":    ReasonMetadata,
		"fd00:ec2::254":    ReasonMetadata,
		"168.63.129.16":    ReasonMetadata,
		"100.100.100.200":  ReasonMetadata,
		"224.0.0.251":      ReasonMulticast,
		"255.255.255.255":  ReasonMulticast,
		"172.16.0.6":       ReasonHostLoopback,
		"172.16.0.1":       ReasonGateway,
		"172.16.0.3":       ReasonGateway,
	} {
		v := f.Decide(testNet, TCP, a(dst), 80)
		if v.Accept || v.Reason != reason {
			t.Errorf("%s: %+v, want refused for %s", dst, v, reason)
		}
	}
	// block_host_addrs false (unprotected): the host's LAN address opens.
	if v := f.Decide(testNet, TCP, a("192.168.1.20"), 22); !v.Accept {
		t.Errorf("host address with block_host_addrs=false: %+v", v)
	}
}

func TestTheHostOpensOnlyThroughAnExactRule(t *testing.T) {
	f := mustParse(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"172.16.0.6/32","ports":[8080],"proto":"tcp","accept":false,"reason":"platform:daemon"},
		{"net":"172.16.0.6","ports":[5432,8080],"accept":true},
		{"net":"192.168.1.20/32","ports":[3000],"accept":true},
		{"net":"0.0.0.0/0","accept":true}]}`)
	if v := f.Decide(testNet, TCP, a("172.16.0.6"), 5432); !v.Accept || !v.ToHostLoopback {
		t.Errorf("listed localhost port: %+v", v)
	}
	if v := f.Decide(testNet, TCP, a("172.16.0.6"), 8080); v.Accept || v.Reason != "platform:daemon" {
		t.Errorf("an earlier exact deny wins: %+v", v)
	}
	if v := f.Decide(testNet, TCP, a("172.16.0.6"), 22); v.Accept || v.Reason != ReasonHostLoopback {
		t.Errorf("unlisted port (0.0.0.0/0 does not count): %+v", v)
	}
	if v := f.Decide(testNet, TCP, a("192.168.1.20"), 3000); !v.Accept || v.ToHostLoopback {
		t.Errorf("exact host address rule: %+v", v)
	}
	if v := f.Decide(testNet, TCP, a("192.168.1.20"), 22); v.Accept || v.Reason != ReasonHostAddress {
		t.Errorf("host address, other port: %+v", v)
	}
}

func TestFirstMatchWinsWithPortsAndProtocols(t *testing.T) {
	// The internet preset's shape: resolver, user allow, defaults, accept.
	f := mustParse(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"10.0.0.53/32","ports":[53],"accept":true},
		{"net":"10.1.2.0/24","ports":[443],"proto":"tcp","accept":true},
		{"net":"203.0.113.0/24","accept":false},
		{"net":"10.0.0.0/8","accept":false,"reason":"default:lan"},
		{"net":"192.168.0.0/16","accept":false,"reason":"default:lan"}]}`)
	cases := []struct {
		proto  Proto
		dst    string
		port   uint16
		accept bool
		reason string
	}{
		{UDP, "10.0.0.53", 53, true, ""},
		{TCP, "10.0.0.53", 53, true, ""},
		{UDP, "10.0.0.53", 54, false, "default:lan"},
		{TCP, "10.1.2.3", 443, true, ""},
		{UDP, "10.1.2.3", 443, false, "default:lan"},
		{TCP, "10.1.2.3", 80, false, "default:lan"},
		{TCP, "203.0.113.9", 443, false, ReasonUserDeny},
		{TCP, "192.168.4.4", 22, false, "default:lan"},
		{TCP, "93.184.216.34", 443, true, ""},
		{TCP, "::ffff:10.9.9.9", 22, false, "default:lan"},
	}
	for _, c := range cases {
		v := f.Decide(testNet, c.proto, a(c.dst), c.port)
		if v.Accept != c.accept || v.Reason != c.reason {
			t.Errorf("%s %s:%d: %+v, want accept=%v reason=%q", c.proto, c.dst, c.port, v, c.accept, c.reason)
		}
	}
}

func TestDefaultDrop(t *testing.T) {
	f := mustParse(t, `{"version":1,"default_accept":false,"rules":[{"net":"1.1.1.1","accept":true}]}`)
	if v := f.Decide(testNet, TCP, a("8.8.8.8"), 53); v.Accept || v.Reason != ReasonDefaultNone {
		t.Errorf("%+v", v)
	}
	if v := f.Decide(testNet, UDP, a("1.1.1.1"), 53); !v.Accept {
		t.Errorf("%+v", v)
	}
}

func TestStripAnswer(t *testing.T) {
	f := mustParse(t, `{"version":1,"default_accept":true,"rules":[
		{"net":"10.1.2.3/32","ports":[443],"accept":true},
		{"net":"10.9.0.0/16","accept":true},
		{"net":"10.0.0.0/8","accept":false,"reason":"default:lan"}]}`)
	for ip, want := range map[string]string{
		"127.0.0.1":       ReasonLoopback,
		"10.0.0.1":        "default:lan",
		"10.1.2.3":        "default:lan", // a port-limited allow does not keep an answer
		"169.254.169.254": ReasonMetadata,
		"192.168.1.20":    ReasonHostAddress,
		"172.16.0.6":      ReasonHostLoopback,
		"10.9.1.1":        "",
		"93.184.216.34":   "",
	} {
		strip, reason := f.StripAnswer(testNet, a(ip))
		if strip != (want != "") || reason != want {
			t.Errorf("%s: strip=%v %q, want %q", ip, strip, reason, want)
		}
	}
}

func TestParseRefusesWhatItCannotRead(t *testing.T) {
	for _, s := range []string{
		`{"version":2,"rules":[]}`,
		`{"version":1,"rules":[{"net":"example.com","accept":true}]}`,
		`{"version":1,"rules":[{"net":"10.0.0.0/33","accept":true}]}`,
		`{"version":1,"rules":[{"net":"10.0.0.0/8","proto":"icmp","accept":true}]}`,
		`{"version":1,"rules":[],"surprise":true}`,
		`not json`,
	} {
		if _, err := Parse([]byte(s)); err == nil {
			t.Errorf("accepted %s", s)
		}
	}
}

func TestStoreKeepsTheOldRulesWhenTheNewFileIsBad(t *testing.T) {
	path := filepath.Join(t.TempDir(), "rules.json")
	write := func(s string) {
		tmp := path + ".tmp"
		if err := os.WriteFile(tmp, []byte(s), 0o600); err != nil {
			t.Fatal(err)
		}
		if err := os.Rename(tmp, path); err != nil {
			t.Fatal(err)
		}
	}
	write(`{"version":1,"default_accept":false,"rules":[]}`)
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	changes := 0
	s.OnChange(func(*File) { changes++ })
	write(`{"version":1,"default_accept":true,"rules":[]}`)
	if changed, err := s.Reload(); !changed || err != nil {
		t.Fatalf("reload: %v %v", changed, err)
	}
	if !s.Current().DefaultAccept || changes != 1 {
		t.Fatalf("not reloaded")
	}
	write(`{"version":1,"default_accept":`)
	if _, err := s.Reload(); err == nil {
		t.Fatal("a broken file reloaded")
	}
	if !s.Current().DefaultAccept || changes != 1 {
		t.Fatal("a broken file replaced the rules")
	}
	if changed, _ := s.Reload(); changed {
		t.Fatal("the same broken file reloaded twice")
	}
}
