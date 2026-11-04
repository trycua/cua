# Verify that the runner is inside an interactive user session, not Session 0.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$sessionId = (Get-Process -Id $PID).SessionId
if ($sessionId -eq 0) {
    throw "The test runner is in Session 0. Start it from the active console/RDP user session."
}

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class CuaInteractiveDesktopProbe {
    const int UOI_NAME = 2;
    const uint DESKTOP_READOBJECTS = 0x0001;
    const uint DESKTOP_SWITCHDESKTOP = 0x0100;

    [DllImport("user32.dll", SetLastError = true)]
    static extern IntPtr GetProcessWindowStation();

    [DllImport("kernel32.dll")]
    static extern uint GetCurrentThreadId();

    [DllImport("user32.dll", SetLastError = true)]
    static extern IntPtr GetThreadDesktop(uint threadId);

    [DllImport("user32.dll", SetLastError = true)]
    static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);

    [DllImport("user32.dll", SetLastError = true)]
    static extern bool CloseDesktop(IntPtr desktop);

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool GetUserObjectInformation(
        IntPtr handle, int index, StringBuilder info, int length, out int needed);

    static string Name(IntPtr handle, string kind) {
        if (handle == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), kind + " handle is null");
        }
        int needed;
        GetUserObjectInformation(handle, UOI_NAME, null, 0, out needed);
        if (needed <= 2) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), kind + " name is unavailable");
        }
        var value = new StringBuilder(needed / 2);
        if (!GetUserObjectInformation(handle, UOI_NAME, value, needed, out needed)) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), kind + " name query failed");
        }
        return value.ToString();
    }

    public static string[] Inspect() {
        var station = Name(GetProcessWindowStation(), "window station");
        var threadDesktop = Name(GetThreadDesktop(GetCurrentThreadId()), "thread desktop");
        var input = OpenInputDesktop(0, false, DESKTOP_READOBJECTS | DESKTOP_SWITCHDESKTOP);
        if (input == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenInputDesktop failed");
        }
        try {
            return new [] { station, threadDesktop, Name(input, "input desktop") };
        } finally {
            CloseDesktop(input);
        }
    }
}
"@

$desktopState = [CuaInteractiveDesktopProbe]::Inspect()
$windowStation = $desktopState[0]
$threadDesktop = $desktopState[1]
$inputDesktop = $desktopState[2]
if ($windowStation -ne "WinSta0") {
    throw "The test runner is attached to window station '$windowStation', not interactive WinSta0."
}
if ($threadDesktop -ne $inputDesktop) {
    throw "The test thread desktop '$threadDesktop' is not the active input desktop '$inputDesktop'."
}

$sessionName = if ($env:SESSIONNAME) { $env:SESSIONNAME } else { "unknown" }
Write-Host "Interactive Windows session verified: id=$sessionId name=$sessionName station=$windowStation desktop=$threadDesktop" -ForegroundColor Green
