#!/bin/sh
# Runs the Pester tests for install.ps1 inside a throwaway Linux container
# (pwsh from the official tarball), for hosts without PowerShell.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
case "$(uname -m)" in arm64 | aarch64) arch=arm64 ;; *) arch=x64 ;; esac
pwsh_version="${PWSH_VERSION:-7.4.6}"
exec docker run --rm --memory=3g -v "$here:/w:ro" ubuntu:24.04 bash -c "
set -e
apt-get update -qq >/dev/null
apt-get install -y -qq curl ca-certificates python3 libicu74 >/dev/null 2>&1
curl -fsSL -o /tmp/p.tgz https://github.com/PowerShell/PowerShell/releases/download/v$pwsh_version/powershell-$pwsh_version-linux-$arch.tar.gz
mkdir -p /opt/pwsh && tar -xzf /tmp/p.tgz -C /opt/pwsh && chmod +x /opt/pwsh/pwsh
/opt/pwsh/pwsh -NoProfile -c 'Install-Module Pester -Force -Scope CurrentUser -MinimumVersion 5.5 | Out-Null; \$r = Invoke-Pester /w/tests -Output Detailed -PassThru; exit \$r.FailedCount'
"
