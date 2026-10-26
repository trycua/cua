#!/usr/bin/env bash
# Install this repo's lefthook hooks without touching a global core.hooksPath.
#
#   brew install lefthook gitleaks
#   scripts/hooks/install.sh
#
# Why not just `lefthook install`? When core.hooksPath is set (we set it globally
# on agent and dev machines, to a directory that holds shared push guards),
# lefthook refuses, and `lefthook install --force` then rewrites the hooks in
# that shared directory. Never use --force or --reset-hooks-path here.
#
# With a hooksPath set, this script writes only small hooks (one per hook in
# lefthook.yml) into the repo's own hooks directory. The shared dispatcher in the
# hooksPath runs them after its own guards (it chains to
# $(git rev-parse --git-common-dir)/hooks/).
# Without a hooksPath, it runs a plain `lefthook install`.
set -eu

cd "$(git rev-parse --show-toplevel)"

if ! command -v lefthook >/dev/null 2>&1; then
  echo "lefthook not found. Install it first: brew install lefthook gitleaks" >&2
  exit 1
fi

if ! hooks_path=$(git config --get core.hooksPath); then
  lefthook install
  exit 0
fi

common_dir=$(cd "$(git rev-parse --git-common-dir)" && pwd)
mkdir -p "$common_dir/hooks"

# Every git hook that lefthook.yml defines.
hooks=$(grep -E '^(pre-commit|prepare-commit-msg|commit-msg|post-commit|pre-push|pre-rebase|post-checkout|post-merge|post-rewrite):' lefthook.yml | sed 's/:.*//')
if [ -z "$hooks" ]; then
  echo "No git hooks found in lefthook.yml." >&2
  exit 1
fi

for hook in $hooks; do
  shim="$common_dir/hooks/$hook"
  if [ -e "$shim" ] && ! grep -q lefthook "$shim"; then
    echo "Not overwriting $shim: it is not a lefthook hook. Merge it by hand." >&2
    exit 1
  fi
  cat >"$shim" <<SHIM
#!/bin/sh
# $hook hook for lefthook, installed by scripts/hooks/install.sh.
# Runs the repo's lefthook.yml; skips quietly when lefthook is not installed.
# --no-auto-install: lefthook must never try to (re)install hooks into a hooksPath.
if command -v lefthook >/dev/null 2>&1; then
  exec lefthook run $hook --no-auto-install "\$@"
fi
echo "lefthook not found; skipping the $hook checks (brew install lefthook)." >&2
exit 0
SHIM
  chmod +x "$shim"
  echo "Installed $shim"
  if ! grep -qs 'git-common-dir' "$hooks_path/$hook"; then
    echo "Warning: $hooks_path/$hook does not chain to the repo's own hooks, so the" >&2
    echo "lefthook $hook checks will not run. Make it run \$(git rev-parse --git-common-dir)/hooks/$hook." >&2
  fi
done

echo "core.hooksPath is $hooks_path and was not modified."
