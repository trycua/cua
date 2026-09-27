SET LOCAL ROLE k8s_reporting_owner;
CREATE OR REPLACE FUNCTION k8s_reporting.usage_sandbox_events(
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
          AND event.uid IS NOT NULL
          AND event.uid <> ''
          AND coalesce(
              coalesce(
            event.object -> 'metadata' -> 'labels' ->> 'osgym.cua.ai/warmpool',
            event.object -> 'metadata' -> 'annotations' ->> 'osgym.cua.ai/origin-warmpool'
        ),
              event.object -> 'metadata' -> 'annotations' ->> 'osgym.cua.ai/origin-warmpool'
          ) <> ''
          AND event.object -> 'spec' -> 'vmTemplate' ->> 'runtime' <> ''
          AND event.object -> 'status' ->> 'vmName' <> ''
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
        coalesce(
            event.object -> 'metadata' -> 'labels' ->> 'osgym.cua.ai/warmpool',
            event.object -> 'metadata' -> 'annotations' ->> 'osgym.cua.ai/origin-warmpool'
        ),
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
GRANT EXECUTE ON FUNCTION k8s_reporting.usage_sandbox_events(text, timestamptz, timestamptz) TO cyclops_usage_reader;
RESET ROLE;
