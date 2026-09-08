package statequery

import (
	"context"
	"os"
	"testing"
)

func TestE2EListsProjectedNamespacesForTenant(t *testing.T) {
	queryDSN := os.Getenv("CYCLOPS_E2E_STATE_QUERY_DSN")
	tenantPassword := os.Getenv("CYCLOPS_E2E_STATE_QUERY_TENANT_PASSWORD")
	tenant := os.Getenv("CYCLOPS_E2E_CAPSULE_TENANT")
	clusterID := os.Getenv("CYCLOPS_E2E_CLUSTER_ID")
	if queryDSN == "" || tenantPassword == "" || tenant == "" || clusterID == "" {
		t.Skip("set CYCLOPS_E2E_STATE_QUERY_DSN, CYCLOPS_E2E_STATE_QUERY_TENANT_PASSWORD, CYCLOPS_E2E_CAPSULE_TENANT, and CYCLOPS_E2E_CLUSTER_ID")
	}

	ctx := context.Background()
	executor, err := NewExecutor(queryDSN, tenantPassword)
	if err != nil {
		t.Fatal(err)
	}
	query := `
		select name, object->'metadata'->'labels'->>'capsule.clastix.io/tenant' as owner
		from k8s_api.current_resources
		where cluster_id = '` + clusterID + `'
		  and api_group = ''
		  and resource = 'namespaces'
		order by name`
	writer := &collectingResultWriter{}
	err = executor.Execute(ctx, tenant, query, writer)
	if err != nil {
		t.Fatal(err)
	}
	if len(writer.rows) == 0 {
		t.Fatal("tenant query returned no projected namespaces")
	}
	for _, row := range writer.rows {
		if len(row) != 2 || row[1] != tenant {
			t.Fatalf("row escaped tenant RLS: %#v, want tenant %q", row, tenant)
		}
		t.Logf("projected namespace: %s", row[0])
	}
}
