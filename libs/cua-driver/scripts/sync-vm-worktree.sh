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
  ALLOW_DIRTY_SYNC      Set to 1 only for diagnostic, non-acceptance pushes.
                        The source marker becomes noncanonical so E2E rejects it.
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

# RSYNC_SSH follows rsync's familiar command-string convention, so callers can
# include options such as `ssh -o BatchMode=yes`. Direct preparatory SSH calls
# need an argv array instead of treating the full string as one executable.
read -r -a rsync_ssh_argv <<< "$rsync_ssh"
if [[ ${#rsync_ssh_argv[@]} -eq 0 ]]; then
  echo "RSYNC_SSH must not be empty." >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
source_sha="$(git -C "$repo_root" rev-parse HEAD)"
if [[ "$mode" == push ]] && [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ]]; then
  if [[ "${ALLOW_DIRTY_SYNC:-}" != 1 ]]; then
    echo "Refusing to label a dirty host worktree as commit $source_sha." >&2
    echo "Commit or clean the source before an acceptance run; use ALLOW_DIRTY_SYNC=1 only for diagnostics." >&2
    exit 2
  fi
  source_sha="${source_sha}-dirty"
fi
artifact_root="$repo_root/artifacts/cua-driver/vm"
target_slug="$(printf '%s' "$target" | tr -c 'A-Za-z0-9_.-' '_')"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"

exclude_args=(
  # A linked worktree stores `.git` as a file, while a normal checkout stores
  # it as a directory. Neither host-specific form belongs on a verification VM.
  --exclude=.git
  --exclude=.DS_Store
  --exclude='._*'
  # Ignored local credentials are still present in a clean Git worktree.
  # Never copy them to a validation machine.
  --exclude=.env
  --exclude=.env.local
  --exclude=.netrc
  --exclude=.npmrc
  --exclude=.pypirc
  --exclude='*.key'
  --exclude='*.p12'
  --exclude='*.pem'
  --exclude=credentials/
  --exclude=secrets/
  --exclude=target/
  --exclude=libs/cua-driver/rust/test-apps/
  --exclude=node_modules/
  --exclude=.venv/
  --exclude=__pycache__/
  --exclude='*.pyc'
  --exclude=dist/
  # Host-side VM evidence is intentionally ignored by Git and can be large.
  # Never send pulled evidence back into a guest on the next source sync.
  --exclude=artifacts/
  --exclude=vm-out/
)

tar_exclude_args=(
  --exclude ./.git
  --exclude ./.DS_Store
  --exclude '._*'
  --exclude ./.env
  --exclude '*/.env'
  --exclude ./.env.local
  --exclude '*/.env.local'
  --exclude ./.netrc
  --exclude '*/.netrc'
  --exclude ./.npmrc
  --exclude '*/.npmrc'
  --exclude ./.pypirc
  --exclude '*/.pypirc'
  --exclude '*.key'
  --exclude '*.p12'
  --exclude '*.pem'
  --exclude '*/credentials'
  --exclude '*/credentials/*'
  --exclude '*/secrets'
  --exclude '*/secrets/*'
  --exclude ./target
  --exclude ./libs/cua-driver/rust/test-apps
  --exclude ./node_modules
  --exclude ./.venv
  --exclude '*/__pycache__'
  --exclude '*.pyc'
  --exclude ./dist
  --exclude ./artifacts
  --exclude ./vm-out
)

remote_mkdir() {
  case "$remote_os" in
    windows)
      "${rsync_ssh_argv[@]}" "$target" \
        "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force '$remote_dir' | Out-Null\""
      ;;
    posix)
      "${rsync_ssh_argv[@]}" "$target" "mkdir -p $remote_dir"
      ;;
    *)
      echo "REMOTE_OS must be posix or windows, got: $remote_os" >&2
      exit 2
      ;;
  esac
}

push_rsync() {
  ssh_cmd=${remote_dir/#\~/"\$HOME"}
  "${rsync_ssh_argv[@]}" "$target" "mkdir -p $ssh_cmd"
  rsync -az --delete -e "$rsync_ssh" "${exclude_args[@]}" "$repo_root/" "$target:$remote_dir/"
}

push_tar() {
  remote_mkdir
  remote_path=${remote_dir/#\~/'$HOME'}
  COPYFILE_DISABLE=1 tar --no-xattrs "${tar_exclude_args[@]}" -czf - -C "$repo_root" . \
    | "${rsync_ssh_argv[@]}" "$target" "tar -xzf - -C \"$remote_path\""
}

write_source_marker() {
  case "$remote_os" in
    windows)
      "${rsync_ssh_argv[@]}" "$target" \
        "powershell -NoProfile -Command \"Set-Content -NoNewline -Path '$remote_dir/.cua-e2e-source-sha' -Value '$source_sha'\""
      ;;
    posix)
      marker_dir=${remote_dir/#\~/"\$HOME"}
      printf '%s\n' "$source_sha" \
        | "${rsync_ssh_argv[@]}" "$target" "mkdir -p $marker_dir && tee $marker_dir/.cua-e2e-source-sha >/dev/null"
      ;;
  esac
}

ensure_remote_runtime_dirs() {
  case "$remote_os" in
    windows)
      "${rsync_ssh_argv[@]}" "$target" \
        "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force '$remote_dir/libs/cua-driver/rust/test-apps' | Out-Null\""
      ;;
    posix)
      remote_path=${remote_dir/#\~/'$HOME'}
      "${rsync_ssh_argv[@]}" "$target" \
        "mkdir -p \"$remote_path/libs/cua-driver/rust/test-apps\""
      ;;
  esac
}

pull_artifacts_rsync() {
  rsync -az -e "$rsync_ssh" "$target:$remote_dir/$remote_artifact_dir/" "$dest/"
}

pull_artifacts_tar() {
  remote_path=${remote_dir/#\~/'$HOME'}
  "${rsync_ssh_argv[@]}" "$target" "tar -czf - -C \"$remote_path/$remote_artifact_dir\" ." \
    | tar -xzf - -C "$dest"
}

pull_code_rsync() {
  rsync -az --backup --suffix=".vm-backup-$timestamp" -e "$rsync_ssh" \
    "${exclude_args[@]}" "$target:$remote_dir/" "$repo_root/"
}

pull_code_tar() {
  remote_path=${remote_dir/#\~/'$HOME'}
  "${rsync_ssh_argv[@]}" "$target" "tar -czf - -C \"$remote_path\" ." \
    | COPYFILE_DISABLE=1 tar --no-xattrs "${tar_exclude_args[@]}" -xzf - -C "$repo_root"
}

case "$mode" in
  push)
    case "$transport" in
      rsync) push_rsync ;;
      tar) push_tar ;;
      *) echo "SYNC_TRANSPORT must be rsync or tar, got: $transport" >&2; exit 2 ;;
    esac
    ensure_remote_runtime_dirs
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
