# Starts cua-spacesd for the `cua-spacesd` scheduled task (installed next to
# cua-spacesd.exe by install-spacesd.ps1), in the desktop user's session with
# no console window.
#
# The Windows workspace this image builds on configures a restrictive Cua
# Driver user policy (CUA_DRIVER_POLICY_FILE) for its own MCP endpoint. That
# policy must not narrow cua-spacesd's linked driver registry: the image's
# claims (and its doctor) cover the full tool set. Only the user and session
# layers are cleared here; an administrator's managed policy still applies.
$ErrorActionPreference = "Stop"
foreach ($name in @("CUA_DRIVER_POLICY_FILE", "CUA_DRIVER_SESSION_POLICY_FILE")) {
  Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}
$exe = Join-Path $PSScriptRoot "cua-spacesd.exe"
$token = Join-Path $env:ProgramData "cua\spacesd\token"
# Bootstrap mode binds every interface explicitly: without a token the
# default bind is loopback only, which the host (QEMU user networking,
# KubeVirt) cannot reach.
& $exe serve --listen 0.0.0.0:3211 --insecure-bootstrap --token-file $token
exit $LASTEXITCODE
