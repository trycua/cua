#!/usr/bin/env bash
# Run only in a disposable Arch Linux guest/container; never replace the installed driver.
set -euo pipefail

if [[ $# != 3 || ${CUA_PACMAN_TEST_DISPOSABLE:-} != 1 ]]; then
  echo "Usage: CUA_PACMAN_TEST_DISPOSABLE=1 $0 CANDIDATE TEST_BINARY NEW_EVIDENCE_DIR" >&2
  exit 2
fi
if [[ $(uname -s) != Linux || $EUID != 0 || ! -x /usr/bin/pacman ]]; then
  echo "Requires root in a disposable Linux guest with /usr/bin/pacman." >&2
  exit 2
fi
if ! grep -qx 'ID=arch' /etc/os-release; then
  echo "Requires a disposable Arch Linux guest/container." >&2
  exit 2
fi

candidate=$(realpath -- "$1")
test_binary=$(realpath -- "$2")
if [[ ! -x $candidate || ! -x $test_binary ]]; then
  echo "Candidate and integration-test binary must be executable." >&2
  exit 2
fi
if /usr/bin/pacman -Q cua-driver-pacman-test >/dev/null 2>&1 ||
   [[ -e /usr/lib/cua-driver-pacman-test || -L /usr/lib/cua-driver-pacman-test ]]; then
  echo "Refusing to overwrite an existing test package or payload path." >&2
  exit 2
fi

# A new directory prevents accidental overwrites and retains all native evidence.
mkdir -- "$3"
evidence=$(realpath -- "$3")
exec > >(tee "$evidence/run.log") 2>&1
echo "Evidence: $evidence"
{
  echo "source_sha=${CUA_E2E_SOURCE_SHA:?Set CUA_E2E_SOURCE_SHA to the tested commit}"
  cat /etc/os-release
  /usr/bin/pacman --version
  rustc --version
  cargo --version
  sha256sum -- "$candidate" "$test_binary"
} | tee "$evidence/provenance.txt"
"$candidate" --version
"$test_binary" --list > "$evidence/tests.txt"
for test in managed_cli_checks_and_apply_return_package_guidance managed_mcp_check_returns_same_unavailable_state fake_path_pacman_cannot_disable_unmanaged_channel_switching; do
  if ! grep -Fx "pacman::$test: test" "$evidence/tests.txt"; then
    echo "Integration-test binary is missing required test: $test" >&2
    exit 2
  fi
done

installed=0
cleanup() {
  local result=$?
  trap - EXIT
  if [[ $installed == 1 ]]; then
    if ! /usr/bin/pacman -R --noconfirm cua-driver-pacman-test; then
      echo "Failed to remove task package cua-driver-pacman-test." >&2
      result=1
    elif [[ -e /usr/lib/cua-driver-pacman-test || -L /usr/lib/cua-driver-pacman-test ]]; then
      echo "Task package payload remains after removal." >&2
      result=1
    fi
  fi
  echo "Native pacman test exit: $result; evidence retained at $evidence"
  exit "$result"
}
trap cleanup EXIT

install -m 755 -- "$candidate" "$evidence/unmanaged-cua-driver"
test_unmanaged() {
  if /usr/bin/pacman -Qoq -- "$evidence/unmanaged-cua-driver"; then
    echo "Unmanaged copy unexpectedly has a package owner." >&2
    exit 1
  fi
  PACMAN_TEST_UNMANAGED_EXECUTABLE="$evidence/unmanaged-cua-driver" \
    timeout 60 "$test_binary" \
    --exact pacman::fake_path_pacman_cannot_disable_unmanaged_channel_switching \
    --nocapture --test-threads=1
}
echo "Testing unmanaged candidate before package installation"
test_unmanaged

mkdir -p -- "$evidence/package/usr/lib/cua-driver-pacman-test"
install -m 755 -- "$candidate" "$evidence/package/usr/lib/cua-driver-pacman-test/cua-driver"
cat > "$evidence/package/.PKGINFO" <<EOF
pkgname = cua-driver-pacman-test
pkgbase = cua-driver-pacman-test
pkgver = 1.0.0-1
pkgdesc = Disposable native package ownership test fixture
builddate = $(date +%s)
packager = Cua test harness
size = $(stat -c %s -- "$candidate")
arch = $(uname -m)
license = MIT
EOF
tar -czf "$evidence/cua-driver-pacman-test.pkg.tar.gz" -C "$evidence/package" .PKGINFO usr
/usr/bin/pacman -U --noconfirm "$evidence/cua-driver-pacman-test.pkg.tar.gz"
installed=1
/usr/bin/pacman -Qoq -- /usr/lib/cua-driver-pacman-test/cua-driver
cmp -- "$candidate" /usr/lib/cua-driver-pacman-test/cua-driver
ln -s /usr/lib/cua-driver-pacman-test/cua-driver "$evidence/managed-symlink"

for executable in /usr/lib/cua-driver-pacman-test/cua-driver "$evidence/managed-symlink"; do
  echo "Testing managed executable: $executable"
  for test in managed_cli_checks_and_apply_return_package_guidance managed_mcp_check_returns_same_unavailable_state; do
    PACMAN_TEST_MANAGED_EXECUTABLE="$executable" CUA_TEST_DRIVER_BIN="$executable" \
      CUA_TEST_DRIVER_STDERR=1 timeout 180 "$test_binary" \
      --ignored --exact "pacman::$test" --nocapture --test-threads=1
  done
done

echo "Testing unmanaged copy alongside installed package"
test_unmanaged
/usr/bin/pacman -R --noconfirm cua-driver-pacman-test
installed=0
if /usr/bin/pacman -Q cua-driver-pacman-test >/dev/null 2>&1 ||
   [[ -e /usr/lib/cua-driver-pacman-test || -L /usr/lib/cua-driver-pacman-test ]]; then
  echo "Task package or payload remains after removal." >&2
  exit 1
fi
echo "Testing unmanaged candidate after verified package removal"
test_unmanaged
