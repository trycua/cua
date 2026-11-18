package relay

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strings"
	"sync"
	"time"
)

// publisher reaches a QEMU guest's ports: QEMU's user-mode network has no
// route into the guest, so the first connection to a guest port asks QEMU
// (over the relay's own QMP socket) to publish it on the namespace's
// loopback, and every connection to that port goes there.
type publisher struct {
	path string

	mu    sync.Mutex
	conn  net.Conn
	r     *bufio.Reader
	ports map[uint16]uint16 // guest port -> loopback port
}

func newPublisher(path string) *publisher {
	return &publisher{path: path, ports: map[uint16]uint16{}}
}

func (p *publisher) dial(ctx context.Context, port uint16) (net.Conn, error) {
	host, err := p.publish(ctx, port)
	if err != nil {
		return nil, err
	}
	var d net.Dialer
	return d.DialContext(ctx, "tcp4", fmt.Sprintf("127.0.0.1:%d", host))
}

// publish returns the loopback port QEMU forwards to guest port `port`,
// asking QEMU for one the first time.
func (p *publisher) publish(ctx context.Context, port uint16) (uint16, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if h, ok := p.ports[port]; ok {
		return h, nil
	}
	if err := p.connect(ctx); err != nil {
		return 0, err
	}
	host, err := freePort()
	if err != nil {
		return 0, err
	}
	reply, err := p.hmp(fmt.Sprintf("hostfwd_add net0 tcp:127.0.0.1:%d-:%d", host, port))
	if err != nil {
		p.drop()
		return 0, err
	}
	// HMP answers nothing on success, its error in words otherwise.
	if strings.TrimSpace(reply) != "" {
		return 0, fmt.Errorf("qemu could not publish guest port %d: %s", port, strings.TrimSpace(reply))
	}
	p.ports[port] = host
	return host, nil
}

// connect opens the monitor (QEMU may still be starting: it is retried
// until ctx ends) and leaves command mode on.
func (p *publisher) connect(ctx context.Context) error {
	if p.conn != nil {
		return nil
	}
	for {
		var d net.Dialer
		c, err := d.DialContext(ctx, "unix", p.path)
		if err == nil {
			p.conn, p.r = c, bufio.NewReader(c)
			_ = c.SetDeadline(time.Now().Add(10 * time.Second))
			// The greeting, then capabilities negotiation.
			if _, err := p.read(); err != nil {
				p.drop()
				return err
			}
			if _, err := p.command(map[string]any{"execute": "qmp_capabilities"}); err != nil {
				p.drop()
				return err
			}
			_ = c.SetDeadline(time.Time{})
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("qemu's monitor for the tree relay: %w", err)
		case <-time.After(200 * time.Millisecond):
		}
	}
}

func (p *publisher) drop() {
	if p.conn != nil {
		p.conn.Close()
	}
	p.conn, p.r = nil, nil
}

// hmp runs one human-monitor command and returns its text.
func (p *publisher) hmp(line string) (string, error) {
	_ = p.conn.SetDeadline(time.Now().Add(10 * time.Second))
	defer func() { _ = p.conn.SetDeadline(time.Time{}) }()
	ret, err := p.command(map[string]any{
		"execute":   "human-monitor-command",
		"arguments": map[string]any{"command-line": line},
	})
	if err != nil {
		return "", err
	}
	var text string
	_ = json.Unmarshal(ret, &text)
	return text, nil
}

// command sends cmd and returns its "return", skipping events.
func (p *publisher) command(cmd map[string]any) (json.RawMessage, error) {
	b, _ := json.Marshal(cmd)
	if _, err := p.conn.Write(append(b, '\n')); err != nil {
		return nil, err
	}
	for {
		m, err := p.read()
		if err != nil {
			return nil, err
		}
		if r, ok := m["return"]; ok {
			return r, nil
		}
		if e, ok := m["error"]; ok {
			return nil, fmt.Errorf("qmp: %s", string(e))
		}
		// An event: not the answer.
	}
}

func (p *publisher) read() (map[string]json.RawMessage, error) {
	line, err := p.r.ReadBytes('\n')
	if err != nil {
		return nil, err
	}
	var m map[string]json.RawMessage
	if err := json.Unmarshal(line, &m); err != nil {
		return nil, errors.New("qmp: not JSON")
	}
	return m, nil
}

// freePort is a TCP port free on the namespace's loopback now.
func freePort() (uint16, error) {
	ln, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	defer ln.Close()
	return uint16(ln.Addr().(*net.TCPAddr).Port), nil
}
