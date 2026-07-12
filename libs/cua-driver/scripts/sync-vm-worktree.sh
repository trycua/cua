#!/usr/bin/env bash
# Sync this host checkout to verification VMs and pull logs/artifacts back.
#
# Source changes should flow from this host to VMs. The guarded pull-code mode
# exists only for recovering intentional VM-side edits.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  sync-vm-worktree.sh push <ssh-target> [remote-dir]
  sync-vm-worktree.sh pull-artifacts <ssh-target> [remote-dir]
  ALLOW_PULL_CODE=1 sync-vm-worktree.sh pull-code <ssh-target> [remote-dir]

Modes:
  push
    Sync this host checkout to a VM. This is the normal way to update Linux
    and Windows verification machines.

  pull-artifacts
    Pull VM logs/artifacts from <remote-dir>/vm-out into this host checkout.

  pull-code
    Pull source changes made on a VM back to this host checkout. This is
    intentionally gated by ALLOW_PULL_CODE=1 because this host is the only
    checkout that should commit or push.

Environment:
  REMOTE_ARTIFACT_DIR  Remote artifact directory, relative to remote-dir.
                       Default: vm-out
  RSYNC_SSH           SSH command for rsync. Default: ssh
  SYNC_TRANSPORT      rsync or tar. Default: rsync
  REMOTE_OS           posix or windows. Only needed for tar push mkdir.
                      Default: posix
USAGE
}

if [[ $# -lt 2 || $# -gt 3 ]]; then
  usage >&2
  exit 2
fi

mode="$1"
target="$2"
remote_dir="${3:-~/cua}"
remote_artifact_dir="${REMOTE_ARTIFACT_DIR:-vm-out}"
rsync_ssh="${RSYNC_SSH:-ssh}"
transport="${SYNC_TRANSPORT:-rsync}"
remote_os="${REMOTE_OS:-posix}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
source_sha="$(git -C "$repo_root" rev-parse HEAD)"
artifact_root="$repo_root/libs/cua-driver/docs/vm-artifacts"
target_slug="$(printf '%s' "$target" | tr -c 'A-Za-z0-9_.-' '_')"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"

exclude_args=(
  # A linked worktree stores `.git` as a file, while a normal checkout stores
  # it as a directory. Neither host-specific form belongs on a verification VM.
  --exclude=.git
  --exclude=.DS_Store
  --exclude='._*'
  --exclude=target/
  --exclude=node_modules/
  --exclude=.venv/
  --exclude=__pycache__/
  --exclude='*.pyc'
  --exclude=dist/
  --exclude=vm-out/
)

tar_exclude_args=(
  --exclude ./.git
  --exclude ./.DS_Store
  --exclude '._*'
  --exclude ./target
  --exclude ./node_modules
  --exclude ./.venv
  --exclude '*/__pycache__'
  --exclude '*.pyc'
  --exclude ./dist
  --exclude ./vm-out
)

remote_mkdir() {
  case "$remote_os" in
    windows)
      "$rsync_ssh" "$target" \
        "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force '$remote_dir' | Out-Null\""
      ;;
    posix)
      "$rsync_ssh" "$target" "mkdir -p $remote_dir"
      ;;
    *)
      echo "REMOTE_OS must be posix or windows, got: $remote_os" >&2
      exit 2
      ;;
  esac
}

push_rsync() {
  ssh_cmd=${remote_dir/#\~/"\$HOME"}
  "$rsync_ssh" "$target" "mkdir -p $ssh_cmd"
  rsync -az --delete -e "$rsync_ssh" "${exclude_args[@]}" "$repo_root/" "$target:$remote_dir/"
}

push_tar() {
  remote_mkdir
  COPYFILE_DISABLE=1 tar "${tar_exclude_args[@]}" -czf - -C "$repo_root" . \
    | "$rsync_ssh" "$target" "tar -xzf - -C \"$remote_dir\""
}

write_source_marker() {
  case "$remote_os" in
    windows)
      "$rsync_ssh" "$target" \
        "powershell -NoProfile -Command \"Set-Content -NoNewline -Path '$remote_dir/.cua-e2e-source-sha' -Value '$source_sha'\""
      ;;
    posix)
      marker_dir=${remote_dir/#\~/"\$HOME"}
      printf '%s\n' "$source_sha" \
        | "$rsync_ssh" "$target" "mkdir -p $marker_dir && tee $marker_dir/.cua-e2e-source-sha >/dev/null"
      ;;
  esac
}

pull_artifacts_rsync() {
  rsync -az -e "$rsync_ssh" "$target:$remote_dir/$remote_artifact_dir/" "$dest/"
}

pull_artifacts_tar() {
  "$rsync_ssh" "$target" "tar -czf - -C \"$remote_dir/$remote_artifact_dir\" ." \
    | tar -xzf - -C "$dest"
}

pull_code_rsync() {
  rsync -az --backup --suffix=".vm-backup-$timestamp" -e "$rsync_ssh" \
    "${exclude_args[@]}" "$target:$remote_dir/" "$repo_root/"
}

pull_code_tar() {
  "$rsync_ssh" "$target" "tar -czf - -C \"$remote_dir\" ." \
    | COPYFILE_DISABLE=1 tar "${tar_exclude_args[@]}" -xzf - -C "$repo_root"
}

case "$mode" in
  push)
    case "$transport" in
      rsync) push_rsync ;;
      tar) push_tar ;;
      *) echo "SYNC_TRANSPORT must be rsync or tar, got: $transport" >&2; exit 2 ;;
    esac
    write_source_marker
    ;;

  pull-artifacts)
    dest="$artifact_root/$target_slug/$timestamp"
    mkdir -p "$dest"
    case "$transport" in
      rsync) pull_artifacts_rsync ;;
      tar) pull_artifacts_tar ;;
      *) echo "SYNC_TRANSPORT must be rsync or tar, got: $transport" >&2; exit 2 ;;
    esac
    echo "Pulled artifacts into $dest"
    ;;

  pull-code)
    if [[ "${ALLOW_PULL_CODE:-}" != "1" ]]; then
      echo "Refusing to pull VM source without ALLOW_PULL_CODE=1." >&2
      exit 2
    fi
    case "$transport" in
      rsync) pull_code_rsync ;;
      tar) pull_code_tar ;;
      *) echo "SYNC_TRANSPORT must be rsync or tar, got: $transport" >&2; exit 2 ;;
    esac
    ;;

  *)
    usage >&2
    exit 2
    ;;
esac
