import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
INSTALLER = ROOT / "scripts/install/install.ps1"
PWSH = shutil.which("pwsh")

HARNESS = r"""
param([string]$Script, [string]$Scenario)
$env:CUA_INSTALL_NO_RUN = '1'
. $Script
$env:CUA_INSTALL_NO_RUN = $null
Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$VerbosePreference = 'Continue'
$Installer = if ($Scenario -in @('msi', 'fallback')) { 'msi' } else { 'nsis' }
$DryRun = $Scenario -eq 'dry-run'
$Mode = 'client'
$RequireSignature = $false
$script:events = [System.Collections.Generic.List[string]]::new()
$script:launches = @()
$script:inspected = @()
$script:certificate = $null
if ($Scenario -notin @('missing-certificate', 'verifier-error')) {
    $cn = switch ($Scenario) {
        'wrong-publisher' { 'Other Publisher' }
        'suffix' { 'Cua AI, Inc. Evil' }
        'case' { 'cua AI, Inc.' }
        default { 'Cua AI, Inc.' }
    }
    $key = [System.Security.Cryptography.RSA]::Create(2048)
    $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
        "CN=`"$cn`", O=Test", $key,
        [System.Security.Cryptography.HashAlgorithmName]::SHA256,
        [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $script:certificate = $request.CreateSelfSigned(
        [DateTimeOffset]::UtcNow.AddDays(-1), [DateTimeOffset]::UtcNow.AddDays(1))
    $key.Dispose()
}
function Confirm-Step { return $true }
function Find-Artifact {
    return $Scenario -ne 'fallback'
}
function Get-Artifact { return '/tmp/cua-spaces-setup.exe' }
function Get-AuthenticodeSignature {
    [CmdletBinding()]
    param([string]$LiteralPath)
    $script:inspected += $LiteralPath
    if ($Scenario -eq 'verifier-error') { throw 'verifier unavailable' }
    $status = if ($Scenario -in @('NotSigned', 'HashMismatch')) { $Scenario } else { 'Valid' }
    return [pscustomobject]@{
        Status = $status
        StatusMessage = "fixture: $status"
        SignerCertificate = $script:certificate
    }
}
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru)
    $script:events.Add('launch')
    $script:launches += @{ file = $FilePath; arguments = @($ArgumentList) }
    return [pscustomobject]@{ ExitCode = 0 }
}
$errorMessage = ''
try {
    Install-App @{} 'https://example.com/manifest.json' 'windows-x64' '/tmp' 4>&1 |
        ForEach-Object { $script:events.Add("$_") }
} catch { $errorMessage = $_.Exception.Message }
@{
    error = $errorMessage
    errorKind = Get-CuaInstallErrorKind $errorMessage
    didApp = $script:DidApp
    inspected = @($script:inspected)
    launches = @($script:launches)
    events = $script:events.ToArray()
} | ConvertTo-Json -Depth 5 -Compress
"""


class SpacesInstallerAuthenticodeTests(unittest.TestCase):
    @unittest.skipUnless(PWSH, "needs PowerShell 7")
    def test_app_execution_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            harness = Path(directory) / "harness.ps1"
            harness.write_text(HARNESS)
            for scenario in (
                "wrong-publisher",
                "valid",
                "NotSigned",
                "HashMismatch",
                "missing-certificate",
                "suffix",
                "case",
                "verifier-error",
                "dry-run",
                "msi",
                "fallback",
            ):
                with self.subTest(scenario=scenario):
                    result = subprocess.run(
                        [
                            PWSH,
                            "-NoProfile",
                            "-File",
                            str(harness),
                            "-Script",
                            str(INSTALLER),
                            "-Scenario",
                            scenario,
                        ],
                        capture_output=True,
                        text=True,
                        timeout=30,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    data = json.loads(result.stdout.splitlines()[-1])
                    if scenario in ("dry-run", "msi"):
                        self.assertEqual(data["inspected"], [])
                    else:
                        self.assertEqual(data["inspected"], ["/tmp/cua-spaces-setup.exe"])
                    if scenario in ("valid", "fallback", "msi"):
                        self.assertEqual(data["error"], "")
                        self.assertTrue(data["didApp"])
                        self.assertEqual(len(data["launches"]), 1)
                        launch = data["launches"][0]
                        if scenario == "msi":
                            self.assertEqual(launch["file"], "msiexec.exe")
                            self.assertIn("/qn", launch["arguments"])
                        else:
                            self.assertEqual(launch["file"], "/tmp/cua-spaces-setup.exe")
                            self.assertIn("/S", launch["arguments"])
                            verified = next(
                                i
                                for i, event in enumerate(data["events"])
                                if "verified the Authenticode signature" in event
                                and "Cua AI, Inc." in event
                            )
                            self.assertLess(verified, data["events"].index("launch"))
                    else:
                        self.assertEqual(data["launches"], [])
                        self.assertFalse(data["didApp"])
                        if scenario == "dry-run":
                            self.assertEqual(data["error"], "")
                        else:
                            self.assertIn("signature", data["error"].lower())
                            self.assertIn("refusing to run installer", data["error"])
                            self.assertEqual(data["errorKind"], "signature")

    @unittest.skipIf(PWSH, "behavioral harness checks ordering with PowerShell")
    def test_signature_precedes_execution_without_powershell(self):
        source = INSTALLER.read_text()
        app = source.split("function Install-App(", 1)[1].split("function Get-DriverBinDir", 1)[0]
        self.assertLess(app.index("if ($DryRun)"), app.index("Get-AuthenticodeSignature"))
        self.assertLess(app.index("Get-AuthenticodeSignature"), app.index("Start-Process"))


if __name__ == "__main__":
    unittest.main()
