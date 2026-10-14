SET LOCAL ROLE k8s_reporting_owner;

CREATE VIEW k8s_reporting.hourly_reservation_usage
WITH (security_barrier = true, security_invoker = false) AS
SELECT
    collection.cluster_id,
    collection.hour_start,
    collection.hour_end,
    collection.covered_seconds,
    collection.discovered_sandboxes,
    count(fact.fact_id)::bigint AS reservation_fact_count,
    coalesce(sum(fact.virtual_cpu_core_seconds), 0)::numeric AS virtual_cpu_core_seconds,
    coalesce(sum(fact.virtual_memory_byte_seconds), 0)::numeric AS virtual_memory_byte_seconds,
    coalesce(sum(fact.ready_seconds), 0)::numeric AS ready_seconds
FROM billing_meter.reservation_hour_collection_current AS collection
LEFT JOIN billing_meter.reservation_hour_current AS fact
  ON fact.cluster_id = collection.cluster_id
 AND fact.hour_start = collection.hour_start
 AND fact.hour_end = collection.hour_end
GROUP BY
    collection.cluster_id,
    collection.hour_start,
    collection.hour_end,
    collection.covered_seconds,
    collection.discovered_sandboxes;

REVOKE ALL ON k8s_reporting.hourly_reservation_usage FROM PUBLIC;
GRANT SELECT ON k8s_reporting.hourly_reservation_usage TO k8s_metabase;

RESET ROLE;
