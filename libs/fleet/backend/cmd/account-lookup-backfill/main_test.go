package main

import (
	"context"
	"errors"
	"testing"
)

type fakeSource struct {
	pages [][]string
	fail  bool
	err   error
}

func (s *fakeSource) ListAccountIDs(context.Context, int, int) ([]string, error) {
	if err := s.err; err != nil {
		return nil, err
	}
	if s.fail {
		return nil, errors.New("private upstream details")
	}
	page := s.pages[0]
	s.pages = s.pages[1:]
	return page, nil
}

type fakeStore struct {
	recorded, completed    int
	recordErr, completeErr error
}

func (s *fakeStore) Record(context.Context, string, string, string, string) error {
	s.recorded++
	return s.recordErr
}
func (s *fakeStore) MarkScanComplete(context.Context, string, string) error {
	s.completed++
	return s.completeErr
}

type privateScanError struct{}

func (*privateScanError) Error() string { return "private upstream details" }

func TestScanPreservesRedactedCauses(t *testing.T) {
	cause := &privateScanError{}
	for _, tc := range []struct {
		name    string
		source  fakeSource
		store   fakeStore
		message string
	}{
		{"read", fakeSource{err: cause}, fakeStore{}, "account scan failed"},
		{"record", fakeSource{pages: [][]string{{"one"}}}, fakeStore{recordErr: cause}, "account mapping write failed"},
		{"complete", fakeSource{pages: [][]string{{}}}, fakeStore{completeErr: cause}, "account scan completion write failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, end, err := scan(context.Background(), &tc.source, &tc.store, "realm", "key", 0, 2, 2, true)
			var typed *privateScanError
			if end || err == nil || err.Error() != tc.message || !errors.Is(err, cause) || !errors.As(err, &typed) || typed != cause || errors.Unwrap(err) != cause {
				t.Fatal("failed scan must redact text while preserving the cause")
			}
		})
	}
}
func TestScanCompletionBoundaries(t *testing.T) {
	for _, tc := range []struct {
		name                string
		offset, max         int
		execute             bool
		pages               [][]string
		end                 bool
		recorded, completed int
	}{
		{"dry run", 0, 2, false, [][]string{{"one"}}, true, 0, 0},
		{"full observed scan", 0, 2, true, [][]string{{"one", "two"}, {}}, true, 2, 1},
		{"resume cannot complete", 2, 2, true, [][]string{{"three"}}, true, 1, 0},
		{"bounded cannot complete", 0, 1, true, [][]string{{"one", "two"}}, false, 2, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			store := &fakeStore{}
			_, end, err := scan(context.Background(), &fakeSource{pages: tc.pages}, store, "realm", "key", tc.offset, 2, tc.max, tc.execute)
			if err != nil || end != tc.end || store.recorded != tc.recorded || store.completed != tc.completed {
				t.Fatalf("end=%t err=%v store=%+v", end, err, store)
			}
		})
	}
}
func TestFailedScanNeverCompletes(t *testing.T) {
	store := &fakeStore{}
	_, end, err := scan(context.Background(), &fakeSource{fail: true}, store, "realm", "key", 0, 2, 2, true)
	if err == nil || end || store.completed != 0 || err.Error() != "account scan failed" {
		t.Fatalf("end=%t err=%v", end, err)
	}
}
