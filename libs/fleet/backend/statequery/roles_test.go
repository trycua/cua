package statequery

import "testing"

func TestParseFixedRoleURLsRequiresExpectedUsers(t *testing.T) {
	urls := FixedRoleURLs{
		Writer:    "postgres://k8s_state_writer:writer@db.example/cyclops",
		Exporter:  "postgres://k8s_state_exporter:exporter@db.example/cyclops",
		Query:     "postgres://k8s_query_broker:query@db.example/cyclops",
		RoleAdmin: "postgres://k8s_role_admin:roles@db.example/cyclops",
	}
	credentials, err := parseFixedRoleURLs(urls)
	if err != nil {
		t.Fatal(err)
	}
	if got := credentials["k8s_query_broker"]; got != "query" {
		t.Fatalf("query broker password = %q", got)
	}

	urls.Writer = "postgres://wrong:writer@db.example/cyclops"
	if _, err := parseFixedRoleURLs(urls); err == nil {
		t.Fatal("expected unexpected writer username to fail")
	}
}

func TestParseFixedRoleURLsAllowsAllURLsAbsent(t *testing.T) {
	credentials, err := parseFixedRoleURLs(FixedRoleURLs{})
	if err != nil {
		t.Fatal(err)
	}
	if len(credentials) != 0 {
		t.Fatalf("credentials = %#v, want empty", credentials)
	}
}

func TestParseFixedRoleURLsRejectsPartialConfiguration(t *testing.T) {
	_, err := parseFixedRoleURLs(FixedRoleURLs{
		Writer: "postgres://k8s_state_writer:writer@db.example/cyclops",
	})
	if err == nil {
		t.Fatal("expected partial fixed-role URL configuration to fail")
	}
}
