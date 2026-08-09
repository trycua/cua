CREATE TABLE k8s_state.resource_state (
    cluster_id text NOT NULL,
    api_group text NOT NULL,
    resource text NOT NULL,
    namespace text NOT NULL,
    name text NOT NULL,
    capsule_tenant text,
    uid text,
    resource_version text,
    schema_hash text NOT NULL,
    watch_epoch bigint NOT NULL,
    observed_sequence bigint NOT NULL,
    labels jsonb NOT NULL DEFAULT '{}'::jsonb,
    object jsonb NOT NULL,
    source_time timestamptz,
    ingested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (cluster_id, api_group, resource, namespace, name),
    CHECK (jsonb_typeof(labels) = 'object'),
    CHECK (jsonb_typeof(object) = 'object')
);

CREATE INDEX resource_state_tenant_gvr_namespace_idx
    ON k8s_state.resource_state
    (capsule_tenant, cluster_id, api_group, resource, namespace, name);

CREATE INDEX resource_state_labels_gin_idx
    ON k8s_state.resource_state USING gin (labels jsonb_path_ops);

CREATE TABLE k8s_state.watch_checkpoint (
    cluster_id text NOT NULL,
    api_group text NOT NULL,
    resource text NOT NULL,
    watch_epoch bigint NOT NULL,
    last_resource_version text NOT NULL,
    last_observed_sequence bigint NOT NULL,
    initial_list_complete boolean NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (cluster_id, api_group, resource)
);

CREATE TABLE k8s_state.resource_event_outbox (
    event_id uuid PRIMARY KEY,
    cluster_id text NOT NULL,
    api_group text NOT NULL,
    resource text NOT NULL,
    namespace text NOT NULL,
    name text NOT NULL,
    capsule_tenant text,
    uid text,
    resource_version text,
    schema_hash text NOT NULL,
    event_type text NOT NULL CHECK (event_type IN ('ADDED', 'MODIFIED', 'DELETED')),
    watch_epoch bigint NOT NULL,
    observed_sequence bigint NOT NULL,
    object jsonb NOT NULL,
    observed_at timestamptz NOT NULL,
    claimed_at timestamptz,
    claim_id uuid,
    exported_at timestamptz,
    UNIQUE (cluster_id, api_group, resource, watch_epoch, observed_sequence),
    CHECK (jsonb_typeof(object) = 'object')
);

CREATE INDEX resource_event_outbox_pending_idx
    ON k8s_state.resource_event_outbox (observed_at, event_id)
    WHERE exported_at IS NULL;

CREATE TABLE k8s_state.resource_schema (
    schema_hash text PRIMARY KEY,
    api_group text NOT NULL,
    api_version text NOT NULL,
    kind text NOT NULL,
    resource text NOT NULL,
    structural_schema jsonb NOT NULL,
    first_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (jsonb_typeof(structural_schema) = 'object')
);

CREATE TABLE k8s_state.query_tenant_role (
    role_name name PRIMARY KEY,
    capsule_tenant text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE FUNCTION k8s_state.tenant_for_role(role_name name)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
    SELECT capsule_tenant
    FROM k8s_state.query_tenant_role
    WHERE query_tenant_role.role_name = tenant_for_role.role_name
$$;

REVOKE ALL ON FUNCTION k8s_state.tenant_for_role(name) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION k8s_state.tenant_for_role(name) TO k8s_query_tenant;

ALTER TABLE k8s_state.resource_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE k8s_state.resource_state FORCE ROW LEVEL SECURITY;

CREATE POLICY writer_current_state ON k8s_state.resource_state
FOR ALL TO k8s_state_writer
USING (true)
WITH CHECK (true);

CREATE POLICY tenant_current_state ON k8s_state.resource_state
FOR SELECT TO k8s_query_tenant
USING (
    capsule_tenant = k8s_state.tenant_for_role(current_user)
    AND (
        namespace <> ''
        OR (api_group = '' AND resource = 'namespaces')
    )
);

CREATE POLICY admin_current_state ON k8s_state.resource_state
FOR SELECT TO k8s_query_admin
USING (true);

CREATE VIEW k8s_api.current_resources
WITH (security_barrier = true, security_invoker = true) AS
SELECT
    cluster_id,
    api_group,
    resource,
    namespace,
    name,
    uid,
    resource_version,
    schema_hash,
    labels,
    object,
    source_time,
    ingested_at
FROM k8s_state.resource_state;

REVOKE ALL ON ALL TABLES IN SCHEMA k8s_state FROM PUBLIC;
REVOKE ALL ON ALL TABLES IN SCHEMA k8s_api FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_state TO k8s_state_writer, k8s_state_exporter;
GRANT SELECT, INSERT, UPDATE, DELETE ON k8s_state.resource_state TO k8s_state_writer;
GRANT SELECT, INSERT, UPDATE ON k8s_state.watch_checkpoint TO k8s_state_writer;
GRANT SELECT, INSERT, UPDATE ON k8s_state.resource_schema TO k8s_state_writer;
GRANT SELECT, INSERT ON k8s_state.resource_event_outbox TO k8s_state_writer;
GRANT SELECT, UPDATE, DELETE ON k8s_state.resource_event_outbox TO k8s_state_exporter;
GRANT USAGE ON SCHEMA k8s_api TO k8s_query_tenant, k8s_query_admin;
GRANT SELECT ON k8s_state.resource_state TO k8s_query_tenant, k8s_query_admin;
GRANT SELECT ON k8s_api.current_resources TO k8s_query_tenant, k8s_query_admin;

CREATE FUNCTION k8s_state.register_tenant_role(p_role_name name, p_capsule_tenant text)
RETURNS void
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
    INSERT INTO k8s_state.query_tenant_role (role_name, capsule_tenant)
    VALUES (p_role_name, p_capsule_tenant)
    ON CONFLICT (role_name) DO UPDATE
    SET capsule_tenant = EXCLUDED.capsule_tenant
$$;

REVOKE ALL ON FUNCTION k8s_state.register_tenant_role(name, text) FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_state TO k8s_role_admin;
GRANT EXECUTE ON FUNCTION k8s_state.register_tenant_role(name, text) TO k8s_role_admin;
