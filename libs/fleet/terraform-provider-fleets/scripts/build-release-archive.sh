#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <version> <dist-dir>" >&2
  exit 2
fi

version=$1
dist_dir=$2
provider_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ -f "$provider_root/native/Cargo.toml" ]; then
  native_root="$provider_root/native"
elif [ -f "$provider_root/../Cargo.toml" ]; then
  native_root="$(cd "$provider_root/.." && pwd)"
else
  echo "Cyclops SDK Rust workspace not found" >&2
  exit 1
fi

bindings_root="$native_root/sdk-bindings/go-uniffi"
target_dir="${CYCLOPS_SDK_TARGET_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/terraform-provider-fleets/cyclops-sdk}"
goos="$(go env GOOS)"
goarch="$(go env GOARCH)"
go_build_cc="${GO_BUILD_CC:-$(go env CC)}"
rust_target="${RUST_TARGET:-}"
cargo_target_args=()

if [ "$goos" = windows ] && command -v cygpath >/dev/null 2>&1; then
  native_root_for_tools="$(cygpath -m "$native_root")"
  bindings_root_for_tools="$(cygpath -m "$bindings_root")"
  target_dir_for_tools="$(cygpath -m "$target_dir")"
else
  native_root_for_tools="$native_root"
  bindings_root_for_tools="$bindings_root"
  target_dir_for_tools="$target_dir"
fi

target_release_dir="$target_dir_for_tools/release"
if [ -n "$rust_target" ]; then
  cargo_target_args=(--target "$rust_target")
  target_release_dir="$target_dir_for_tools/$rust_target/release"
  rust_target_env="${rust_target^^}"
  rust_target_env="${rust_target_env//-/_}"
  export "CARGO_TARGET_${rust_target_env}_LINKER=$go_build_cc"
  export "CC_${rust_target//-/_}=$go_build_cc"
fi

case "$goos" in
  linux|darwin)
    static_library="$target_release_dir/deps/libcyclops_sdk.a"
    executable="terraform-provider-fleets_v${version}"
    ;;
  windows)
    if [ -n "$rust_target" ]; then
      static_library="$target_release_dir/deps/libcyclops_sdk.a"
    else
      static_library="$target_release_dir/deps/cyclops_sdk.lib"
    fi
    executable="terraform-provider-fleets_v${version}.exe"
    ;;
  *)
    echo "unsupported release platform: $goos/$goarch" >&2
    exit 1
    ;;
esac

mkdir -p "$dist_dir"
build_log="$(mktemp)"
trap 'rm -f "$build_log"' EXIT

CARGO_TERM_COLOR=never cargo rustc \
  --locked \
  --manifest-path "$native_root_for_tools/Cargo.toml" \
  --package cyclops-sdk \
  --release \
  "${cargo_target_args[@]}" \
  --target-dir "$target_dir_for_tools" \
  -- \
  --crate-type staticlib \
  --print native-static-libs 2>&1 | tee "$build_log"

[ -f "$static_library" ] || {
  echo "native SDK static library was not built: $static_library" >&2
  exit 1
}

native_static_libs="$(sed -n 's/.*native-static-libs: //p' "$build_log" | tail -1)"
[ -n "$native_static_libs" ] || {
  echo "Rust did not report native static libraries" >&2
  exit 1
}

if [ "$goos" = windows ]; then
  windows_linker_flags=()
  for library in $native_static_libs; do
    case "$library" in
      /defaultlib:*.lib)
        windows_linker_flags+=("-l${library#/defaultlib:}")
        ;;
      /defaultlib:*)
        windows_linker_flags+=("-l${library#/defaultlib:}")
        ;;
      *.lib)
        packaged_library=
        cargo_registry_src="${CARGO_HOME:-$HOME/.cargo}/registry/src"
        if [ -d "$cargo_registry_src" ]; then
          while IFS= read -r candidate; do
            packaged_library="$candidate"
            break
          done < <(find "$cargo_registry_src" -type f -name "$library" -print)
        fi
        if [ -n "$packaged_library" ]; then
          windows_linker_flags+=("$(cygpath -m "$packaged_library")")
        else
          windows_linker_flags+=("-l${library%.lib}")
        fi
        ;;
      *)
        windows_linker_flags+=("$library")
        ;;
    esac
  done
  native_static_libs="${windows_linker_flags[*]}"
fi

build_dir="$(mktemp -d)"
trap 'rm -f "$build_log"; rm -rf "$build_dir"' EXIT

(
  cd "$provider_root"
  CGO_ENABLED=1 \
  CC="$go_build_cc" \
  CGO_CFLAGS="-I$bindings_root_for_tools/fleet_sdk -I$bindings_root_for_tools/cyclops_sdk_schema" \
  CGO_LDFLAGS="$static_library $native_static_libs" \
  go build \
    -trimpath \
    -ldflags "-s -w -X main.version=$version" \
    -o "$build_dir/$executable" \
    .
)

case "$goos" in
  linux)
    if ldd "$build_dir/$executable" | grep -q cyclops_sdk; then
      echo "provider has a runtime dependency on libcyclops_sdk" >&2
      exit 1
    fi
    ;;
  darwin)
    if otool -L "$build_dir/$executable" | grep -q cyclops_sdk; then
      echo "provider has a runtime dependency on libcyclops_sdk" >&2
      exit 1
    fi
    ;;
esac

archive="$(cd "$dist_dir" && pwd)/terraform-provider-fleets_${version}_${goos}_${goarch}.zip"
if command -v python3 >/dev/null 2>&1; then
  python_bin=python3
else
  python_bin=python
fi
"$python_bin" -m zipfile -c "$archive" "$build_dir/$executable"
printf '%s\n' "$archive"
