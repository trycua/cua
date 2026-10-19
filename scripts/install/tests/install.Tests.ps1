# Pester 5 tests for install.ps1. Hermetic: temp dirs, file:// release, no
# real installs (the app step is exercised with -DryRun only).
#   pwsh -c "Invoke-Pester scripts/install/tests -Output Detailed"

BeforeAll {
    function global:ConvertTo-FileUrl([string]$Path) {
        $p = $Path -replace '\\', '/'
        if (-not $p.StartsWith('/')) { $p = "/$p" }
        return "file://$p"
    }
    $script:Installer = Join-Path $PSScriptRoot '..' 'install.ps1'
    $script:Tool = Join-Path $PSScriptRoot '..' 'release_manifest.py'
    $script:Python = (Get-Command python3 -ErrorAction SilentlyContinue) ?? (Get-Command python)

    $script:Root = Join-Path ([IO.Path]::GetTempPath()) ("cua-ps-tests-" + [Guid]::NewGuid().ToString('N'))
    $script:Rel = Join-Path $Root 'release'
    New-Item -ItemType Directory -Force -Path $Rel, (Join-Path $Root 'stage') | Out-Null
    Set-Content -LiteralPath (Join-Path $Root 'stage/cua.exe') -Value 'fake cua'
    foreach ($arch in 'x64', 'arm64') {
        Compress-Archive -Path (Join-Path $Root 'stage/cua.exe') -DestinationPath (Join-Path $Rel "cua-cli-1.2.3-windows-$arch.zip")
        Set-Content -LiteralPath (Join-Path $Rel "cua-spaces-1.2.3-windows-$arch.msi") -Value 'msi'
        Set-Content -LiteralPath (Join-Path $Rel "cua-spaces-1.2.3-windows-$arch-setup.exe") -Value 'nsis'
    }
    $cli = Join-Path $Root 'cli.json'
    & $Python.Source $Tool --component cli --version 1.2.3 --dir $Rel --out $cli
    & $Python.Source $Tool --component app --version 1.2.3 --dir $Rel --merge $cli --out (Join-Path $Rel 'release-artifacts.json')
    $env:CUA_TEST_MANIFEST_URL = ConvertTo-FileUrl (Join-Path $Rel 'release-artifacts.json')
    $env:CUA_TEST_ROOT = $Root
    $env:CUA_TEST_INSTALLER = $Installer

    function script:Invoke-Installer([hashtable]$Params, [hashtable]$Vars = @{}) {
        $saved = @{}
        # Install telemetry may only reach a loopback endpoint here.
        $all = @{ CUA_INSTALL_MANIFEST_URL = $env:CUA_TEST_MANIFEST_URL; CUA_INSTALL_ARCH = 'AMD64'; CUA_HOME = (Join-Path $env:CUA_TEST_ROOT 'cuahome'); CUA_TELEMETRY_FORBID_NETWORK = '1' }
        foreach ($k in $Vars.Keys) { $all[$k] = $Vars[$k] }
        foreach ($k in $all.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $all[$k]) }
        try {
            return (& $env:CUA_TEST_INSTALLER @Params *>&1 | Out-String)
        } finally {
            foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
        }
    }
}

AfterAll {
    Remove-Item -Recurse -Force -LiteralPath $env:CUA_TEST_ROOT -ErrorAction SilentlyContinue
}

Describe 'functions' {
    BeforeAll {
        $env:CUA_INSTALL_NO_RUN = '1'
        . $env:CUA_TEST_INSTALLER
        $env:CUA_INSTALL_NO_RUN = $null
    }

    It 'only downloads over HTTPS (loopback HTTP for tests)' {
        { Assert-DownloadUrl 'https://github.com/trycua/cua/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://127.0.0.1:8080/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://localhost/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'ftp://example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'http://127.0.0.1:1@example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'file:///tmp/a.zip' 'https://github.com/r.json' } | Should -Throw '*remote manifest*'
        { Assert-DownloadUrl 'file:///tmp/a.zip' 'file:///tmp/r.json' } | Should -Not -Throw
    }

    It 'maps architectures' {
        Get-CuaPlatform -Arch 'AMD64' | Should -Be 'windows-x64'
        Get-CuaPlatform -Arch 'ARM64' | Should -Be 'windows-arm64'
        { Get-CuaPlatform -Arch 'x86' } | Should -Throw '*unsupported CPU architecture*'
    }

    It 'builds silent MSI and NSIS commands with the install mode' {
        $msi = Get-AppInstallCommand 'C:\t\a.msi' 'msi' 'host'
        $msi.FilePath | Should -Be 'msiexec.exe'
        $msi.ArgumentList | Should -Contain '/qn'
        $msi.ArgumentList | Should -Contain 'CUA_SPACES_MODE=host'
        $nsis = Get-AppInstallCommand 'C:\t\a.exe' 'nsis' 'client'
        $nsis.ArgumentList | Should -Be @('/S', '/MODE=client')
        (Get-AppInstallCommand 'C:\t\a.msi' 'msi' '').ArgumentList | Should -Not -Contain 'CUA_SPACES_MODE='
    }

    It 'parses item lists' {
        ConvertTo-CuaItems 'cli,cua-driver, host,cua-driver' | Should -Be @('cua-driver', 'host')
        @(ConvertTo-CuaItems '') | Should -HaveCount 0
        { ConvertTo-CuaItems 'bogus' } | Should -Throw "*unknown item 'bogus' (valid: cli, spaces, cua-driver, host)*"
    }

    It 'selects defaults, -Select, -Only and the legacy switches' {
        $d = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode ''
        @($d.cli, $d.spaces, $d['cua-driver'], $d.host) | Should -Be @($true, $true, $false, $false)
        (Get-CuaSelection -SelectList '' -OnlyList '' -Cli $true -App $false -InstallMode '').spaces | Should -BeFalse
        (Get-CuaSelection -SelectList 'cua-driver' -OnlyList '' -Cli $false -App $false -InstallMode '')['cua-driver'] | Should -BeTrue
        $o = Get-CuaSelection -SelectList '' -OnlyList 'host' -Cli $false -App $false -InstallMode ''
        @($o.cli, $o.spaces, $o.host) | Should -Be @($true, $false, $true)
        $a = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $true -InstallMode ''
        @($a.cli, $a.spaces) | Should -Be @($false, $true)
        (Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode 'host').spaces | Should -BeTrue
        (Get-CuaSelection -SelectList '' -OnlyList '' -Cli $true -App $false -InstallMode 'host').spaces | Should -BeFalse
        (Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $false -App $false -InstallMode '').spaces | Should -BeTrue
    }

    It 'rejects conflicting selections' {
        { Get-CuaSelection -SelectList 'host' -OnlyList 'host' -Cli $false -App $false -InstallMode '' } | Should -Throw '*mutually exclusive*'
        { Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $true -App $false -InstallMode '' } | Should -Throw '*use -Only spaces*'
        { Get-CuaSelection -SelectList '' -OnlyList 'cua-driver' -Cli $false -App $true -InstallMode '' } | Should -Throw '*cua-driver and host need*'
        { Get-CuaSelection -SelectList '' -OnlyList '' -Cli $true -App $true -InstallMode '' } | Should -Throw '*mutually exclusive*'
    }

    It 'toggles the Cua Spaces and Cua Driver rows; there is no CLI row and no hosting row' {
        # 2 turns Cua Driver on, 3 is past the last row, 1 turns the app off.
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@('2 3', '1', ''))
        $sel = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $sel = Read-CuaSelection $sel { $script:answers.Dequeue() } 6>$null
        @($sel.cli, $sel.spaces, $sel['cua-driver'], $sel.host) | Should -Be @($true, $false, $true, $false)
        $script:Picked | Should -BeTrue
        $script:Picked = $false
    }

    It 'shows Spaces in the list, where it can be unticked' {
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@('1', ''))
        $sel = Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $sel = Read-CuaSelection $sel { $script:answers.Dequeue() } 6>$null
        @($sel.spaces, $sel['cua-driver']) | Should -Be @($false, $false)
        $script:Picked = $false
    }

    It 'says what Cua Spaces is and what is always installed, and describes each row' {
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@(''))
        $sel = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $shown = @(& { $null = Read-CuaSelection $sel { $script:answers.Dequeue() } } 6>&1 | ForEach-Object { "$($_.MessageData)" })
        $text = $shown -join "`n"
        $text | Should -Match 'Cua Spaces lets your AI agents use computers, and lets you watch them work\.'
        $text | Should -Match 'Needed, so always installed:'
        $text | Should -Match 'Command-line tools: the "cua" program that the app and your AI agents use\.'
        $text | Should -Match '1 \[x\] Cua Spaces app: lets your AI agents use computers'
        $text | Should -Match '2 \[ \] Cua Driver: lets AI agents use your apps while you keep working'
        $text | Should -Match 'For advanced users\. Adds it to Claude Code, Codex and other coding agents\.'
        $text | Should -Not -Match 'Host this machine'
        $text | Should -Not -Match 'cua CLI \(required\)'
        $script:Picked = $false
    }

    It 'says a scripted -Select host is on, though the list has no hosting row' {
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@(''))
        $sel = Get-CuaSelection -SelectList 'host' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $shown = @(& { $null = Read-CuaSelection $sel { $script:answers.Dequeue() } } 6>&1 | ForEach-Object { "$($_.MessageData)" })
        ($shown -join "`n") | Should -Match 'Also: your other devices will be able to use this computer'
        $script:Picked = $false
    }

    It 'forwards -RequireSignature and -NoPathUpdate to the cua-driver installer' {
        $RequireSignature = $false; $NoModifyPath = $true
        Get-DriverInstallArgs | Should -Be @('-NoPathUpdate')
        $RequireSignature = $true; $NoModifyPath = $false
        Get-DriverInstallArgs | Should -Be @('-RequireSignature')
    }

    It 'runs the cua-driver installer, then registers the skill and MCP server' -Skip:($IsWindows) {
        $root = Join-Path $env:CUA_TEST_ROOT 'drv'
        $prefix = Join-Path $root 'prefix'
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        $log = Join-Path $root 'driver.log'
        $cuaLog = Join-Path $root 'cua.log'
        $fakeDriver = Join-Path $root 'install.ps1'
        Set-Content -LiteralPath $fakeDriver -Value @"
param([switch]`$NoPathUpdate, [switch]`$RequireSignature)
"NoPathUpdate=`$NoPathUpdate RequireSignature=`$RequireSignature dir=`$env:CUA_DRIVER_RS_INSTALL_DIR" | Set-Content '$log'
New-Item -ItemType Directory -Force -Path `$env:CUA_DRIVER_RS_INSTALL_DIR | Out-Null
Set-Content (Join-Path `$env:CUA_DRIVER_RS_INSTALL_DIR 'cua-driver.exe') 'fake'
"@
        $fakeCua = Join-Path $root 'cua'
        Set-Content -LiteralPath $fakeCua -Value "#!/bin/sh`necho `"`$*`" >>'$cuaLog'"
        chmod +x $fakeCua
        $env:CUA_INSTALL_DRIVER_URL = ConvertTo-FileUrl $fakeDriver
        $Prefix = $prefix; $DryRun = $false; $RequireSignature = $true; $NoModifyPath = $true
        try {
            Install-CuaDriver $fakeCua (New-Item -ItemType Directory -Force -Path (Join-Path $root 'tmp')).FullName 'file:///r.json' 6>$null
        } finally { $env:CUA_INSTALL_DRIVER_URL = $null }
        $bin = Join-Path $prefix 'bin'
        Get-Content $log | Should -Be "NoPathUpdate=True RequireSignature=True dir=$bin"
        Get-Content $cuaLog | Should -Be "agents setup --cua-driver --agents all --yes --mcp-command $(Join-Path $bin 'cua-driver.exe')"
        $env:CUA_DRIVER_RS_INSTALL_DIR | Should -BeNullOrEmpty
    }

    It 'refuses a plain-HTTP cua-driver installer URL' {
        $env:CUA_INSTALL_DRIVER_URL = 'http://example.invalid/install.ps1'
        $DryRun = $false
        try { { Install-CuaDriver 'cua' $env:CUA_TEST_ROOT 'https://x/r.json' 6>$null } | Should -Throw '*non-HTTPS*' }
        finally { $env:CUA_INSTALL_DRIVER_URL = $null }
    }

    It 'resolves relative artifact URLs against the manifest' {
        Resolve-ArtifactUrl 'a.zip' 'https://x/rel/release-artifacts.json' | Should -Be 'https://x/rel/a.zip'
        Resolve-ArtifactUrl 'https://y/b.zip' 'https://x/r.json' | Should -Be 'https://y/b.zip'
    }
}

Describe 'install.ps1' {
    It 'installs the CLI into -Prefix\bin and verifies the checksum' {
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p1'
        $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = $prefix }
        Join-Path $prefix 'bin/cua.exe' | Should -Exist
        $out | Should -Match 'Installed the command-line tools \(downloads checked\)\.'
        $out | Should -Match 'Next: run "cua auth login" to sign in, then "cua agents setup" to connect your AI agents\.'
        $out | Should -Match 'To let your other devices use this computer, run "cua host setup"\.'
        $out | Should -Match 'is not on your PATH'
    }

    It 'prints one plain line per step by default, and the detail with -Verbose' {
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p-quiet'
        $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = $prefix }
        $out | Should -Match 'Installing the command-line tools to '
        $out | Should -Not -Match 'cua-install:'
        $out | Should -Not -Match 'manifest|checksum|sha256'
        $verbose = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p-verbose'); Verbose = $true }
        $verbose | Should -Match 'cua-install: platform windows-x64, manifest '
    }

    # CUA-1704: `cua` must work in the next terminal without a manual step.
    It 'adds the CLI to the user PATH by default; -CliOnly neither asks nor signs in' {
        $out = Invoke-Installer @{ DryRun = $true; CliOnly = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p11') }
        $out | Should -Match '\[dry-run\] add .*p11.bin to the user PATH'
        $out | Should -Not -Match 'is not on your PATH'
        $out | Should -Not -Match 'Install the command-line tools to'
        $out | Should -Not -Match 'signing in'
        $out | Should -Not -Match 'setup.exe'
    }

    # Cua Spaces for Windows is the Electron app's NSIS setup.exe.
    It 'dry run: -Mode plans the CLI and the silent setup.exe, and changes nothing' {
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p2'
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; Prefix = $prefix; Mode = 'host' } @{ CUA_INSTALL_ARCH = 'ARM64' }
        $out | Should -Match 'cua-cli-1.2.3-windows-arm64.zip'
        $out | Should -Match 'cua-spaces-1.2.3-windows-arm64-setup.exe'
        $out | Should -Match '/S'
        $out | Should -Not -Match 'msiexec'
        $prefix | Should -Not -Exist
    }

    It 'dry run: -AppOnly plans only the silent setup.exe' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; AppOnly = $true; Mode = 'client' }
        $out | Should -Match 'cua-spaces-1.2.3-windows-x64-setup.exe'
        $out | Should -Not -Match 'cua-cli-1.2.3'
    }

    # Clean machine: the manifest lists signatures but neither minisign nor
    # cosign is installed. The signature note reads $script:SigNoted,
    # which crashed under Set-StrictMode when it was never initialised.
    It 'installs under StrictMode when the manifest has signatures but no verifier is installed' {
        $sig = Join-Path $env:CUA_TEST_ROOT 'sig'
        New-Item -ItemType Directory -Force -Path $sig | Out-Null
        Copy-Item (Join-Path $env:CUA_TEST_ROOT 'release/*') $sig
        Get-ChildItem $sig -Filter 'cua-cli-*.zip' | ForEach-Object { Set-Content -LiteralPath "$($_.FullName).minisig" -Value 'sig' }
        $sigManifest = Join-Path $sig 'release-artifacts.json'
        & $Python.Source $Tool --component cli --version 1.2.3 --dir $sig --out $sigManifest
        (Get-Content -Raw $sigManifest) | Should -Match 'minisig'
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p-sig'
        $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = $prefix } @{
            CUA_INSTALL_MANIFEST_URL = (ConvertTo-FileUrl $sigManifest)
            PATH = ((Get-Command python3).Source | Split-Path)
        }
        $out | Should -Not -Match 'has not been set'
        $out | Should -Match 'Installed the command-line tools \(downloads checked\)\.'
        $out | Should -Not -Match 'cosign|minisign|signatures were not'
        Join-Path $prefix 'bin/cua.exe' | Should -Exist
        # The detail is for -Verbose.
        $verbose = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p-sig-v'); Verbose = $true } @{
            CUA_INSTALL_MANIFEST_URL = (ConvertTo-FileUrl $sigManifest)
            PATH = ((Get-Command python3).Source | Split-Path)
        }
        $verbose | Should -Match 'release signatures were not checked'
    }

    It 'aborts on a checksum mismatch' {
        $bad = Join-Path $env:CUA_TEST_ROOT 'bad'
        New-Item -ItemType Directory -Force -Path $bad | Out-Null
        Copy-Item (Join-Path (Join-Path $env:CUA_TEST_ROOT 'release') '*') $bad
        $json = Get-Content -Raw (Join-Path $bad 'release-artifacts.json')
        ($json -replace '"sha256":"[0-9a-f]{8}', '"sha256":"00000000') | Set-Content (Join-Path $bad 'release-artifacts.json')
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p3'
        { Invoke-Installer @{ CliOnly = $true; Yes = $true; Prefix = $prefix } @{ CUA_INSTALL_MANIFEST_URL = ConvertTo-FileUrl (Join-Path $bad 'release-artifacts.json') } } |
            Should -Throw '*checksum mismatch*'
        Join-Path $prefix 'bin/cua.exe' | Should -Not -Exist
    }

    It 'dry run: -Pair installs the CLI and runs host setup with the code, no sign-in or checklist' {
        $out = Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD'; Relay = 'https://relay.example.com' } @{ CUA_INSTALL_REINSTALL = '1' }
        $out | Should -Match 'cua-cli-1.2.3-windows-x64.zip'
        $out | Should -Match 'host setup --pair K7QM-4XPB-2RHD --relay https://relay.example.com'
        $out | Should -Not -Match 'auth login'
        $out | Should -Not -Match 'Choose what to install'
    }

    It '-Pair on a computer that already has cua: uses it and downloads nothing' {
        $dir = Join-Path $env:CUA_TEST_ROOT 'existing-cua'
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $sep = [IO.Path]::PathSeparator
        if ($IsWindows) {
            Set-Content -LiteralPath (Join-Path $dir 'cua.cmd') -Value "@echo off`r`necho       --pair ^<CODE^>  add this machine with a code"
        } else {
            $fake = Join-Path $dir 'cua'
            Set-Content -LiteralPath $fake -Value "#!/bin/sh`necho '      --pair <CODE>  add this machine with a code'"
            chmod +x $fake
        }
        $vars = @{ PATH = "$dir$sep$env:PATH"; CUA_INSTALL_REINSTALL = $null }
        $out = Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD' } $vars
        $out | Should -Match 'already installed at'
        $out | Should -Match 'host setup --pair K7QM-4XPB-2RHD'
        $out | Should -Not -Match 'cua-cli-1.2.3'
        $out | Should -Not -Match 'manifest'
        # -Version and CUA_INSTALL_REINSTALL install anyway.
        (Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD'; Version = '1.2.3' } $vars) | Should -Match 'cua-cli-1.2.3'
        (Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD' } @{ PATH = $vars.PATH; CUA_INSTALL_REINSTALL = '1' }) | Should -Match 'cua-cli-1.2.3'
    }

    It '-Pair refuses codes and relays with shell characters' {
        { Invoke-Installer @{ DryRun = $true; Pair = 'K7QM;calc' } } | Should -Throw '*not a pairing code*'
        { Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD'; Relay = 'http://evil.example' } } | Should -Throw '*https*'
        { Invoke-Installer @{ DryRun = $true; Pair = 'K7QM-4XPB-2RHD'; Relay = 'http://localhost.evil.example' } } | Should -Throw '*https*'
        { Invoke-Installer @{ DryRun = $true; Relay = 'https://relay.example.com' } } | Should -Throw '*goes with -Pair*'
    }

    It 'rejects -CliOnly with -AppOnly' {
        { Invoke-Installer @{ CliOnly = $true; AppOnly = $true } } | Should -Throw '*mutually exclusive*'
    }

    It '-RequireSignature fails without a verifiable signature' {
        { Invoke-Installer @{ CliOnly = $true; Yes = $true; RequireSignature = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p4') } } |
            Should -Throw '*no verifiable signature*'
    }

    It 'reads the versioned manifest for -Version' {
        $base = ConvertTo-FileUrl $env:CUA_TEST_ROOT
        { Invoke-Installer @{ DryRun = $true; Yes = $true; CliOnly = $true; Version = 'v9.9.9' } @{ CUA_INSTALL_MANIFEST_URL = $null; CUA_INSTALL_BASE_URL = $base } } |
            Should -Throw '*cua-sdk-v9.9.9*release-artifacts.json*'
    }

    It 'verifies a cosign bundle against the exact release workflow and tag' {
        $sig = Join-Path $env:CUA_TEST_ROOT 'csig'
        $bin = Join-Path $env:CUA_TEST_ROOT 'fakebin-cosign'
        New-Item -ItemType Directory -Force -Path $sig, $bin | Out-Null
        Copy-Item (Join-Path $env:CUA_TEST_ROOT 'release/cua-cli-1.2.3-windows-x64.zip') $sig
        Set-Content -LiteralPath (Join-Path $sig 'cua-cli-1.2.3-windows-x64.zip.sigstore.json') -Value '{}'
        & $Python.Source $Tool --component cli --version 1.2.3 --dir $sig --out (Join-Path $sig 'release-artifacts.json')
        $log = Join-Path $env:CUA_TEST_ROOT 'cosign.log'
        $want = 'https://github.com/trycua/cua/.github/workflows/cd-cua-sdk.yml@refs/tags/cua-sdk-v1.2.3'
        if ($IsWindows) {
            # A batch file stands in for cosign.exe on Windows.
            Set-Content -LiteralPath (Join-Path $bin 'cosign.cmd') -Value "@echo off`r`necho %*>>`"$log`"`r`necho %* | findstr /C:`"--certificate-identity $want`" >nul && exit /b 0`r`nexit /b 1"
        } else {
            $fake = Join-Path $bin 'cosign'
            Set-Content -LiteralPath $fake -Value "#!/bin/sh`nprintf '%s\n' `"`$*`" >>'$log'`nfor a in `"`$@`"; do [ `"`$a`" = '$want' ] && exit 0; done`nexit 1"
            chmod +x $fake
        }
        $vars = @{ CUA_INSTALL_MANIFEST_URL = (ConvertTo-FileUrl (Join-Path $sig 'release-artifacts.json')); PATH = "$bin$([IO.Path]::PathSeparator)$env:PATH" }
        Invoke-Installer @{ CliOnly = $true; Yes = $true; RequireSignature = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p9') } $vars | Out-Null
        Get-Content $log | Should -Match ([regex]::Escape("--certificate-identity $want"))
        Get-Content $log | Should -Not -Match 'identity-regexp'
    }

    It 'dry run: Spaces is on by default on Windows' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true }
        $out | Should -Match 'cua-cli-1.2.3-windows-x64.zip'
        $out | Should -Match 'cua-spaces-1.2.3-windows-x64-setup.exe /S'
        $out | Should -Not -Match 'msiexec'
        $out | Should -Not -Match 'cua-driver'
    }

    It 'dry run: -Only spaces installs the CLI and the setup.exe' {
        $out = Invoke-Installer @{ DryRun = $true; Only = 'spaces' }
        $out | Should -Match 'cua-cli-1.2.3-windows-x64.zip'
        $out | Should -Match 'cua-spaces-1.2.3-windows-x64-setup.exe'
        $out | Should -Not -Match 'msiexec'
    }

    It 'dry run: -Select cua-driver,host prints the driver, agents and host steps' {
        $out = Invoke-Installer @{ DryRun = $true; Select = 'cua-driver,host'; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p10') } @{ CUA_INSTALL_NONINTERACTIVE = '1'; CUA_INSTALL_DRIVER_URL = 'https://example.test/driver/install.ps1' }
        $out | Should -Match '\[dry-run\] download https://example.test/driver/install.ps1'
        $out | Should -Match 'cua-driver-install.ps1'
        $out | Should -Not -Match 'NoPathUpdate'
        $out | Should -Match 'agents setup --cua-driver --agents all --yes --mcp-command .*p10.bin.cua-driver.exe'
        $out | Should -Match '\[dry-run\] .*cua.exe host setup'
        $out | Should -Not -Match 'msiexec'
    }

    It 'rejects unknown items and conflicting selections' {
        { Invoke-Installer @{ DryRun = $true; Select = 'bogus' } } | Should -Throw '*unknown item*'
        { Invoke-Installer @{ DryRun = $true; Select = 'host'; Only = 'host' } } | Should -Throw '*mutually exclusive*'
        { Invoke-Installer @{ DryRun = $true; CliOnly = $true; Select = 'spaces' } } | Should -Throw '*use -Only spaces*'
    }

    It 'dry run: -Mode writes the install-mode file when the app is installed' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; AppOnly = $true; Mode = 'host' }
        $out | Should -Match 'spaces-install-mode'
    }
}

Describe 'install telemetry' {
    BeforeAll {
        $env:CUA_INSTALL_NO_RUN = '1'
        . $script:Installer
        Remove-Item Env:CUA_INSTALL_NO_RUN
        $script:Fixture = Get-Content -Raw (Join-Path $PSScriptRoot 'fixtures/installer-step.json') | ConvertFrom-Json
        $script:Switches = @('DO_NOT_TRACK', 'CUA_TELEMETRY', 'CUA_TELEMETRY_ENABLED', 'CUA_TELEMETRY_DISABLED') + $script:TelCiVars
        function script:With-Env([hashtable]$Vars, [scriptblock]$Body) {
            $saved = @{}
            foreach ($k in $script:Switches + @($Vars.Keys)) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $null) }
            foreach ($k in $Vars.Keys) { [Environment]::SetEnvironmentVariable($k, $Vars[$k]) }
            try { return (& $Body) } finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
        }
        function script:Get-Events([string]$Out) {
            return @(($Out -split "`r?`n") | Where-Object { $_ -like '`[cua telemetry`] *' } | ForEach-Object { ($_.Substring(16) | ConvertFrom-Json).batch[0] })
        }
    }

    It 'follows the same switches as every Cua program' {
        $h = Join-Path $env:CUA_TEST_ROOT 'tel-home'
        New-Item -ItemType Directory -Force -Path $h | Out-Null
        With-Env @{} { Test-TelemetryEnabled $h } | Should -BeTrue
        With-Env @{ DO_NOT_TRACK = '1'; CUA_TELEMETRY = '1' } { Test-TelemetryEnabled $h } | Should -BeFalse
        With-Env @{ CUA_TELEMETRY = '0' } { Test-TelemetryEnabled $h } | Should -BeFalse
        With-Env @{ CUA_TELEMETRY_ENABLED = 'false' } { Test-TelemetryEnabled $h } | Should -BeFalse
        With-Env @{ CUA_TELEMETRY_DISABLED = '1' } { Test-TelemetryEnabled $h } | Should -BeFalse
        With-Env @{ GITHUB_ACTIONS = 'true' } { Test-TelemetryEnabled $h } | Should -BeFalse
        With-Env @{ CI = 'true'; CUA_TELEMETRY = '1' } { Test-TelemetryEnabled $h } | Should -BeTrue
        Set-Content -LiteralPath (Join-Path $h 'config.toml') -Value "[other]`nenabled = `"on`"`n[telemetry]`nenabled = `"off`""
        With-Env @{} { Test-TelemetryEnabled $h } | Should -BeFalse
    }

    It 'names a failure with a fixed word, never the message' {
        Get-CuaInstallErrorKind 'cua-install: error: checksum mismatch for cua.zip' | Should -Be 'checksum'
        Get-CuaInstallErrorKind 'cua-install: error: cosign signature check failed' | Should -Be 'signature'
        Get-CuaInstallErrorKind 'cua-install: error: unsupported CPU architecture: x86' | Should -Be 'unsupported_platform'
        Get-CuaInstallErrorKind 'Unable to connect to the remote server' | Should -Be 'network'
        Get-CuaInstallErrorKind 'C:\Users\alice\secret' | Should -Be 'other'
    }

    It 'sends nothing on a dry run, with DO_NOT_TRACK, or to a real endpoint in tests' {
        foreach ($vars in @(@{ DO_NOT_TRACK = '1' }, @{ CUA_TELEMETRY_ENDPOINT = 'https://eu.i.posthog.com/batch/' })) {
            $vars.CUA_TELEMETRY_DEBUG = '1'
            $vars.CUA_HOME = Join-Path $env:CUA_TEST_ROOT ('tel-off-' + [Guid]::NewGuid().ToString('N'))
            $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p-tel-off') } $vars
            $out | Should -Not -Match 'cua telemetry'
            $out | Should -Not -Match 'anonymous data about this install'
            Test-Path (Join-Path $vars.CUA_HOME 'telemetry/install_id') | Should -BeFalse
        }
        $out = Invoke-Installer @{ DryRun = $true; CliOnly = $true } @{ CUA_TELEMETRY_DEBUG = '1'; CUA_TELEMETRY_ENDPOINT = 'http://127.0.0.1:9/batch/' }
        $out | Should -Not -Match 'cua telemetry'
    }

    It 'sends started then completed with the fixture properties and the stored install id' {
        $cuaHome = Join-Path $env:CUA_TEST_ROOT 'tel-on'
        $vars = @{ CUA_TELEMETRY_DEBUG = '1'; CUA_TELEMETRY_ENDPOINT = 'http://127.0.0.1:9/batch/'; CUA_HOME = $cuaHome; CUA_POSTHOG_KEY = 'phc_test' }
        $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p-tel') } $vars
        $out | Should -Match 'anonymous data about this install'
        $events = Get-Events $out
        @($events | ForEach-Object { $_.properties.step }) | Should -Be @('installer_started', 'installer_completed')
        $id = (Get-Content -Raw (Join-Path $cuaHome 'telemetry/install_id')).Trim()
        $ref = $script:Fixture.events[2].properties
        foreach ($e in $events) {
            $e.distinct_id | Should -Be $id
            $e.event | Should -Be 'cua_installer_step'
            @($e.properties.PSObject.Properties.Name | Sort-Object) | Should -Be @($ref.PSObject.Properties.Name | Sort-Object)
            $e.properties.installer | Should -Be 'ps1'
            $e.properties.items | Should -Be 'cli'
            $e.properties.'$lib' | Should -Be 'cua-installer'
        }
        $events[1].properties.product_version | Should -Be '1.2.3'
        $out | Should -Not -Match ([regex]::Escape('"api_key":"phc_eSk'))
    }

    It 'sends installer_failed with the stage and kind when the install stops' {
        $vars = @{ CUA_INSTALL_MANIFEST_URL = (ConvertTo-FileUrl (Join-Path $env:CUA_TEST_ROOT 'missing/release-artifacts.json'))
            CUA_INSTALL_ARCH = 'AMD64'; CUA_HOME = (Join-Path $env:CUA_TEST_ROOT 'tel-fail'); CUA_TELEMETRY_FORBID_NETWORK = '1'
            CUA_TELEMETRY_DEBUG = '1'; CUA_TELEMETRY_ENDPOINT = 'http://127.0.0.1:9/batch/'
        }
        $out = With-Env $vars {
            $params = @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; NoModifyPath = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p-tel-fail') }
            $lines = & { try { & $env:CUA_TEST_INSTALLER @params } catch { "threw: $_" } } *>&1
            ($lines | ForEach-Object { "$_" }) -join "`n"
        }
        $out | Should -Match 'threw: '
        $events = Get-Events $out
        @($events | ForEach-Object { $_.properties.step }) | Should -Be @('installer_started', 'installer_failed')
        $failed = $events[1].properties
        $failed.install_stage | Should -Be 'manifest'
        $failed.outcome | Should -Be 'error'
        $failed.error_kind | Should -BeIn @('network', 'other')
        ($events | ConvertTo-Json -Depth 6) | Should -Not -Match 'missing'
    }
}
