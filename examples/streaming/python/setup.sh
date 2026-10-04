#!/usr/bin/env bash
# Builds the cua-sdk native library and installs the `cua` Python binding
# into examples/streaming/python/.venv (Python 3.12, uv).
#
#   ./setup.sh             # cargo build --release -p cua-sdk, then install
#   ./setup.sh --no-build  # reuse an existing target/release library
#
# CARGO_TARGET_DIR defaults to libs/cua/target.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CUA="$REPO/libs/cua"
TARGET_DIR="${CARGO_TARGET_DIR:-$CUA/target}"
case "$(uname -s)" in
    Darwin) LIB=libcua_sdk.dylib ;;
    MINGW*|MSYS*|CYGWIN*) LIB=cua_sdk.dll ;;
    *) LIB=libcua_sdk.so ;;
esac

if [ "${1:-}" != "--no-build" ]; then
    (cd "$CUA" && CARGO_TARGET_DIR="$TARGET_DIR" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
        cargo build --release -p cua-sdk)
fi
[ -f "$TARGET_DIR/release/$LIB" ] || { echo "missing $TARGET_DIR/release/$LIB" >&2; exit 1; }

uv venv --quiet --allow-existing --python 3.12 "$HERE/.venv"
# Non-editable install of the pure-Python part (cua/_native.py etc.) ...
uv pip install --quiet --reinstall --no-deps --python "$HERE/.venv/bin/python" "$CUA/python"
# ... plus the freshly built native library next to _native.py.
SITE="$("$HERE/.venv/bin/python" -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')"
cp "$TARGET_DIR/release/$LIB" "$SITE/cua/$LIB"
"$HERE/.venv/bin/python" -c 'import cua; print("cua", cua.cua_sdk_version(), "ok")'
