#!/usr/bin/env bash
set -euo pipefail

if [[ "${CYCLOPS_TEST_DATABASE_MIGRATOR_ISOLATED_CLUSTER:-}" != "1" ]]; then
  echo "set CYCLOPS_TEST_DATABASE_MIGRATOR_ISOLATED_CLUSTER=1 for a disposable local PostgreSQL cluster" >&2
  exit 2
fi
: "${CYCLOPS_TEST_DATABASE_ADMIN_URL:?set the PostgreSQL maintenance URL}"

admin_url="$CYCLOPS_TEST_DATABASE_ADMIN_URL"
seed="${CYCLOPS_TEST_DATABASE_SUFFIX:-${GITHUB_RUN_ID:-local}-${GITHUB_JOB:-postgres}-${GITHUB_RUN_ATTEMPT:-1}}"
token="$(printf '%s' "$seed" | sha256sum | cut -c1-16)"
migration_owner="cyclops_test_migrator_${token}"
database_name="cyclops_test_${token}"
migration_password="migration-${token}"
static_roles=(
  cyclops_app
  k8s_state_owner
  k8s_state_writer
  k8s_state_exporter
  k8s_query_tenant
  k8s_query_admin
  k8s_role_admin
  k8s_reporting_owner
  billing_meter_owner
  k8s_metabase
  cyclops_usage_reader
  cyclops_meter_writer
)

base_url="${admin_url%%\?*}"
query_suffix="${admin_url#"$base_url"}"
authority="${base_url%/*}"
authority="${authority#*://}"
authority="${authority##*@}"
scheme="${base_url%%://*}"

# Do not allow inherited libpq settings to override this validated maintenance URL.
for variable in "${!PG@}"; do
  unset "$variable"
done

# Keep the maintenance URL out of process arguments; psql reads libpq settings.
if ! connection_settings="$(CYCLOPS_TEST_DATABASE_ADMIN_URL="$admin_url" python3 - <<'PY'
import os
import shlex
from urllib.parse import parse_qs, unquote, urlparse

parsed = urlparse(os.environ["CYCLOPS_TEST_DATABASE_ADMIN_URL"])
if parsed.scheme not in {"postgres", "postgresql"} or not parsed.hostname:
    raise SystemExit("CYCLOPS_TEST_DATABASE_ADMIN_URL must be a PostgreSQL URL")
if parsed.hostname not in {"localhost", "127.0.0.1", "::1"}:
    raise SystemExit("CYCLOPS_TEST_DATABASE_ADMIN_URL host must be loopback")
if unquote(parsed.path.lstrip("/")) != "postgres":
    raise SystemExit("CYCLOPS_TEST_DATABASE_ADMIN_URL must use maintenance database postgres")

settings = {
    "PGHOST": parsed.hostname,
    "PGPORT": str(parsed.port or 5432),
    "PGUSER": unquote(parsed.username or "postgres"),
    "PGPASSWORD": unquote(parsed.password or ""),
    "PGDATABASE": "postgres",
}
sslmode = parse_qs(parsed.query).get("sslmode", [None])[0]
if sslmode:
    settings["PGSSLMODE"] = sslmode
for name, value in settings.items():
    print(f"export {name}={shlex.quote(value)}")
PY
)"; then
  exit 2
fi
eval "$connection_settings"
unset CYCLOPS_TEST_DATABASE_ADMIN_URL admin_url connection_settings

url_for() {
  local role="$1"
  local password="$2"
  printf '%s://%s:%s@%s/%s%s' "$scheme" "$role" "$password" "$authority" "$database_name" "$query_suffix"
}

migration_url="$(url_for "$migration_owner" "$migration_password")"
app_url="$(url_for cyclops_app cyclops-app-password)"
writer_url="$(url_for k8s_state_writer state-writer-password)"
exporter_url="$(url_for k8s_state_exporter state-exporter-password)"
role_admin_url="$(url_for k8s_role_admin role-admin-password)"
metabase_url="$(url_for k8s_metabase metabase-password)"
usage_url="$(url_for cyclops_usage_reader usage-reader-password)"
meter_url="$(url_for cyclops_meter_writer meter-writer-password)"

env_file="${CYCLOPS_TEST_DATABASE_ENV_FILE:-${GITHUB_ENV:-}}"
if [[ -z "$env_file" ]]; then
  echo "set CYCLOPS_TEST_DATABASE_ENV_FILE or GITHUB_ENV" >&2
  exit 2
fi

run_psql() {
  psql --set ON_ERROR_STOP=1 "$@"
}

if [[ "$(printf 'select rolsuper from pg_roles where rolname = current_user;\n' | run_psql -At)" != "t" ]]; then
  echo "CYCLOPS_TEST_DATABASE_ADMIN_URL user must be a true PostgreSQL superuser" >&2
  exit 2
fi

role_literals="'$migration_owner'"
for role in "${static_roles[@]}"; do
  role_literals+=", '$role'"
done
preexisting="$(printf "select exists (select 1 from pg_database where datname = '%s') or exists (select 1 from pg_roles where rolname in (%s));\n" "$database_name" "$role_literals" | run_psql -At)"
if [[ "$preexisting" == "t" ]]; then
  echo "refusing to modify pre-existing Cyclops test database or static roles" >&2
  exit 2
fi

created_migration_owner=0
created_database=0
created_static_roles=0
cleanup() {
  local status=$?
  trap - EXIT
  set +e
  if ((created_database)); then
    {
      printf "select pg_terminate_backend(pid) from pg_stat_activity where datname = '%s' and pid <> pg_backend_pid();\n" "$database_name"
      printf 'drop database "%s";\n' "$database_name"
    } | run_psql
  fi
  if ((created_static_roles)); then
    for role in "${static_roles[@]}"; do
      printf 'drop role "%s";\n' "$role" | run_psql
    done
  fi
  if ((created_migration_owner)); then
    printf 'drop role "%s";\n' "$migration_owner" | run_psql
  fi
  exit "$status"
}
trap cleanup EXIT

run_psql <<EOF_SQL
create role "${migration_owner}" login createrole nosuperuser nocreatedb password '${migration_password}';
EOF_SQL
created_migration_owner=1

run_psql <<EOF_SQL
create database "${database_name}" owner "${migration_owner}";
EOF_SQL
created_database=1

created_static_roles=1
MIGRATION_DATABASE_URL="$migration_url" \
DATABASE_URL="$app_url" \
STATE_WRITER_DATABASE_URL="$writer_url" \
STATE_EXPORTER_DATABASE_URL="$exporter_url" \
STATE_ROLE_ADMIN_DATABASE_URL="$role_admin_url" \
METABASE_DATABASE_URL="$metabase_url" \
USAGE_DATABASE_URL="$usage_url" \
METER_DATABASE_URL="$meter_url" \
go run ./cmd/db-migrate

cat >> "$env_file" <<EOF_ENV
CYCLOPS_TEST_RUNTIME_DATABASE_URL=${app_url}
CYCLOPS_TEST_MIGRATION_DATABASE_URL=${migration_url}
CYCLOPS_TEST_STATE_WRITER_DATABASE_URL=${writer_url}
CYCLOPS_TEST_STATE_ROLE_ADMIN_DATABASE_URL=${role_admin_url}
CYCLOPS_TEST_DATABASE_MIGRATOR_ISOLATED_CLUSTER=1
EOF_ENV

trap - EXIT
