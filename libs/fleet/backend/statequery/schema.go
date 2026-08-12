package statequery

import (
	"context"
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"sort"
	"strings"

	"github.com/jackc/pgx/v5"
)

//go:embed migrations/*.sql
var migrations embed.FS

type migrationFile struct {
	Name   string
	SQL    string
	Digest string
}

func migrationSQL(name string) (string, error) {
	contents, err := migrations.ReadFile("migrations/" + name)
	if err != nil {
		return "", fmt.Errorf("read state migration %s: %w", name, err)
	}
	return string(contents), nil
}

func embeddedMigrations() ([]migrationFile, error) {
	entries, err := fs.ReadDir(migrations, "migrations")
	if err != nil {
		return nil, fmt.Errorf("list state migrations: %w", err)
	}

	files := make([]migrationFile, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".sql") {
			continue
		}
		sql, err := migrationSQL(entry.Name())
		if err != nil {
			return nil, err
		}
		digest := sha256.Sum256([]byte(sql))
		files = append(files, migrationFile{
			Name:   entry.Name(),
			SQL:    sql,
			Digest: hex.EncodeToString(digest[:]),
		})
	}
	sort.Slice(files, func(i, j int) bool { return files[i].Name < files[j].Name })
	return files, nil
}

func checkAppliedDigest(name, applied, current string) error {
	if applied != current {
		return fmt.Errorf("state migration %s changed after application: recorded %s, current %s", name, applied, current)
	}
	return nil
}

// Migrate applies embedded state-query migrations exactly once in filename order.
func Migrate(ctx context.Context, adminURL string) error {
	if adminURL == "" {
		return nil
	}
	files, err := embeddedMigrations()
	if err != nil {
		return err
	}
	config, err := pgx.ParseConfig(adminURL)
	if err != nil {
		return fmt.Errorf("parse state migration database URL: %w", err)
	}
	conn, err := pgx.ConnectConfig(ctx, config)
	if err != nil {
		return fmt.Errorf("connect state migration database: %w", err)
	}
	defer conn.Close(ctx)

	tx, err := conn.Begin(ctx)
	if err != nil {
		return fmt.Errorf("begin state migration transaction: %w", err)
	}
	defer tx.Rollback(ctx)

	if _, err := tx.Exec(ctx, `select pg_advisory_xact_lock(hashtext('cyclops-k8s-state-migrations'))`); err != nil {
		return fmt.Errorf("lock state migrations: %w", err)
	}
	if _, err := tx.Exec(ctx, `create schema if not exists k8s_state authorization k8s_state_owner`); err != nil {
		return fmt.Errorf("create state migration schema: %w", err)
	}
	if _, err := tx.Exec(ctx, `alter schema k8s_state owner to k8s_state_owner`); err != nil {
		return fmt.Errorf("own state migration schema: %w", err)
	}
	if _, err := tx.Exec(ctx, `create schema if not exists k8s_api authorization k8s_state_owner`); err != nil {
		return fmt.Errorf("create state API schema: %w", err)
	}
	if _, err := tx.Exec(ctx, `alter schema k8s_api owner to k8s_state_owner`); err != nil {
		return fmt.Errorf("own state API schema: %w", err)
	}
	if _, err := tx.Exec(ctx, `revoke create on schema public, k8s_state, k8s_api from public`); err != nil {
		return fmt.Errorf("revoke public state schema creation: %w", err)
	}
	if _, err := tx.Exec(ctx, `
		create table if not exists k8s_state.schema_migrations (
			filename text primary key,
			sha256 text not null,
			applied_at timestamptz not null default clock_timestamp()
		)`); err != nil {
		return fmt.Errorf("create state migration ledger: %w", err)
	}
	if _, err := tx.Exec(ctx, `alter table k8s_state.schema_migrations owner to k8s_state_owner`); err != nil {
		return fmt.Errorf("own state migration ledger: %w", err)
	}
	if _, err := tx.Exec(ctx, `set local role k8s_state_owner`); err != nil {
		return fmt.Errorf("assume state owner role: %w", err)
	}

	for _, file := range files {
		var appliedDigest string
		err := tx.QueryRow(ctx, `select sha256 from k8s_state.schema_migrations where filename = $1`, file.Name).Scan(&appliedDigest)
		switch {
		case err == nil:
			if err := checkAppliedDigest(file.Name, appliedDigest, file.Digest); err != nil {
				return err
			}
			continue
		case !errors.Is(err, pgx.ErrNoRows):
			return fmt.Errorf("read state migration %s: %w", file.Name, err)
		}

		if _, err := tx.Exec(ctx, file.SQL); err != nil {
			return fmt.Errorf("apply state migration %s: %w", file.Name, err)
		}
		if _, err := tx.Exec(ctx,
			`insert into k8s_state.schema_migrations (filename, sha256) values ($1, $2)`,
			file.Name, file.Digest,
		); err != nil {
			return fmt.Errorf("record state migration %s: %w", file.Name, err)
		}
	}

	if err := tx.Commit(ctx); err != nil {
		return fmt.Errorf("commit state migrations: %w", err)
	}
	return nil
}
