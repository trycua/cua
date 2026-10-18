package main

import (
	"context"
	"errors"
	"math"
	"os"
	"strconv"
	"time"

	modal "github.com/modal-labs/modal-client/go"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// modalAPI implements api with Modal's official Go SDK.
type modalAPI struct {
	c           *modal.Client
	profile     string
	environment string
}

// connectModal signs in with MODAL_TOKEN_ID + MODAL_TOKEN_SECRET when both
// are set, else with the request's ~/.modal.toml profile (the active one
// when empty), which the SDK reads itself.
func connectModal(r request) (api, error) {
	id, secret := os.Getenv("MODAL_TOKEN_ID"), os.Getenv("MODAL_TOKEN_SECRET")
	params := &modal.ClientParams{Environment: r.Environment}
	switch {
	case id != "" && secret != "":
		params.TokenID, params.TokenSecret = id, secret
	case id != "" || secret != "":
		return nil, kinded{"auth", errors.New("MODAL_TOKEN_ID and MODAL_TOKEN_SECRET must both be set")}
	default:
		if r.Profile != "" {
			// The SDK picks the profile from MODAL_PROFILE.
			_ = os.Setenv("MODAL_PROFILE", r.Profile)
		}
	}
	c, err := modal.NewClientWithOptions(params)
	if err != nil {
		return nil, kinded{"auth", err}
	}
	return &modalAPI{c: c, profile: r.Profile, environment: r.Environment}, nil
}

// createParams maps a request onto the SDK's sandbox options.
func createParams(r request) *modal.SandboxCreateParams {
	timeout := time.Duration(r.TimeoutSecs) * time.Second
	if timeout <= 0 {
		timeout = time.Hour
	}
	return &modal.SandboxCreateParams{
		// Physical cores, two vCPUs each; Modal's smallest request is 0.125.
		CPU:            math.Max(r.CPUs/2, 0.125),
		MemoryMiB:      r.MemoryMiB,
		Timeout:        timeout,
		Command:        r.Argv,
		Env:            r.Env,
		EncryptedPorts: r.Ports,
		Tags:           r.Tags,
		Name:           r.Name,
		Runtime:        modal.SandboxRuntime(r.Runtime),
		GPU:            r.GPU,
	}
}

// entrypointCleared is the layer that makes Argv the whole command: Modal
// keeps an image's ENTRYPOINT and passes the sandbox command as its args.
var entrypointCleared = []string{"ENTRYPOINT []"}

func (m *modalAPI) create(ctx context.Context, r request) (*sandbox, error) {
	app, err := m.c.Apps.FromName(ctx, r.App, &modal.AppFromNameParams{CreateIfMissing: true, Environment: m.environment})
	if err != nil {
		return nil, wrap(err)
	}
	image := m.c.Images.FromRegistry(r.Image, nil).DockerfileCommands(entrypointCleared, nil)
	sb, err := m.c.Sandboxes.Create(ctx, app, image, createParams(r))
	if err != nil {
		return nil, wrap(err)
	}
	out, err := describe(ctx, sb, r.Ports, 5*time.Minute)
	if err != nil {
		// Nothing is left behind by a create that failed half way.
		_, _ = sb.Terminate(context.Background(), nil)
		return nil, err
	}
	return out, nil
}

func (m *modalAPI) get(ctx context.Context, id string, ports []int) (*sandbox, error) {
	sb, err := m.c.Sandboxes.FromID(ctx, id, nil)
	if err != nil {
		return nil, wrap(err)
	}
	return describe(ctx, sb, ports, 30*time.Second)
}

// describe reports a sandbox's status and, while it runs, its tunnels.
func describe(ctx context.Context, sb *modal.Sandbox, ports []int, wait time.Duration) (*sandbox, error) {
	out := &sandbox{ID: sb.SandboxID, Status: "running", Tunnels: map[string]string{}}
	code, err := sb.Poll(ctx, nil)
	if err != nil {
		return nil, wrap(err)
	}
	if code != nil {
		out.Status = "stopped"
		return out, nil
	}
	if tags, err := sb.GetTags(ctx, nil); err == nil {
		out.Tags = tags
	}
	if len(ports) > 0 {
		tunnels, err := sb.Tunnels(ctx, wait, nil)
		if err != nil {
			return nil, wrap(err)
		}
		for port, t := range tunnels {
			out.Tunnels[strconv.Itoa(port)] = t.URL()
		}
	}
	return out, nil
}

func (m *modalAPI) list(ctx context.Context, appName string, tags map[string]string) ([]sandbox, error) {
	params := &modal.SandboxListParams{Tags: tags, Environment: m.environment}
	if appName != "" {
		app, err := m.c.Apps.FromName(ctx, appName, &modal.AppFromNameParams{Environment: m.environment})
		if err != nil {
			if k := wrap(err); k.kind == "not_found" {
				// No App yet: nothing was ever created there.
				return nil, nil
			}
			return nil, wrap(err)
		}
		params.AppID = app.AppID
	}
	it, err := m.c.Sandboxes.List(ctx, params)
	if err != nil {
		return nil, wrap(err)
	}
	var out []sandbox
	for sb, err := range it {
		if err != nil {
			return nil, wrap(err)
		}
		s := sandbox{ID: sb.SandboxID, Status: "running"}
		if t, err := sb.GetTags(ctx, nil); err == nil {
			s.Tags = t
		}
		out = append(out, s)
		if len(out) >= 1000 {
			break
		}
	}
	return out, nil
}

func (m *modalAPI) check(ctx context.Context, appName string, tags map[string]string) (*account, error) {
	sbs, err := m.list(ctx, appName, tags)
	if err != nil {
		return nil, err
	}
	return &account{Profile: m.profile, Environment: m.environment, Sandboxes: len(sbs)}, nil
}

func (m *modalAPI) delete(ctx context.Context, id string) error {
	sb, err := m.c.Sandboxes.FromID(ctx, id, nil)
	if err == nil {
		_, err = sb.Terminate(ctx, nil)
	}
	if err != nil {
		if k := wrap(err); k.kind == "not_found" {
			return nil
		}
		return wrap(err)
	}
	return nil
}

// wrap names the kind of an SDK error.
func wrap(err error) kinded {
	var nf modal.NotFoundError
	var inv modal.InvalidError
	switch {
	case errors.As(err, &nf):
		return kinded{"not_found", err}
	case errors.As(err, &inv):
		return kinded{"invalid", err}
	case errors.Is(err, context.DeadlineExceeded):
		return kinded{"timeout", err}
	}
	if s, ok := status.FromError(err); ok {
		switch s.Code() {
		case codes.Unauthenticated, codes.PermissionDenied:
			return kinded{"auth", err}
		case codes.NotFound:
			return kinded{"not_found", err}
		case codes.InvalidArgument, codes.FailedPrecondition:
			return kinded{"invalid", err}
		case codes.DeadlineExceeded:
			return kinded{"timeout", err}
		}
	}
	return kinded{"other", err}
}
