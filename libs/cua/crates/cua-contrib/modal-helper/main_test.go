package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
	"time"

	modal "github.com/modal-labs/modal-client/go"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

type fake struct {
	created []request
	live    map[string]*sandbox
}

func (f *fake) create(_ context.Context, r request) (*sandbox, error) {
	f.created = append(f.created, r)
	s := &sandbox{ID: "sb-1", Status: "running", Tags: r.Tags, Tunnels: map[string]string{"3211": "https://abc.w.modal.host"}}
	f.live[s.ID] = s
	return s, nil
}

func (f *fake) get(_ context.Context, id string, _ []int) (*sandbox, error) {
	if s, ok := f.live[id]; ok {
		return s, nil
	}
	return nil, kinded{"not_found", errors.New("sandbox not found")}
}

func (f *fake) list(context.Context, string, map[string]string) ([]sandbox, error) {
	var out []sandbox
	for _, s := range f.live {
		out = append(out, *s)
	}
	return out, nil
}

func (f *fake) check(context.Context, string, map[string]string) (*account, error) {
	return &account{Environment: "env", Sandboxes: len(f.live)}, nil
}

func (f *fake) delete(_ context.Context, id string) error {
	delete(f.live, id)
	return nil
}

func run(t *testing.T, f *fake, req string) (response, int) {
	t.Helper()
	var out bytes.Buffer
	code := serve(strings.NewReader(req), &out, func(request) (api, error) { return f, nil })
	var r response
	if err := json.Unmarshal(out.Bytes(), &r); err != nil {
		t.Fatalf("bad response %q: %v", out.String(), err)
	}
	return r, code
}

func TestProtocolRoundTrip(t *testing.T) {
	f := &fake{live: map[string]*sandbox{}}
	r, code := run(t, f, `{"op":"create","app":"cua","image":"ghcr.io/trycua/linux@sha256:ab","argv":["/entrypoint.sh"],"cpus":2,"memory_mib":4096,"ports":[3211],"tags":{"cua.name":"x"}}`)
	if code != 0 || r.Sandbox == nil || r.Sandbox.Tunnels["3211"] == "" {
		t.Fatalf("create: %d %+v", code, r)
	}
	if got := f.created[0]; got.App != "cua" || got.Ports[0] != 3211 || got.Tags["cua.name"] != "x" {
		t.Fatalf("request not passed through: %+v", got)
	}
	r, _ = run(t, f, `{"op":"list","tags":{"cua.managed":"true"}}`)
	if len(r.Sandboxes) != 1 {
		t.Fatalf("list: %+v", r)
	}
	if _, code = run(t, f, `{"op":"delete","id":"sb-1"}`); code != 0 {
		t.Fatalf("delete failed")
	}
	r, code = run(t, f, `{"op":"get","id":"sb-1"}`)
	if code != 2 || r.Error == nil || r.Error.Kind != "not_found" {
		t.Fatalf("get after delete: %d %+v", code, r)
	}
}

func TestRuntimeNameEnvironmentAndCheck(t *testing.T) {
	f := &fake{live: map[string]*sandbox{}}
	var seen request
	code := serve(strings.NewReader(`{"op":"create","image":"i","argv":["a"],"runtime":"vm","name":"box","profile":"p","environment":"e"}`),
		&bytes.Buffer{}, func(r request) (api, error) { seen = r; return f, nil })
	if code != 0 || f.created[0].Runtime != "vm" || f.created[0].Name != "box" || seen.Profile != "p" || seen.Environment != "e" {
		t.Fatalf("create: %d %+v %+v", code, f.created, seen)
	}
	p := createParams(request{Argv: []string{"a"}, Runtime: "gvisor", Name: "n"})
	if p.Runtime != modal.SandboxRuntimeGVisor || p.Name != "n" {
		t.Fatalf("params: %+v", p)
	}
	r, code := run(t, f, `{"op":"create","image":"i","argv":["a"],"runtime":"firecracker"}`)
	if code != 2 || r.Error == nil || r.Error.Kind != "invalid" {
		t.Fatalf("bad runtime: %d %+v", code, r)
	}
	r, code = run(t, f, `{"op":"check","environment":"e"}`)
	if code != 0 || r.Account == nil || r.Account.Sandboxes != 1 {
		t.Fatalf("check: %d %+v", code, r)
	}
}

func TestRefusals(t *testing.T) {
	f := &fake{live: map[string]*sandbox{}}
	for _, req := range []string{`{"op":"create","image":"x"}`, `{"op":"nope"}`, `not json`} {
		r, code := run(t, f, req)
		if code != 2 || r.Error == nil || r.Error.Kind != "invalid" {
			t.Fatalf("%s: %d %+v", req, code, r)
		}
	}
	var out bytes.Buffer
	code := serve(strings.NewReader(`{"op":"list"}`), &out, func(request) (api, error) {
		return nil, kinded{"auth", errors.New("MODAL_TOKEN_ID and MODAL_TOKEN_SECRET must both be set")}
	})
	if code != 2 || !strings.Contains(out.String(), `"kind":"auth"`) {
		t.Fatalf("auth: %d %s", code, out.String())
	}
}

func TestCreateParams(t *testing.T) {
	p := createParams(request{Argv: []string{"a"}, CPUs: 4, MemoryMiB: 8192, Ports: []int{3211, 6080}, TimeoutSecs: 600})
	if p.CPU != 2 || p.MemoryMiB != 8192 || p.Timeout != 10*time.Minute {
		t.Fatalf("resources: %+v", p)
	}
	if len(p.EncryptedPorts) != 2 || p.Command[0] != "a" {
		t.Fatalf("ports/command: %+v", p)
	}
	if createParams(request{GPU: "A100"}).GPU != "A100" || createParams(request{}).GPU != "" {
		t.Fatal("the GPU type reaches the sandbox, and only when asked for")
	}
	if createParams(request{CPUs: 0.1}).CPU != 0.125 || createParams(request{}).Timeout != time.Hour {
		t.Fatalf("defaults")
	}
}

func TestErrorKinds(t *testing.T) {
	cases := map[string]error{
		"not_found": modal.NotFoundError{Exception: "gone"},
		"invalid":   modal.InvalidError{Exception: "bad"},
		"auth":      status.Error(codes.Unauthenticated, "token"),
		"timeout":   context.DeadlineExceeded,
		"other":     errors.New("boom"),
	}
	for want, err := range cases {
		if got := wrap(err).kind; got != want {
			t.Fatalf("%v: got %s want %s", err, got, want)
		}
	}
}
