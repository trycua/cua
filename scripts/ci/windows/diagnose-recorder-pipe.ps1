Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class EncoderPause {
    [DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr handle);
    [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr handle);
}
'@
$root = Join-Path $PWD 'artifacts/recorder-observations'
New-Item -ItemType Directory -Force $root | Out-Null
$shim = (Get-Command ffmpeg.exe).Source
$direct = @(Get-ChildItem C:/ProgramData/chocolatey/lib/ffmpeg -Filter ffmpeg.exe -Recurse)
if ($direct.Count -ne 1) { throw 'Expected one package-owned FFmpeg executable' }
$results = @()
foreach ($route in @('shim', 'direct')) {
    foreach ($close in @($false, $true)) {
        foreach ($iteration in 1..5) {
            $label = "$route-close-$close-$iteration"
            $destination = Join-Path $root $label
            New-Item -ItemType Directory -Force $destination | Out-Null
            $info = [Diagnostics.ProcessStartInfo]::new()
            $info.FileName = if ($route -eq 'shim') { $shim } else { $direct[0].FullName }
            $info.UseShellExecute = $false
            $info.RedirectStandardInput = $true
            $info.RedirectStandardError = $true
            $info.RedirectStandardOutput = $true
            foreach ($argument in @('-y','-loglevel','error','-f','gdigrab','-framerate','30','-draw_mouse','1','-i','desktop','-vf','pad=ceil(iw/2)*2:ceil(ih/2)*2','-c:v','libx264','-preset','ultrafast','-pix_fmt','yuv420p','-movflags','+faststart','-g','30',(Join-Path $destination 'recording.mp4'))) {
                $info.ArgumentList.Add($argument)
            }
            $process = [Diagnostics.Process]::Start($info)
            $stderr = $process.StandardError.ReadToEndAsync()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $encoder = $null
            $suspended = $false
            try {
                Start-Sleep -Milliseconds 1800
                if ($process.HasExited) { throw "Encoder exited before observation: $($process.ExitCode)" }
                if ($route -eq 'shim') {
                    $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($process.Id)" | Where-Object Name -eq 'ffmpeg.exe')
                    if ($children.Count -ne 1) { throw 'Expected one FFmpeg child of shim' }
                    $encoder = [Diagnostics.Process]::GetProcessById($children[0].ProcessId)
                } else { $encoder = $process }
                $suspendStatus = [EncoderPause]::NtSuspendProcess($encoder.Handle)
                if ($suspendStatus -ne 0) { throw "NtSuspendProcess failed: $suspendStatus" }
                $suspended = $true
                $process.StandardInput.Write("q`n")
                $process.StandardInput.Flush()
                if ($close) { $process.StandardInput.Close() }
                Start-Sleep -Milliseconds 100
                $resumeStatus = [EncoderPause]::NtResumeProcess($encoder.Handle)
                if ($resumeStatus -ne 0) { throw "NtResumeProcess failed: $resumeStatus" }
                $suspended = $false
                $watch = [Diagnostics.Stopwatch]::StartNew()
                $exited = $process.WaitForExit(3000)
                $row = @{ route = $route; close_stdin = $close; iteration = $iteration; exited = $exited; elapsed_ms = $watch.ElapsedMilliseconds; exit_code = $(if ($exited) { $process.ExitCode } else { $null }); encoder_pid = $encoder.Id; wrapper_pid = $process.Id }
                if (-not $exited) { $process.Kill($true); $process.WaitForExit() }
                $stderr.GetAwaiter().GetResult() | Set-Content (Join-Path $destination 'stderr.txt')
                $stdout.GetAwaiter().GetResult() | Set-Content (Join-Path $destination 'stdout.txt')
                $row | ConvertTo-Json | Tee-Object -FilePath (Join-Path $destination 'observation.json')
                $results += $row
            } finally {
                if ($suspended) { $null = [EncoderPause]::NtResumeProcess($encoder.Handle) }
                if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
                $process.Dispose()
            }
        }
    }
}
$results | ConvertTo-Json | Set-Content (Join-Path $root 'results.json')
Get-FileHash $shim, $direct[0].FullName | ConvertTo-Json | Set-Content (Join-Path $root 'executable-hashes.json')
if (@($results | Where-Object { -not $_.exited -or $_.exit_code -ne 0 }).Count -gt 0) { throw 'Encoder shutdown failure captured; see retained observations' }
