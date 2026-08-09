package statequery

import (
	"strings"
	"testing"
)

func TestValidateRejectsUnsafeSQL(t *testing.T) {
	for _, query := range []string{
		"select 1; select 2",
		"delete from k8s_api.current_resources",
		"with gone as (delete from k8s_api.current_resources returning *) select * from gone",
		"select * into leaked from k8s_api.current_resources",
		"copy k8s_api.current_resources to stdout",
		"set role k8s_query_admin",
		"select set_config('role', 'k8s_query_admin', false)",
		"select * from pg_catalog.pg_roles",
		"select pg_sleep(60)",
		"select * from public.current_resources",
		"select * from current_resources",
	} {
		t.Run(query, func(t *testing.T) {
			if _, err := Validate(query, DefaultLimits()); err == nil {
				t.Fatalf("accepted unsafe SQL: %s", query)
			}
		})
	}
}

func TestValidateAcceptsReadOnlyStateQueries(t *testing.T) {
	for _, query := range []string{
		`select name, object->'metadata'->>'name' from k8s_api.current_resources order by name`,
		`with owned as (select * from k8s_api.current_resources where resource = 'namespaces') select count(*) from owned`,
		`select capsule_tenant, count(*) from k8s_api.current_resources group by capsule_tenant`,
		`select name from k8s_api.current_resources where labels @> '{"team":"a"}'::jsonb`,
	} {
		t.Run(query, func(t *testing.T) {
			validated, err := Validate(query, Limits{MaxRows: 25, TimeoutMS: 5000, MaxSQLBytes: 65536})
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(validated.SQL, "LIMIT 26") {
				t.Fatalf("validated SQL does not include truncation sentinel: %s", validated.SQL)
			}
			if validated.MaxRows != 25 {
				t.Fatalf("MaxRows = %d", validated.MaxRows)
			}
		})
	}
}

func TestValidateEnforcesInputLimits(t *testing.T) {
	if _, err := Validate("", DefaultLimits()); err == nil {
		t.Fatal("accepted empty SQL")
	}
	if _, err := Validate("select * from k8s_api.current_resources", Limits{MaxRows: 0, TimeoutMS: 1, MaxSQLBytes: 1}); err == nil {
		t.Fatal("accepted SQL exceeding byte limit")
	}
}
