SET LOCAL ROLE billing_meter_owner;

GRANT USAGE ON SCHEMA billing_meter TO k8s_metabase;
GRANT SELECT ON ALL TABLES IN SCHEMA billing_meter TO k8s_metabase;
ALTER DEFAULT PRIVILEGES IN SCHEMA billing_meter GRANT SELECT ON TABLES TO k8s_metabase;

RESET ROLE;
