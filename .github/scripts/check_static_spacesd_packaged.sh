#!/usr/bin/env bash
# check_static_spacesd_packaged.sh DIR [VERSION]
#
# Drift guard for the release packages: checks that every Linux cua CLI package in DIR ships a static cua-spacesd
# where find_binary (libs/cua/crates/cua-vmm/src/inject.rs) looks after a
# normal install: `cua-spacesd-linux-<arch>` in the same directory as the
# `cua` executable (or ~/.cua/bin, or on PATH).
#
# DIR holds downloaded release artifacts, flat or in per-artifact
# subdirectories (gh run download layout):
#   *.tgz   npm packages; trycua-cua-linux-{x64,arm64}-gnu-* are checked
#   *.whl   wheels; manylinux x86_64/aarch64 are checked
#   *.deb   any deb; checked when it contains a `cua` ELF
#   cua-cli-*-linux-*.tar.gz, cua-*-linux-*-gnu.tar.gz   install.sh archives
#   native-linux-*/   extracted native artifacts
#
# Per package: cua and cua-spacesd-linux-<arch> must be siblings, and the
# latter must be a 64-bit little-endian ELF of the right machine with no
# PT_INTERP (static musl). If VERSION is given, the binary must contain it.
# Exit status: 0 all pass, 1 any package fails, 2 nothing to check.
# Extracts into a scratch dir ($RUNNER_TEMP or $TMPDIR) and removes it on exit.
set -u

DIR="${1:?usage: verify.sh DIR [VERSION]}"
VERSION="${2:-}"
WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/spacesd-check.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
FAILED=()
CHECKED=0

# Prints the reason and returns 1 when FILE is not a static ELF for ARCH.
elf_check() { # FILE ARCH
  python3 - "$1" "$2" <<'PY'
import struct, sys
p, arch = sys.argv[1], sys.argv[2]
with open(p, "rb") as f:
    b = f.read(1 << 20)  # headers sit at the start
if len(b) < 64 or b[:4] != b"\x7fELF":
    print("not an ELF file"); sys.exit(1)
if b[4] != 2 or b[5] != 1:
    print("not 64-bit little-endian"); sys.exit(1)
mach = struct.unpack_from("<H", b, 18)[0]
want = {"x86_64": 62, "aarch64": 183}[arch]
if mach != want:
    print("wrong architecture (e_machine %d, want %d for %s)" % (mach, want, arch)); sys.exit(1)
phoff = struct.unpack_from("<Q", b, 32)[0]
phentsize, phnum = struct.unpack_from("<HH", b, 54)
for i in range(phnum):
    o = phoff + i * phentsize
    if struct.unpack_from("<I", b, o)[0] == 3:  # PT_INTERP
        off, = struct.unpack_from("<Q", b, o + 8)
        sz, = struct.unpack_from("<Q", b, o + 32)
        print("dynamically linked, interpreter " + b[off:off + sz].rstrip(b"\0").decode()); sys.exit(1)
PY
}

fail() { echo "FAIL  $1: $2"; FAILED+=("$1"); }

# check_dir NAME DIR ARCH: DIR holds the unpacked package.
check_dir() {
  local name="$1" d="$2" arch="$3" cua bin why other
  CHECKED=$((CHECKED + 1))
  cua="$(find "$d" -type f -name cua | head -n1)"
  if [ -z "$cua" ]; then fail "$name" "no cua executable in the package"; return; fi
  bin="$(dirname "$cua")/cua-spacesd-linux-$arch"
  if [ ! -f "$bin" ]; then
    other="$(find "$d" -type f -name 'cua-spacesd*' | head -n3 | tr '\n' ' ')"
    fail "$name" "no cua-spacesd-linux-$arch next to ${cua#"$d"/}${other:+ (elsewhere: $other)}"
    return
  fi
  if ! why="$(elf_check "$bin" "$arch")"; then fail "$name" "${bin#"$d"/}: $why"; return; fi
  if [ -n "$VERSION" ] && ! grep -aqF "$VERSION" "$bin"; then
    fail "$name" "${bin#"$d"/} does not contain version string $VERSION"; return
  fi
  echo "ok    $name: ${bin#"$d"/} (static, $arch, $(wc -c <"$bin" | tr -d ' ') bytes)"
}

arch_of() {
  case "$1" in
    *x64* | *x86_64* | *amd64*) echo x86_64 ;;
    *arm64* | *aarch64*) echo aarch64 ;;
  esac
}

n=0
while IFS= read -r f; do
  base="$(basename "$f")"; n=$((n + 1)); x="$WORK/$n"
  case "$base" in
    trycua-cua-linux-*-gnu-*.tgz)
      mkdir -p "$x"; tar -xzf "$f" -C "$x" || { fail "$base" "cannot unpack"; continue; }
      check_dir "npm $base" "$x" "$(arch_of "$base")" ;;
    *.whl)
      case "$base" in *manylinux*x86_64*) a=x86_64 ;; *manylinux*aarch64*) a=aarch64 ;; *) continue ;; esac
      mkdir -p "$x"; python3 -m zipfile -e "$f" "$x" || { fail "$base" "cannot unpack"; continue; }
      check_dir "wheel $base" "$x" "$a" ;;
    *.deb)
      mkdir -p "$x"
      if command -v dpkg-deb >/dev/null 2>&1; then dpkg-deb -x "$f" "$x" 2>/dev/null
      else (cd "$x" && ar x "$f" && tar -xf data.tar.* 2>/dev/null); fi
      found=""
      while IFS= read -r c; do
        if head -c4 "$c" | grep -aq ELF; then found="$c"; break; fi
      done < <(find "$x" -type f -name cua 2>/dev/null)
      [ -n "$found" ] || { echo "skip  $base: no cua CLI inside"; continue; }
      a="$(arch_of "$base")"; [ -n "$a" ] || a=x86_64
      check_dir "deb $base" "$x" "$a" ;;
    cua-cli-*-linux-*.tar.gz | cua-*-linux-*-gnu.tar.gz)
      a="$(arch_of "$base")"; [ -n "$a" ] || continue
      mkdir -p "$x"; tar -xzf "$f" -C "$x" || { fail "$base" "cannot unpack"; continue; }
      check_dir "archive $base" "$x" "$a" ;;
  esac
  rm -rf "$x"  # keep disk use modest
done < <(find "$DIR" -type f \( -name '*.tgz' -o -name '*.whl' -o -name '*.deb' -o -name '*.tar.gz' \) | sort)

while IFS= read -r d; do
  a="$(arch_of "$(basename "$d")")"; [ -n "$a" ] || continue
  check_dir "native dir $(basename "$d")" "$d" "$a"
done < <(find "$DIR" -type d -name 'native-linux-*' | sort)

echo "checked $CHECKED package(s), ${#FAILED[@]} failed"
[ "$CHECKED" -gt 0 ] || { echo "nothing to check in $DIR" >&2; exit 2; }
if [ "${#FAILED[@]}" -gt 0 ]; then
  printf 'FAILED: %s\n' "${FAILED[@]}"
  exit 1
fi
