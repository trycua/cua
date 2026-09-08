#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace_dir="$repo_root/cyclops-cs"
generator="$workspace_dir/scripts/generate-sdk-bindings.sh"
compat_generator="$workspace_dir/scripts/generate-compat-sdk-bindings.sh"
compat_normalizer="$workspace_dir/scripts/normalize-compat-sdk-bindings.py"
bindings_dir="$workspace_dir/sdk-bindings"
temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-sdk-bindings-test.XXXXXX")"
regression_target_root="${CYCLOPS_SDK_BINDINGS_REGRESSION_TARGET_ROOT:-$workspace_dir/target/sdk-bindings-regression}"
mkdir -p "$regression_target_root"
if cargo_bin="$(command -v cargo)" && [ -n "$cargo_bin" ]; then
  :
else
  echo "error: cargo must be available on PATH" >&2
  exit 127
fi
resolver_manifest="$workspace_dir/bindgen-cli/Cargo.toml"
resolver_fixtures="$workspace_dir/bindgen-cli/tests/fixtures"
handwritten_file="$bindings_dir/python/tests/task10-harness-preserved.txt"
handwritten_directory="$(dirname "$handwritten_file")"
handwritten_directory_created=false
runtime_copy=""
cargo_config_directory="$workspace_dir/.cargo"
cargo_config_file="$cargo_config_directory/config.toml"
cargo_config_directory_created=false
cargo_config_active=false
compat_go_backup=""
compat_node_backup=""
compat_snapshots_mutated=false

restore_compat_snapshots() {
  [ "$compat_snapshots_mutated" = true ] || return 0
  [ -n "$compat_go_backup" ] && [ -f "$compat_go_backup" ] || return 1
  [ -n "$compat_node_backup" ] && [ -f "$compat_node_backup" ] || return 1
  cp "$compat_go_backup" "$go_schema_source"
  cp "$compat_node_backup" "$node_schema_source"
  compat_snapshots_mutated=false
}

cleanup_resources() {
  cleanup_directory="$1"
  restore_compat_snapshots
  if [ "$cargo_config_active" = true ] && [ -f "$cargo_config_file" ]; then
    rm -f "$cargo_config_file"
  fi
  if [ "$cargo_config_directory_created" = true ]; then
    # Trap cleanup must not override the harness result if the directory is nonempty.
    rmdir "$cargo_config_directory" 2>/dev/null || true  # lint-ignore: error-masking
  fi
  rm -rf "$cleanup_directory"
}

handle_exit() {
  status=$?
  cleanup_directory="$1"
  trap - EXIT HUP INT TERM
  set +e
  cleanup_resources "$cleanup_directory"
  exit "$status"
}

handle_signal() {
  cleanup_directory="$1"
  signal_status="$2"
  trap - EXIT HUP INT TERM
  set +e
  cleanup_resources "$cleanup_directory"
  exit "$signal_status"
}
trap 'handle_exit "$temporary_directory"' EXIT
trap 'handle_signal "$temporary_directory" 129' HUP
trap 'handle_signal "$temporary_directory" 130' INT
trap 'handle_signal "$temporary_directory" 143' TERM
mode_for() {
  if stat -f '%Lp' "$1" >/dev/null 2>&1; then
    stat -f '%Lp' "$1"
  else
    stat -c '%a' "$1"
  fi
}

fail() {
  echo "error: $1" >&2
  exit 1
}

expect_check_failure() {
  label="$1"
  if "$generator" --check > "$temporary_directory/$label.log" 2>&1; then
    fail "expected --check to reject $label"
  fi
}

expect_compat_signal_restore() {
  signal_name="$1"
  expected_status="$2"
  compat_cleanup_probe="$temporary_directory/compat-cleanup-$signal_name"
  mkdir "$compat_cleanup_probe"
  cp "$go_schema_source" "$compat_cleanup_probe/go"
  cp "$node_schema_source" "$compat_cleanup_probe/node"
  if (
    compat_go_backup="$compat_cleanup_probe/go"
    compat_node_backup="$compat_cleanup_probe/node"
    compat_snapshots_mutated=true
    trap 'handle_exit "$compat_cleanup_probe"' EXIT
    trap 'handle_signal "$compat_cleanup_probe" 129' HUP
    trap 'handle_signal "$compat_cleanup_probe" 130' INT
    trap 'handle_signal "$compat_cleanup_probe" 143' TERM
    printf '\n// compat cleanup %s probe\n' "$signal_name" >> "$go_schema_source"
    printf '\n// compat cleanup %s probe\n' "$signal_name" >> "$node_schema_source"
    kill -s "$signal_name" "$BASHPID"
  ); then
    actual_status=0
  else
    actual_status=$?
  fi
  if [ "$actual_status" -ne "$expected_status" ]; then
    fail "compat cleanup trap returned $actual_status for $signal_name, expected $expected_status"
  fi
  if grep -Fq -- "compat cleanup $signal_name probe" "$go_schema_source" || grep -Fq -- "compat cleanup $signal_name probe" "$node_schema_source"; then
    fail "compat cleanup trap did not restore snapshots after $signal_name"
  fi
}

tree_hash() {
  root="$1"
  (
    cd "$root"
    find -P . -print | LC_ALL=C sort | while IFS= read -r path; do
      if [ -L "$path" ]; then
        printf 'link %s %s\n' "$path" "$(readlink "$path")"
      elif [ -d "$path" ]; then
        printf 'directory %s %s\n' "$path" "$(mode_for "$path")"
      elif [ -f "$path" ]; then
        printf 'file %s %s ' "$path" "$(mode_for "$path")"
        cksum "$path"
      else
        printf 'other %s\n' "$path"
      fi
    done
  ) | cksum
}

find_cyclops_sdk_library() {
  metadata_output="$temporary_directory/cargo-metadata.json"
  build_messages="$temporary_directory/cargo-build-messages.json"
  if ! "$cargo_bin" metadata --locked --format-version 1 --no-deps --manifest-path "$workspace_dir/Cargo.toml" > "$metadata_output"; then
    cat "$metadata_output" >&2
    return 1
  fi
  if ! "$cargo_bin" build --locked --release --manifest-path "$workspace_dir/Cargo.toml" -p cyclops-sdk \
    --message-format=json-render-diagnostics > "$build_messages"; then
    cat "$build_messages" >&2
    return 1
  fi
  "$cargo_bin" run --quiet --locked --manifest-path "$resolver_manifest" --target "$host_triple" -- \
    resolve-cdylib --sdk-manifest "$workspace_dir/sdk/Cargo.toml" --metadata "$metadata_output" \
    --build-messages "$build_messages"
}
assert_no_harness_artifacts() {
  if [ -e "$handwritten_file" ] || [ -L "$handwritten_file" ]; then
    fail "harness fixture was not removed"
  fi
  if [ "$handwritten_directory_created" = true ]; then
    if [ -e "$handwritten_directory" ] || [ -L "$handwritten_directory" ]; then
      fail "harness-created test directory was not removed"
    fi
  fi
  [ -z "$runtime_copy" ] || [ ! -e "$runtime_copy" ] || fail "temporary Python runtime library was not removed"
}

assert_tree_unchanged() {
  expected_hash="$1"
  label="$2"
  actual_hash="$(tree_hash "$bindings_dir")"
  [ "$expected_hash" = "$actual_hash" ] || fail "$label changed sdk-bindings"
  assert_no_harness_artifacts
}

expect_transaction_failure() {
  point="$1"
  baseline_hash="$2"
  if CYCLOPS_SDK_BINDINGS_TEST_FAIL_TRANSACTION_POINT="$point" "$generator" \
    > "$temporary_directory/failure-$point.log" 2>&1; then
    fail "expected injected transaction failure at $point"
  fi
  assert_tree_unchanged "$baseline_hash" "transaction failure at $point"
  "$generator" --check
}

expect_transaction_signal() {
  point="$1"
  signal="$2"
  baseline_hash="$3"
  if CYCLOPS_SDK_BINDINGS_TEST_SIGNAL_TRANSACTION_POINT="$point:$signal" "$generator" \
    > "$temporary_directory/signal-$point-$signal.log" 2>&1; then
    fail "expected injected $signal at $point"
  fi
  assert_tree_unchanged "$baseline_hash" "transaction signal $signal at $point"
  "$generator" --check
}


expect_resolver_fixture() {
  fixture="$1"
  expected_library="$2"
  actual_library="$("$cargo_bin" run --quiet --locked --manifest-path "$resolver_manifest" \
    -- resolve-build-messages --package-id 'exact 0.1.0' \
    --build-messages "$resolver_fixtures/$fixture")" || fail "resolver rejected $fixture"
  [ "$actual_library" = "$expected_library" ] || fail "resolver selected the wrong library for $fixture"
}

expect_resolver_fixture_failure() {
  fixture="$1"
  expected_error="$2"
  if "$cargo_bin" run --quiet --locked --manifest-path "$resolver_manifest" -- \
    resolve-build-messages --package-id 'exact 0.1.0' \
    --build-messages "$resolver_fixtures/$fixture" > "$temporary_directory/$fixture.log" 2>&1; then
    fail "resolver unexpectedly accepted $fixture"
  fi
  grep -F "$expected_error" "$temporary_directory/$fixture.log" >/dev/null || \
    fail "resolver error for $fixture did not mention $expected_error"
}

run_resolver_fixture_regressions() {
  expect_resolver_fixture 'resolver-valid-reordered.jsonl' '/tmp/exact/libcyclops_sdk.so'
  expect_resolver_fixture_failure 'resolver-multiple-artifacts.jsonl' 'multiple matching compiler-artifact records'
  expect_resolver_fixture_failure 'resolver-multiple-libraries.jsonl' 'multiple host library filenames'
  expect_resolver_fixture_failure 'resolver-no-match.jsonl' 'no matching compiler-artifact record'
}


host_triple="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$host_triple" ] || fail "could not determine the Rust host target"

run_resolver_fixture_regressions

run_target_layout_regressions() {
  shared_target_directory="$regression_target_root/shared"
  build_target_directory="$shared_target_directory"
  CARGO_TARGET_DIR="$build_target_directory" CARGO_BUILD_TARGET="$host_triple" "$generator"
  CARGO_TARGET_DIR="$build_target_directory" CARGO_BUILD_TARGET="$host_triple" "$generator" --check

  if [ -e "$cargo_config_file" ] || [ -L "$cargo_config_file" ]; then
    echo "skipping workspace Cargo config target regression: $cargo_config_file already exists" >&2
    return 0
  fi
  if [ ! -d "$cargo_config_directory" ]; then
    mkdir -p "$cargo_config_directory"
    cargo_config_directory_created=true
  fi
  cat > "$cargo_config_file" <<EOF_CONFIG
[build]
target = "$host_triple"
EOF_CONFIG
  cargo_config_active=true

  config_target_directory="$shared_target_directory"
  CARGO_TARGET_DIR="$config_target_directory" "$generator"
  CARGO_TARGET_DIR="$config_target_directory" "$generator" --check

  rm "$cargo_config_file"
  cargo_config_active=false
  if [ "$cargo_config_directory_created" = true ]; then
    rmdir "$cargo_config_directory"
    cargo_config_directory_created=false
  fi
}
run_target_layout_regressions
"$generator"
run_language_linkage_audits() {
  python3 - "$bindings_dir" <<'LANGUAGE_AUDIT'
import re
import sys
from pathlib import Path

bindings = Path(sys.argv[1])
ruby_sdk = (bindings / "ruby/cyclops_sdk/sdk.rb").read_text(encoding="utf-8")
ruby_schema = (bindings / "ruby/cyclops_sdk/schema.rb").read_text(encoding="utf-8")
ruby_facade = (bindings / "ruby/cyclops_sdk.rb").read_text(encoding="utf-8")
if "CyclopsSdkSchema.const_get(:RustBufferStream, false)" not in ruby_facade:
    raise AssertionError("Ruby facade does not reflectively bridge the private schema stream")
if "CyclopsSdkSchema::RustBufferStream" in ruby_facade:
    raise AssertionError("Ruby facade directly references a private schema constant")
for prefix in ("check_lower", "read", "write"):
    references = set(re.findall(rf"\b{prefix}_Type[A-Za-z0-9_]+", ruby_sdk))
    sdk_definitions = set(re.findall(rf"^\s*def (?:self\.)?({prefix}_Type[A-Za-z0-9_]+)", ruby_sdk, re.MULTILINE))
    schema_definitions = set(re.findall(rf"^\s*def (?:self\.)?({prefix}_Type[A-Za-z0-9_]+)", ruby_schema, re.MULTILINE))
    external = references - sdk_definitions
    unresolved = sorted(external - schema_definitions)
    if unresolved:
        raise AssertionError(f"Ruby SDK has unresolved schema methods: {unresolved}")
    unbridged = sorted(method for method in external if not re.search(rf"^\s+{re.escape(method)}$", ruby_facade, re.MULTILINE))
    if unbridged:
        raise AssertionError(f"Ruby facade does not bridge schema methods: {unbridged}")

kotlin_sdk = (bindings / "kotlin/ai/cua/cyclops/sdk/fleet_sdk.kt").read_text(encoding="utf-8")
kotlin_schema = (bindings / "kotlin/ai/cua/cyclops/sdk/schema/cyclops_sdk_schema.kt").read_text(encoding="utf-8")
if "package ai.cua.cyclops.sdk" not in kotlin_sdk or "package ai.cua.cyclops.sdk.schema" not in kotlin_schema:
    raise AssertionError("Kotlin components are not generated into distinct configured packages")
if "var `spec`: OsGymSandboxWarmPoolSpec" not in kotlin_sdk or "data class OsGymSandboxWarmPoolSpec" not in kotlin_schema:
    raise AssertionError("Kotlin OsGymSandboxWarmPoolSpec external type is not linked through the schema package")

swift_sdk = (bindings / "swift/CyclopsSdk.swift").read_text(encoding="utf-8")
swift_schema = (bindings / "swift/CyclopsSdkSchema.swift").read_text(encoding="utf-8")
if "public var spec: OsGymSandboxWarmPoolSpec" not in swift_sdk or "public struct OsGymSandboxWarmPoolSpec" not in swift_schema:
    raise AssertionError("Swift OsGymSandboxWarmPoolSpec external type is not available for one-module compilation")
if "public struct OsGymSandboxWarmPoolSpec: Equatable, Hashable {" not in swift_schema:
    raise AssertionError("Swift OsGymSandboxWarmPoolSpec is missing Equatable and Hashable conformances")
# The legacy PoolSpec carried Rust-backed ==/hash methods (it embedded a
# PreservedJson object); no surviving schema record does, so records that
# embed objects must instead NOT claim the conformances they cannot satisfy.
if "public struct VmTemplate: Equatable" in swift_schema:
    raise AssertionError("Swift VmTemplate (object-bearing record) must not claim Equatable")
LANGUAGE_AUDIT
}
run_language_linkage_audits
initial_hash="$(tree_hash "$bindings_dir")"
if [ ! -d "$handwritten_directory" ]; then
  mkdir -p "$handwritten_directory"
  handwritten_directory_created=true
fi
printf 'handwritten test fixture\n' > "$handwritten_file"
"$generator"
cmp -s "$handwritten_file" <(printf 'handwritten test fixture\n') || fail "normal generation removed handwritten test fixture"
rm "$handwritten_file"
if [ "$handwritten_directory_created" = true ]; then
  rmdir "$handwritten_directory"
fi
assert_tree_unchanged "$initial_hash" "handwritten fixture cleanup"
"$generator" --check

runtime_library="$(find_cyclops_sdk_library)" || fail "could not read a cyclops-sdk cdylib from Cargo compiler-artifact output"
[ -f "$runtime_library" ] || fail "Cargo reported a missing cyclops-sdk cdylib: $runtime_library"
runtime_copy="$bindings_dir/python/fleet_sdk/$(basename "$runtime_library")"
cp "$runtime_library" "$runtime_copy"
PYTHONPATH="$bindings_dir/python" python3 - "$bindings_dir/python/fleet_sdk/_sdk.py" <<'PYTHON_SMOKE'
import asyncio
import json
import re
import sys

import fleet_sdk

sdk_source = open(sys.argv[1], encoding="utf-8").read()
required_private_exports = set(re.findall(r"\bcyclops_sdk\.(_[A-Za-z_][A-Za-z0-9_]*)", sdk_source))
missing = sorted(required_private_exports.difference(vars(fleet_sdk)))
if missing:
    raise AssertionError(f"missing schema-private SDK dependencies: {missing}")
public_exports = set(fleet_sdk.__all__)
leaked = sorted(required_private_exports.intersection(public_exports))
if leaked:
    raise AssertionError(f"schema-private dependencies leaked into __all__: {leaked}")

class CallbackHttpClient(fleet_sdk.HttpClient):
    def __init__(self):
        self.requests = []

    async def execute(self, request):
        self.requests.append(request)
        if request.url.endswith("/protocol/openid-connect/token"):
            body = {"access_token": "offline-token", "expires_in": 3600}
        elif request.url.endswith("/api/namespaces"):
            body = {"metadata": {"name": "default"}}
        elif request.url.endswith("/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxwarmpools"):
            body = {
                "apiVersion": "osgym.cua.ai/v1alpha1",
                "kind": "OSGymSandboxWarmPool",
                "metadata": {"namespace": "default", "name": "offline-pool"},
                "spec": {
                    "replicas": 1,
                    "sandboxTemplateRef": {"name": "default"},
                },
            }
        else:
            raise AssertionError(f"unexpected callback request: {request.method} {request.url}")
        return fleet_sdk.HttpResponse(status=201, headers=[], body=json.dumps(body).encode())

async def smoke_create_pool():
    transport = CallbackHttpClient()
    client = fleet_sdk.CyclopsClient.connect(
        fleet_sdk.CyclopsConfiguration(
            base_url="https://cyclops.invalid",
            token_url="https://keycloak.invalid/realms/offline/protocol/openid-connect/token",
            credentials=fleet_sdk.CyclopsCredentials("client-id", "client-secret"),
            pool_poll_interval_ms=1,
            pool_poll_limit=1,
            claim_poll_interval_ms=1,
            claim_poll_limit=1,
        ),
        transport,
    )
    spec = fleet_sdk.OsGymSandboxWarmPoolSpecBuilder().replicas(1).sandbox_template_ref(
        fleet_sdk.SandboxTemplateRefBuilder().name("default").build()
    ).build()
    request = fleet_sdk.CreatePoolRequestBuilder().namespace("default").spec(spec).build()
    assert type(spec) is fleet_sdk.OsGymSandboxWarmPoolSpec
    assert type(request) is fleet_sdk.CreatePoolRequest
    assert spec.autoscaling is None
    pool = await client.create_pool(request)
    assert pool.metadata.name == "offline-pool"
    assert any(request.url.endswith("/osgymsandboxwarmpools") for request in transport.requests)

asyncio.run(smoke_create_pool())
PYTHON_SMOKE
rm "$runtime_copy"
runtime_copy=""
"$generator" --check
python_sdk_source="$bindings_dir/python/fleet_sdk/_sdk.py"
python_schema_source="$bindings_dir/python/fleet_sdk/_schema.py"
kotlin_sdk_source="$bindings_dir/kotlin/ai/cua/cyclops/sdk/fleet_sdk.kt"
kotlin_schema_source="$bindings_dir/kotlin/ai/cua/cyclops/sdk/schema/cyclops_sdk_schema.kt"
swift_sdk_source="$bindings_dir/swift/CyclopsSdk.swift"
swift_schema_source="$bindings_dir/swift/CyclopsSdkSchema.swift"
ruby_sdk_source="$bindings_dir/ruby/cyclops_sdk/sdk.rb"
ruby_schema_source="$bindings_dir/ruby/cyclops_sdk/schema.rb"
node_sdk_source="$bindings_dir/ts-uniffi/fleet_sdk.ts"
node_schema_source="$bindings_dir/ts-uniffi/cyclops_sdk_schema.ts"
browser_sdk_source="$bindings_dir/ts-uniffi-browser/ts/fleet_sdk.ts"
browser_schema_source="$bindings_dir/ts-uniffi-browser/ts/cyclops_sdk_schema.ts"
go_sdk_source="$bindings_dir/go-uniffi/fleet_sdk/fleet_sdk.go"
go_schema_source="$bindings_dir/go-uniffi/cyclops_sdk_schema/cyclops_sdk_schema.go"

compat_go_backup="$temporary_directory/compat-go-backup"
compat_node_backup="$temporary_directory/compat-node-backup"
compat_raw_go="$temporary_directory/compat-raw.go"
compat_raw_node="$temporary_directory/compat-raw.ts"
cp "$go_schema_source" "$compat_go_backup"
cp "$node_schema_source" "$compat_node_backup"

grep -Fq -- 'required_go_generator_version="uniffi-bindgen 0.7.1+v0.31.0"' "$compat_generator" || fail "compat generator does not pin uniffi-bindgen-go"
grep -Fq -- "CARGO_PROFILE_DEV_DEBUG=0 cargo install --debug --git https://github.com/NordSecurity/uniffi-bindgen-go.git --tag 'v0.7.1+v0.31.0' --locked uniffi-bindgen-go" "$compat_generator" || fail "compat generator does not document the resource-safe install command"
grep -Fq -- 'command -v gofmt' "$compat_generator" || fail "compat generator does not require gofmt"
grep -Fq -- 'gofmt is required to normalize compatibility Go bindings' "$compat_generator" || fail "compat generator does not explain a missing gofmt"
grep -Fq -- 'command -v uniffi-bindgen-go' "$compat_generator" || fail "compat generator does not reject a missing uniffi-bindgen-go"
grep -Fq -- "actual_go_generator_version=\"\$(uniffi-bindgen-go --version)\"" "$compat_generator" || fail "compat generator does not validate uniffi-bindgen-go version"
"$compat_generator" --check

node - "$node_schema_source" <<'NODE'
const fs = require("node:fs");

const source = fs.readFileSync(process.argv[2], "utf8");
const declarations = new Set(
  [...source.matchAll(/^const\s+(FfiConverterType[A-Za-z0-9_]+)\s*=/gm)].map(
    (match) => match[1],
  ),
);
const references = new Set(
  [...source.matchAll(/\b(FfiConverterType[A-Za-z0-9_]+)\b/g)].map(
    (match) => match[1],
  ),
);
const missing = [...references].filter((reference) => !declarations.has(reference));
if (!declarations.has("FfiConverterTypeJsonValueError")) {
  throw new Error("Node compatibility snapshot omits FfiConverterTypeJsonValueError");
}
if (!declarations.has("FfiConverterTypeSchemaBuildError")) {
  throw new Error("Node compatibility snapshot omits FfiConverterTypeSchemaBuildError");
}
if (missing.length > 0) {
  throw new Error(`Node compatibility snapshot has unresolved converters: ${missing.join(", ")}`);
}
NODE

compat_cleanup_probe="$temporary_directory/compat-cleanup-probe"
mkdir "$compat_cleanup_probe"
cp "$go_schema_source" "$compat_cleanup_probe/go"
cp "$node_schema_source" "$compat_cleanup_probe/node"
if (
  compat_go_backup="$compat_cleanup_probe/go"
  compat_node_backup="$compat_cleanup_probe/node"
  compat_snapshots_mutated=true
  trap 'handle_exit "$compat_cleanup_probe"' EXIT
  trap 'handle_signal "$compat_cleanup_probe" 129' HUP
  trap 'handle_signal "$compat_cleanup_probe" 130' INT
  trap 'handle_signal "$compat_cleanup_probe" 143' TERM
  printf '\n// compat cleanup probe\n' >> "$go_schema_source"
  printf '\n// compat cleanup probe\n' >> "$node_schema_source"
  exit 97
); then
  fail "compat cleanup probe unexpectedly succeeded"
fi
if grep -Fq -- 'compat cleanup probe' "$go_schema_source" || grep -Fq -- 'compat cleanup probe' "$node_schema_source"; then
  fail "compat cleanup trap did not restore snapshots after a failing subprocess"
fi
expect_compat_signal_restore INT 130
expect_compat_signal_restore TERM 143
cp "$go_schema_source" "$compat_raw_go"
cp "$node_schema_source" "$compat_raw_node"
cat >> "$compat_raw_go" <<'EOF_COMPAT_GO'

// compat raw fixture
type CompatFixtureBuilder struct{}

func (CompatFixtureBuilder) Build() {}

type CompatFixtureRecordBuilderMetadata struct{}
EOF_COMPAT_GO
cat >> "$compat_raw_node" <<'EOF_COMPAT_NODE'

// compat raw fixture
export class CompatFixtureBuilder {
    build(): void {}
}
export const CompatFixtureBuildError = Object.freeze({});
const FfiConverterTypeCompatFixtureBuildError = Object.freeze({});
export class CompatFixtureRecordBuilderMetadata {}
const FfiConverterTypeCompatFixtureRecordBuilderMetadata = Object.freeze({});
function compatFixtureChecksums() {
    if (nativeModule().uniffi_compat_record_builder_metadata() !== 1) {
        throw new Error("record builder metadata");
    }
}
EOF_COMPAT_NODE
compat_snapshots_mutated=true
if ! "$compat_normalizer" --raw-go "$compat_raw_go" --raw-node "$compat_raw_node"; then
  restore_compat_snapshots
  fail "compat normalizer rejected the raw fixture"
fi
if ! grep -Fq -- 'compat raw fixture' "$go_schema_source" || ! grep -Fq -- 'compat raw fixture' "$node_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer did not derive snapshots from raw fixture output"
fi
if grep -Fq -- 'CompatFixtureBuilder' "$go_schema_source" || grep -Fq -- 'CompatFixtureBuilder' "$node_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer retained builder ABI from raw fixture output"
fi
if ! grep -Fq -- 'CompatFixtureRecordBuilderMetadata' "$go_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer removed interior Builder Go fixture symbols"
fi
if ! grep -Fq -- 'CompatFixtureBuildError' "$node_schema_source" || ! grep -Fq -- 'FfiConverterTypeCompatFixtureBuildError' "$node_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer removed non-builder BuildError fixture symbols"
fi
if ! grep -Fq -- 'CompatFixtureRecordBuilderMetadata' "$node_schema_source" || ! grep -Fq -- 'FfiConverterTypeCompatFixtureRecordBuilderMetadata' "$node_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer removed interior Builder fixture symbols"
fi
if ! grep -Fq -- 'uniffi_compat_record_builder_metadata' "$node_schema_source"; then
  restore_compat_snapshots
  fail "compat normalizer removed an interior builder snake-case symbol"
fi
if ! "$compat_normalizer" --raw-go "$compat_raw_go" --raw-node "$compat_raw_node" --check; then
  restore_compat_snapshots
  fail "compat normalizer does not reproduce fixture output"
fi
restore_compat_snapshots

compat_snapshots_mutated=true
printf '\n// stale manual compatibility edit\n' >> "$go_schema_source"
if "$compat_generator" --check > "$temporary_directory/compat-stale-go.log" 2>&1; then
  restore_compat_snapshots
  fail "compat --check accepted a stale manual Go snapshot edit"
fi
restore_compat_snapshots
compat_snapshots_mutated=true
printf '\n// stale manual compatibility edit\n' >> "$node_schema_source"
if "$compat_generator" --check > "$temporary_directory/compat-stale-node.log" 2>&1; then
  restore_compat_snapshots
  fail "compat --check accepted a stale manual Node snapshot edit"
fi
restore_compat_snapshots
"$compat_generator" --check

for separate_binding in "$node_sdk_source" "$node_schema_source" "$go_sdk_source" "$go_schema_source"; do
  if grep -Fq -- "Builder" "$separate_binding"; then
    fail "separately generated Go/Node binding unexpectedly contains authoritative builders: $separate_binding"
  fi
done

for browser_builder in \
  VmTemplateBuilder \
  WarmPoolAutoscalingBuilder \
  CreatePoolRequestBuilder \
  CyclopsTokenProviderConfigurationBuilder \
  CreateClaimRequestBuilder \
  CreateSignedServiceUrlRequestBuilder \
  CreateUserApiKeyRequestBuilder \
  TemplateBuilder; do
  if ! grep -Fq -- "class $browser_builder" "$browser_schema_source" && \
    ! grep -Fq -- "class $browser_builder" "$browser_sdk_source"; then
    fail "Browser/WASM bindings omit $browser_builder"
  fi
done
grep -Fq -- "class VmTemplateBuilder" "$python_schema_source" || fail "Python bindings omit VmTemplateBuilder"
grep -Fq -- "class CreatePoolRequestBuilder" "$python_sdk_source" || fail "Python bindings omit CreatePoolRequestBuilder"
grep -Fq -- "class CreateSignedServiceUrlRequestBuilder" "$python_sdk_source" || fail "Python bindings omit CreateSignedServiceUrlRequestBuilder"
grep -Fq -- "open class VmTemplateBuilder" "$kotlin_schema_source" || fail "Kotlin bindings omit VmTemplateBuilder"
grep -Fq -- "open class CreatePoolRequestBuilder" "$kotlin_sdk_source" || fail "Kotlin bindings omit CreatePoolRequestBuilder"
grep -Fq -- "open class CreateSignedServiceUrlRequestBuilder" "$kotlin_sdk_source" || fail "Kotlin bindings omit CreateSignedServiceUrlRequestBuilder"
grep -Fq -- "open class VmTemplateBuilder" "$swift_schema_source" || fail "Swift bindings omit VmTemplateBuilder"
grep -Fq -- "open class CreatePoolRequestBuilder" "$swift_sdk_source" || fail "Swift bindings omit CreatePoolRequestBuilder"
grep -Fq -- "open class CreateSignedServiceUrlRequestBuilder" "$swift_sdk_source" || fail "Swift bindings omit CreateSignedServiceUrlRequestBuilder"
grep -Fq -- "class VmTemplateBuilder" "$ruby_schema_source" || fail "Ruby bindings omit VmTemplateBuilder"
grep -Fq -- "class CreatePoolRequestBuilder" "$ruby_sdk_source" || fail "Ruby bindings omit CreatePoolRequestBuilder"
grep -Fq -- "class CreateSignedServiceUrlRequestBuilder" "$ruby_sdk_source" || fail "Ruby bindings omit CreateSignedServiceUrlRequestBuilder"
grep -Fq -- "alloc_from_TypeOSGymSandboxTemplateSpec" "$bindings_dir/ruby/cyclops_sdk.rb" || \
  fail "Ruby facade omits cross-component record allocation adapter"
if grep -Fq -- "execute_authenticated" "$python_sdk_source"; then fail "Python bindings export execute_authenticated"; fi
if grep -Fq -- "executeAuthenticated" "$kotlin_sdk_source"; then fail "Kotlin bindings export executeAuthenticated"; fi
if grep -Fq -- "executeAuthenticated" "$swift_sdk_source"; then fail "Swift bindings export executeAuthenticated"; fi
if grep -Fq -- "execute_authenticated" "$ruby_sdk_source"; then fail "Ruby bindings export execute_authenticated"; fi
if grep -Fq -- "executeAuthenticated" "$node_sdk_source"; then fail "Node bindings export executeAuthenticated"; fi
if grep -Fq -- "ExecuteAuthenticated" "$go_sdk_source"; then fail "Go bindings export ExecuteAuthenticated"; fi
for method in list_namespaces list_user_api_keys create_user_api_key delete_user_api_key; do
  grep -Fq -- "async def $method" "$python_sdk_source" || fail "Python bindings omit $method"
done
for method in listNamespaces listUserApiKeys createUserApiKey deleteUserApiKey; do
  grep -Fq -- "suspend fun \`$method\`" "$kotlin_sdk_source" || fail "Kotlin bindings omit $method"
  grep -Fq -- "func $method" "$swift_sdk_source" || fail "Swift bindings omit $method"
done
for method in list_namespaces list_user_api_keys create_user_api_key delete_user_api_key; do
  grep -Fq -- "def $method" "$ruby_sdk_source" || fail "Ruby bindings omit $method"
done
for method in listNamespaces listUserApiKeys createUserApiKey deleteUserApiKey; do
  grep -Fq -- "$method(" "$node_sdk_source" || fail "Node bindings omit $method"
done
for method in ListNamespaces ListUserApiKeys CreateUserApiKey DeleteUserApiKey; do
  grep -Fq -- "$method(" "$go_sdk_source" || fail "Go bindings omit $method"
done
grep -Fq -- "creation_timestamp:typing.Optional[str]" "$python_sdk_source" || fail "Python bindings omit creation_timestamp"
grep -Fq -- "var \`creationTimestamp\`: kotlin.String?" "$kotlin_sdk_source" || fail "Kotlin bindings omit creationTimestamp"
grep -Fq -- "public var creationTimestamp: String?" "$swift_sdk_source" || fail "Swift bindings omit creationTimestamp"
grep -Fq -- "attr_reader :namespace, :name, :labels, :creation_timestamp" "$ruby_sdk_source" || fail "Ruby bindings omit creation_timestamp"
grep -Fq -- "creationTimestamp?: string" "$node_sdk_source" || fail "Node bindings omit creationTimestamp"
grep -Fq -- "CreationTimestamp *string" "$go_sdk_source" || fail "Go bindings omit CreationTimestamp"
for symbol in SignedServiceUrl SignedServiceUrlsUnavailable; do
  grep -Fq -- "$symbol" "$browser_sdk_source" || fail "Browser/WASM bindings omit $symbol"
  grep -Fq -- "$symbol" "$python_sdk_source" || fail "Python bindings omit $symbol"
  grep -Fq -- "$symbol" "$kotlin_sdk_source" || fail "Kotlin bindings omit $symbol"
  grep -Fq -- "$symbol" "$swift_sdk_source" || fail "Swift bindings omit $symbol"
  grep -Fq -- "$symbol" "$ruby_sdk_source" || fail "Ruby bindings omit $symbol"
done
for method in createSignedServiceUrl listSignedServiceUrls revokeSignedServiceUrl; do
  grep -Fq -- "$method(" "$browser_sdk_source" || fail "Browser/WASM bindings omit $method"
  grep -Fq -- "suspend fun \`$method\`" "$kotlin_sdk_source" || fail "Kotlin bindings omit $method"
  grep -Fq -- "func $method" "$swift_sdk_source" || fail "Swift bindings omit $method"
done
for method in create_signed_service_url list_signed_service_urls revoke_signed_service_url; do
  grep -Fq -- "async def $method" "$python_sdk_source" || fail "Python bindings omit $method"
  grep -Fq -- "def $method" "$ruby_sdk_source" || fail "Ruby bindings omit $method"
done
for legacy_symbol in PublicUrl public_url publicUrl; do
  for binding in "$browser_sdk_source" "$python_sdk_source" "$kotlin_sdk_source" "$swift_sdk_source" "$ruby_sdk_source"; do
    if grep -Fq -- "$legacy_symbol" "$binding"; then
      fail "authoritative binding exports legacy public URL symbol $legacy_symbol: $binding"
    fi
  done
done

node - "$python_sdk_source" "$kotlin_sdk_source" "$swift_sdk_source" "$ruby_sdk_source" "$browser_sdk_source" <<'NODE'
const fs = require("node:fs");

const [pythonPath, kotlinPath, swiftPath, rubyPath, browserPath] = process.argv.slice(2);
const sources = Object.fromEntries(
  Object.entries({ python: pythonPath, kotlin: kotlinPath, swift: swiftPath, ruby: rubyPath, browser: browserPath })
    .map(([language, path]) => [language, fs.readFileSync(path, "utf8")]),
);
const signedFields = ["id", "namespace", "claim", "sandbox", "service", "label", "url", "created_at", "expires_at", "revoked_at"];
const camelFields = ["id", "namespace", "claim", "sandbox", "service", "label", "url", "createdAt", "expiresAt", "revokedAt"];

function fail(message) { throw new Error(`signed service URL structural guard: ${message}`); }
function text(source, value, label) { if (!source.includes(value)) fail(`${label}: missing ${value}`); }
function regex(source, value, label) { if (!value.test(source)) fail(`${label}: pattern is missing`); }
function all(source, values, label) { values.forEach((value) => text(source, value, label)); }
function after(source, marker, label) { const index = source.indexOf(marker); if (index < 0) fail(`${label}: start is missing`); return index; }
function nextIndex(source, from, patterns) {
  const positions = patterns.map((pattern) => source.indexOf(pattern, from)).filter((index) => index >= 0);
  return positions.length ? Math.min(...positions) : source.length;
}
function pythonClass(source, marker, label) {
  const start = after(source, marker, label);
  return source.slice(start, nextIndex(source, start + marker.length, ["\nclass ", "\n# "]));
}
function rubyClass(source, marker, label) {
  const start = after(source, marker, label);
  return source.slice(start, nextIndex(source, start + marker.length, ["\nclass ", "\n  class "]));
}
function braceBlock(source, marker, label) {
  const start = after(source, marker, label);
  const open = source.indexOf("{", start + marker.length);
  if (open < 0) fail(`${label}: opening brace is missing`);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  fail(`${label}: closing brace is missing`);
}
function member(source, marker, nextPatterns, label) {
  const start = after(source, marker, label);
  return source.slice(start, nextIndex(source, start + marker.length, nextPatterns));
}
function braceMember(source, marker, openingMarker, label) {
  const start = after(source, marker, label);
  const opening = source.indexOf(openingMarker, start);
  if (opening < 0) fail(`${label}: method opening is missing`);
  const open = opening + openingMarker.lastIndexOf("{");
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  fail(`${label}: method closing brace is missing`);
}
function identifiers(source) { return source.match(/[A-Za-z_][A-Za-z0-9_]*/g) ?? []; }
function noLegacy(source, label) {
  const legacy = identifiers(source).filter((identifier) => /PublicUrl|public_url|publicUrl/.test(identifier));
  if (legacy.length) fail(`${label}: legacy identifier ${legacy[0]} is present`);
}

function assertPython(source) {
  const record = pythonClass(source, "class SignedServiceUrl:", "Python SignedServiceUrl record");
  signedFields.forEach((field) => text(record, `self.${field}`, "Python SignedServiceUrl fields"));
  const converter = pythonClass(source, "class _UniffiFfiConverterTypeSignedServiceUrl(", "Python SignedServiceUrl converter");
  all(converter, ["def read(buf):", "return SignedServiceUrl(", "def write(value, buf):"], "Python SignedServiceUrl converter");
  signedFields.forEach((field) => { text(converter, `${field}=`, "Python SignedServiceUrl decoder"); text(converter, `value.${field}`, "Python SignedServiceUrl encoder"); });
  const client = pythonClass(source, "class CyclopsClient(CyclopsClientProtocol):", "Python CyclopsClient");
  const starts = ["    async def create_signed_service_url", "    async def list_signed_service_urls", "    async def revoke_signed_service_url"];
  const create = member(client, starts[0], ["\n    async def "], "Python create");
  all(create, ["-> SignedServiceUrl:", "_UniffiFfiConverterTypeCreateSignedServiceUrlRequest.lower", "_UniffiFfiConverterTypeSignedServiceUrl.lift", "uniffi_cyclops_sdk_fn_method_cyclopsclient_create_signed_service_url", "_UniffiFfiConverterTypeSdkError"], "Python concrete create");
  const list = member(client, starts[1], ["\n    async def "], "Python list");
  all(list, ["-> typing.List[SignedServiceUrl]:", "_UniffiFfiConverterTypeSandbox.lower", "_UniffiFfiConverterSequenceTypeSignedServiceUrl.lift", "uniffi_cyclops_sdk_fn_method_cyclopsclient_list_signed_service_urls", "_UniffiFfiConverterTypeSdkError"], "Python concrete list");
  const revoke = member(client, starts[2], ["\n    async def "], "Python revoke");
  all(revoke, ["-> None:", "_UniffiFfiConverterTypeSignedServiceUrl.check_lower", "uniffi_cyclops_sdk_fn_method_cyclopsclient_revoke_signed_service_url", "ffi_cyclops_sdk_rust_future_poll_void", "_UniffiFfiConverterTypeSdkError"], "Python concrete revoke");
  const builder = pythonClass(source, "class CreateSignedServiceUrlRequestBuilder(", "Python signed URL builder");
  const setterFfi = [["sandbox", "createsignedserviceurlrequestbuilder_sandbox"], ["service", "createsignedserviceurlrequestbuilder_service"], ["expires_in_seconds", "createsignedserviceurlrequestbuilder_expires_in_seconds"], ["label", "createsignedserviceurlrequestbuilder_label"]];
  setterFfi.forEach(([name, ffi]) => text(member(builder, `    def ${name}(`, ["\n    def "], `Python builder ${name}`), ffi, `Python builder ${name}`));
  all(member(builder, "    def build(", ["\n    def "], "Python builder build"), ["-> CreateSignedServiceUrlRequest:", "uniffi_cyclops_sdk_fn_method_createsignedserviceurlrequestbuilder_build", "_UniffiFfiConverterTypeCreateSignedServiceUrlRequest.lift", "_UniffiFfiConverterTypeSdkBuildError"], "Python builder build");
  const errors = pythonClass(source, "class _UniffiFfiConverterTypeSdkError(", "Python SdkError converter");
  regex(errors, /if variant == 7:\s+return SdkError\.SignedServiceUrlsUnavailable\(/m, "Python SignedServiceUrlsUnavailable decoder ordinal");
  regex(errors, /if isinstance\(value, SdkError\.SignedServiceUrlsUnavailable\):\s+buf\.write_i32\(7\)/m, "Python SignedServiceUrlsUnavailable encoder ordinal");
  const buildErrors = pythonClass(source, "class _UniffiFfiConverterTypeSdkBuildError(", "Python SdkBuildError converter");
  regex(buildErrors, /if variant == 1:\s+return SdkBuildError\.MissingRequiredField\(/m, "Python SdkBuildError decoder");
  noLegacy(source, "Python");
}

function assertKotlin(source) {
  const record = source.slice(after(source, "data class SignedServiceUrl", "Kotlin SignedServiceUrl record"), after(source, "public object FfiConverterTypeSignedServiceUrl", "Kotlin SignedServiceUrl converter"));
  camelFields.forEach((field) => text(record, `\`${field}\``, "Kotlin SignedServiceUrl fields"));
  const converter = braceBlock(source, "public object FfiConverterTypeSignedServiceUrl", "Kotlin SignedServiceUrl converter");
  all(converter, ["override fun read", "return SignedServiceUrl(", "override fun write"], "Kotlin SignedServiceUrl converter");
  camelFields.forEach((field) => text(converter, `value.\`${field}\``, "Kotlin SignedServiceUrl encoder"));
  const client = braceBlock(source, "open class CyclopsClient:", "Kotlin CyclopsClient");
  const methods = [["override suspend fun `createSignedServiceUrl`", ") : SignedServiceUrl {", "create_signed_service_url", "FfiConverterTypeCreateSignedServiceUrlRequest.lower", "FfiConverterTypeSignedServiceUrl.lift"], ["override suspend fun `listSignedServiceUrls`", ") : List<SignedServiceUrl> {", "list_signed_service_urls", "FfiConverterTypeSandbox.lower", "FfiConverterSequenceTypeSignedServiceUrl.lift"], ["override suspend fun `revokeSignedServiceUrl`", ") {", "revoke_signed_service_url", "FfiConverterTypeSignedServiceUrl.lower", "ffi_cyclops_sdk_rust_future_poll_void"]];
  methods.forEach(([start, opening, ffi, lowering, lifting]) => all(braceMember(client, start, opening, `Kotlin ${start}`), [`uniffi_cyclops_sdk_fn_method_cyclopsclient_${ffi}`, lowering, lifting, "SdkException.ErrorHandler"], `Kotlin concrete ${start}`));
  const builder = braceBlock(source, "open class CreateSignedServiceUrlRequestBuilder:", "Kotlin signed URL builder");
  [["sandbox", "sandbox"], ["service", "service"], ["expiresInSeconds", "expires_in_seconds"], ["label", "label"]].forEach(([name, ffi]) => text(member(builder, `    override fun \`${name}\``, ["\n    override fun "], `Kotlin builder ${name}`), `createsignedserviceurlrequestbuilder_${ffi}`, `Kotlin builder ${name}`));
  all(member(builder, "    @Throws(SdkBuildException::class)override fun `build`", ["\n    override fun "], "Kotlin builder build"), ["CreateSignedServiceUrlRequest", "createsignedserviceurlrequestbuilder_build", "uniffiRustCallWithError(SdkBuildException)"], "Kotlin builder build");
  regex(source, /7 -> SdkException\.SignedServiceUrlsUnavailable\(\)/, "Kotlin SignedServiceUrlsUnavailable decoder");
  noLegacy(source, "Kotlin");
}

function assertSwift(source) {
  const record = braceBlock(source, "public struct SignedServiceUrl:", "Swift SignedServiceUrl record");
  camelFields.forEach((field) => text(record, `public var ${field}:`, "Swift SignedServiceUrl fields"));
  const converter = braceBlock(source, "public struct FfiConverterTypeSignedServiceUrl", "Swift SignedServiceUrl converter");
  all(converter, ["try SignedServiceUrl(", "static func write"], "Swift SignedServiceUrl converter");
  camelFields.forEach((field) => text(converter, `value.${field}`, "Swift SignedServiceUrl encoder"));
  const client = braceBlock(source, "open class CyclopsClient:", "Swift CyclopsClient");
  const methods = [["open func createSignedServiceUrl", " {", "create_signed_service_url", "FfiConverterTypeCreateSignedServiceUrlRequest_lower", "FfiConverterTypeSignedServiceUrl_lift"], ["open func listSignedServiceUrls", " {", "list_signed_service_urls", "FfiConverterTypeSandbox_lower", "FfiConverterSequenceTypeSignedServiceUrl.lift"], ["open func revokeSignedServiceUrl", " {", "revoke_signed_service_url", "FfiConverterTypeSignedServiceUrl_lower", "ffi_cyclops_sdk_rust_future_poll_void"]];
  methods.forEach(([start, opening, ffi, lowering, lifting]) => all(braceMember(client, start, opening, `Swift ${start}`), [`uniffi_cyclops_sdk_fn_method_cyclopsclient_${ffi}`, lowering, lifting, "FfiConverterTypeSdkError_lift"], `Swift concrete ${start}`));
  const builder = braceBlock(source, "open class CreateSignedServiceUrlRequestBuilder:", "Swift signed URL builder");
  [["sandbox", "sandbox"], ["service", "service"], ["expiresInSeconds", "expires_in_seconds"], ["label", "label"]].forEach(([name, ffi]) => text(member(builder, `open func ${name}`, ["\nopen func "], `Swift builder ${name}`), `createsignedserviceurlrequestbuilder_${ffi}`, `Swift builder ${name}`));
  all(member(builder, "open func build", ["\nopen func "], "Swift builder build"), ["CreateSignedServiceUrlRequest", "createsignedserviceurlrequestbuilder_build", "FfiConverterTypeSdkBuildError_lift"], "Swift builder build");
  regex(source, /case 7: return \.SignedServiceUrlsUnavailable/, "Swift SignedServiceUrlsUnavailable decoder");
  noLegacy(source, "Swift");
}

function assertRuby(source) {
  const record = rubyClass(source, "\nclass SignedServiceUrl\n", "Ruby SignedServiceUrl record");
  text(record, `attr_reader :${signedFields.join(", :")}`, "Ruby SignedServiceUrl fields");
  const stream = rubyClass(source, "class RustBufferStream", "Ruby RustBufferStream");
  const bufferBuilder = rubyClass(source, "class RustBufferBuilder", "Ruby RustBufferBuilder");
  const read = member(stream, "  def readTypeSignedServiceUrl", ["\n  def "], "Ruby SignedServiceUrl decoder");
  const write = member(bufferBuilder, "  def write_TypeSignedServiceUrl", ["\n  def "], "Ruby SignedServiceUrl encoder");
  all(read, ["SignedServiceUrl.new("], "Ruby SignedServiceUrl decoder"); signedFields.forEach((field) => { text(read, `${field}:`, "Ruby SignedServiceUrl decoder"); text(write, `v.${field}`, "Ruby SignedServiceUrl encoder"); });
  const client = rubyClass(source, "  class CyclopsClient", "Ruby CyclopsClient");
  const methods = [["  def create_signed_service_url", "create_signed_service_url", "TypeCreateSignedServiceUrlRequest(request)", "consumeIntoTypeSignedServiceUrl"], ["  def list_signed_service_urls", "list_signed_service_urls", "TypeSandbox(sandbox)", "consumeIntoSequenceTypeSignedServiceUrl"], ["  def revoke_signed_service_url", "revoke_signed_service_url", "TypeSignedServiceUrl(signed_service_url)", "uniffi_rust_future_void"]];
  methods.forEach(([start, ffi, input, output]) => all(member(client, start, ["\n  def "], `Ruby ${start}`), [`cyclopsclient_${ffi}`, input, output, "SdkError"], `Ruby concrete ${start}`));
  const builder = rubyClass(source, "  class CreateSignedServiceUrlRequestBuilder", "Ruby signed URL builder");
  [["sandbox", "sandbox"], ["service", "service"], ["expires_in_seconds", "expires_in_seconds"], ["label", "label"]].forEach(([name, ffi]) => text(member(builder, `  def ${name}(`, ["\n  def "], `Ruby builder ${name}`), `createsignedserviceurlrequestbuilder_${ffi}`, `Ruby builder ${name}`));
  all(member(builder, "  def build()", ["\n  def "], "Ruby builder build"), ["consumeIntoTypeCreateSignedServiceUrlRequest", "createsignedserviceurlrequestbuilder_build", "rust_call_with_error(SdkBuildError"], "Ruby builder build");
  const errorRead = member(stream, "  def readTypeSdkError", ["\n  def "], "Ruby SdkError decoder");
  regex(errorRead, /if variant == 7\s+return SdkError::SignedServiceUrlsUnavailable\.new/m, "Ruby SignedServiceUrlsUnavailable decoder ordinal");
  noLegacy(source, "Ruby");
}

function assertBrowser(source) {
  const record = braceBlock(source, "export type SignedServiceUrl", "browser SignedServiceUrl record");
  camelFields.forEach((field) => regex(record, new RegExp(`\\b${field}\\??:`), "browser SignedServiceUrl fields"));
  const converter = braceBlock(source, "const FfiConverterTypeSignedServiceUrl", "browser SignedServiceUrl converter");
  all(converter, ["read(from: RustBuffer)", "write(value: TypeName"], "browser SignedServiceUrl converter");
  camelFields.forEach((field) => { text(converter, `${field}: FfiConverter`, "browser SignedServiceUrl decoder"); text(converter, `value.${field}`, "browser SignedServiceUrl encoder"); });
  const client = braceBlock(source, "export class CyclopsClient", "browser CyclopsClient");
  const methods = [["  async createSignedServiceUrl(", "): Promise<SignedServiceUrl> /*throws*/ {", "create_signed_service_url", "FfiConverterTypeCreateSignedServiceUrlRequest.lower", "FfiConverterTypeSignedServiceUrl.lift.bind"], ["  async listSignedServiceUrls(", "): Promise<Array<SignedServiceUrl>> /*throws*/ {", "list_signed_service_urls", "FfiConverterTypeSandbox.lower", "FfiConverterSequenceTypeSignedServiceUrl.lift.bind"], ["  async revokeSignedServiceUrl(", "): Promise<void> /*throws*/ {", "revoke_signed_service_url", "FfiConverterTypeSignedServiceUrl.lower", "ffi_cyclops_sdk_rust_future_poll_void"]];
  methods.forEach(([start, opening, ffi, lowering, lifting]) => all(braceMember(client, start, opening, `browser ${start}`), [`ubrn_uniffi_cyclops_sdk_fn_method_cyclopsclient_${ffi}`, lowering, lifting, "FfiConverterTypeSdkError.lift.bind"], `browser concrete ${start}`));
  const builder = braceBlock(source, "export class CreateSignedServiceUrlRequestBuilder", "browser signed URL builder");
  [["sandbox", "sandbox"], ["service", "service"], ["expiresInSeconds", "expires_in_seconds"], ["label", "label"]].forEach(([name, ffi]) => text(braceMember(builder, `  ${name}(`, "): CreateSignedServiceUrlRequestBuilderLike {", `browser builder ${name}`), `createsignedserviceurlrequestbuilder_${ffi}`, `browser builder ${name}`));
  all(braceMember(builder, "  build()", "build(): CreateSignedServiceUrlRequest /*throws*/ {", "browser builder build"), ["createsignedserviceurlrequestbuilder_build", "FfiConverterTypeSdkBuildError.lift.bind"], "browser builder build");
  const error = braceBlock(source, "const FfiConverterTypeSdkError", "browser SdkError converter");
  regex(error, /case 7:\s+return new SdkError\.SignedServiceUrlsUnavailable\(\)/m, "browser SignedServiceUrlsUnavailable decoder ordinal");
  regex(error, /case SdkError_Tags\.SignedServiceUrlsUnavailable: \{\s+ordinalConverter\.write\(7, into\)/m, "browser SignedServiceUrlsUnavailable encoder ordinal");
  noLegacy(source, "browser");
}

function expectRejected(label, assertion, source) {
  try { assertion(source); } catch (_error) { console.log(`signed service URL structural mutation fixture rejected ${label}`); return; }
  fail(`mutation fixture unexpectedly passed: ${label}`);
}
function replaceOnce(source, from, to, label) { const result = source.replace(from, to); if (result === source) fail(`mutation fixture could not apply: ${label}`); return result; }
function within(source, start, endMarker, from, to, label) { const offset = after(source, start, label); const end = source.indexOf(endMarker, offset + start.length); if (end < 0) fail(`mutation fixture boundary is missing: ${label}`); return source.slice(0, offset) + replaceOnce(source.slice(offset, end), from, to, label) + source.slice(end); }
function withinAfter(source, parent, start, endMarker, from, to, label) {
  const parentOffset = after(source, parent, label);
  const offset = after(source.slice(parentOffset), start, label) + parentOffset;
  const end = source.indexOf(endMarker, offset + start.length);
  if (end < 0) fail(`mutation fixture boundary is missing: ${label}`);
  return source.slice(0, offset) + replaceOnce(source.slice(offset, end), from, to, label) + source.slice(end);
}

assertPython(sources.python); assertKotlin(sources.kotlin); assertSwift(sources.swift); assertRuby(sources.ruby); assertBrowser(sources.browser);
expectRejected("Python class SignedServiceUrl: pass", assertPython, replaceOnce(sources.python, /class SignedServiceUrl:[\s\S]*?\nclass _UniffiFfiConverterTypeSignedServiceUrl/m, "class SignedServiceUrl:\n    pass\n\nclass _UniffiFfiConverterTypeSignedServiceUrl", "Python record pass"));
expectRejected("renamed Python SignedServiceUrl converter", assertPython, replaceOnce(sources.python, "class _UniffiFfiConverterTypeSignedServiceUrl(", "class _UniffiFfiConverterTypeRenamedSignedServiceUrl(", "Python converter rename"));
expectRejected("Python CreatePoolRequest lowering", assertPython, withinAfter(sources.python, "class CyclopsClient(CyclopsClientProtocol):", "    async def create_signed_service_url", "\n    async def ", "_UniffiFfiConverterTypeCreateSignedServiceUrlRequest.lower", "_UniffiFfiConverterTypeCreatePoolRequest.lower", "Python create lowering"));
expectRejected("Swift FFI moved to EOF", assertSwift, `${within(sources.swift, "open func createSignedServiceUrl", "\nopen func ", "uniffi_cyclops_sdk_fn_method_cyclopsclient_create_signed_service_url", "uniffi_cyclops_sdk_fn_method_cyclopsclient_create_wrong", "Swift create FFI")}\n// uniffi_cyclops_sdk_fn_method_cyclopsclient_create_signed_service_url`);
expectRejected("browser service setter FFI", assertBrowser, within(sources.browser, "  service(value: string)", "\n  uniffiDestroy", "createsignedserviceurlrequestbuilder_service", "createsignedserviceurlrequestbuilder_label", "browser setter FFI"));
expectRejected("Python concrete SdkError", assertPython, withinAfter(sources.python, "class CyclopsClient(CyclopsClientProtocol):", "    async def create_signed_service_url", "\n    async def ", "_UniffiFfiConverterTypeSdkError", "_UniffiFfiConverterTypeSdkBuildError", "Python create error"));
expectRejected("Python swapped SignedServiceUrlsUnavailable ordinal", assertPython, replaceOnce(sources.python, "if variant == 7:\n            return SdkError.SignedServiceUrlsUnavailable(", "if variant == 8:\n            return SdkError.SignedServiceUrlsUnavailable(", "Python ordinal"));
expectRejected("Ruby swapped SignedServiceUrlsUnavailable ordinal", assertRuby, replaceOnce(sources.ruby, "if variant == 7\n        return SdkError::SignedServiceUrlsUnavailable.new", "if variant == 8\n        return SdkError::SignedServiceUrlsUnavailable.new", "Ruby ordinal"));
expectRejected("browser swapped SignedServiceUrlsUnavailable ordinal", assertBrowser, replaceOnce(sources.browser, "case 7:\n          return new SdkError.SignedServiceUrlsUnavailable();", "case 8:\n          return new SdkError.SignedServiceUrlsUnavailable();", "browser ordinal"));

NODE

for typescript_binding in "$node_schema_source" "$browser_schema_source"; do
  grep -Fq -- "ttlSecondsAfterCreated?: number" "$typescript_binding" || fail "TypeScript bindings omit ttlSecondsAfterCreated: $typescript_binding"
  grep -Fq -- "ttlSecondsAfterCreated: FfiConverterOptionalUInt32.read(from)" "$typescript_binding" || fail "TypeScript bindings do not read ttlSecondsAfterCreated: $typescript_binding"
  grep -Fq -- "FfiConverterOptionalUInt32.write(value.ttlSecondsAfterCreated, into)" "$typescript_binding" || fail "TypeScript bindings do not write ttlSecondsAfterCreated: $typescript_binding"
done
node - "$node_schema_source" "$browser_schema_source" <<'NODE'
const fs = require("node:fs");

function converterBlock(source, label, start, end) {
  const startIndex = source.indexOf(start);
  const endIndex = source.indexOf(end, startIndex + start.length);
  if (startIndex < 0 || endIndex < 0) {
    throw new Error(`${label} converter block is missing`);
  }
  return source.slice(startIndex, endIndex);
}

function requireOrder(source, label, first, second) {
  const firstIndex = source.indexOf(first);
  const secondIndex = source.indexOf(second, firstIndex + first.length);
  if (firstIndex < 0 || secondIndex < 0) {
    throw new Error(`${label} has stale record field order`);
  }
}

for (const path of process.argv.slice(2)) {
  const source = fs.readFileSync(path, "utf8");
  const defaults = source.match(/ttlSecondsAfterCreated: undefined/g) ?? [];
  if (defaults.length < 2) {
    throw new Error(`${path} omits an optional TTL constructor default`);
  }
  const claim = converterBlock(
    source,
    `${path} ClaimSpec`,
    "const FfiConverterTypeClaimSpec = (() => {",
    "export type OsGymSandboxClaimCondition",
  );
  const warmPool = converterBlock(
    source,
    `${path} WarmPool`,
    "const FfiConverterTypeOSGymSandboxWarmPoolSpec = (() => {",
    "export type OsGymSandboxWarmPoolStatus",
  );
  requireOrder(
    claim,
    `${path} ClaimSpec read`,
    "lifecycle: FfiConverterOptionalTypeClaimLifecycle.read(from)",
    "ttlSecondsAfterCreated: FfiConverterOptionalUInt32.read(from)",
  );
  requireOrder(
    claim,
    `${path} ClaimSpec write`,
    "FfiConverterOptionalTypeClaimLifecycle.write(value.lifecycle, into)",
    "FfiConverterOptionalUInt32.write(value.ttlSecondsAfterCreated, into)",
  );
  requireOrder(
    warmPool,
    `${path} WarmPool read`,
    "autoscaling: FfiConverterOptionalTypeWarmPoolAutoscaling.read(from)",
    "ttlSecondsAfterCreated: FfiConverterOptionalUInt32.read(from)",
  );
  requireOrder(
    warmPool,
    `${path} WarmPool write`,
    "FfiConverterOptionalTypeWarmPoolAutoscaling.write(",
    "FfiConverterOptionalUInt32.write(value.ttlSecondsAfterCreated, into)",
  );
}
NODE
grep -Fq -- "TtlSecondsAfterCreated *uint32" "$go_schema_source" || fail "Go bindings omit TtlSecondsAfterCreated"
grep -Fq -- "FfiConverterOptionalUint32INSTANCE.Write(writer, value.TtlSecondsAfterCreated)" "$go_schema_source" || fail "Go bindings do not write TtlSecondsAfterCreated"

for typescript_binding in "$node_sdk_source" "$browser_sdk_source"; do
  grep -Fq -- "timeoutSecs?: bigint" "$typescript_binding" || fail "TypeScript bindings omit HttpRequest.timeoutSecs: $typescript_binding"
  grep -Fq -- "timeoutSecs: FfiConverterOptionalUInt64.read(from)" "$typescript_binding" || fail "TypeScript bindings do not read HttpRequest.timeoutSecs: $typescript_binding"
  grep -Fq -- "FfiConverterOptionalUInt64.write(value.timeoutSecs, into)" "$typescript_binding" || fail "TypeScript bindings do not write HttpRequest.timeoutSecs: $typescript_binding"
  grep -Fq -- "FfiConverterOptionalUInt64.allocationSize(value.timeoutSecs)" "$typescript_binding" || fail "TypeScript bindings do not allocate HttpRequest.timeoutSecs: $typescript_binding"
  grep -Fq -- "const FfiConverterOptionalUInt64 = new FfiConverterOptional" "$typescript_binding" || fail "TypeScript bindings omit the optional UInt64 converter: $typescript_binding"
done
grep -Fq -- "TimeoutSecs *uint64" "$go_sdk_source" || fail "Go bindings omit HttpRequest.TimeoutSecs"
grep -Fq -- "FfiConverterOptionalUint64INSTANCE.Read(reader)" "$go_sdk_source" || fail "Go bindings do not read HttpRequest.TimeoutSecs"
grep -Fq -- "FfiConverterOptionalUint64INSTANCE.Write(writer, value.TimeoutSecs)" "$go_sdk_source" || fail "Go bindings do not write HttpRequest.TimeoutSecs"
grep -Fq -- "FfiDestroyerOptionalUint64{}.Destroy(r.TimeoutSecs)" "$go_sdk_source" || fail "Go bindings do not destroy HttpRequest.TimeoutSecs"
grep -Fq -- "type FfiConverterOptionalUint64 struct{}" "$go_sdk_source" || fail "Go bindings omit the optional uint64 converter"
grep -Fq -- "@uniffi_handle_map = UniffiHandleMap.new" "$ruby_sdk_source" || fail "Ruby callback bindings do not retain native callback objects"
grep -Fq -- "module UniffiCallbackInterfaceHttpClient" "$ruby_sdk_source" || fail "Ruby callback bindings do not register an HTTP callback vtable"
grep -Fq -- "[VTableCallbackInterfaceHttpClient.by_ref]" "$ruby_sdk_source" || fail "Ruby callback vtable initializer has the wrong FFI signature"
grep -Fq -- "def self.uniffi_rust_future_rust_buffer" "$ruby_sdk_source" || fail "Ruby bindings do not resolve Rust-buffer futures"
grep -Fq -- "def self.uniffi_trait_interface_call" "$ruby_sdk_source" || fail "Ruby callback bindings do not report callback results to Rust"
grep -Fq -- "def self.uniffi_lower_http_error" "$ruby_sdk_source" || fail "Ruby callback bindings do not serialize HttpError values"
grep -Fq -- "builder.write_U32(1)" "$ruby_sdk_source" || fail "Ruby callback bindings encode HttpError variant tags"
grep -Fq -- "reason = reason.fetch(:reason)" "$ruby_sdk_source" || fail "Ruby callback bindings normalize HttpError keyword payloads"
grep -Fq -- "def self.uniffi_is_error_type?" "$ruby_sdk_source" || fail "Ruby callback bindings do not classify callback errors"
grep -Fq -- "FleetSdk.uniffi_rust_future_rust_buffer" "$ruby_sdk_source" || fail "Ruby async methods do not resolve Rust-buffer futures"
grep -Fq -- "def self.uniffi_rust_future_void" "$ruby_sdk_source" || fail "Ruby bindings do not resolve void futures"
grep -Fq -- "FleetSdk.uniffi_rust_future_void" "$ruby_sdk_source" || fail "Ruby async void methods do not resolve Rust futures"
if grep -Fq -- ",,RustCallStatus.new" "$ruby_sdk_source"; then fail "Ruby zero-argument async methods emit a duplicate status separator"; fi
grep -Fq -- "UniFFILib.uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(uniffi_clone_handle(),RustBuffer.alloc_from_TypeCreatePoolRequest(request),RustCallStatus.new)" "$ruby_sdk_source" || fail "Ruby async factories do not pass the generated status placeholder"
grep -Fq -- "    readTypeOSGymSandboxWarmPoolSpec" "$bindings_dir/ruby/cyclops_sdk.rb" || fail "Ruby facade does not delegate schema record readers"
if grep -Fq -- "OsGym" "$ruby_sdk_source"; then
  fail "Ruby SDK retains cross-crate OsGym helper names"
fi
grep -Fq -- "    readTypeOSGymSandboxWarmPoolStatus" "$bindings_dir/ruby/cyclops_sdk.rb" || fail "Ruby facade does not delegate schema status readers"
if grep -Fq -- "return 0 if @handle.nil?" "$ruby_sdk_source"; then
  fail "Ruby callback bindings lower native callbacks to an invalid zero handle"
fi

baseline_hash="$(tree_hash "$bindings_dir")"
assert_no_harness_artifacts

stale_manifest="$bindings_dir/python/.cyclops-sdk-generated-files"
stale_obsolete_root="$bindings_dir/python/fleet_sdk/obsolete"
stale_nested_root="$stale_obsolete_root/nested directory"
stale_handwritten_root="$bindings_dir/python/fleet_sdk/handwritten sibling"
mkdir -p "$stale_nested_root" "$stale_handwritten_root"
printf 'stale generated file\n' > "$stale_nested_root/stale generated.py"
printf 'handwritten sibling\n' > "$stale_handwritten_root/keep.txt"
cat >> "$stale_manifest" <<'EOF_MANIFEST'
d fleet_sdk/obsolete
d fleet_sdk/obsolete/nested directory
f fleet_sdk/obsolete/nested directory/stale generated.py
EOF_MANIFEST
expect_check_failure stale-manifest
"$generator"
if [ -e "$stale_obsolete_root" ] || [ -L "$stale_obsolete_root" ]; then
  fail "normal generation retained stale manifest-owned directory"
fi
cmp -s "$stale_handwritten_root/keep.txt" <(printf 'handwritten sibling\n') || fail "normal generation removed handwritten sibling"
"$generator" --check
rm "$stale_handwritten_root/keep.txt"
rmdir "$stale_handwritten_root"
assert_tree_unchanged "$baseline_hash" "stale manifest cleanup"

content_file="$bindings_dir/python/fleet_sdk/__init__.py"
content_file_mode="$(mode_for "$content_file")"
cp "$content_file" "$temporary_directory/content-file"
printf '\n# task10 content drift\n' >> "$content_file"
expect_check_failure content
mv "$temporary_directory/content-file" "$content_file"
chmod "$content_file_mode" "$content_file"
"$generator" --check

mode_file="$bindings_dir/ruby/cyclops_sdk.rb"
mode_file_mode="$(mode_for "$mode_file")"
chmod 600 "$mode_file"
expect_check_failure file-mode
chmod "$mode_file_mode" "$mode_file"
"$generator" --check

type_file="$bindings_dir/python/fleet_sdk/_schema.py"
mv "$type_file" "$temporary_directory/type-file"
mkdir "$type_file"
expect_check_failure file-type
rmdir "$type_file"
mv "$temporary_directory/type-file" "$type_file"
"$generator" --check

file_link="$bindings_dir/python/fleet_sdk/_sdk.py"
mv "$file_link" "$temporary_directory/file-link-target"
ln -s "$temporary_directory/file-link-target" "$file_link"
expect_check_failure file-symlink
[ -L "$file_link" ] || fail "file symlink disappeared during --check"
[ -f "$temporary_directory/file-link-target" ] || fail "file symlink target changed during --check"
rm "$file_link"
mv "$temporary_directory/file-link-target" "$file_link"
"$generator" --check

directory_link="$bindings_dir/kotlin/ai"
mv "$directory_link" "$temporary_directory/ai"
ln -s "$temporary_directory/ai" "$directory_link"
expect_check_failure directory-symlink
[ -L "$directory_link" ] || fail "directory symlink disappeared during --check"
[ -d "$temporary_directory/ai" ] || fail "directory symlink target changed during --check"
rm "$directory_link"
mv "$temporary_directory/ai" "$directory_link"
"$generator" --check

root_mode="$(mode_for "$bindings_dir/python")"
chmod 700 "$bindings_dir/python"
expect_check_failure root-mode
chmod "$root_mode" "$bindings_dir/python"
"$generator" --check

expect_transaction_failure before-backup-move "$baseline_hash"
expect_transaction_failure after-backup-move "$baseline_hash"
expect_transaction_failure after-new-live "$baseline_hash"
expect_transaction_signal after-backup-move INT "$baseline_hash"
expect_transaction_signal after-new-live TERM "$baseline_hash"
"$generator"
assert_tree_unchanged "$baseline_hash" "normal root replacement"
"$generator" --check

printf 'generate-sdk-bindings regression checks passed.\n'
