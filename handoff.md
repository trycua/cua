# Apple Silicon Local Linux Docker E2E Handoff

## Objective

Validate `trycua/cua#3257` on a real Apple Silicon Mac using Docker Desktop. The run must prove that the ARM64 image starts through `cua-sandbox`, computer-server is reachable through Docker's published host port, desktop screenshots and clipboard operations work, and the ephemeral container is removed.

Do not modify code, create commits, push branches, or change the pull request. Run the test and report evidence only.

## Prerequisites

- Apple Silicon Mac (`uname -m` returns `arm64`)
- Docker Desktop running
- `uv` installed
- Local clone of `https://github.com/trycua/cua`
- Branch `codex/cua-sandbox-arm64-linux`
- Candidate Docker image tag or digest, if testing before `docker-latest` is published

## Checkout

```bash
git fetch origin codex/cua-sandbox-arm64-linux
git switch codex/cua-sandbox-arm64-linux
git pull --ff-only origin codex/cua-sandbox-arm64-linux
git status --short
```

Stop if the worktree is not clean.

## Record Environment

```bash
uname -a
uname -m
docker version
uv --version
```

## Candidate Image Run

Use the immutable Docker image produced by `trycua/cloud#7099`. A local Docker tag is also accepted.

```bash
IMAGE='<candidate-image-tag-or-digest>'

docker pull "$IMAGE" || docker image inspect "$IMAGE"

docker image inspect "$IMAGE" \
  --format 'repo_digests={{json .RepoDigests}} architecture={{.Architecture}} os={{.Os}}'

./libs/python/cua-sandbox/scripts/live_local_linux_docker.py \
  --image ""
```

Expected terminal result:

```text
PASS: local Linux Docker sandbox is healthy
```

## Published Default Run

Run this only after `public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest` exists. This verifies the image resolution shipped by `cua#3257` rather than an explicit override.

```bash
./libs/python/cua-sandbox/scripts/live_local_linux_docker.py
```

## Success Criteria

The script must verify all of the following:

- host architecture is Apple Silicon ARM64;
- the Linux container reports `aarch64`;
- computer-server accepts shell commands through the published Docker port;
- screenshot is a valid PNG larger than 10 KB;
- screen dimensions are positive;
- clipboard write/read returns the exact marker;
- the ephemeral Docker container is removed after exit.

Artifacts are written to `/tmp/cua-linux-arm64-live`:

- `summary.json`
- `screenshot.png`
- `docker-inspect.json` on failure
- `docker.log` on failure

## Failure Report

Return:

1. Full command output.
2. `/tmp/cua-linux-arm64-live/summary.json`.
3. `/tmp/cua-linux-arm64-live/docker.log`, if present.
4. `/tmp/cua-linux-arm64-live/docker-inspect.json`, if present.
5. Output from:

```bash
docker ps -a --filter label=cua.sandbox=true
docker images --digests | grep -E 'cua-ubuntu|cua-xfce|xfce-cua' || true
git status --short
git rev-parse HEAD
```

Do not mark the validation successful if the test only passes under `--platform linux/amd64`; native ARM64 execution is required.
