CREATE ROLE billing_meter_owner NOLOGIN INHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
CREATE ROLE cyclops_meter_writer LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
GRANT billing_meter_owner TO CURRENT_USER WITH INHERIT FALSE, SET TRUE;

CREATE SCHEMA billing_meter AUTHORIZATION billing_meter_owner;
REVOKE CREATE ON SCHEMA billing_meter FROM PUBLIC;

SET LOCAL ROLE billing_meter_owner;
CREATE TABLE billing_meter.reservation_hour_collection (
    collection_run_id uuid PRIMARY KEY,
    logical_key text NOT NULL,
    revision integer NOT NULL CHECK (revision > 0),
    cluster_id text NOT NULL,
    hour_start timestamptz NOT NULL,
    hour_end timestamptz NOT NULL,
    covered_seconds numeric(12, 6) NOT NULL CHECK (covered_seconds >= 0 AND covered_seconds <= 3600),
    discovered_sandboxes integer NOT NULL CHECK (discovered_sandboxes >= 0),
    inserted_facts integer NOT NULL CHECK (inserted_facts >= 0),
    unchanged_facts integer NOT NULL CHECK (unchanged_facts >= 0),
    source_sha256 text NOT NULL CHECK (source_sha256 ~ '^[0-9a-f]{64}$'),
    supersedes_collection_run_id uuid REFERENCES billing_meter.reservation_hour_collection(collection_run_id),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (logical_key, revision),
    CHECK (logical_key <> '' AND length(logical_key) <= 1024),
    CHECK (cluster_id <> ''),
    CHECK (date_trunc('hour', hour_start) = hour_start),
    CHECK (hour_end = hour_start + interval '1 hour'),
    CHECK ((revision = 1 AND supersedes_collection_run_id IS NULL) OR (revision > 1 AND supersedes_collection_run_id IS NOT NULL))
);

CREATE TABLE billing_meter.reservation_hour_fact (
    fact_id uuid PRIMARY KEY,
    logical_key text NOT NULL,
    revision integer NOT NULL CHECK (revision > 0),
    cluster_id text NOT NULL,
    capsule_tenant text NOT NULL,
    namespace text NOT NULL,
    sandbox_uid text NOT NULL,
    sandbox_name text NOT NULL,
    pool_name text NOT NULL,
    runtime text NOT NULL,
    hour_start timestamptz NOT NULL,
    hour_end timestamptz NOT NULL,
    virtual_cpu_core_seconds numeric(20, 6) NOT NULL CHECK (virtual_cpu_core_seconds >= 0),
    virtual_memory_byte_seconds numeric(30, 6) NOT NULL CHECK (virtual_memory_byte_seconds >= 0),
    ready_seconds numeric(12, 6) NOT NULL CHECK (ready_seconds >= 0 AND ready_seconds <= 3600),
    covered_seconds numeric(12, 6) NOT NULL CHECK (covered_seconds >= 0 AND covered_seconds <= 3600),
    scrape_interval_seconds integer NOT NULL CHECK (scrape_interval_seconds > 0 AND scrape_interval_seconds <= 300),
    source_sha256 text NOT NULL CHECK (source_sha256 ~ '^[0-9a-f]{64}$'),
    collection_run_id uuid NOT NULL,
    supersedes_fact_id uuid REFERENCES billing_meter.reservation_hour_fact(fact_id),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (logical_key, revision),
    CHECK (logical_key <> '' AND length(logical_key) <= 1024),
    CHECK (cluster_id <> '' AND capsule_tenant <> '' AND namespace <> ''),
    CHECK (sandbox_uid <> '' AND sandbox_name <> '' AND pool_name <> '' AND runtime <> ''),
    CHECK (date_trunc('hour', hour_start) = hour_start),
    CHECK (hour_end = hour_start + interval '1 hour'),
    CHECK ((revision = 1 AND supersedes_fact_id IS NULL) OR (revision > 1 AND supersedes_fact_id IS NOT NULL))
);

CREATE INDEX reservation_hour_fact_tenant_hour_idx
    ON billing_meter.reservation_hour_fact (capsule_tenant, hour_start, pool_name);
CREATE INDEX reservation_hour_fact_sandbox_hour_idx
    ON billing_meter.reservation_hour_fact (cluster_id, sandbox_uid, hour_start, revision DESC);

CREATE VIEW billing_meter.reservation_hour_collection_current AS
SELECT DISTINCT ON (logical_key)
    collection_run_id,
    logical_key,
    revision,
    cluster_id,
    hour_start,
    hour_end,
    covered_seconds,
    discovered_sandboxes,
    inserted_facts,
    unchanged_facts,
    source_sha256,
    supersedes_collection_run_id,
    created_at
FROM billing_meter.reservation_hour_collection
ORDER BY logical_key, revision DESC;

CREATE VIEW billing_meter.reservation_hour_current AS
SELECT DISTINCT ON (logical_key)
    fact_id,
    logical_key,
    revision,
    cluster_id,
    capsule_tenant,
    namespace,
    sandbox_uid,
    sandbox_name,
    pool_name,
    runtime,
    hour_start,
    hour_end,
    virtual_cpu_core_seconds,
    virtual_memory_byte_seconds,
    ready_seconds,
    covered_seconds,
    scrape_interval_seconds,
    source_sha256,
    collection_run_id,
    supersedes_fact_id,
    created_at
FROM billing_meter.reservation_hour_fact
ORDER BY logical_key, revision DESC;

CREATE FUNCTION billing_meter.reject_reservation_hour_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'billing_meter reservation hours are append-only';
END
$$;

CREATE TRIGGER reservation_hour_collection_immutable
BEFORE UPDATE OR DELETE OR TRUNCATE ON billing_meter.reservation_hour_collection
FOR EACH STATEMENT EXECUTE FUNCTION billing_meter.reject_reservation_hour_mutation();
CREATE TRIGGER reservation_hour_fact_immutable
BEFORE UPDATE OR DELETE OR TRUNCATE ON billing_meter.reservation_hour_fact
FOR EACH STATEMENT EXECUTE FUNCTION billing_meter.reject_reservation_hour_mutation();

REVOKE ALL ON ALL TABLES IN SCHEMA billing_meter FROM PUBLIC;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA billing_meter FROM PUBLIC;
GRANT USAGE ON SCHEMA billing_meter TO cyclops_meter_writer, k8s_reporting_owner;
GRANT SELECT, INSERT ON TABLE billing_meter.reservation_hour_collection, billing_meter.reservation_hour_fact TO cyclops_meter_writer;
GRANT SELECT ON TABLE billing_meter.reservation_hour_collection_current, billing_meter.reservation_hour_current TO cyclops_meter_writer, k8s_reporting_owner;
RESET ROLE;

SET LOCAL ROLE k8s_state_owner;
CREATE FUNCTION k8s_api.sandbox_meter_tenant(
    p_cluster_id text,
    p_namespace text,
    p_sandbox_uid text
)
RETURNS text
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
DECLARE
    resolved_tenant text;
BEGIN
    IF p_cluster_id IS NULL OR p_cluster_id = '' OR p_namespace IS NULL OR p_namespace = '' OR p_sandbox_uid IS NULL OR p_sandbox_uid = '' THEN
        RAISE EXCEPTION 'cluster, namespace, and sandbox UID are required';
    END IF;

    SELECT event.capsule_tenant
    INTO resolved_tenant
    FROM k8s_state.resource_event_outbox AS event
    WHERE event.cluster_id = p_cluster_id
      AND event.api_group = 'osgym.cua.ai'
      AND event.resource = 'osgymsandboxes'
      AND event.namespace = p_namespace
      AND event.uid = p_sandbox_uid
      AND event.capsule_tenant IS NOT NULL
      AND event.capsule_tenant <> ''
    ORDER BY event.observed_at DESC, event.observed_sequence DESC, event.event_id DESC
    LIMIT 1;

    IF resolved_tenant IS NULL THEN
        RAISE EXCEPTION 'sandbox tenant identity is unavailable';
    END IF;
    RETURN resolved_tenant;
END
$$;
REVOKE ALL ON FUNCTION k8s_api.sandbox_meter_tenant(text, text, text) FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_api TO cyclops_meter_writer;
GRANT EXECUTE ON FUNCTION k8s_api.sandbox_meter_tenant(text, text, text) TO cyclops_meter_writer;
RESET ROLE;

SET LOCAL ROLE k8s_reporting_owner;
CREATE FUNCTION k8s_reporting.reservation_hour_facts(
    p_capsule_tenant text,
    p_start timestamptz,
    p_end timestamptz
)
RETURNS TABLE (
    namespace text,
    sandbox_uid text,
    sandbox_name text,
    pool_name text,
    runtime text,
    hour_start timestamptz,
    hour_end timestamptz,
    virtual_cpu_core_seconds numeric,
    virtual_memory_byte_seconds numeric,
    ready_seconds numeric,
    covered_seconds numeric
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = billing_meter, pg_catalog
AS $$
BEGIN
    IF p_capsule_tenant IS NULL OR p_capsule_tenant = '' THEN
        RAISE EXCEPTION 'capsule tenant is required';
    END IF;
    IF p_start IS NULL OR p_end IS NULL OR p_end <= p_start OR p_end - p_start > interval '31 days' THEN
        RAISE EXCEPTION 'reservation usage window must be positive and at most 31 days';
    END IF;

    RETURN QUERY
    SELECT
        fact.namespace,
        fact.sandbox_uid,
        fact.sandbox_name,
        fact.pool_name,
        fact.runtime,
        fact.hour_start,
        fact.hour_end,
        fact.virtual_cpu_core_seconds,
        fact.virtual_memory_byte_seconds,
        fact.ready_seconds,
        fact.covered_seconds
    FROM billing_meter.reservation_hour_current AS fact
    JOIN billing_meter.reservation_hour_collection_current AS collection
      ON collection.cluster_id = fact.cluster_id
     AND collection.hour_start = fact.hour_start
    WHERE fact.capsule_tenant = p_capsule_tenant
      AND fact.hour_start < p_end
      AND fact.hour_end > p_start
    ORDER BY fact.hour_start, fact.namespace, fact.pool_name, fact.sandbox_uid;
END
$$;
REVOKE ALL ON FUNCTION k8s_reporting.reservation_hour_facts(text, timestamptz, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION k8s_reporting.reservation_hour_facts(text, timestamptz, timestamptz) TO cyclops_usage_reader;

CREATE FUNCTION k8s_reporting.reservation_meter_status(
    p_cluster_id text,
    p_start timestamptz,
    p_end timestamptz
)
RETURNS TABLE (
    data_as_of timestamptz,
    complete boolean
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = billing_meter, pg_catalog
AS $$
BEGIN
    IF p_cluster_id IS NULL OR p_cluster_id = '' THEN
        RAISE EXCEPTION 'cluster is required';
    END IF;
    IF p_start IS NULL OR p_end IS NULL OR p_end <= p_start OR p_end - p_start > interval '31 days' OR date_trunc('hour', p_start) <> p_start OR date_trunc('hour', p_end) <> p_end THEN
        RAISE EXCEPTION 'reservation meter window must contain exact UTC hours and be at most 31 days';
    END IF;

    RETURN QUERY
    SELECT
        coalesce(max(collection.hour_end), p_start),
        count(*) = extract(epoch FROM p_end - p_start)::bigint / 3600
    FROM billing_meter.reservation_hour_collection_current AS collection
    WHERE collection.cluster_id = p_cluster_id
      AND collection.hour_start >= p_start
      AND collection.hour_end <= p_end;
END
$$;
REVOKE ALL ON FUNCTION k8s_reporting.reservation_meter_status(text, timestamptz, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION k8s_reporting.reservation_meter_status(text, timestamptz, timestamptz) TO cyclops_usage_reader;
RESET ROLE;
