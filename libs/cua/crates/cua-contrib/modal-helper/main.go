// Command cua-modal-helper is how the cua SDK's Modal provider
// (libs/cua/crates/cua-contrib/src/modal.rs) talks to Modal.
//
// Modal documents no HTTP API: its clients speak an internal gRPC API whose
// proto says direct use is discouraged and carries no compatibility
// guarantee. This helper therefore goes through Modal's official Go SDK
// (github.com/modal-labs/modal-client/go, Apache-2.0), so Modal's own
// client keeps the wire protocol current.
//
// Protocol: one JSON request on stdin, one JSON response on stdout, exit 0.
// A failure is a response with "error": {"kind", "message"} and exit 2.
// Credentials come from MODAL_TOKEN_ID and MODAL_TOKEN_SECRET in the
// environment, else from the Modal CLI's profile in ~/.modal.toml (the
// request's `profile`, else the active one), read by the SDK itself; the
// helper never writes them anywhere.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strconv"
	"time"
)

type request struct {
	// create | get | list | delete | check
	Op string `json:"op"`
	// Modal App the sandboxes belong to (created when missing).
	App string `json:"app"`
	// Registry reference (pinned by digest when the SDK could read it).
	Image string `json:"image"`
	// The process to run. The image's ENTRYPOINT is cleared, so this is the
	// whole command (the image's ENTRYPOINT + CMD, or the create's command).
	Argv []string `json:"argv"`
	// vCPUs (Modal counts physical cores: two vCPUs each).
	CPUs      float64 `json:"cpus"`
	MemoryMiB int     `json:"memory_mib"`
	// Lifetime backstop (Modal's sandbox timeout).
	TimeoutSecs int `json:"timeout_secs"`
	// Guest ports to tunnel (TLS, HTTP/1.1).
	Ports []int             `json:"ports"`
	Env   map[string]string `json:"env"`
	Tags  map[string]string `json:"tags"`
	// A Modal GPU type ("T4", "A100"); empty: none.
	GPU string `json:"gpu"`
	// get / delete.
	ID string `json:"id"`
	// The ~/.modal.toml profile (empty: MODAL_TOKEN_* or the active one).
	Profile string `json:"profile"`
	// The Modal environment (empty: the profile's).
	Environment string `json:"environment"`
	// gvisor | vm (empty: Modal's default).
	Runtime string `json:"runtime"`
	// Sandbox name, unique within the App (empty: none).
	Name string `json:"name"`
	// Budget of the whole request.
	DeadlineSecs int `json:"deadline_secs"`
}

type sandbox struct {
	ID string `json:"id"`
	// The sandbox name, when it has one.
	Name string `json:"name,omitempty"`
	// running | stopped
	Status  string            `json:"status"`
	Tags    map[string]string `json:"tags,omitempty"`
	Tunnels map[string]string `json:"tunnels,omitempty"`
}

type account struct {
	Profile     string `json:"profile"`
	Environment string `json:"environment"`
	// Sandboxes with the tags the check asked for (read-only proof of access).
	Sandboxes int `json:"sandboxes"`
}

type apiError struct {
	// not_found | auth | invalid | timeout | other
	Kind    string `json:"kind"`
	Message string `json:"message"`
}

type response struct {
	// check: the profile and environment the credentials reached.
	Account   *account  `json:"account,omitempty"`
	Sandbox   *sandbox  `json:"sandbox,omitempty"`
	Sandboxes []sandbox `json:"sandboxes,omitempty"`
	Error     *apiError `json:"error,omitempty"`
}

// api is what the helper needs from Modal; modalAPI implements it with the
// official SDK, tests with a fake.
type api interface {
	create(ctx context.Context, r request) (*sandbox, error)
	get(ctx context.Context, id string, ports []int) (*sandbox, error)
	// list: the App's sandboxes (every one in the environment when app is empty).
	list(ctx context.Context, app string, tags map[string]string) ([]sandbox, error)
	delete(ctx context.Context, id string) error
	// check proves the credentials reach the environment (read-only).
	check(ctx context.Context, app string, tags map[string]string) (*account, error)
}

// maxRequest bounds what the helper reads from stdin.
const maxRequest = 1 << 20

func serve(in io.Reader, out io.Writer, connect func(r request) (api, error)) int {
	var r request
	if err := json.NewDecoder(io.LimitReader(in, maxRequest)).Decode(&r); err != nil {
		return reply(out, response{Error: &apiError{"invalid", "bad request: " + err.Error()}})
	}
	deadline := time.Duration(r.DeadlineSecs) * time.Second
	if deadline <= 0 {
		deadline = 10 * time.Minute
	}
	ctx, cancel := context.WithTimeout(context.Background(), deadline)
	defer cancel()
	a, err := connect(r)
	if err != nil {
		return reply(out, response{Error: classify(err)})
	}
	var resp response
	switch r.Op {
	case "create":
		if r.Image == "" || len(r.Argv) == 0 {
			return reply(out, response{Error: &apiError{"invalid", "create needs image and argv"}})
		}
		if r.Runtime != "" && r.Runtime != "gvisor" && r.Runtime != "vm" {
			return reply(out, response{Error: &apiError{"invalid", "runtime must be gvisor or vm, not " + strconv.Quote(r.Runtime)}})
		}
		resp.Sandbox, err = a.create(ctx, r)
	case "get":
		resp.Sandbox, err = a.get(ctx, r.ID, r.Ports)
	case "list":
		resp.Sandboxes, err = a.list(ctx, r.App, r.Tags)
	case "delete":
		err = a.delete(ctx, r.ID)
	case "check":
		resp.Account, err = a.check(ctx, r.App, r.Tags)
	default:
		return reply(out, response{Error: &apiError{"invalid", "unknown op " + strconv.Quote(r.Op)}})
	}
	if err != nil {
		return reply(out, response{Error: classify(err)})
	}
	return reply(out, resp)
}

func reply(out io.Writer, r response) int {
	_ = json.NewEncoder(out).Encode(r)
	if r.Error != nil {
		return 2
	}
	return 0
}

// kinded lets the SDK adapter name an error kind.
type kinded struct {
	kind string
	err  error
}

func (k kinded) Error() string { return k.err.Error() }
func (k kinded) Unwrap() error { return k.err }

func classify(err error) *apiError {
	var k kinded
	if errors.As(err, &k) {
		return &apiError{k.kind, k.err.Error()}
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return &apiError{"timeout", err.Error()}
	}
	return &apiError{"other", fmt.Sprint(err)}
}

func main() {
	os.Exit(serve(os.Stdin, os.Stdout, connectModal))
}
