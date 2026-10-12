param([Parameter(Mandatory = $true)][string]$OutputDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$compiler = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $compiler)) { throw "Windows .NET Framework C# compiler is required: $compiler" }
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$output = Join-Path $OutputDirectory "CuaTestHarness.WinFormsModal.exe"
& $compiler /nologo /target:winexe "/out:$output" /reference:System.Windows.Forms.dll /reference:System.Drawing.dll (Join-Path $PSScriptRoot "Program.cs")
if ($LASTEXITCODE -ne 0) { throw "WinForms modal fixture compilation failed" }
