CREATE ROLE cyclops_usage_reader LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;
ALTER ROLE cyclops_usage_reader SET default_transaction_read_only = on;
ALTER ROLE cyclops_usage_reader SET statement_timeout = '10000ms';
ALTER ROLE cyclops_usage_reader SET idle_in_transaction_session_timeout = '10000ms';

SET LOCAL ROLE k8s_state_owner;
CREATE INDEX resource_event_outbox_usage_lookup_idx
ON k8s_state.resource_event_outbox
(capsule_tenant, api_group, resource, observed_at, namespace, name);
GRANT SELECT ON k8s_state.resource_event_outbox TO k8s_reporting_owner;
RESET ROLE;

SET LOCAL ROLE k8s_reporting_owner;
CREATE FUNCTION k8s_reporting.usage_sandbox_events(
    p_capsule_tenant text,
    p_start timestamptz,
    p_end timestamptz
)
RETURNS TABLE (
    event_id uuid,
    namespace text,
    sandbox_name text,
    sandbox_uid text,
    pool_name text,
    runtime text,
    vm_name text,
    event_type text,
    observed_at timestamptz
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = k8s_state, pg_catalog
AS $$
BEGIN
    IF p_capsule_tenant IS NULL OR p_capsule_tenant = '' THEN
        RAISE EXCEPTION 'capsule tenant is required';
    END IF;
    IF p_start IS NULL OR p_end IS NULL OR p_end <= p_start OR p_end - p_start > interval '31 days' THEN
        RAISE EXCEPTION 'usage window must be positive and at most 31 days';
    END IF;

    RETURN QUERY
    WITH tenant_events AS (
        SELECT event.*
        FROM k8s_state.resource_event_outbox AS event
        WHERE event.capsule_tenant = p_capsule_tenant
          AND event.api_group = 'osgym.cua.ai'
          AND event.resource = 'osgymsandboxes'
          AND event.observed_at < p_end
    ), baseline AS (
        SELECT DISTINCT ON (event.namespace, event.uid) event.*
        FROM tenant_events AS event
        WHERE event.observed_at < p_start
        ORDER BY event.namespace, event.uid, event.observed_at DESC, event.observed_sequence DESC, event.event_id DESC
    ), window_events AS (
        SELECT event.*
        FROM tenant_events AS event
        WHERE event.observed_at >= p_start
    )
    SELECT
        event.event_id,
        event.namespace,
        event.name,
        event.uid,
        event.object -> 'metadata' -> 'labels' ->> 'osgym.cua.ai/warmpool',
        event.object -> 'spec' -> 'vmTemplate' ->> 'runtime',
        event.object -> 'status' ->> 'vmName',
        event.event_type,
        event.observed_at
    FROM (
        SELECT * FROM baseline
        UNION ALL
        SELECT * FROM window_events
    ) AS event
    ORDER BY event.namespace, event.uid, event.observed_at, event.observed_sequence, event.event_id;
END
$$;
REVOKE ALL ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) FROM PUBLIC;
GRANT USAGE ON SCHEMA k8s_reporting TO cyclops_usage_reader;
GRANT EXECUTE ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) TO cyclops_usage_reader;
RESET ROLE;
