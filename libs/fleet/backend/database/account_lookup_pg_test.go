package database

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"

	"cyclops-cs-backend/accountlookup"
	"cyclops-cs-backend/keycloak"
	"github.com/jackc/pgx/v5"
)

type fixtureAccountDirectory struct{}

func (fixtureAccountDirectory) LookupAccount(_ context.Context, id string) (*keycloak.Account, error) {
	return &keycloak.Account{ID: id}, nil
}

func TestAccountLookupUpgradeFromPreviousSchema(t *testing.T) {
	maintenanceURL := requireMigratorIntegration(t)
	ctx := context.Background()
	migrationURL, credentials := isolatedMigrationDatabase(t, ctx, maintenanceURL)
	files, err := embeddedMigrations()
	if err != nil {
		t.Fatal(err)
	}
	connection := connect(t, ctx, migrationURL)
	defer connection.Close(ctx)
	tx, err := connection.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	if err = newRuntimeDDL(tx).ensureMigrationLedger(ctx); err != nil {
		t.Fatal(err)
	}
	for _, file := range files[:11] {
		prepared, prepareErr := prepareMigrationExecution(file)
		if prepareErr != nil {
			t.Fatal(prepareErr)
		}
		if _, err = tx.Exec(ctx, prepared.SQL); err != nil {
			t.Fatal(err)
		}
		if _, err = tx.Exec(ctx, insertAppliedMigrationStatement, file.Version, file.Name, file.SHA256); err != nil {
			t.Fatal(err)
		}
	}
	if err = tx.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	store, err := accountlookup.NewStore(ctx, migrationURL)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	serviceCtx, cancel := context.WithCancel(ctx)
	service := accountlookup.New(serviceCtx, store, fixtureAccountDirectory{}, "test", "test", nil)
	defer func() { cancel(); service.Wait() }()
	if _, err = service.Lookup(ctx, "admin", accountlookup.Request{Kind: "account_id", Value: "test"}); !errors.Is(err, accountlookup.ErrUnavailable) {
		t.Fatal("pre-migration feature should be unavailable")
	}
	if _, _, err = store.Resolve(ctx, "test", "test", "test"); err == nil {
		t.Fatal("pre-migration table unexpectedly present")
	}
	for i := 0; i < 2; i++ {
		summary := captureRunSummary(t, func() error { return Run(ctx, Config{MigrationURL: migrationURL, Credentials: credentials}) })
		wantPending := 1 - i
		if summary.Pending != wantPending || summary.Applied != wantPending || summary.Target != 12 {
			t.Fatalf("upgrade/no-op summary=%+v", summary)
		}
	}
	if _, _, err = store.Resolve(ctx, "test", "test", "test"); err != nil {
		t.Fatal("existing pool failed to recover after migration")
	}
	if result, err := service.Lookup(ctx, "admin", accountlookup.Request{Kind: "account_id", Value: "test"}); err != nil || result.Status != "found" {
		t.Fatal("lookup failed to recover after migration")
	}
}

func assertAccountLookupContract(t *testing.T, ctx context.Context, ownerURL, appURL, reportingURL, tenantURL string) {
	t.Helper()
	first, err := accountlookup.NewStore(ctx, appURL)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	second, err := accountlookup.NewStore(ctx, appURL)
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	if err = first.Record(ctx, "realm", "key", "pseudonym", "subject"); err != nil {
		t.Fatal(err)
	}
	if err = first.Record(ctx, "realm", "key", "pseudonym", "subject"); err != nil {
		t.Fatal(err)
	}
	if err = first.Record(ctx, "realm", "key", "pseudonym", "other-subject"); err == nil {
		t.Fatal("mapping overwrite succeeded")
	}
	subject, found, err := second.Resolve(ctx, "realm", "key", "pseudonym")
	if err != nil || !found || subject != "subject" {
		t.Fatalf("mapping lookup failed: found=%t err=%v", found, err)
	}
	for _, scope := range [][2]string{{"other-realm", "key"}, {"realm", "other-key"}} {
		_, found, err = second.Resolve(ctx, scope[0], scope[1], "pseudonym")
		if err != nil || found {
			t.Fatalf("mapping crossed scope: found=%t err=%v", found, err)
		}
	}
	if err = first.MarkScanComplete(ctx, "realm", "key"); err != nil {
		t.Fatal(err)
	}
	for _, scope := range []struct {
		realm, key string
		want       bool
	}{{"realm", "key", true}, {"other-realm", "key", false}, {"realm", "other-key", false}} {
		complete, err := second.Complete(ctx, scope.realm, scope.key)
		if err != nil || complete != scope.want {
			t.Fatalf("completion=%t err=%v", complete, err)
		}
	}
	if err = first.Audit(ctx, "actor", "found"); err != nil {
		t.Fatal(err)
	}
	if err = first.Audit(ctx, "actor", "arbitrary-target-text"); err == nil {
		t.Fatal("unbounded audit outcome accepted")
	}
	var allowed atomic.Int32
	var wg sync.WaitGroup
	for i := 0; i < 30; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			s := first
			if i%2 == 0 {
				s = second
			}
			ok, err := s.Allow(ctx, "actor")
			if err != nil {
				t.Error(err)
			}
			if ok {
				allowed.Add(1)
			}
		}(i)
	}
	wg.Wait()
	if allowed.Load() != 10 {
		t.Fatalf("allowed=%d want 10", allowed.Load())
	}
	owner, err := pgx.Connect(ctx, ownerURL)
	if err != nil {
		t.Fatal(err)
	}
	defer owner.Close(ctx)
	if _, err = owner.Exec(ctx, `UPDATE account_lookup_private.rate_limit SET attempts=ARRAY[clock_timestamp()-interval '61 seconds'] WHERE actor='actor'`); err != nil {
		t.Fatal(err)
	}
	if ok, err := first.Allow(ctx, "actor"); err != nil || !ok {
		t.Fatalf("expired window allowed=%t err=%v", ok, err)
	}
	var owned bool
	if err = owner.QueryRow(ctx, `SELECT nspowner=(SELECT oid FROM pg_roles WHERE rolname=current_user) FROM pg_namespace WHERE nspname='account_lookup_private'`).Scan(&owned); err != nil || !owned {
		t.Fatalf("schema ownership=%t err=%v", owned, err)
	}
	for _, url := range []string{reportingURL, tenantURL} {
		for _, table := range []string{"mapping", "audit", "backfill", "rate_limit"} {
			assertStatementFails(t, ctx, url, "SELECT * FROM account_lookup_private."+table)
		}
	}
	assertStatementFails(t, ctx, appURL, "SELECT * FROM account_lookup_private.audit")
	assertStatementFails(t, ctx, appURL, "DELETE FROM account_lookup_private.mapping")
	assertStatementFails(t, ctx, appURL, "CREATE TABLE account_lookup_private.unexpected (id int)")
}
