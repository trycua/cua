# Runs `cua-spacesd doctor` in the guest's interactive session against the
# running service, with the token its first Init persisted.
# scripts/images/image-doctor-windows.sh sends it (with __STRICT__ filled in)
# and decodes the report blocks it prints.
$ErrorActionPreference = "Stop"
$strict = __STRICT__
$dir = Join-Path $env:TEMP ("cua-doctor-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$exe = Join-Path $env:ProgramFiles "Cua\spacesd\cua-spacesd.exe"
$doctorArgs = @("doctor", "--format", "human", "--effects", "virtual", "--expect-runtime", "qemu",
  "--token-file", (Join-Path $env:ProgramData "cua\spacesd\token"),
  "--out", (Join-Path $dir "report.json"), "--junit", (Join-Path $dir "report.xml"), "--timeout", "600")
if ($strict) { $doctorArgs += "--strict" }
# Native stderr lines are error records in Windows PowerShell; keep going.
$ErrorActionPreference = "Continue"
& $exe @doctorArgs 2>&1 | ForEach-Object { "$_" }
$rc = $LASTEXITCODE
foreach ($b in @(@("REPORT", "report.json"), @("JUNIT", "report.xml"))) {
  $f = Join-Path $dir $b[1]
  if (Test-Path $f) {
    "-----BEGIN $($b[0])-----"
    [Convert]::ToBase64String([IO.File]::ReadAllBytes($f))
    "-----END $($b[0])-----"
  }
}
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
exit $rc
