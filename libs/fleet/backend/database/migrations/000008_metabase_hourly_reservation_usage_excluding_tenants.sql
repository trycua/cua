SET LOCAL ROLE k8s_reporting_owner;

CREATE VIEW k8s_reporting.hourly_reservation_usage_excluding_tenants
WITH (security_barrier = true, security_invoker = false) AS
SELECT
    collection.cluster_id,
    collection.hour_start,
    collection.hour_end,
    collection.covered_seconds,
    count(DISTINCT fact.sandbox_uid)::integer AS discovered_sandboxes,
    count(fact.fact_id)::bigint AS reservation_fact_count,
    coalesce(sum(fact.virtual_cpu_core_seconds), 0)::numeric AS virtual_cpu_core_seconds,
    coalesce(sum(fact.virtual_memory_byte_seconds), 0)::numeric AS virtual_memory_byte_seconds,
    coalesce(sum(fact.ready_seconds), 0)::numeric AS ready_seconds
FROM billing_meter.reservation_hour_collection_current AS collection
LEFT JOIN billing_meter.reservation_hour_current AS fact
  ON fact.cluster_id = collection.cluster_id
 AND fact.hour_start = collection.hour_start
 AND fact.hour_end = collection.hour_end
 AND fact.capsule_tenant NOT IN (
    'user-f039fe89-9b5f-43dc-8ccd-d100ae732246',
    'user-30a53246-881d-4f1a-8005-979f2a07933e'
 )
GROUP BY
    collection.cluster_id,
    collection.hour_start,
    collection.hour_end,
    collection.covered_seconds;

REVOKE ALL ON k8s_reporting.hourly_reservation_usage_excluding_tenants FROM PUBLIC;
GRANT SELECT ON k8s_reporting.hourly_reservation_usage_excluding_tenants TO k8s_metabase;

RESET ROLE;
