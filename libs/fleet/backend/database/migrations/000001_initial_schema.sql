-- 1. Static roles.
CREATE ROLE cyclops_app LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_state_owner NOLOGIN INHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_state_writer LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_state_exporter LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_query_tenant NOLOGIN INHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_query_admin NOLOGIN INHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_role_admin LOGIN NOINHERIT CREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_reporting_owner NOLOGIN INHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE k8s_metabase LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;

-- 2. Fixed PostgreSQL 16 role memberships.
GRANT k8s_state_owner TO CURRENT_USER WITH INHERIT FALSE, SET TRUE;
GRANT k8s_reporting_owner TO CURRENT_USER WITH INHERIT FALSE, SET TRUE;
GRANT k8s_query_tenant TO k8s_role_admin WITH ADMIN TRUE, INHERIT FALSE, SET FALSE;
GRANT k8s_query_admin TO k8s_reporting_owner WITH INHERIT TRUE, SET FALSE;

-- 3. GitHub trust state.
CREATE TABLE public.github_trust_policies (
    id text PRIMARY KEY,
    owner_sub text NOT NULL,
    name text NOT NULL,
    repository text NOT NULL,
    allowed_namespaces text[] NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX idx_github_trust_policies_owner_sub ON public.github_trust_policies (owner_sub);
CREATE INDEX idx_github_trust_policies_repository ON public.github_trust_policies (repository);
REVOKE ALL ON TABLE public.github_trust_policies FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.github_trust_policies TO cyclops_app;

-- 4. State storage, tenant query API, and RLS boundary.
CREATE SCHEMA k8s_state AUTHORIZATION k8s_state_owner;
CREATE SCHEMA k8s_api AUTHORIZATION k8s_state_owner;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
SET LOCAL ROLE k8s_state_owner;
REVOKE CREATE ON SCHEMA k8s_state, k8s_api FROM PUBLIC;
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
    credential_fingerprint text NOT NULL,
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

CREATE FUNCTION k8s_state.register_tenant_role(p_role_name name, p_capsule_tenant text, p_credential_fingerprint text)
RETURNS void
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
    INSERT INTO k8s_state.query_tenant_role (role_name, capsule_tenant, credential_fingerprint)
    VALUES (p_role_name, p_capsule_tenant, p_credential_fingerprint)
    ON CONFLICT (role_name) DO UPDATE
    SET capsule_tenant = EXCLUDED.capsule_tenant,
        credential_fingerprint = EXCLUDED.credential_fingerprint
$$;

CREATE FUNCTION k8s_state.unregister_tenant_role(p_role_name name)
RETURNS void
LANGUAGE sql
VOLATILE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
    DELETE FROM k8s_state.query_tenant_role
    WHERE role_name = p_role_name
$$;

REVOKE ALL ON FUNCTION k8s_state.register_tenant_role(name, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION k8s_state.unregister_tenant_role(name) FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_state TO k8s_role_admin;
GRANT SELECT ON k8s_state.query_tenant_role TO k8s_role_admin;
GRANT EXECUTE ON FUNCTION k8s_state.register_tenant_role(name, text, text) TO k8s_role_admin;
GRANT EXECUTE ON FUNCTION k8s_state.unregister_tenant_role(name) TO k8s_role_admin;
GRANT USAGE ON SCHEMA k8s_state TO k8s_reporting_owner;
GRANT SELECT ON k8s_state.resource_state TO k8s_reporting_owner;
RESET ROLE;

-- 5. Read-only reporting boundary for Metabase.

DO $$
DECLARE
    membership record;
    schema_name text;
    relation record;
    function_oid oid;
BEGIN
    FOR membership IN
        SELECT parent.rolname
        FROM pg_auth_members AS member
        JOIN pg_roles AS parent ON parent.oid = member.roleid
        WHERE member.member = 'k8s_metabase'::regrole
    LOOP
        EXECUTE format('REVOKE %I FROM k8s_metabase', membership.rolname);
    END LOOP;

    FOR schema_name IN
        SELECT namespace.nspname
        FROM pg_namespace AS namespace
        JOIN LATERAL aclexplode(namespace.nspacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND acl.grantee = 'k8s_metabase'::regrole
    LOOP
        EXECUTE format('REVOKE ALL ON SCHEMA %I FROM k8s_metabase', schema_name);
    END LOOP;

    FOR relation IN
        SELECT namespace.nspname, class.relname
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid = class.relnamespace
        JOIN LATERAL aclexplode(class.relacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND class.relkind IN ('r', 'p', 'v', 'm', 'f')
          AND acl.grantee = 'k8s_metabase'::regrole
    LOOP
        EXECUTE format('REVOKE ALL ON TABLE %I.%I FROM k8s_metabase', relation.nspname, relation.relname);
    END LOOP;

    FOR relation IN
        SELECT namespace.nspname, class.relname
        FROM pg_class AS class
        JOIN pg_namespace AS namespace ON namespace.oid = class.relnamespace
        JOIN LATERAL aclexplode(class.relacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND class.relkind = 'S'
          AND acl.grantee = 'k8s_metabase'::regrole
    LOOP
        EXECUTE format('REVOKE ALL ON SEQUENCE %I.%I FROM k8s_metabase', relation.nspname, relation.relname);
    END LOOP;

    FOR function_oid IN
        SELECT procedure.oid
        FROM pg_proc AS procedure
        JOIN pg_namespace AS namespace ON namespace.oid = procedure.pronamespace
        JOIN LATERAL aclexplode(procedure.proacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND acl.grantee = 'k8s_metabase'::regrole
    LOOP
        EXECUTE format('REVOKE ALL ON ROUTINE %s FROM k8s_metabase', function_oid::regprocedure);
    END LOOP;
END
$$;
RESET ROLE;

ALTER ROLE k8s_metabase SET default_transaction_read_only = on;
ALTER ROLE k8s_metabase SET statement_timeout = '20000ms';
ALTER ROLE k8s_metabase SET idle_in_transaction_session_timeout = '20000ms';
CREATE SCHEMA k8s_reporting AUTHORIZATION k8s_reporting_owner;
SET LOCAL ROLE k8s_reporting_owner;
REVOKE ALL ON SCHEMA k8s_reporting FROM PUBLIC;
CREATE VIEW k8s_reporting.current_resources
WITH (security_barrier = true, security_invoker = false) AS
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
REVOKE ALL ON ALL TABLES IN SCHEMA k8s_reporting FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA k8s_reporting REVOKE ALL ON TABLES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA k8s_reporting REVOKE ALL ON SEQUENCES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES IN SCHEMA k8s_reporting REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_reporting TO k8s_metabase;
GRANT SELECT ON k8s_reporting.current_resources TO k8s_metabase;
RESET ROLE;

-- 6. Runtime roles may inspect, but never modify, migration state.
REVOKE ALL ON SCHEMA cyclops_migrations FROM PUBLIC;
REVOKE ALL ON TABLE cyclops_migrations.applied_migrations FROM PUBLIC;
GRANT USAGE ON SCHEMA cyclops_migrations TO cyclops_app, k8s_state_writer, k8s_state_exporter, k8s_role_admin, k8s_metabase;
GRANT SELECT ON TABLE cyclops_migrations.applied_migrations TO cyclops_app, k8s_state_writer, k8s_state_exporter, k8s_role_admin, k8s_metabase;

-- 7. Fail closed if reporting reaches state or application data.
SET LOCAL ROLE k8s_state_owner;
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM pg_auth_members AS member
        WHERE member.member = 'k8s_metabase'::regrole
    ) THEN
        RAISE EXCEPTION 'k8s_metabase retained role memberships';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM pg_proc AS procedure
        JOIN pg_namespace AS namespace ON namespace.oid = procedure.pronamespace
        JOIN LATERAL aclexplode(
            COALESCE(procedure.proacl, acldefault('f'::"char", procedure.proowner))
        ) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND procedure.prokind IN ('f', 'p')
          AND procedure.prosecdef
          AND acl.grantee = 0
          AND acl.privilege_type = 'EXECUTE'
    ) THEN
        RAISE EXCEPTION 'PUBLIC-executable SECURITY DEFINER routine blocks reporting access';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM pg_namespace AS namespace
        JOIN LATERAL aclexplode(namespace.nspacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND namespace.nspname <> 'k8s_reporting'
          AND namespace.nspname <> 'cyclops_migrations'
          AND acl.grantee = 'k8s_metabase'::regrole
    ) OR EXISTS (
        SELECT 1
        FROM pg_class AS relation
        JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        JOIN LATERAL aclexplode(relation.relacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND namespace.nspname <> 'k8s_reporting'
          AND namespace.nspname <> 'cyclops_migrations'
          AND relation.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')
          AND acl.grantee = 'k8s_metabase'::regrole
    ) OR EXISTS (
        SELECT 1
        FROM pg_proc AS procedure
        JOIN pg_namespace AS namespace ON namespace.oid = procedure.pronamespace
        JOIN LATERAL aclexplode(procedure.proacl) AS acl ON true
        WHERE namespace.nspname !~ '^pg_'
          AND namespace.nspname <> 'information_schema'
          AND namespace.nspname <> 'k8s_reporting'
          AND namespace.nspname <> 'cyclops_migrations'
          AND acl.grantee = 'k8s_metabase'::regrole
    ) THEN
        RAISE EXCEPTION 'k8s_metabase retained direct non-reporting ACLs';
    END IF;

    IF has_schema_privilege('k8s_metabase', 'k8s_state', 'USAGE')
       OR has_schema_privilege('k8s_metabase', 'k8s_api', 'USAGE')
       OR has_table_privilege('k8s_metabase', 'k8s_state.resource_state', 'SELECT')
       OR has_table_privilege('k8s_metabase', 'k8s_api.current_resources', 'SELECT')
       OR has_table_privilege('k8s_metabase', 'public.github_trust_policies', 'SELECT') THEN
        RAISE EXCEPTION 'k8s_metabase retained known source or application access';
    END IF;
END
$$;
RESET ROLE;
