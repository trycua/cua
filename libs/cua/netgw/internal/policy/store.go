package policy

import (
	"bytes"
	"os"
	"sync"
	"sync/atomic"
	"time"
)

// Store holds the rules in force and reloads them when their file changes.
//
// cua replaces the file atomically (a new file renamed over it). A file
// that does not parse is reported and ignored: the rules in force stay,
// so a bad write never opens the network.
type Store struct {
	path    string
	current atomic.Pointer[File]

	mu       sync.Mutex
	last     []byte
	onChange []func(*File)
}

// Open loads path; the gateway does not start without valid rules.
func Open(path string) (*Store, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	f, err := Parse(data)
	if err != nil {
		return nil, err
	}
	s := &Store{path: path, last: data}
	s.current.Store(f)
	return s, nil
}

// Static is a Store that never reloads (tests).
func Static(f *File) *Store {
	s := &Store{}
	s.current.Store(f)
	return s
}

// Current is the rules in force.
func (s *Store) Current() *File {
	return s.current.Load()
}

// OnChange registers fn, called after every reload that changed the rules.
func (s *Store) OnChange(fn func(*File)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.onChange = append(s.onChange, fn)
}

// Reload reads the file again. It returns whether the rules changed, and
// the error that kept the old rules.
func (s *Store) Reload() (bool, error) {
	if s.path == "" {
		return false, nil
	}
	data, err := os.ReadFile(s.path)
	if err != nil {
		return false, err
	}
	s.mu.Lock()
	if bytes.Equal(data, s.last) {
		s.mu.Unlock()
		return false, nil
	}
	f, err := Parse(data)
	if err != nil {
		s.mu.Unlock()
		return false, err
	}
	s.last = data
	s.current.Store(f)
	fns := append([]func(*File){}, s.onChange...)
	s.mu.Unlock()
	for _, fn := range fns {
		fn(f)
	}
	return true, nil
}

// Watch reloads every interval (and when kick receives) until stop
// closes; report gets each reload error once per distinct message.
func (s *Store) Watch(interval time.Duration, kick <-chan struct{}, stop <-chan struct{}, report func(error)) {
	t := time.NewTicker(interval)
	defer t.Stop()
	var lastErr string
	for {
		select {
		case <-stop:
			return
		case <-t.C:
		case <-kick:
		}
		if _, err := s.Reload(); err != nil {
			if err.Error() != lastErr {
				lastErr = err.Error()
				report(err)
			}
		} else {
			lastErr = ""
		}
	}
}
