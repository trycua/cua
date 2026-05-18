# cua-driver-rs install.ps1 redirect shim.
#
# The canonical Windows installer now lives at
# `libs/cua-driver/scripts/install.ps1`. This shim exists so existing
# one-liners that reference the old URL keep working forever:
#
#   irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.ps1 | iex
#
# It fetches and `iex`-pipes the canonical installer with the same args.
# Old `$env:CUA_DRIVER_RS_VERSION` env vars and `-Release` / `-AutoStart`
# params flow through unchanged.

$ErrorActionPreference = "Stop"
$ProgressPreference    = "SilentlyContinue"

$CanonicalUrl = "https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.ps1"

Write-Host "note: cua-driver-rs/scripts/install.ps1 is deprecated; fetching the" -ForegroundColor Yellow
Write-Host "      canonical installer from cua-driver/scripts/install.ps1" -ForegroundColor Yellow
Write-Host "      The Rust port is still what gets installed on Windows — only the" -ForegroundColor Yellow
Write-Host "      URL moved." -ForegroundColor Yellow
Write-Host ""

# Re-execute the canonical installer in the current scope so PARAMS / env
# vars / progress output flow through naturally. The canonical installer
# prints its own post-install hints (including the `cua-driver skills
# install` opt-in), so the shim doesn't duplicate them here.
$CanonicalScript = (Invoke-WebRequest -Uri $CanonicalUrl -UseBasicParsing).Content
Invoke-Expression $CanonicalScript
