#!/bin/bash
# The macOS golden image recipe of `cua ci`: what a CI VM has installed
# before any job. It runs once, as the VM's admin user, inside a build
# Space made from the base macOS image (`cua ci golden refresh`); the
# result is checkpointed with its memory warm and every job VM is a fork
# of that checkpoint. It is idempotent: a second run only repairs.
#
# Pinned versions come in as environment variables the refresh job sets
# from the golden spec (so a version change changes the image digest):
#   CUA_CI_XCODE   expected `xcodebuild -version` (for example 26.0.1); the
#                  recipe refuses to continue on a mismatch instead of
#                  guessing. Unset, Xcode is optional (an image with only the
#                  command line tools still gets the rest)
#   CUA_CI_NODE    Node major or version (22)
#   CUA_CI_PNPM    pnpm version (10.4.1)
#   CUA_CI_RUST    Rust toolchain (stable, 1.90.0)
#   CUA_CI_BREW    extra Homebrew formulae, space separated
#
# Needs the network (Homebrew, rustup, npm registry). It is run by the
# refresh job only, never in a job's path.
set -euo pipefail
export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 NONINTERACTIVE=1
say() { printf '== %s\n' "$*"; }

say "xcode"
want="${CUA_CI_XCODE:-}"
have="$(xcodebuild -version 2>/dev/null | awk 'NR==1{print $2}' || true)"
if [ -n "$want" ] && [ "$have" != "$want" ]; then
  echo "the base image has Xcode ${have:-none}, the spec wants $want" >&2
  exit 70
fi
sudo -n xcodebuild -license accept 2>/dev/null || true
sudo -n xcodebuild -runFirstLaunch 2>/dev/null || true

say "homebrew"
if ! command -v brew >/dev/null 2>&1; then
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
eval "$(/opt/homebrew/bin/brew shellenv)"
brew install git git-lfs jq pkg-config cmake ninja sccache "node@${CUA_CI_NODE:-22}" ${CUA_CI_BREW:-}
brew link --overwrite --force "node@${CUA_CI_NODE:-22}" || true

say "pnpm"
corepack enable
corepack prepare "pnpm@${CUA_CI_PNPM:-latest}" --activate

say "rust"
if ! command -v rustup >/dev/null 2>&1; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${CUA_CI_RUST:-stable}"
fi
. "$HOME/.cargo/env"
rustup toolchain install "${CUA_CI_RUST:-stable}" --profile minimal -c clippy -c rustfmt
rustup default "${CUA_CI_RUST:-stable}"

say "caches are read-only mounts; the restore targets exist and are writable"
mkdir -p "$HOME/.cache/cua-ci/pnpm-store" "$HOME/.cache/cua-ci/sccache" \
  "$HOME/.cargo/registry" "$HOME/.cargo/git" "$HOME/Library/Caches/org.swift.swiftpm"

say "warm the toolchains (their pages stay hot in the memory checkpoint)"
tmp="$(mktemp -d)"
( cd "$tmp" \
  && mkdir -p swiftwarm && cd swiftwarm \
  && swift package init --type library --name Warm >/dev/null \
  && swift build >/dev/null ) || true
( cd "$tmp" && cargo new --quiet --bin rustwarm && cd rustwarm \
  && SCCACHE_DIR="$tmp/sccache" RUSTC_WRAPPER=sccache cargo build --quiet ) || true
( cd "$tmp" && mkdir -p jswarm && cd jswarm && echo '{"name":"w","version":"1.0.0"}' > package.json \
  && npm_config_store_dir="$tmp/store" pnpm --silent install --offline 2>/dev/null ) || true
git --version >/dev/null; { [ -z "$want" ] || xcodebuild -version >/dev/null; }; node --version >/dev/null; sccache --version >/dev/null
rm -rf "$tmp"

say "versions"
{
  echo "xcode=$(xcodebuild -version | awk 'NR==1{print $2}')"
  echo "swift=$(swift --version 2>&1 | head -1 | sed 's/.*version \([0-9.]*\).*/\1/')"
  echo "node=$(node --version | tr -d v)"
  echo "pnpm=$(pnpm --version)"
  echo "rustc=$(rustc --version | awk '{print $2}')"
  echo "git=$(git --version | awk '{print $3}')"
} | tee "$HOME/.cua-ci-versions"

say "clean"
rm -rf "$HOME/actions-runner/_work" "$HOME/actions-runner/_diag" 2>/dev/null || true
sync
