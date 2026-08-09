package statequery

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5"
)

type FixedRoleURLs struct {
	Writer    string
	Exporter  string
	Query     string
	RoleAdmin string
}

var fixedLoginRoles = []struct {
	name string
	url  func(FixedRoleURLs) string
	attr string
}{
	{"k8s_state_writer", func(urls FixedRoleURLs) string { return urls.Writer }, "LOGIN INHERIT NOCREATEROLE"},
	{"k8s_state_exporter", func(urls FixedRoleURLs) string { return urls.Exporter }, "LOGIN INHERIT NOCREATEROLE"},
	{"k8s_query_broker", func(urls FixedRoleURLs) string { return urls.Query }, "LOGIN NOINHERIT NOCREATEROLE"},
	{"k8s_role_admin", func(urls FixedRoleURLs) string { return urls.RoleAdmin }, "LOGIN NOINHERIT CREATEROLE"},
}

func parseFixedRoleURLs(urls FixedRoleURLs) (map[string]string, error) {
	configured := 0
	for _, role := range fixedLoginRoles {
		if role.url(urls) != "" {
			configured++
		}
	}
	if configured == 0 {
		return map[string]string{}, nil
	}
	if configured != len(fixedLoginRoles) {
		return nil, fmt.Errorf("all fixed state role database URLs must be configured together")
	}

	credentials := make(map[string]string, len(fixedLoginRoles))
	for _, role := range fixedLoginRoles {
		config, err := pgx.ParseConfig(role.url(urls))
		if err != nil {
			return nil, fmt.Errorf("parse database URL for %s: %w", role.name, err)
		}
		if config.User != role.name {
			return nil, fmt.Errorf("database URL for %s uses unexpected user %q", role.name, config.User)
		}
		if config.Password == "" {
			return nil, fmt.Errorf("database URL for %s has no password", role.name)
		}
		credentials[role.name] = config.Password
	}
	return credentials, nil
}

// ReconcileFixedRoles creates the fixed least-privilege roles and rotates their passwords.
func ReconcileFixedRoles(ctx context.Context, adminURL string, urls FixedRoleURLs) error {
	credentials, err := parseFixedRoleURLs(urls)
	if err != nil {
		return err
	}
	if len(credentials) == 0 {
		return nil
	}
	adminConfig, err := pgx.ParseConfig(adminURL)
	if err != nil {
		return fmt.Errorf("parse state role administrator URL: %w", err)
	}
	conn, err := pgx.ConnectConfig(ctx, adminConfig)
	if err != nil {
		return fmt.Errorf("connect state role administrator: %w", err)
	}
	defer conn.Close(ctx)

	tx, err := conn.Begin(ctx)
	if err != nil {
		return fmt.Errorf("begin fixed role reconciliation: %w", err)
	}
	defer tx.Rollback(ctx)
	if _, err := tx.Exec(ctx, `select pg_advisory_xact_lock(hashtext('cyclops-k8s-state-fixed-roles'))`); err != nil {
		return fmt.Errorf("lock fixed role reconciliation: %w", err)
	}

	for _, role := range []struct{ name, attr string }{
		{"k8s_state_owner", "NOLOGIN INHERIT NOCREATEROLE"},
		{"k8s_query_tenant", "NOLOGIN INHERIT NOCREATEROLE"},
		{"k8s_query_admin", "NOLOGIN INHERIT NOCREATEROLE"},
	} {
		if err := ensureRole(ctx, tx, role.name, role.attr, ""); err != nil {
			return err
		}
	}
	for _, role := range fixedLoginRoles {
		if err := ensureRole(ctx, tx, role.name, role.attr, credentials[role.name]); err != nil {
			return err
		}
	}

	adminIdentifier := pgx.Identifier{adminConfig.User}.Sanitize()
	for _, statement := range []string{
		"grant k8s_state_owner to " + adminIdentifier,
		"grant k8s_query_admin to k8s_query_broker",
		"grant k8s_query_tenant to k8s_role_admin with admin option",
	} {
		if _, err := tx.Exec(ctx, statement); err != nil {
			return fmt.Errorf("apply fixed role membership: %w", err)
		}
	}

	if err := tx.Commit(ctx); err != nil {
		return fmt.Errorf("commit fixed role reconciliation: %w", err)
	}
	return nil
}

func ensureRole(ctx context.Context, tx pgx.Tx, name, attributes, password string) error {
	var exists bool
	if err := tx.QueryRow(ctx, `select exists(select 1 from pg_roles where rolname = $1)`, name).Scan(&exists); err != nil {
		return fmt.Errorf("check role %s: %w", name, err)
	}
	identifier := pgx.Identifier{name}.Sanitize()
	if !exists {
		if _, err := tx.Exec(ctx, "create role "+identifier); err != nil {
			return fmt.Errorf("create role %s: %w", name, err)
		}
	}
	statement := "alter role " + identifier + " " + attributes
	if password != "" {
		var passwordClause string
		if err := tx.QueryRow(ctx, `select format(' password %L', $1::text)`, password).Scan(&passwordClause); err != nil {
			return fmt.Errorf("quote password for role %s: %w", name, err)
		}
		statement += passwordClause
	}
	if _, err := tx.Exec(ctx, statement); err != nil {
		return fmt.Errorf("configure role %s: %w", name, err)
	}
	return nil
}
