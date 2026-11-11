package relay

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLoad(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "tree-relay.json")
	body := `{"peers":"tree.json","ingress":"/t/198.19.0.1.sock","guest_ip":"10.0.2.15","listen":"0.0.0.0:41919","events":"network-events.jsonl","qmp":"unix:/t/q.sock"}`
	if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	c, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if c.Peers != filepath.Join(dir, "tree.json") || c.Events != filepath.Join(dir, "network-events.jsonl") {
		t.Fatalf("relative files: %+v", c)
	}
	if c.QMP != "unix:/t/q.sock" {
		t.Fatalf("qmp: %q", c.QMP)
	}
	if err := os.WriteFile(path, []byte(`{"peers":"tree.json","guest_ip":"10.0.2.15"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(path); err == nil {
		t.Fatal("ingress and listen are required")
	}
}
