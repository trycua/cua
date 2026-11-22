# Dismiss the Windows "temporary paging file" notice during an E2E lane.
#
# Hosted Windows runners show a system dialog ("System Properties": "Windows
# created a temporary paging file on your computer ...") in the interactive
# desktop at an unpredictable moment, sometimes mid-lane. It is a system
# window outside the target app, so foreground SendInput typing stalls on it
# (the first unit is never read by the target) until someone clicks OK. The
# release gate saw this as text_delivery_stalled in the WPF multiline
# type_text case (cua-driver-rs 0.56.1, 0.56.5, 0.56.7 and 0.58.0).
#
# Dot-source this file, then wrap the lane:
#   $guard = Start-SystemDialogGuard -LogPath $path
#   try { ... } finally { Stop-SystemDialogGuard $guard }
# `-SelfTest` raises a look-alike dialog and checks the guard dismisses it.
param([switch]$SelfTest)

Set-StrictMode -Version Latest

$script:SystemDialogGuardBody = {
    param([string]$LogPath)
    Add-Type -Namespace Cua -Name DialogWin -MemberDefinition @'
public delegate bool EnumProc(System.IntPtr hwnd, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumChildWindows(System.IntPtr h, EnumProc p, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder s, int n);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr h, System.Text.StringBuilder s, int n);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool PostMessage(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
'@
    function Get-Text([IntPtr]$h) {
        $sb = New-Object System.Text.StringBuilder 1024
        [void][Cua.DialogWin]::GetWindowText($h, $sb, $sb.Capacity)
        $sb.ToString()
    }
    function Get-Class([IntPtr]$h) {
        $sb = New-Object System.Text.StringBuilder 256
        [void][Cua.DialogWin]::GetClassName($h, $sb, $sb.Capacity)
        $sb.ToString()
    }
    $BM_CLICK = 0x00F5
    while ($true) {
        $dialogs = New-Object System.Collections.ArrayList
        $outer = [Cua.DialogWin+EnumProc]{
            param($h, $l)
            if ([Cua.DialogWin]::IsWindowVisible($h) -and (Get-Class $h) -eq '#32770' -and (Get-Text $h) -eq 'System Properties') {
                [void]$dialogs.Add($h)
            }
            $true
        }
        [void][Cua.DialogWin]::EnumWindows($outer, [IntPtr]::Zero)
        foreach ($dialog in $dialogs) {
            $text = New-Object System.Collections.ArrayList
            $ok = @{ Handle = [IntPtr]::Zero }
            $inner = [Cua.DialogWin+EnumProc]{
                param($c, $l)
                $class = Get-Class $c
                if ($class -eq 'Static') { [void]$text.Add((Get-Text $c)) }
                if ($class -eq 'Button' -and $ok.Handle -eq [IntPtr]::Zero) { $ok.Handle = $c }
                $true
            }
            [void][Cua.DialogWin]::EnumChildWindows($dialog, $inner, [IntPtr]::Zero)
            # Only the known paging-file notice; any other dialog is left alone.
            if (($text -join ' ') -match 'temporary paging file' -and $ok.Handle -ne [IntPtr]::Zero) {
                [void][Cua.DialogWin]::PostMessage($ok.Handle, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
                Add-Content -Path $LogPath -Value ("{0:o} dismissed paging-file notice" -f (Get-Date))
            }
        }
        Start-Sleep -Milliseconds 250
    }
}

function Start-SystemDialogGuard {
    param([Parameter(Mandatory)][string]$LogPath)
    Start-Job -ScriptBlock $script:SystemDialogGuardBody -ArgumentList $LogPath
}

function Stop-SystemDialogGuard {
    param($Job)
    if ($null -ne $Job) {
        Stop-Job $Job -ErrorAction SilentlyContinue
        Remove-Job $Job -Force -ErrorAction SilentlyContinue
    }
}

if ($SelfTest) {
    $log = Join-Path ([System.IO.Path]::GetTempPath()) "cua-dialog-guard-selftest.log"
    Remove-Item $log -ErrorAction SilentlyContinue
    $guard = Start-SystemDialogGuard -LogPath $log
    try {
        # Blocks until the guard clicks OK; the 20 s ceiling is the failure signal.
        $box = Start-Job { Add-Type -Namespace Cua -Name Box2 -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MessageBox(System.IntPtr h, string t, string c, uint ty);'; [Cua.Box2]::MessageBox([IntPtr]::Zero, 'Windows created a temporary paging file on your computer.', 'System Properties', 0) }
        if (-not (Wait-Job $box -Timeout 20)) { throw "guard did not dismiss the paging-file notice within 20 s" }
        Write-Host "SelfTest ok: $(Get-Content $log -Raw)"
    } finally {
        Stop-SystemDialogGuard $guard
        if ($box) { Remove-Job $box -Force -ErrorAction SilentlyContinue }
    }
}
