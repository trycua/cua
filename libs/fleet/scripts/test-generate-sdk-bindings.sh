#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace_dir="$repo_root/cyclops-cs"
generator="$workspace_dir/scripts/generate-sdk-bindings.sh"
bindings_dir="$workspace_dir/sdk-bindings"
temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-sdk-bindings-test.XXXXXX")"
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

cleanup() {
  if [ "$cargo_config_active" = true ] && [ -f "$cargo_config_file" ]; then
    rm -f "$cargo_config_file"
  fi
  if [ "$cargo_config_directory_created" = true ]; then
    # Trap cleanup must not override the harness result if the directory is nonempty.
    rmdir "$cargo_config_directory" 2>/dev/null || true  # lint-ignore: error-masking
  fi
  rm -rf "$temporary_directory"
}
trap cleanup EXIT HUP INT TERM
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
  build_target_directory="$temporary_directory/build-target-env"
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

  config_target_directory="$temporary_directory/build-target-config"
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

kotlin_sdk = (bindings / "kotlin/ai/cua/cyclops/sdk/cyclops_sdk.kt").read_text(encoding="utf-8")
kotlin_schema = (bindings / "kotlin/ai/cua/cyclops/sdk/schema/cyclops_sdk_schema.kt").read_text(encoding="utf-8")
if "package ai.cua.cyclops.sdk" not in kotlin_sdk or "package ai.cua.cyclops.sdk.schema" not in kotlin_schema:
    raise AssertionError("Kotlin components are not generated into distinct configured packages")
if "var `spec`: PoolSpec" not in kotlin_sdk or "data class PoolSpec" not in kotlin_schema:
    raise AssertionError("Kotlin PoolSpec external type is not linked through the schema package")

swift_sdk = (bindings / "swift/CyclopsSdk.swift").read_text(encoding="utf-8")
swift_schema = (bindings / "swift/CyclopsSdkSchema.swift").read_text(encoding="utf-8")
if "public var spec: PoolSpec" not in swift_sdk or "public struct PoolSpec" not in swift_schema:
    raise AssertionError("Swift PoolSpec external type is not available for one-module compilation")
pool_spec = re.search(
    r"public struct PoolSpec: Equatable, Hashable \{(?P<body>.*?)\n\}\n\n#if compiler",
    swift_schema,
    re.DOTALL,
)
if pool_spec is None:
    raise AssertionError("Swift PoolSpec is missing explicit Equatable and Hashable conformances")
for method in ("public static func ==", "public func hash(into hasher: inout Hasher)"):
    if method not in pool_spec.group("body"):
        raise AssertionError(f"Swift PoolSpec is missing Rust-backed conformance method: {method}")
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
runtime_copy="$bindings_dir/python/cyclops_sdk/$(basename "$runtime_library")"
cp "$runtime_library" "$runtime_copy"
PYTHONPATH="$bindings_dir/python" python3 - "$bindings_dir/python/cyclops_sdk/_sdk.py" <<'PYTHON_SMOKE'
import asyncio
import json
import re
import sys

import cyclops_sdk

sdk_source = open(sys.argv[1], encoding="utf-8").read()
required_private_exports = set(re.findall(r"\bcyclops_sdk\.(_[A-Za-z_][A-Za-z0-9_]*)", sdk_source))
missing = sorted(required_private_exports.difference(vars(cyclops_sdk)))
if missing:
    raise AssertionError(f"missing schema-private SDK dependencies: {missing}")
public_exports = set(cyclops_sdk.__all__)
leaked = sorted(required_private_exports.intersection(public_exports))
if leaked:
    raise AssertionError(f"schema-private dependencies leaked into __all__: {leaked}")

class CallbackHttpClient(cyclops_sdk.HttpClient):
    def __init__(self):
        self.requests = []

    async def execute(self, request):
        self.requests.append(request)
        if request.url.endswith("/protocol/openid-connect/token"):
            body = {"access_token": "offline-token", "expires_in": 3600}
        elif request.url.endswith("/api/namespaces"):
            body = {"metadata": {"name": "default"}}
        elif request.url.endswith("/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools"):
            body = {
                "apiVersion": "cua.ai/v1",
                "kind": "OSGymWorkspacePool",
                "metadata": {"namespace": "default", "name": "offline-pool"},
                "spec": {
                    "replicas": 1,
                    "template": {"containerDiskImage": "registry.example/desktop:offline"},
                },
            }
        else:
            raise AssertionError(f"unexpected callback request: {request.method} {request.url}")
        return cyclops_sdk.HttpResponse(status=201, headers=[], body=json.dumps(body).encode())

async def smoke_create_pool():
    transport = CallbackHttpClient()
    client = cyclops_sdk.CyclopsClient.connect(
        cyclops_sdk.CyclopsConfiguration(
            base_url="https://cyclops.invalid",
            token_url="https://keycloak.invalid/realms/offline/protocol/openid-connect/token",
            credentials=cyclops_sdk.CyclopsCredentials("client-id", "client-secret"),
            pool_poll_interval_ms=1,
            pool_poll_limit=1,
            claim_poll_interval_ms=1,
            claim_poll_limit=1,
        ),
        transport,
    )
    spec = cyclops_sdk.PoolSpec(
        replicas=1,
        template=cyclops_sdk.PoolTemplate(
            runtime=None,
            runtime_class_name=None,
            node_selector=None,
            tolerations=None,
            command=None,
            container_disk_image="registry.example/desktop:offline",
            image_pull_secret=None,
            cpu_cores=None,
            memory=None,
            firmware=None,
            probes=None,
            oidc=None,
        ),
        autoscaling=None,
        services=None,
    )
    pool = await client.create_pool(cyclops_sdk.CreatePoolRequest(namespace="default", spec=spec))
    assert pool.metadata.name == "offline-pool"
    assert any(request.url.endswith("/osgymworkspacepools") for request in transport.requests)

asyncio.run(smoke_create_pool())
PYTHON_SMOKE
rm "$runtime_copy"
runtime_copy=""
"$generator" --check
ruby_sdk_source="$bindings_dir/ruby/cyclops_sdk/sdk.rb"
grep -Fq -- "@uniffi_handle_map = UniffiHandleMap.new" "$ruby_sdk_source" || fail "Ruby callback bindings do not retain native callback objects"
grep -Fq -- "module UniffiCallbackInterfaceHttpClient" "$ruby_sdk_source" || fail "Ruby callback bindings do not register an HTTP callback vtable"
grep -Fq -- "[VTableCallbackInterfaceHttpClient.by_ref]" "$ruby_sdk_source" || fail "Ruby callback vtable initializer has the wrong FFI signature"
grep -Fq -- "def self.uniffi_rust_future_rust_buffer" "$ruby_sdk_source" || fail "Ruby bindings do not resolve Rust-buffer futures"
grep -Fq -- "def self.uniffi_trait_interface_call" "$ruby_sdk_source" || fail "Ruby callback bindings do not report callback results to Rust"
grep -Fq -- "def self.uniffi_lower_http_error" "$ruby_sdk_source" || fail "Ruby callback bindings do not serialize HttpError values"
grep -Fq -- "builder.write_U32(1)" "$ruby_sdk_source" || fail "Ruby callback bindings encode HttpError variant tags"
grep -Fq -- "reason = reason.fetch(:reason)" "$ruby_sdk_source" || fail "Ruby callback bindings normalize HttpError keyword payloads"
grep -Fq -- "def self.uniffi_is_error_type?" "$ruby_sdk_source" || fail "Ruby callback bindings do not classify callback errors"
grep -Fq -- "CyclopsSdk.uniffi_rust_future_rust_buffer" "$ruby_sdk_source" || fail "Ruby async methods do not resolve Rust-buffer futures"
grep -Fq -- "def self.uniffi_rust_future_void" "$ruby_sdk_source" || fail "Ruby bindings do not resolve void futures"
grep -Fq -- "CyclopsSdk.uniffi_rust_future_void" "$ruby_sdk_source" || fail "Ruby async void methods do not resolve Rust futures"
grep -Fq -- "UniFFILib.uniffi_cyclops_sdk_fn_method_cyclopsclient_create_pool(uniffi_clone_handle(),RustBuffer.alloc_from_TypeCreatePoolRequest(request),RustCallStatus.new)" "$ruby_sdk_source" || fail "Ruby async factories do not pass the generated status placeholder"
grep -Fq -- "    readTypePoolSpec" "$bindings_dir/ruby/cyclops_sdk.rb" || fail "Ruby facade does not delegate schema record readers"
if grep -Fq -- "OsGym" "$ruby_sdk_source"; then
  fail "Ruby SDK retains cross-crate OsGym helper names"
fi
grep -Fq -- "    readTypeOSGymWorkspacePoolStatus" "$bindings_dir/ruby/cyclops_sdk.rb" || fail "Ruby facade does not delegate schema optional readers"
if grep -Fq -- "return 0 if @handle.nil?" "$ruby_sdk_source"; then
  fail "Ruby callback bindings lower native callbacks to an invalid zero handle"
fi

baseline_hash="$(tree_hash "$bindings_dir")"
assert_no_harness_artifacts

stale_manifest="$bindings_dir/python/.cyclops-sdk-generated-files"
stale_obsolete_root="$bindings_dir/python/cyclops_sdk/obsolete"
stale_nested_root="$stale_obsolete_root/nested directory"
stale_handwritten_root="$bindings_dir/python/cyclops_sdk/handwritten sibling"
mkdir -p "$stale_nested_root" "$stale_handwritten_root"
printf 'stale generated file\n' > "$stale_nested_root/stale generated.py"
printf 'handwritten sibling\n' > "$stale_handwritten_root/keep.txt"
cat >> "$stale_manifest" <<'EOF_MANIFEST'
d cyclops_sdk/obsolete
d cyclops_sdk/obsolete/nested directory
f cyclops_sdk/obsolete/nested directory/stale generated.py
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

content_file="$bindings_dir/python/cyclops_sdk/__init__.py"
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

type_file="$bindings_dir/python/cyclops_sdk/_schema.py"
mv "$type_file" "$temporary_directory/type-file"
mkdir "$type_file"
expect_check_failure file-type
rmdir "$type_file"
mv "$temporary_directory/type-file" "$type_file"
"$generator" --check

file_link="$bindings_dir/python/cyclops_sdk/_sdk.py"
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
