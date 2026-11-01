// The unmodified modal-client Go SDK against the gateway over mock Spaces
// (the mock's process vocabulary: echo, fail N, cat, sleep MS).
//
//	go run . <env.json>
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"time"

	modal "github.com/modal-labs/modal-client/go"
)

var (
	passed   int
	failures []string
)

func check(name string, fn func() error) {
	if err := fn(); err != nil {
		failures = append(failures, name)
		fmt.Printf("FAIL %s: %v\n", name, err)
		return
	}
	passed++
	fmt.Printf("PASS %s\n", name)
}

var yes = true

func main() {
	raw, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	env := map[string]string{}
	if err := json.Unmarshal(raw, &env); err != nil {
		panic(err)
	}
	for k, v := range env {
		os.Setenv(k, v)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Minute)
	defer cancel()
	mc, err := modal.NewClient()
	if err != nil {
		panic(err)
	}
	app, err := mc.Apps.FromName(ctx, "go-hermetic", &modal.AppFromNameParams{CreateIfMissing: true})
	if err != nil {
		panic(err)
	}
	image := mc.Images.FromRegistry("ghcr.io/trycua/linux:24.04", nil)
	create := func(command ...string) (*modal.Sandbox, error) {
		return mc.Sandboxes.Create(ctx, app, image, &modal.SandboxCreateParams{Command: command, Timeout: 2 * time.Minute})
	}
	sb, err := create("sleep", "600000")
	if err != nil {
		panic(err)
	}
	defer sb.Terminate(context.Background(), nil)

	check("exec_stdout_wait", func() error {
		p, err := sb.Exec(ctx, []string{"echo", "hi", "there"}, nil)
		if err != nil {
			return err
		}
		out, err := io.ReadAll(p.Stdout)
		if err != nil {
			return err
		}
		code, err := p.Wait(ctx, nil)
		if err != nil {
			return err
		}
		if string(out) != "hi there\n" || code != 0 {
			return fmt.Errorf("got %q, %d", out, code)
		}
		return nil
	})
	check("exec_exit_code", func() error {
		p, err := sb.Exec(ctx, []string{"fail", "7"}, nil)
		if err != nil {
			return err
		}
		code, err := p.Wait(ctx, nil)
		if err != nil {
			return err
		}
		if code != 7 {
			return fmt.Errorf("exit code %d", code)
		}
		return nil
	})
	check("exec_stdin", func() error {
		p, err := sb.Exec(ctx, []string{"cat"}, nil)
		if err != nil {
			return err
		}
		if _, err := p.Stdin.Write([]byte("alpha\n")); err != nil {
			return err
		}
		if err := p.Stdin.Close(); err != nil {
			return err
		}
		out, err := io.ReadAll(p.Stdout)
		if err != nil {
			return err
		}
		if string(out) != "alpha\n" {
			return fmt.Errorf("got %q", out)
		}
		return nil
	})
	check("tags_and_from_id", func() error {
		s, err := create("sleep", "600000")
		if err != nil {
			return err
		}
		defer s.Terminate(context.Background(), nil)
		if err := s.SetTags(ctx, map[string]string{"suite": "go"}, nil); err != nil {
			return err
		}
		tags, err := s.GetTags(ctx, nil)
		if err != nil || tags["suite"] != "go" {
			return fmt.Errorf("tags %v %v", tags, err)
		}
		again, err := mc.Sandboxes.FromID(ctx, s.SandboxID, nil)
		if err != nil {
			return err
		}
		if again.SandboxID != s.SandboxID {
			return fmt.Errorf("different sandbox")
		}
		return nil
	})
	check("filesystem_roundtrip_and_errors", func() error {
		fs := sb.Filesystem
		if err := fs.WriteText(ctx, "héllo\n", "/tmp/go-fs/a.txt", nil); err != nil {
			return err
		}
		got, err := fs.ReadText(ctx, "/tmp/go-fs/a.txt", nil)
		if err != nil || got != "héllo\n" {
			return fmt.Errorf("read %q %v", got, err)
		}
		st, err := fs.Stat(ctx, "/tmp/go-fs/a.txt", nil)
		if err != nil || st.Size != 7 {
			return fmt.Errorf("stat %v %v", st, err)
		}
		list, err := fs.ListFiles(ctx, "/tmp/go-fs", nil)
		if err != nil || len(list) != 1 || list[0].Name != "a.txt" {
			return fmt.Errorf("list %v %v", list, err)
		}
		if _, err := fs.ReadText(ctx, "/tmp/go-fs/missing", nil); err == nil {
			return fmt.Errorf("expected an error for a missing file")
		} else if _, ok := err.(modal.SandboxFilesystemNotFoundError); !ok {
			return fmt.Errorf("wrong error type %T: %v", err, err)
		}
		if err := fs.MakeDirectory(ctx, "/tmp/go-fs/d/e", &modal.SandboxFilesystemMakeDirectoryParams{CreateParents: &yes}); err != nil {
			return err
		}
		return fs.Remove(ctx, "/tmp/go-fs", &modal.SandboxFilesystemRemoveParams{Recursive: true})
	})
	check("poll_tunnels_connect_token", func() error {
		code, err := sb.Poll(ctx, nil)
		if err != nil || code != nil {
			return fmt.Errorf("poll %v %v", code, err)
		}
		s, err := mc.Sandboxes.Create(ctx, app, image, &modal.SandboxCreateParams{Command: []string{"sleep", "600000"}, Timeout: 2 * time.Minute, EncryptedPorts: []int{8080}})
		if err != nil {
			return err
		}
		defer s.Terminate(context.Background(), nil)
		t, err := s.Tunnels(ctx, 30*time.Second, nil)
		if err != nil || t[8080] == nil || t[8080].URL() == "" {
			return fmt.Errorf("tunnels %v %v", t, err)
		}
		c, err := s.CreateConnectToken(ctx, &modal.SandboxCreateConnectTokenParams{Port: 8080, UserMetadata: "m"})
		if err != nil || c.Token == "" || c.URL == "" {
			return fmt.Errorf("token %v %v", c, err)
		}
		return nil
	})
	check("snapshot_filesystem_and_boot_from_it", func() error {
		img, err := sb.SnapshotFilesystem(ctx, nil)
		if err != nil {
			return err
		}
		s, err := mc.Sandboxes.Create(ctx, app, img, &modal.SandboxCreateParams{Command: []string{"sleep", "600000"}, Timeout: time.Minute})
		if err != nil {
			return err
		}
		_, err = s.Terminate(context.Background(), nil)
		return err
	})
	check("name_lookup_and_list", func() error {
		s, err := mc.Sandboxes.Create(ctx, app, image, &modal.SandboxCreateParams{Command: []string{"sleep", "600000"}, Timeout: time.Minute, Name: "go-lookup"})
		if err != nil {
			return err
		}
		defer s.Terminate(context.Background(), nil)
		again, err := mc.Sandboxes.FromName(ctx, app.Name, "go-lookup", nil)
		if err != nil || again.SandboxID != s.SandboxID {
			return fmt.Errorf("from name %v %v", again, err)
		}
		it, err := mc.Sandboxes.List(ctx, &modal.SandboxListParams{AppID: app.AppID})
		if err != nil {
			return err
		}
		for sb2, err := range it {
			if err != nil {
				return err
			}
			if sb2.SandboxID == s.SandboxID {
				return nil
			}
		}
		return fmt.Errorf("not listed")
	})
	check("volumes_secrets_queues_images", func() error {
		vol, err := mc.Volumes.FromName(ctx, "go-vol", &modal.VolumeFromNameParams{CreateIfMissing: true})
		if err != nil || vol.VolumeID == "" {
			return fmt.Errorf("volume %v %v", vol, err)
		}
		if err := mc.Volumes.Delete(ctx, "go-vol", nil); err != nil {
			return err
		}
		if _, err := mc.Secrets.FromMap(ctx, map[string]string{"A": "1"}, nil); err != nil {
			return err
		}
		q, err := mc.Queues.FromName(ctx, "go-q", &modal.QueueFromNameParams{CreateIfMissing: true})
		if err != nil {
			return err
		}
		if err := q.PutMany(ctx, []any{int64(1), int64(2)}, nil); err != nil {
			return err
		}
		n, err := q.Len(ctx, nil)
		if err != nil || n != 2 {
			return fmt.Errorf("len %d %v", n, err)
		}
		if err := mc.Queues.Delete(ctx, "go-q", nil); err != nil {
			return err
		}
		built, err := mc.Images.FromRegistry("ghcr.io/trycua/linux:24.04", nil).DockerfileCommands([]string{"RUN echo go-layer"}, nil).Build(ctx, app, nil)
		if err != nil || built.ImageID == "" {
			return fmt.Errorf("build %v %v", built, err)
		}
		return nil
	})
	check("entrypoint_output", func() error {
		s, err := create("echo", "from the entrypoint")
		if err != nil {
			return err
		}
		out, err := io.ReadAll(s.Stdout)
		if err != nil {
			return err
		}
		if string(out) != "from the entrypoint\n" {
			return fmt.Errorf("got %q", out)
		}
		return nil
	})
	fmt.Printf("%d passed, %d failed: %v\n", passed, len(failures), failures)
	if len(failures) > 0 {
		os.Exit(1)
	}
}
