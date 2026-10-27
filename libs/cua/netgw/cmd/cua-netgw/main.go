// cua-netgw is the Cua network gateway: the user-mode network of one VM
// (a macOS microVM, a QEMU VM on macOS or Windows, a macOS VM), where every
// connection the guest opens is checked against the Space's network rules.
//
//	cua-netgw -config gateway.json
//
// The configuration and the rules file are written by cua (crates/cua-vmm,
// netgw.rs). The rules file is reloaded when it changes (and on SIGHUP),
// and a change closes the open connections the new rules refuse.
//
// Built on gvisor-tap-vsock (Apache-2.0); see NOTICE.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/url"
	"os"
	"os/signal"
	"strconv"
	"syscall"
	"time"

	"github.com/containers/gvisor-tap-vsock/pkg/transport"
	log "github.com/sirupsen/logrus"
	"golang.org/x/sync/errgroup"

	"github.com/trycua/cua/libs/cua/netgw/internal/config"
	"github.com/trycua/cua/libs/cua/netgw/internal/events"
	"github.com/trycua/cua/libs/cua/netgw/internal/gateway"
	"github.com/trycua/cua/libs/cua/netgw/internal/policy"
)

// version is set at build time (-ldflags "-X main.version=...").
var version = "dev"

func main() {
	if err := run(); err != nil {
		fmt.Fprintf(os.Stderr, "cua-netgw: %v\n", err)
		os.Exit(1)
	}
}

func run() error {
	cfgPath := flag.String("config", "", "the gateway's configuration (JSON)")
	check := flag.String("check-rules", "", "check a rules file and exit")
	showVersion := flag.Bool("version", false, "print the version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Println("cua-netgw", version)
		return nil
	}
	if *check != "" {
		if _, err := policy.Load(*check); err != nil {
			return err
		}
		fmt.Println("ok")
		return nil
	}
	if *cfgPath == "" {
		return errors.New("-config is required")
	}
	cfg, err := config.Load(*cfgPath)
	if err != nil {
		return err
	}
	switch cfg.LogLevel {
	case "debug":
		log.SetLevel(log.DebugLevel)
	case "info":
		log.SetLevel(log.InfoLevel)
	case "error":
		log.SetLevel(log.ErrorLevel)
	default:
		log.SetLevel(log.WarnLevel)
	}
	rules, err := policy.Open(cfg.Rules)
	if err != nil {
		return fmt.Errorf("rules: %w", err)
	}
	var sink *events.Sink
	if cfg.Events != "" {
		f, err := events.OpenFile(cfg.Events, 1<<20)
		if err != nil {
			return fmt.Errorf("events: %w", err)
		}
		defer f.Close()
		sink = events.New(f, 10*time.Second, 50)
	}
	gw, err := gateway.New(cfg, rules, sink)
	if err != nil {
		return err
	}
	if cfg.PIDFile != "" {
		if err := os.WriteFile(cfg.PIDFile, []byte(strconv.Itoa(os.Getpid())), 0o600); err != nil {
			return fmt.Errorf("pid file: %w", err)
		}
		defer os.Remove(cfg.PIDFile)
	}

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()
	kick := make(chan struct{}, 1)
	hup := make(chan os.Signal, 1)
	signal.Notify(hup, syscall.SIGHUP)
	go func() {
		for range hup {
			select {
			case kick <- struct{}{}:
			default:
			}
		}
	}()
	go rules.Watch(250*time.Millisecond, kick, ctx.Done(), func(err error) {
		log.Errorf("rules not reloaded (the previous rules stay in force): %v", err)
	})

	g, ctx := errgroup.WithContext(ctx)
	if cfg.Listen.Vfkit != "" {
		conn, err := transport.ListenUnixgram(cfg.Listen.Vfkit)
		if err != nil {
			return fmt.Errorf("listen %s: %w", cfg.Listen.Vfkit, err)
		}
		g.Go(func() error {
			<-ctx.Done()
			conn.Close()
			if u, err := url.Parse(cfg.Listen.Vfkit); err == nil {
				os.Remove(u.Path)
			}
			return nil
		})
		g.Go(func() error {
			vf, err := transport.AcceptVfkit(conn)
			if err != nil {
				if ctx.Err() != nil {
					return nil
				}
				return fmt.Errorf("vfkit: %w", err)
			}
			if err := gw.AcceptVfkit(ctx, vf); err != nil && ctx.Err() == nil {
				log.Warnf("vfkit connection ended: %v", err)
			}
			// The VM is gone: so is its network.
			return errVMGone
		})
	}
	if cfg.Listen.Qemu != "" {
		ln, err := transport.Listen(cfg.Listen.Qemu)
		if err != nil {
			return fmt.Errorf("listen %s: %w", cfg.Listen.Qemu, err)
		}
		g.Go(func() error {
			<-ctx.Done()
			ln.Close()
			if u, err := url.Parse(cfg.Listen.Qemu); err == nil && u.Scheme == "unix" {
				os.Remove(u.Path)
			}
			return nil
		})
		g.Go(func() error {
			// One VM: serve each connection in turn (QEMU reconnects after
			// a reset with -netdev stream,reconnect-ms).
			for {
				c, err := ln.Accept()
				if err != nil {
					if ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
						return nil
					}
					return fmt.Errorf("qemu: %w", err)
				}
				if err := gw.AcceptQemu(ctx, c); err != nil && ctx.Err() == nil {
					log.Warnf("qemu connection ended: %v", err)
				}
			}
		})
	}
	if err := g.Wait(); err != nil && !errors.Is(err, errVMGone) {
		return err
	}
	return nil
}

var errVMGone = errors.New("the VM disconnected")
