package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const good = `{"subnet":"172.16.0.0/29","gateway_ip":"172.16.0.1","gateway_mac":"5a:94:ef:e4:0c:dd",
 "host_ip":"172.16.0.6","listen":{"vfkit":"unixgram:///r/gv.sock"},
 "rules":"netgw-rules.json","events":"network-events.jsonl","pid_file":"netgw.pid"}`

func load(t *testing.T, s string) (*Config, error) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "netgw.json")
	if err := os.WriteFile(path, []byte(s), 0o600); err != nil {
		t.Fatal(err)
	}
	return Load(path)
}

func TestFilesAreBesideTheConfig(t *testing.T) {
	c, err := load(t, good)
	if err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{c.Rules, c.Events, c.PIDFile} {
		if !filepath.IsAbs(p) {
			t.Errorf("%s is relative", p)
		}
	}
	if c.MTU != 1500 || c.TCPConnectTimeoutSecs != 30 {
		t.Errorf("defaults: %+v", c)
	}
}

func TestBadConfigsAreRefused(t *testing.T) {
	for want, s := range map[string]string{
		"subnet":     strings.Replace(good, "172.16.0.0/29", "fd00::/64", 1),
		"gateway_ip": strings.Replace(good, `"gateway_ip":"172.16.0.1"`, `"gateway_ip":"10.0.0.1"`, 1),
		"host_ip":    strings.Replace(good, `"host_ip":"172.16.0.6"`, `"host_ip":"172.16.0.1"`, 1),
		"listen":     strings.Replace(good, `{"vfkit":"unixgram:///r/gv.sock"}`, `{}`, 1),
		"rules":      strings.Replace(good, `"rules":"netgw-rules.json",`, ``, 1),
		"unknown":    strings.Replace(good, `"subnet"`, `"surprise":1,"subnet"`, 1),
	} {
		if _, err := load(t, s); err == nil || (want != "unknown" && !strings.Contains(err.Error(), want)) {
			t.Errorf("%s: %v", want, err)
		}
	}
}
