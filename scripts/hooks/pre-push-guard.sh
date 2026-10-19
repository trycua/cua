#!/usr/bin/env bash
# lefthook pre-push guard for trycua/cua. Usage (from lefthook, with the git
# pre-push stdin attached): pre-push-guard.sh <remote-name-or-url> <remote-url>
#
# Blocks, before anything leaves the machine:
#   1. a push to main on trycua/cua (open a PR instead);
#   2. a push to trycua/cua of a ref whose history has a root commit that is not
#      cua's own, i.e. another repository's history (the 13 Oct 2026 leak);
#   3. any push that carries more than CUA_MAX_PUSH_COMMITS new commits
#      (default 300). A real PR branch is far smaller; a mirrored history is not;
#   4. high-confidence secrets (provider-pattern gitleaks rules) in the new
#      commits, and tailnet hostnames (*.ts.net) in the lines they add.
#
# Large legitimate pushes: CUA_ALLOW_LARGE_PUSH=1. Agents must not set it, or
# CUA_PUSH_GUARD_OVERRIDE, without Francesco's approval.
set -u

remote="${1:-}"
url="${2:-$remote}"
max_commits="${CUA_MAX_PUSH_COMMITS:-300}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
roots_file="$here/cua-root-commits"
zero=0000000000000000000000000000000000000000
# High-signal gitleaks rules only: the generic entropy rules flag test fixtures
# and would train people to bypass the hook.
rules="aws-access-token,private-key,slack-bot-token,slack-user-token,slack-app-token,slack-webhook-url,github-pat,github-fine-grained-pat,github-oauth,github-app-token,gcp-api-key,openai-api-key,anthropic-api-key,stripe-access-token,npm-access-token,pypi-upload-token,cloudflare-api-key,cloudflare-global-api-key,cloudflare-origin-ca-key,huggingface-access-token,sendgrid-api-token,twilio-api-key"

block() {
  echo "pre-push: BLOCKED: $*" >&2
  exit 1
}

to_cua=0
shopt -s nocasematch
[[ "$url" =~ github\.com[:/]+trycua/cua(\.git)?/?$ ]] && to_cua=1
shopt -u nocasematch

while read -r lref lsha rref rsha; do
  [ -z "${lref:-}" ] && continue
  [ "$lsha" = "$zero" ] && continue # deleting a ref

  if [ "$to_cua" = 1 ] && [ "$rref" = refs/heads/main ]; then
    block "pushing to main on trycua/cua is not allowed. Push a branch and open a PR."
  fi

  if [ "$to_cua" = 1 ]; then
    for root in $(git rev-list --max-parents=0 "$lsha" 2>/dev/null); do
      grep -qx "$root" "$roots_file" ||
        block "$lref contains history from another repository (root commit $root is not a cua root)."
    done
  fi

  # New commits: everything not already on a remote-tracking ref, or since the
  # remote's current tip when the ref exists there.
  if [ "$rsha" = "$zero" ] || ! git cat-file -e "$rsha^{commit}" 2>/dev/null; then
    range=("$lsha" --not --remotes)
  else
    range=("$rsha..$lsha")
  fi

  count=$(git rev-list --count "${range[@]}" 2>/dev/null || echo 0)
  if [ "$count" -gt "$max_commits" ] && [ "${CUA_ALLOW_LARGE_PUSH:-}" != 1 ]; then
    block "$lref carries $count new commits (limit $max_commits). Check you are on the right branch. Set CUA_ALLOW_LARGE_PUSH=1 only if this is intended."
  fi

  [ "$count" -eq 0 ] && continue

  if command -v gitleaks >/dev/null 2>&1; then
    log_opts="${range[*]}"
    if ! gitleaks git --no-banner --redact --enable-rule "$rules" --log-opts="$log_opts" >&2; then
      block "gitleaks found a likely secret in the commits for $lref (values redacted above)."
    fi
  else
    echo "pre-push: gitleaks not installed, skipping the secret scan (brew install gitleaks)." >&2
  fi

  if git log -p --no-color --format= "${range[@]}" 2>/dev/null |
    grep -E '^\+' | grep -vE '^\+\+\+' |
    grep -qE '[a-zA-Z0-9-]+\.tail[0-9a-f]{4,12}\.ts\.net'; then
    block "$lref adds a tailnet hostname (*.ts.net). Internal hostnames must not go in the public repo."
  fi
done

exit 0
