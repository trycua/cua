// account-lookup-backfill performs an explicitly authorized, bounded account scan.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"time"

	"cyclops-cs-backend/accountlookup"
	"cyclops-cs-backend/internal/redactederror"
	"cyclops-cs-backend/keycloak"
	"cyclops-cs-backend/productanalytics"
)

type accountLister interface {
	ListAccountIDs(context.Context, int, int) ([]string, error)
}
type mappingStore interface {
	Record(context.Context, string, string, string, string) error
	MarkScanComplete(context.Context, string, string) error
}

func scan(ctx context.Context, source accountLister, store mappingStore, realm, key string, offset, pageSize, maxPages int, execute bool) (int, bool, error) {
	count := 0
	for page := 0; page < maxPages; page++ {
		ids, err := source.ListAccountIDs(ctx, offset+count, pageSize)
		if err != nil {
			return count, false, redactederror.New("account scan failed", err)
		}
		if len(ids) > pageSize {
			return count, false, errors.New("account scan exceeded page bound")
		}
		for _, id := range ids {
			if strings.TrimSpace(id) == "" {
				return count, false, errors.New("account scan returned invalid account")
			}
			if execute {
				if err := store.Record(ctx, realm, accountlookup.KeyID(key), productanalytics.PseudonymForUserID(id, key), id); err != nil {
					return count, false, redactederror.New("account mapping write failed", err)
				}
			}
			count++
		}
		if len(ids) < pageSize {
			if execute && offset == 0 {
				if err := store.MarkScanComplete(ctx, realm, accountlookup.KeyID(key)); err != nil {
					return count, false, redactederror.New("account scan completion write failed", err)
				}
			}
			return count, true, nil
		}
	}
	return count, false, nil
}

func run() error {
	execute := flag.Bool("execute", false, "write mappings; default only reads Keycloak")
	offset := flag.Int("offset", 0, "resume offset (never marks complete)")
	pageSize := flag.Int("page-size", 100, "accounts per page (1-100)")
	maxPages := flag.Int("max-pages", 100, "maximum pages this invocation (1-10000)")
	flag.Parse()
	if flag.NArg() != 0 || *offset < 0 || *offset > 1000000000 || *pageSize < 1 || *pageSize > 100 || *maxPages < 1 || *maxPages > 10000 {
		return errors.New("invalid backfill bounds")
	}
	key := strings.TrimSpace(os.Getenv("POSTHOG_IDENTITY_KEY"))
	realm := strings.TrimSpace(os.Getenv("KC_REALM"))
	baseURL := strings.TrimSpace(os.Getenv("KC_BASE_URL"))
	clientID := strings.TrimSpace(os.Getenv("KC_ADMIN_CLIENT_ID"))
	secret := os.Getenv("KC_ADMIN_CLIENT_SECRET")
	if key == "" || realm == "" || baseURL == "" || clientID == "" || secret == "" {
		return errors.New("explicit Keycloak configuration and POSTHOG_IDENTITY_KEY are required")
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()
	ctx, deadline := context.WithTimeout(ctx, 30*time.Minute)
	defer deadline()
	var store *accountlookup.Store
	if *execute {
		var err error
		store, err = accountlookup.NewStore(ctx, os.Getenv("DATABASE_URL"))
		if err != nil {
			return redactederror.New("backfill database configuration invalid", err)
		}
		defer store.Close()
	}
	source := keycloak.NewAdmin(baseURL, realm, clientID, secret, "", "")
	count, end, err := scan(ctx, source, store, realm, key, *offset, *pageSize, *maxPages, *execute)
	fmt.Printf("execute=%t scanned=%d next_offset=%d observed_end=%t completion_eligible=%t\n", *execute, count, *offset+count, end, *execute && *offset == 0 && end && err == nil)
	return err
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
