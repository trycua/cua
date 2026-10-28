#!/usr/bin/env bash
# Layering guard for the elastic pool (CUA-1962): dependencies point down only.
#
#   surfaces (cua-cli, cua-sdk, cua-spaces-cli, cua-spaces-ffi, cua-compat,
#             cua-daemon, cua-spaces-cluster, cua-spaces)
#     -> cua-spaces::elastic (orchestration)
#     -> cua-cluster::elastic (pure solver)  +  cua-sandbox-core::byoc (contract)
#     -> cua-byoc::hosts (one host lifecycle) -> cua-byoc::{aws,gcp,hetzner,modal}
#
# Fails when:
#   1. a surface or orchestration crate names a provider module
#      (cua_byoc::aws::, ::gcp::, ::hetzner::, ::modal::): providers are
#      reached only through the contract (CloudApi, CloudManager);
#   2. cua-cluster (the solver's crate) depends on cua-byoc, cua-spaces,
#      cua-sandbox-core or an HTTP client: the solver stays pure;
#   3. cua-cluster's elastic module does I/O (std::fs, std::net, tokio::fs,
#      reqwest) outside its tests;
#   4. a provider module mentions the solver (cua_cluster::elastic): a
#      provider never decides what to buy or where to place.
#
#   scripts/check-elastic-layering.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/libs/cua/crates"

fail=0
say() { echo "check-elastic-layering: $*" >&2; fail=1; }

surfaces=(cua-cli cua-sdk cua-spaces-cli cua-spaces-ffi cua-compat cua-daemon cua-spaces-cluster cua-spaces)
for c in "${surfaces[@]}"; do
  [ -d "$c/src" ] || continue
  if hits=$(grep -rnE 'cua_byoc::(aws|gcp|hetzner|modal)::' "$c/src" 2>/dev/null); then
    say "$c names a provider module (go through CloudApi/CloudManager):"
    echo "$hits" >&2
  fi
done

deps=$(sed -n '/^\[dependencies\]/,/^\[/p' cua-cluster/Cargo.toml)
for d in cua-byoc cua-spaces cua-sandbox-core reqwest hyper; do
  if grep -qE "^$d([ .=]|$)" <<<"$deps"; then
    say "cua-cluster depends on $d: the solver stays pure"
  fi
done

for f in cua-cluster/src/elastic/*.rs; do
  case "$f" in *tests.rs|*sim_tests.rs) continue ;; esac
  # The `tests` modules at the end of a file may use the filesystem.
  body=$(awk '/^#\[cfg\(test\)\]/{exit} {print}' "$f")
  if grep -nE 'std::fs::|std::net::|tokio::fs|reqwest::' <<<"$body" | grep -v 'read_link("/etc/localtime")' >/dev/null; then
    hits=$(grep -nE 'std::fs::|std::net::|tokio::fs|reqwest::' <<<"$body" | grep -v 'read_link')
    if [ -n "$hits" ]; then
      say "$f does I/O (the solver is pure; the time-zone lookup in geo.rs is the one exception):"
      echo "$hits" >&2
    fi
  fi
done

for p in aws gcp hetzner modal; do
  if grep -n 'cua_cluster::elastic' "cua-byoc/src/$p.rs" >/dev/null 2>&1; then
    say "cua-byoc/src/$p.rs mentions the solver: providers only declare and create"
  fi
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "check-elastic-layering: ok"
