#!/bin/zsh
set -euo pipefail

# Install the Codex 0.147 code-mode companion into a disposable macOS seed.
# The source path is deliberately not persisted or printed.

expected_sha256=a059beb029cdbc989e72e23f8680be9f703cb6cf83d9598d91041f82178d018d
expected_team_id=2DC432GLL2
target=/usr/local/bin/codex-code-mode-host

if [[ ${EUID} -ne 0 ]]; then
  print -u2 "install-cdb-codex-companion: root is required"
  exit 64
fi
if [[ $# -ne 1 || $1 != /* || ! -f $1 || -L $1 ]]; then
  print -u2 "usage: install-cdb-codex-companion.sh /absolute/path/to/codex-code-mode-host"
  exit 64
fi

source_binary=$1
staged_binary=$(/usr/bin/mktemp /private/tmp/cdb-codex-companion.XXXXXX)
trap '/bin/rm -f "$staged_binary"' EXIT
/usr/bin/install -o root -g wheel -m 0755 "$source_binary" "$staged_binary"

actual_sha256=$(/usr/bin/shasum -a 256 "$staged_binary" | /usr/bin/awk '{print $1}')
if [[ $actual_sha256 != $expected_sha256 ]]; then
  print -u2 "install-cdb-codex-companion: source digest mismatch"
  exit 65
fi
if ! /usr/bin/file -b "$staged_binary" | /usr/bin/grep -Fq 'Mach-O 64-bit executable arm64'; then
  print -u2 "install-cdb-codex-companion: source architecture mismatch"
  exit 65
fi
if ! /usr/bin/codesign --verify --strict "$staged_binary" >/dev/null 2>&1; then
  print -u2 "install-cdb-codex-companion: source signature is invalid"
  exit 65
fi
team_id=$(
  /usr/bin/codesign -dv --verbose=2 "$staged_binary" 2>&1 |
    /usr/bin/awk -F= '$1 == "TeamIdentifier" {print $2}'
)
if [[ $team_id != $expected_team_id ]]; then
  print -u2 "install-cdb-codex-companion: source signer mismatch"
  exit 65
fi

/usr/bin/install -d -o root -g wheel -m 0755 /usr/local/bin
/usr/bin/install -o root -g wheel -m 0755 "$staged_binary" "$target"

installed_sha256=$(/usr/bin/shasum -a 256 "$target" | /usr/bin/awk '{print $1}')
installed_identity=$(/usr/bin/stat -f '%Su:%Sg:%Lp' "$target")
if [[ $installed_sha256 != $expected_sha256 || $installed_identity != root:wheel:755 ]]; then
  print -u2 "install-cdb-codex-companion: installed identity mismatch"
  exit 66
fi
/usr/bin/codesign --verify --strict "$target" >/dev/null 2>&1
/usr/bin/printf '%s  %s\n' "$installed_sha256" "$target"
