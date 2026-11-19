# Pester 5 tests for uninstall.ps1. Hermetic: temp folders for LOCALAPPDATA,
# APPDATA and CUA_HOME, and every step that would touch the machine (the app's
# uninstaller, scheduled tasks, Run keys, the user PATH, Credential Manager,
# running programs, the cua CLI) is replaced by a mock that records its call.
#   pwsh -c "Invoke-Pester scripts/install/tests -Output Detailed"

BeforeAll {
    $script:Uninstaller = Join-Path $PSScriptRoot '..' 'uninstall.ps1'
    $env:CUA_UNINSTALL_NO_RUN = '1'
    . $script:Uninstaller
    $env:CUA_UNINSTALL_NO_RUN = $null
    $script:Saved = @{}
    foreach ($name in 'LOCALAPPDATA', 'APPDATA', 'CUA_HOME', 'CUA_UNINSTALL_NONINTERACTIVE') {
        $script:Saved[$name] = [Environment]::GetEnvironmentVariable($name)
    }
}

AfterAll {
    foreach ($name in $script:Saved.Keys) { [Environment]::SetEnvironmentVariable($name, $script:Saved[$name]) }
}

Describe 'helpers' {
    BeforeEach {
        $script:Root = Join-Path ([IO.Path]::GetTempPath()) ("cua-ps-uninstall-" + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $script:Root | Out-Null
        $env:LOCALAPPDATA = Join-Path $script:Root 'local'
        $env:APPDATA = Join-Path $script:Root 'roaming'
        $KeepData = [switch]$false
        $DeleteSpaces = [switch]$false
    }
    AfterEach { Remove-Item -Recurse -Force -LiteralPath $script:Root -ErrorAction SilentlyContinue }

    It 'keeps the sign-in entries, the device key and the Keyvault apart' {
        $targets = @('cua-cli.run.cua.ai', 'cua-cli.device-key.run.cua.ai', 'cua-cli.agent-keys.run.cua.ai', 'cua-cli.staging.run.cua.ai',
            'vault-1.Cua Keyvault', 'vault-1.Cua Keyvault Generation', 'MicrosoftAccount:user=a@b.c', 'git:https://github.com', 'cua-cli.run.example.com')
        $picked = @(Select-CuaCredentialTargets $targets)
        $picked | Should -Be @('cua-cli.run.cua.ai', 'cua-cli.device-key.run.cua.ai', 'cua-cli.agent-keys.run.cua.ai', 'cua-cli.staging.run.cua.ai')
        $KeepData = [switch]$true
        @(Select-CuaCredentialTargets $targets) | Should -Be @('cua-cli.run.cua.ai', 'cua-cli.staging.run.cua.ai')
        $KeepData = [switch]$false
        $DeleteSpaces = [switch]$true
        @(Select-CuaCredentialTargets $targets) | Should -Contain 'vault-1.Cua Keyvault'
        @(Select-CuaCredentialTargets $targets) | Should -Contain 'vault-1.Cua Keyvault Generation'
    }

    It 'never deletes a drive root, the profile or the app data folders' {
        Test-SafePath $env:LOCALAPPDATA | Should -BeFalse
        Test-SafePath $env:APPDATA | Should -BeFalse
        Test-SafePath $HOME | Should -BeFalse
        Test-SafePath '' | Should -BeFalse
        Test-SafePath 'relative\path' | Should -BeFalse
        Test-SafePath (Join-Path $env:LOCALAPPDATA 'x\..\..') | Should -BeFalse
        Test-SafePath (Join-Path $env:LOCALAPPDATA 'Programs\Cua Spaces') | Should -BeTrue
    }
}

Describe 'Invoke-CuaUninstall' {
    BeforeEach {
        $script:Root = Join-Path ([IO.Path]::GetTempPath()) ("cua-ps-uninstall-" + [Guid]::NewGuid().ToString('N'))
        $script:Local = Join-Path $script:Root 'local'
        $script:Roaming = Join-Path $script:Root 'roaming'
        $script:CuaHome = Join-Path $script:Root 'cuahome'
        New-Item -ItemType Directory -Force -Path $script:Local, $script:Roaming, $script:CuaHome | Out-Null
        $env:LOCALAPPDATA = $script:Local
        $env:APPDATA = $script:Roaming
        $env:CUA_HOME = $script:CuaHome
        $env:CUA_UNINSTALL_NONINTERACTIVE = '1'
        $KeepData = [switch]$false; $DeleteSpaces = [switch]$false; $All = [switch]$false
        $NoModifyPath = [switch]$false; $DryRun = [switch]$false; $Yes = [switch]$false; $Prefix = ''
        $script:Calls = New-Object System.Collections.Generic.List[string]
        $script:SpacesText = ''
        $script:OldCua = $false
        $script:PlanFails = $false
        $script:Credentials = @('cua-cli.run.cua.ai', 'cua-cli.device-key.run.cua.ai', 'vault-1.Cua Keyvault', 'git:https://github.com')
        $script:UserPath = "C:\Windows;$(Join-Path $script:Local 'Programs\cua\bin');C:\Tools"

        # What install.ps1 and the app leave behind.
        $script:Bin = Join-Path $script:Local 'Programs\cua\bin'
        New-Item -ItemType Directory -Force -Path $script:Bin | Out-Null
        Set-Content -LiteralPath (Join-Path $script:Bin 'cua.exe') -Value 'fake cua'
        foreach ($rel in 'host\config.json', 'config.toml', 'credentials.json', 'cache\blob', 'telemetry\install_channel',
            'volume\cache\block', 'volume\data\notes.txt', 'sandboxes\studio\disk.img', 'vmm\state') {
            $path = Join-Path $script:CuaHome $rel
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
            Set-Content -LiteralPath $path -Value 'x'
        }
        New-Item -ItemType Directory -Force -Path (Join-Path $script:Roaming 'Cua Spaces') | Out-Null
        Set-Content -LiteralPath (Join-Path $script:Roaming 'Cua Spaces\settings.json') -Value '{}'
        $script:Entry = [pscustomobject]@{ Key = 'k'; InstallLocation = (Join-Path $script:Local 'Programs\Cua Spaces'); UninstallString = 'x'; QuietUninstallString = 'x'; WindowsInstaller = $false }

        Mock Stop-CuaSpacesApp { $script:Calls.Add('stop-app') }
        Mock Get-AppUninstallEntry { $script:Entry }
        Mock Invoke-AppUninstaller { $script:Calls.Add('app-uninstaller'); 0 }
        Mock Remove-AppRegistryKey { $script:Calls.Add('registry-key') }
        Mock Remove-HostTask { $script:Calls.Add('host-task') }
        Mock Remove-RunValue { param($Name) $script:Calls.Add("run-value $Name") }
        Mock Get-UserPath { $script:UserPath }
        Mock Set-UserPath { param($Value) $script:Calls.Add("user-path $Value"); $script:UserPath = $Value }
        Mock Get-CredentialTargets { $script:Credentials }
        Mock Remove-CredentialTarget { param($Target) $script:Calls.Add("credential $Target"); $true }
        # The cua CLI: a file that says "fake cua" is the Cua CLI; anything else is not.
        Mock Invoke-Bounded {
            param($File, $Arguments, $Seconds)
            $args1 = @($Arguments) -join ' '
            if ($args1 -eq '--version') {
                if ((Test-Path -LiteralPath $File) -and ((Get-Content -Raw -LiteralPath $File) -match 'fake cua')) {
                    return [pscustomobject]@{ ExitCode = 0; Output = "cua 1.2.3 (test)`n" }
                }
                return [pscustomobject]@{ ExitCode = 0; Output = "something else 9.9`n" }
            }
            $script:Calls.Add("cua[$env:CUA_HOME] $args1")
            # A cua before the agents fix has neither --include-cua-driver nor --dry-run.
            if ($args1 -eq 'agents remove --help') {
                $flags = if ($script:OldCua) { '--agents --skills-only --mcp-only --yes' } else { '--agents --skills-only --mcp-only --yes --include-cua-driver --dry-run' }
                return [pscustomobject]@{ ExitCode = 0; Output = "Usage: cua agents remove [OPTIONS]`n$flags`n" }
            }
            if ($args1 -like 'agents remove * --dry-run') {
                # One agent's config unreadable: the other rows are still listed, and the exit status is 1.
                $failed = if ($script:PlanFails) { "  FAILED MCP server cua: ~/.codex/config.toml (left unchanged)`n" } else { '' }
                return [pscustomobject]@{ ExitCode = [int][bool]$script:PlanFails; Output = "Dry run: nothing is changed.`n  would remove MCP server cua: ~/.claude.json`n  MCP server cua-driver: ~/.claude.json kept (Cua Driver's; --include-cua-driver removes it)`n$failed" }
            }
            if ($args1 -eq 'sandbox ls --local --no-size') { return [pscustomobject]@{ ExitCode = 0; Output = $script:SpacesText } }
            if ($args1 -eq 'host remove --force --uninstalling') { Remove-Item -Recurse -Force -LiteralPath (Join-Path $env:CUA_HOME 'host') }
            return [pscustomobject]@{ ExitCode = 0; Output = '' }
        }
    }
    AfterEach {
        $env:CUA_UNINSTALL_NONINTERACTIVE = $null
        Remove-Item -Recurse -Force -LiteralPath $script:Root -ErrorAction SilentlyContinue
    }

    It 'prints the plan with -DryRun and changes nothing' {
        $DryRun = [switch]$true; $All = [switch]$true; $DeleteSpaces = [switch]$true
        $env:CUA_UNINSTALL_DRIVER_URL = 'https://example.invalid/driver/uninstall.ps1'
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $env:CUA_UNINSTALL_DRIVER_URL = $null
        $out | Should -Match 'Found on this PC'
        $out | Should -Match '\[dry-run\] remove'
        $out | Should -Match '\[dry-run\] cua host remove --force --uninstalling'
        # The agent entries are listed by the same command a real run uses, with --dry-run.
        $out | Should -Match '\[dry-run\] cua agents remove --agents all --include-cua-driver --dry-run:'
        $out | Should -Match '\[dry-run\]   would remove MCP server cua: ~/.claude.json'
        $out | Should -Match 'MCP server cua-driver: ~/.claude.json kept'
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeTrue
        Test-Path (Join-Path $script:CuaHome 'config.toml') | Should -BeTrue
        Test-Path (Join-Path $script:Roaming 'Cua Spaces\settings.json') | Should -BeTrue
        $script:Calls | Should -Not -Contain 'app-uninstaller'
        # Only read-only commands run: the listing of Spaces, and the help and the preview of `agents remove`.
        @($script:Calls | Where-Object {
                $_ -like 'credential *' -or $_ -like 'user-path *' -or
                ($_ -like 'cua*' -and $_ -notlike '*sandbox ls --local*' -and $_ -notlike '*agents remove --help' -and $_ -notlike '*agents remove * --dry-run')
            }) | Should -BeNullOrEmpty
    }

    It 'removes programs and settings by default and keeps Spaces data' {
        $Yes = [switch]$true
        $script:SpacesText = "ID            STATUS   LOCATION  KIND       RUNTIME  EXPIRES`nlocal:studio  ready    local     container  gvisor   -`nlocal:old     stopped  local     vm         qemu     -"
        Invoke-CuaUninstall *>&1 | Out-Null
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeFalse
        foreach ($gone in 'host', 'config.toml', 'credentials.json', 'cache', 'telemetry', 'volume\cache') {
            Test-Path (Join-Path $script:CuaHome $gone) | Should -BeFalse -Because $gone
        }
        Get-Content -Raw (Join-Path $script:CuaHome 'volume\data\notes.txt') | Should -Match 'x'
        Test-Path (Join-Path $script:CuaHome 'sandboxes\studio\disk.img') | Should -BeTrue
        Test-Path (Join-Path $script:CuaHome 'vmm\state') | Should -BeTrue
        Test-Path (Join-Path $script:Roaming 'Cua Spaces') | Should -BeFalse
        $script:UserPath | Should -Be 'C:\Windows;C:\Tools'
        $script:Calls | Should -Contain 'credential cua-cli.run.cua.ai'
        $script:Calls | Should -Contain 'credential cua-cli.device-key.run.cua.ai'
        $script:Calls | Should -Not -Contain 'credential vault-1.Cua Keyvault'
        $script:Calls | Should -Not -Contain 'credential git:https://github.com'
        $script:Calls | Should -Contain 'app-uninstaller'
        $script:Calls | Should -Contain 'host-task'
        # Running Spaces are stopped (their disks stay); nothing is deleted.
        $script:Calls | Should -Contain "cua[$script:CuaHome] spaces stop local:studio"
        @($script:Calls | Where-Object { $_ -like '*spaces stop local:old*' -or $_ -like '*sandbox rm*' }) | Should -BeNullOrEmpty
        # In a safe order: Spaces, hosting, agents, daemon, sign-out.
        $steps = @('spaces stop local:studio', 'host remove --force --uninstalling', 'agents remove --agents all --yes', 'daemon stop', 'auth logout')
        $indexes = $steps | ForEach-Object { $s = $_; [array]::FindIndex($script:Calls.ToArray(), [Predicate[string]] { param($c) $c -like "*$s*" }) }
        $indexes | Should -Not -Contain -1
        ($indexes -join ',') | Should -Be (($indexes | Sort-Object) -join ',')
        # Cua Driver's skill and MCP entries stay: its flag is only passed with -All.
        @($script:Calls | Where-Object { $_ -like '*--include-cua-driver*' }) | Should -BeNullOrEmpty
    }

    It 'removes Cua Driver''s agent entries too only with -All' {
        $Yes = [switch]$true; $All = [switch]$true
        Mock Invoke-OtherUninstaller { param($Name) $script:Calls.Add("other-uninstaller $Name") }
        Invoke-CuaUninstall *>&1 | Out-Null
        $script:Calls | Should -Contain "cua[$script:CuaHome] agents remove --agents all --include-cua-driver --yes"
        $script:Calls | Should -Contain 'other-uninstaller cua-driver'
    }

    It 'skips the agent step, says what stays and still succeeds when cua is too old to leave Cua Driver''s entries alone' {
        $Yes = [switch]$true
        $script:OldCua = $true
        $global:LASTEXITCODE = 0
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $out | Should -Match 'older than the version that removes only Cua Spaces'
        $out | Should -Match 'agent step is skipped'
        $out | Should -Match 'run: cua agents remove'
        # A note, not a failure: the run says it is done.
        $out | Should -Not -Match 'need your attention'
        $out | Should -Match 'cua-uninstall: done'
        $global:LASTEXITCODE | Should -Not -Be 1
        @($script:Calls | Where-Object { $_ -like '*agents remove --agents all*' -and $_ -notlike '*--help' }) | Should -BeNullOrEmpty
        # Everything else still goes.
        $script:Calls | Should -Contain 'app-uninstaller'
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeFalse
    }

    It 'still lists the rows in a dry run when one agent''s config cannot be read' {
        $DryRun = [switch]$true
        $script:PlanFails = $true
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $out | Should -Match '\[dry-run\]   would remove MCP server cua: ~/.claude.json'
        $out | Should -Match '\[dry-run\]   FAILED MCP server cua: ~/.codex/config.toml'
        $out | Should -Match 'reported a problem \(see above\), so the list may be incomplete'
    }

    It 'says so in a dry run when cua is too old to list the agent entries' {
        $DryRun = [switch]$true
        $script:OldCua = $true
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $out | Should -Match '\[dry-run\] cua agents remove --agents all: skipped, this cua is older than'
        $out | Should -Match '\[dry-run\] cua host remove --force --uninstalling'
    }

    It 'keeps the cua home and the device key with -KeepData but still signs out' {
        $Yes = [switch]$true; $KeepData = [switch]$true
        Invoke-CuaUninstall *>&1 | Out-Null
        foreach ($kept in 'config.toml', 'cache\blob', 'volume\cache\block', 'sandboxes\studio\disk.img') {
            Test-Path (Join-Path $script:CuaHome $kept) | Should -BeTrue -Because $kept
        }
        Test-Path (Join-Path $script:CuaHome 'credentials.json') | Should -BeFalse
        Test-Path (Join-Path $script:Roaming 'Cua Spaces\settings.json') | Should -BeTrue
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeFalse
        $script:Calls | Should -Contain 'credential cua-cli.run.cua.ai'
        $script:Calls | Should -Not -Contain 'credential cua-cli.device-key.run.cua.ai'
        @($script:Calls | Where-Object { $_ -like '*auth logout*' }) | Should -Not -BeNullOrEmpty
    }

    It 'stops, then deletes the Spaces, the home and the Keyvault with -DeleteSpaces -Yes' {
        $Yes = [switch]$true; $DeleteSpaces = [switch]$true
        $script:SpacesText = "ID            STATUS   LOCATION  KIND       RUNTIME  EXPIRES`nlocal:studio  ready    local     container  gvisor   -"
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $out | Should -Match 'DELETES the Spaces'
        $stop = $script:Calls.FindIndex([Predicate[string]] { param($c) $c -like '*spaces stop local:studio*' })
        $rm = $script:Calls.FindIndex([Predicate[string]] { param($c) $c -like '*sandbox rm local:studio --force*' })
        $stop | Should -BeGreaterThan -1
        $rm | Should -BeGreaterThan $stop
        Test-Path $script:CuaHome | Should -BeFalse
        $script:Calls | Should -Contain 'credential vault-1.Cua Keyvault'
    }

    It 'refuses to remove anything without -Yes when there is no terminal' {
        { Invoke-CuaUninstall *>&1 | Out-Null } | Should -Throw '*rerun with -Yes*'
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeTrue
        Test-Path (Join-Path $script:CuaHome 'config.toml') | Should -BeTrue
    }

    It 'leaves a cua.exe that is not the Cua CLI alone' {
        $Yes = [switch]$true
        Set-Content -LiteralPath (Join-Path $script:Bin 'cua.exe') -Value 'some other program'
        Invoke-CuaUninstall *>&1 | Out-Null
        Test-Path (Join-Path $script:Bin 'cua.exe') | Should -BeTrue
        $script:UserPath | Should -Match 'Programs'
    }

    It 'never deletes a CUA_HOME that holds no Cua files' {
        $Yes = [switch]$true; $DeleteSpaces = [switch]$true
        $other = Join-Path $script:Root 'elsewhere'
        New-Item -ItemType Directory -Force -Path $other | Out-Null
        Set-Content -LiteralPath (Join-Path $other 'notes.txt') -Value 'mine'
        $env:CUA_HOME = $other
        Invoke-CuaUninstall *>&1 | Out-Null
        Test-Path (Join-Path $other 'notes.txt') | Should -BeTrue
    }

    It 'says so and succeeds when nothing is installed' {
        $Yes = [switch]$true
        Remove-Item -Recurse -Force -LiteralPath $script:Bin, $script:CuaHome, (Join-Path $script:Roaming 'Cua Spaces')
        $script:Entry = $null
        $script:Credentials = @()
        Mock Get-AppUninstallEntry { $null }
        $out = Invoke-CuaUninstall *>&1 | Out-String
        $out | Should -Match 'nothing to remove'
        $script:Calls.Count | Should -Be 0
    }
}
