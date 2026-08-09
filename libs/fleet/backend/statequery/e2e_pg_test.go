package statequery

import (
	"context"
	"os"
	"testing"
)

func TestE2EListsProjectedNamespacesForTenant(t *testing.T) {
	queryURL := os.Getenv("CYCLOPS_E2E_STATE_QUERY_URL")
	roleAdminURL := os.Getenv("CYCLOPS_E2E_STATE_ROLE_ADMIN_URL")
	tenant := os.Getenv("CYCLOPS_E2E_CAPSULE_TENANT")
	clusterID := os.Getenv("CYCLOPS_E2E_CLUSTER_ID")
	if queryURL == "" || roleAdminURL == "" || tenant == "" || clusterID == "" {
		t.Skip("set CYCLOPS_E2E_STATE_QUERY_URL, CYCLOPS_E2E_STATE_ROLE_ADMIN_URL, CYCLOPS_E2E_CAPSULE_TENANT, and CYCLOPS_E2E_CLUSTER_ID")
	}

	ctx := context.Background()
	executor, err := NewExecutor(ctx, queryURL, roleAdminURL)
	if err != nil {
		t.Fatal(err)
	}
	defer executor.Close()
	query, err := Validate(`
		select name, object->'metadata'->'labels'->>'capsule.clastix.io/tenant' as owner
		from k8s_api.current_resources
		where cluster_id = '`+clusterID+`'
		  and api_group = ''
		  and resource = 'namespaces'
		order by name`, DefaultLimits())
	if err != nil {
		t.Fatal(err)
	}
	result, err := executor.Execute(ctx, tenant, false, query)
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Rows) == 0 {
		t.Fatal("tenant query returned no projected namespaces")
	}
	for _, row := range result.Rows {
		if len(row) != 2 || row[1] != tenant {
			t.Fatalf("row escaped tenant RLS: %#v, want tenant %q", row, tenant)
		}
		t.Logf("projected namespace: %s", row[0])
	}
}
