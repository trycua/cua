#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: generate-sdk-bindings.sh [--check]

Generate checked-in UniFFI Python, Kotlin, Swift, and Ruby bindings. --check
compares fresh output to the checked-in generated source without modifying it.
USAGE
}

case "${1:-}" in
  "") check_only=false ;;
  --check) check_only=true ;;
  *) usage; exit 2 ;;
esac

if [ "$#" -gt 1 ]; then
  usage
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace_dir="$repo_root/cyclops-cs"
workspace="$workspace_dir/Cargo.toml"
bindings_dir="$workspace_dir/sdk-bindings"
languages="python kotlin swift ruby"
manifest_name=".cyclops-sdk-generated-files"

if cargo_bin="$(command -v cargo)" && [ -n "$cargo_bin" ]; then
  :
else
  echo "error: cargo must be available on PATH" >&2
  exit 127
fi

if rustc_bin="$(command -v rustc)" && [ -n "$rustc_bin" ]; then
  :
else
  echo "error: rustc must be available on PATH" >&2
  exit 127
fi

host_triple="$("$rustc_bin" -vV | sed -n 's/^host: //p')"
if [ -z "$host_triple" ]; then
  echo "error: could not determine the Rust host target" >&2
  exit 1
fi

temporary_output="$(mktemp -d "$workspace_dir/.generate-sdk-bindings.XXXXXX")"
replacement_root=""
backup_container=""
backup_root=""
transaction_state="preparing"
mode_for() {
  if stat -f '%Lp' "$1" >/dev/null 2>&1; then
    stat -f '%Lp' "$1"
  else
    stat -c '%a' "$1"
  fi
}

reject_symlinks() {
  root="$1"
  label="$2"
  symlink_list="$temporary_output/symlinks-$label"

  if [ -L "$root" ]; then
    echo "generated symlink is not allowed: $label" >&2
    return 1
  fi
  if [ ! -d "$root" ]; then
    echo "generated root is missing or not a directory: $label" >&2
    return 1
  fi

  find -P "$root" -type l -print > "$symlink_list"
  if [ -s "$symlink_list" ]; then
    while IFS= read -r path; do
      echo "generated symlink is not allowed: $label/${path#"$root"/}" >&2
    done < "$symlink_list"
    return 1
  fi
  return 0
}
write_manifest() {
  root="$1"
  manifest="$2"
  (
    cd "$root"
    {
      find -P . -mindepth 1 -type d -print | sed 's#^\./#d #'
      find -P . -mindepth 1 -type f -print | sed 's#^\./#f #'
    } | LC_ALL=C sort
  ) > "$manifest"
}

validate_manifest_path() {
  path="$1"
  case "$path" in
    ""|/*|.|..|../*|*/../*|*/..) return 1 ;;
    *) return 0 ;;
  esac
}

grep_matches_or_empty() {
  pattern="$1"
  source_file="$2"
  if grep -Eo "$pattern" "$source_file"; then
    return 0
  else
    status="$?"
    [ "$status" -eq 1 ] && return 0
    return "$status"
  fi
}

ruby_method_names() {
  source_file="$1"
  method_prefix="$2"
  grep_matches_or_empty "${method_prefix}_Type[A-Za-z0-9_]+" "$source_file" | LC_ALL=C sort -u
}

ruby_defined_method_names() {
  source_file="$1"
  method_prefix="$2"
  sed -n -E "s/^[[:space:]]*def (self\\.)?(${method_prefix}_Type[A-Za-z0-9_]+).*/\\2/p" "$source_file" | LC_ALL=C sort -u
}

write_ruby_method_array() {
  array_name="$1"
  methods_file="$2"
  facade_file="$3"
  printf '  %s = %%i[\n' "$array_name" >> "$facade_file"
  while IFS= read -r method_name; do
    [ -n "$method_name" ] || continue
    printf '    %s\n' "$method_name" >> "$facade_file"
  done < "$methods_file"
  printf '  ].freeze\n' >> "$facade_file"
}

write_ruby_facade() {
  sdk_file="$1"
  schema_file="$2"
  facade_file="$3"
  for method_prefix in check_lower read write; do
    references="$temporary_output/ruby-$method_prefix-references"
    definitions="$temporary_output/ruby-$method_prefix-definitions"
    schema_definitions="$temporary_output/ruby-schema-$method_prefix-definitions"
    missing="$temporary_output/ruby-$method_prefix-external"
    ruby_method_names "$sdk_file" "$method_prefix" > "$references"
    ruby_defined_method_names "$sdk_file" "$method_prefix" > "$definitions"
    ruby_defined_method_names "$schema_file" "$method_prefix" > "$schema_definitions"
    comm -23 "$references" "$definitions" > "$missing"
    unresolved="$temporary_output/ruby-$method_prefix-unresolved"
    case "$method_prefix" in
      check_lower) sed "s/OsGym/OSGym/g" "$missing" > "$temporary_output/ruby-$method_prefix-normalized-external" ;;
      *) cp "$missing" "$temporary_output/ruby-$method_prefix-normalized-external" ;;
    esac
    comm -23 "$temporary_output/ruby-$method_prefix-normalized-external" "$schema_definitions" > "$unresolved"
    if [ -s "$unresolved" ]; then
      echo "error: generated Ruby SDK references methods missing from schema: $method_prefix" >&2
      cat "$unresolved" >&2
      return 1
    fi
  done

  cat > "$facade_file" <<'RUBY_FACADE_HEADER'
require_relative "cyclops_sdk/schema"
require_relative "cyclops_sdk/sdk"

module CyclopsSdk
  CyclopsSdkSchema.constants(false).each do |name|
    const_set(name, CyclopsSdkSchema.const_get(name)) unless const_defined?(name, false)
  end
RUBY_FACADE_HEADER
  write_ruby_method_array "SCHEMA_CHECK_LOWER_METHODS" "$temporary_output/ruby-check_lower-external" "$facade_file"
  write_ruby_method_array "SCHEMA_READ_METHODS" "$temporary_output/ruby-read-external" "$facade_file"
  write_ruby_method_array "SCHEMA_WRITE_METHODS" "$temporary_output/ruby-write-external" "$facade_file"
  cat >> "$facade_file" <<'RUBY_FACADE_FOOTER'

  schema_rust_buffer = CyclopsSdkSchema::RustBuffer
  schema_stream = CyclopsSdkSchema.const_get(:RustBufferStream, false)

  SCHEMA_CHECK_LOWER_METHODS.each do |method_name|
    RustBuffer.define_singleton_method(method_name) do |value|
      schema_rust_buffer.public_send(method_name.to_s.gsub("OsGym", "OSGym"), value)
    end
  end

  SCHEMA_READ_METHODS.each do |method_name|
    RustBufferStream.define_method(method_name) do
      schema_buffer = schema_rust_buffer.new(@rbuf.pointer)
      external_stream = schema_stream.new(schema_buffer)
      external_stream.instance_variable_set(:@offset, @offset)
      value = external_stream.public_send(method_name)
      @offset = external_stream.instance_variable_get(:@offset)
      value
    end
  end

  SCHEMA_WRITE_METHODS.each do |method_name|
    RustBufferBuilder.define_method(method_name) do |value|
      buffer = schema_rust_buffer.public_send(method_name.to_s.sub("write_", "alloc_from_"), value)
      begin
        write(buffer.data.read_bytes(buffer.len))
      ensure
        buffer.free
      end
    end
  end

  private_constant :SCHEMA_CHECK_LOWER_METHODS, :SCHEMA_READ_METHODS, :SCHEMA_WRITE_METHODS
end
RUBY_FACADE_FOOTER
}

write_python_facade() {
  sdk_file="$1"
  schema_file="$2"
  facade_file="$3"
  private_exports="$temporary_output/python-schema-private-exports"

  grep_matches_or_empty 'cyclops_sdk\._[A-Za-z_][A-Za-z0-9_]*' "$sdk_file" \
    | sed 's/^cyclops_sdk\.//' | LC_ALL=C sort -u > "$private_exports"

  cat > "$facade_file" <<'PYTHON_FACADE_HEADER'
from . import _schema as _schema_component
from ._schema import *
PYTHON_FACADE_HEADER
  while IFS= read -r symbol; do
    [ -n "$symbol" ] || continue
    if ! grep -Eq "^(class|def) ${symbol}|^${symbol}[[:space:]]*=" "$schema_file"; then
      echo "error: generated Python SDK references missing schema symbol: $symbol" >&2
      return 1
    fi
    printf '%s = _schema_component.%s\n' "$symbol" "$symbol" >> "$facade_file"
  done < "$private_exports"
  cat >> "$facade_file" <<'PYTHON_FACADE_FOOTER'

from . import _sdk as _sdk_component
from ._sdk import *

__all__ = [*_schema_component.__all__, *_sdk_component.__all__]

del _schema_component
del _sdk_component
PYTHON_FACADE_FOOTER
}

normalize_generated_text() {
  for language in $languages; do
    find -P "$temporary_output/generated/$language" -type f -print | while IFS= read -r file; do
      normalized="$file.normalized"
      awk '
        {
          sub(/[[:space:]]+$/, "")
          if ($0 == "") {
            blank_lines++
            next
          }
          while (blank_lines > 0) {
            print ""
            blank_lines--
          }
          print
        }
      ' "$file" > "$normalized"
      mv "$normalized" "$file"
    done
  done
}

set_generated_modes() {
  source_root="$1"
  destination_root="$2"
  manifest="$source_root/$manifest_name"

  chmod "$(mode_for "$source_root")" "$destination_root"
  while IFS=' ' read -r type path; do
    if ! validate_manifest_path "$path"; then
      echo "error: invalid generated manifest path: $path" >&2
      return 1
    fi
    case "$type" in
      d|f) chmod "$(mode_for "$source_root/$path")" "$destination_root/$path" ;;
      *) echo "error: invalid generated manifest entry: $type $path" >&2; return 1 ;;
    esac
  done < "$manifest"
  chmod "$(mode_for "$manifest")" "$destination_root/$manifest_name"
}

remove_previous_generated_files() {
  destination_root="$1"
  previous_manifest="$2"

  [ -f "$previous_manifest" ] || return 0

  while IFS=' ' read -r type path; do
    if ! validate_manifest_path "$path"; then
      echo "error: invalid checked-in generated manifest path: $path" >&2
      return 1
    fi
    case "$type" in
      d|f) ;;
      *) echo "error: invalid checked-in generated manifest entry: $type $path" >&2; return 1 ;;
    esac
  done < "$previous_manifest"

  # Remove manifest-owned files (including file symlinks) before their parents.
  while IFS=' ' read -r type path; do
    if [ "$type" = "f" ]; then
      rm -f "$destination_root/$path"
    fi
  done < "$previous_manifest"

  # Remove manifest-owned directories deepest-first, preserving unlisted children.
  while IFS= read -r path; do
    # Nonempty directories contain unlisted files that must be preserved.
    rmdir "$destination_root/$path" 2>/dev/null || true  # lint-ignore: error-masking
  done < <(
    LC_ALL=C awk '
      substr($0, 1, 2) == "d " {
        path = substr($0, 3)
        depth_path = path
        depth = gsub(/\//, "/", depth_path)
        printf "%08d %s%c", depth, path, 10
      }
    ' "$previous_manifest" | LC_ALL=C sort -r | sed 's/^[0-9][0-9]* //'
  )

  rm -f "$destination_root/$manifest_name"
}

prepare_replacement_root() {
  language="$1"
  source_root="$bindings_dir/$language"
  generated_root="$temporary_output/generated/$language"
  destination_root="$replacement_root/$language"

  if [ -e "$source_root" ] || [ -L "$source_root" ]; then
    reject_symlinks "$source_root" "$language"
    remove_previous_generated_files "$destination_root" "$source_root/$manifest_name"
  else
    mkdir -p "$destination_root"
  fi

  cp -pR "$generated_root/." "$destination_root/"
  set_generated_modes "$generated_root" "$destination_root"
}
compare_root() {
  expected_root="$1"
  actual_root="$2"
  language="$3"
  failed=false

  if ! reject_symlinks "$actual_root" "$language"; then
    return 1
  fi
  if [ "$(mode_for "$expected_root")" != "$(mode_for "$actual_root")" ]; then
    echo "generated mode differs: $language" >&2
    failed=true
  fi

  expected_manifest="$expected_root/$manifest_name"
  actual_manifest="$actual_root/$manifest_name"
  if [ -L "$actual_manifest" ]; then
    echo "generated symlink is not allowed: $language/$manifest_name" >&2
    return 1
  fi
  if [ ! -f "$actual_manifest" ]; then
    echo "missing generated path: $language/$manifest_name" >&2
    return 1
  fi
  if ! cmp -s "$expected_manifest" "$actual_manifest"; then
    echo "generated content differs: $language/$manifest_name" >&2
    failed=true
  fi
  if [ "$(mode_for "$expected_manifest")" != "$(mode_for "$actual_manifest")" ]; then
    echo "generated mode differs: $language/$manifest_name" >&2
    failed=true
  fi

  while IFS=' ' read -r type path; do
    if ! validate_manifest_path "$path"; then
      echo "invalid generated manifest path: $language/$path" >&2
      failed=true
      continue
    fi
    expected="$expected_root/$path"
    actual="$actual_root/$path"
    if [ -L "$actual" ]; then
      echo "generated symlink is not allowed: $language/$path" >&2
      failed=true
      continue
    fi
    case "$type" in
      d)
        if [ ! -d "$actual" ]; then
          echo "generated path type differs: $language/$path" >&2
          failed=true
          continue
        fi
        ;;
      f)
        if [ ! -f "$actual" ]; then
          echo "generated path type differs: $language/$path" >&2
          failed=true
          continue
        fi
        if ! cmp -s "$expected" "$actual"; then
          echo "generated content differs: $language/$path" >&2
          failed=true
        fi
        ;;
      *)
        echo "invalid generated manifest entry: $language/$type $path" >&2
        failed=true
        continue
        ;;
    esac
    if [ "$(mode_for "$expected")" != "$(mode_for "$actual")" ]; then
      echo "generated mode differs: $language/$path" >&2
      failed=true
    fi
  done < "$expected_manifest"

  "$failed" && return 1 || return 0
}

prepare_complete_replacement_root() {
  if [ -L "$bindings_dir" ] || [ ! -d "$bindings_dir" ]; then
    echo "error: sdk-bindings root is missing, not a directory, or a symlink" >&2
    return 1
  fi

  replacement_root="$(mktemp -d "$workspace_dir/.sdk-bindings.new.XXXXXX")"
  cp -pR "$bindings_dir/." "$replacement_root/"
  for language in $languages; do
    prepare_replacement_root "$language"
  done
}

inject_transaction_point() {
  point="$1"
  if [ "${CYCLOPS_SDK_BINDINGS_TEST_FAIL_TRANSACTION_POINT:-}" = "$point" ]; then
    echo "error: injected generated binding transaction failure at $point" >&2
    return 1
  fi

  signal_spec="${CYCLOPS_SDK_BINDINGS_TEST_SIGNAL_TRANSACTION_POINT:-}"
  if [ -n "$signal_spec" ] && [ "$signal_spec" = "$point:${signal_spec#*:}" ]; then
    signal_name="${signal_spec#*:}"
    case "$signal_name" in
      HUP|INT|TERM)
        echo "error: injected generated binding transaction signal $signal_name at $point" >&2
        kill -s "$signal_name" "$$"
        ;;
      *)
        echo "error: invalid transaction signal injection: $signal_spec" >&2
        return 1
        ;;
    esac
  fi
}

rollback_transaction() {
  if [ "$transaction_state" = "finalized" ] || [ "$transaction_state" = "rolled_back" ]; then
    return 0
  fi

  if [ -n "$backup_root" ] && { [ -e "$backup_root" ] || [ -L "$backup_root" ]; }; then
    if [ -e "$bindings_dir" ] || [ -L "$bindings_dir" ]; then
      rm -rf "$bindings_dir" || {
        echo "error: could not remove interrupted replacement root" >&2
        return 1
      }
    fi
    mv "$backup_root" "$bindings_dir" || {
      echo "error: could not restore original sdk-bindings root from $backup_root" >&2
      return 1
    }
  fi

  transaction_state="rolled_back"
}

cleanup_transaction() {
  cleanup_failed=false
  if [ "$transaction_state" != "finalized" ]; then
    if ! rollback_transaction; then
      cleanup_failed=true
      echo "error: original sdk-bindings backup retained at $backup_root" >&2
    fi
  fi

  rm -rf "$temporary_output"
  if [ -n "$replacement_root" ] && { [ -e "$replacement_root" ] || [ -L "$replacement_root" ]; }; then
    rm -rf "$replacement_root"
  fi
  if [ -n "$backup_container" ] && [ ! -e "$backup_root" ] && [ ! -L "$backup_root" ]; then
    # Removing the temporary backup parent is best-effort exit cleanup.
    rmdir "$backup_container" 2>/dev/null || true  # lint-ignore: error-masking
  fi

  "$cleanup_failed" && return 1 || return 0
}

handle_exit() {
  status="$?"
  trap - EXIT HUP INT TERM
  set +e
  if ! cleanup_transaction; then
    status=1
  fi
  exit "$status"
}

handle_signal() {
  signal_name="$1"
  trap - EXIT HUP INT TERM
  set +e
  if ! cleanup_transaction; then
    echo "error: failed to restore sdk-bindings after $signal_name" >&2
  fi
  exit 1
}

replace_binding_root() {
  backup_container="$(mktemp -d "$workspace_dir/.sdk-bindings.backup.XXXXXX")"
  backup_root="$backup_container/original"

  inject_transaction_point before-backup-move
  mv "$bindings_dir" "$backup_root"
  transaction_state="backup_moved"

  inject_transaction_point after-backup-move
  mv "$replacement_root" "$bindings_dir"
  transaction_state="new_live"

  inject_transaction_point after-new-live
  transaction_state="finalized"

  # Signals are ignored only after the root swap has fully committed.
  trap '' HUP INT TERM
  if ! rm -rf "$backup_container"; then
    echo "warning: could not remove finalized sdk-bindings backup: $backup_container" >&2
  fi
  trap 'handle_signal HUP' HUP
  trap 'handle_signal INT' INT
  trap 'handle_signal TERM' TERM
}

trap 'handle_exit' EXIT
trap 'handle_signal HUP' HUP
trap 'handle_signal INT' INT
trap 'handle_signal TERM' TERM
cd "$workspace_dir"
metadata_output="$temporary_output/cargo-metadata.json"
if ! "$cargo_bin" metadata --locked --format-version 1 --no-deps --manifest-path "$workspace" > "$metadata_output"; then
  echo "error: cargo metadata failed while resolving the target directory" >&2
  cat "$metadata_output" >&2
  exit 1
fi
build_messages="$temporary_output/cargo-build-messages.json"
if ! "$cargo_bin" build --locked --release --manifest-path "$workspace" -p cyclops-sdk \
  --message-format=json-render-diagnostics > "$build_messages"; then
  echo "error: cargo failed to build cyclops-sdk" >&2
  cat "$build_messages" >&2
  exit 1
fi
if ! library="$("$cargo_bin" run --quiet --locked --manifest-path "$workspace" -p cyclops-sdk-bindgen --target "$host_triple" -- \
  resolve-cdylib --sdk-manifest "$workspace_dir/sdk/Cargo.toml" --metadata "$metadata_output" \
  --build-messages "$build_messages")"; then
  echo "error: could not resolve the cyclops-sdk cdylib from Cargo JSON output" >&2
  exit 1
fi
if [ ! -f "$library" ]; then
  echo "error: resolved cyclops-sdk cdylib does not exist: $library" >&2
  exit 1
fi

raw_output="$temporary_output/raw"
generated_root="$temporary_output/generated"
mkdir -p "$raw_output" "$generated_root/python/cyclops_sdk" "$generated_root/kotlin" \
  "$generated_root/swift" "$generated_root/ruby/cyclops_sdk"
"$cargo_bin" run --locked --manifest-path "$workspace" -p cyclops-sdk-bindgen --target "$host_triple" -- \
  generate --library "$library" \
  --language python --language kotlin --language swift --language ruby \
  --out-dir "$raw_output" --no-format

mv "$raw_output/cyclops_sdk.py" "$generated_root/python/cyclops_sdk/_sdk.py"
mv "$raw_output/cyclops_sdk_schema.py" "$generated_root/python/cyclops_sdk/_schema.py"
write_python_facade \
  "$generated_root/python/cyclops_sdk/_sdk.py" \
  "$generated_root/python/cyclops_sdk/_schema.py" \
  "$generated_root/python/cyclops_sdk/__init__.py"

mv "$raw_output/ai" "$generated_root/kotlin/"
mv "$raw_output/CyclopsSdk.swift" "$generated_root/swift/"
mv "$raw_output/CyclopsSdkSchema.swift" "$generated_root/swift/"
mv "$raw_output/CyclopsSdkFFI.h" "$generated_root/swift/"
mv "$raw_output/CyclopsSdkFFI.modulemap" "$generated_root/swift/"
mv "$raw_output/CyclopsSdkSchemaFFI.h" "$generated_root/swift/"
mv "$raw_output/CyclopsSdkSchemaFFI.modulemap" "$generated_root/swift/"
cat "$generated_root/swift/CyclopsSdkFFI.modulemap" "$generated_root/swift/CyclopsSdkSchemaFFI.modulemap" > "$generated_root/swift/CyclopsSdk.modulemap"

mv "$raw_output/cyclops_sdk.rb" "$generated_root/ruby/cyclops_sdk/sdk.rb"
mv "$raw_output/cyclops_sdk_schema.rb" "$generated_root/ruby/cyclops_sdk/schema.rb"
# UniFFI 0.31.0 emits `OsGym` in cross-crate Ruby helper references while the
# schema component exports `OSGym`; normalize the generated helper calls.
sed -i.bak "s/check_lower_TypeOsGym/check_lower_TypeOSGym/g" "$generated_root/ruby/cyclops_sdk/sdk.rb"
rm "$generated_root/ruby/cyclops_sdk/sdk.rb.bak"
# UniFFI 0.31.0 lowers a foreign callback interface as an object clone. Ruby
# callback implementations have no Rust handle, so avoid cloning nil handles.
python3 - "$generated_root/ruby/cyclops_sdk/sdk.rb" <<'PYTHON_RUBY_PATCH'
import pathlib
import sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = """  def uniffi_clone_handle()
    return CyclopsSdk.rust_call(
      :uniffi_cyclops_sdk_fn_clone_httpclient,
      @handle
    )
  end
"""
new = """  def uniffi_clone_handle()
    return 0 if @handle.nil?
    return CyclopsSdk.rust_call(
      :uniffi_cyclops_sdk_fn_clone_httpclient,
      @handle
    )
  end
"""
if old not in text:
    raise SystemExit("expected HttpClient clone method not found")
path.write_text(text.replace(old, new, 1))
PYTHON_RUBY_PATCH
write_ruby_facade \
  "$generated_root/ruby/cyclops_sdk/sdk.rb" \
  "$generated_root/ruby/cyclops_sdk/schema.rb" \
  "$generated_root/ruby/cyclops_sdk.rb"

normalize_generated_text
for language in $languages; do
  find -P "$generated_root/$language" -type d -exec chmod 755 {} \;
  find -P "$generated_root/$language" -type f -exec chmod 644 {} \;
  write_manifest "$generated_root/$language" "$temporary_output/$language.manifest"
  mv "$temporary_output/$language.manifest" "$generated_root/$language/$manifest_name"
  chmod 644 "$generated_root/$language/$manifest_name"
done

if "$check_only"; then
  check_failed=false
  for language in $languages; do
    if ! compare_root "$generated_root/$language" "$bindings_dir/$language" "$language"; then
      check_failed=true
    fi
  done
  "$check_failed" && exit 1
  echo "SDK bindings are up to date."
  exit 0
fi


prepare_complete_replacement_root
replace_binding_root

echo "Generated SDK bindings in $bindings_dir."
