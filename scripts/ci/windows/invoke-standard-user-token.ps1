# Run a command line in the current interactive session with a standard-user
# token derived from the caller's token.
#
# GitHub-hosted Windows runs jobs as an administrator with a full token. Cua
# Driver refuses to launch an isolated browser from an installation the
# current token can modify, which any administrator token can do for Program
# Files. This helper asks Windows for the SAFER "Normal User" level of the
# caller's own token (the same level `runas /trustlevel:0x20000` uses): the
# Administrators group becomes deny-only and administrative privileges are
# removed, while the user, session, and desktop stay the same. It is a CI
# stand-in for a standard user's logged-in desktop, not a sandbox.
param(
    [Parameter(Mandatory = $true)]
    [string]$CommandLine,
    [Parameter(Mandatory = $true)]
    [string]$WorkingDirectory,
    [int]$TimeoutSeconds = 900
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class CuaStandardUserToken {
    const uint SAFER_SCOPEID_USER = 2;
    const uint SAFER_LEVELID_NORMALUSER = 0x20000;
    const uint SAFER_LEVEL_OPEN = 1;
    const uint CREATE_NO_WINDOW = 0x08000000;
    const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
    const uint WAIT_OBJECT_0 = 0;

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION {
        public IntPtr hProcess, hThread;
        public int dwProcessId, dwThreadId;
    }

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool SaferCreateLevel(uint scopeId, uint levelId, uint openFlags, out IntPtr level, IntPtr reserved);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool SaferComputeTokenFromLevel(IntPtr level, IntPtr inToken, out IntPtr outToken, uint flags, IntPtr reserved);

    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool SaferCloseLevel(IntPtr level);

    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessAsUser(
        IntPtr token, string application, StringBuilder commandLine, IntPtr processAttributes,
        IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment,
        string currentDirectory, ref STARTUPINFO startupInfo, out PROCESS_INFORMATION processInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr process, uint exitCode);

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);

    // Returns the child's exit code, or -1 after terminating it on timeout.
    public static int Run(string commandLine, string workingDirectory, uint timeoutMilliseconds) {
        IntPtr level;
        if (!SaferCreateLevel(SAFER_SCOPEID_USER, SAFER_LEVELID_NORMALUSER, SAFER_LEVEL_OPEN, out level, IntPtr.Zero)) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "SaferCreateLevel failed");
        }
        IntPtr token = IntPtr.Zero;
        try {
            if (!SaferComputeTokenFromLevel(level, IntPtr.Zero, out token, 0, IntPtr.Zero)) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "SaferComputeTokenFromLevel failed");
            }
        } finally {
            SaferCloseLevel(level);
        }
        try {
            var startup = new STARTUPINFO();
            startup.cb = Marshal.SizeOf(typeof(STARTUPINFO));
            startup.lpDesktop = "winsta0\\default";
            PROCESS_INFORMATION process;
            // A null environment inherits this process's environment block.
            if (!CreateProcessAsUser(token, null, new StringBuilder(commandLine), IntPtr.Zero, IntPtr.Zero,
                    false, CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT, IntPtr.Zero,
                    workingDirectory, ref startup, out process)) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateProcessAsUser failed");
            }
            try {
                if (WaitForSingleObject(process.hProcess, timeoutMilliseconds) != WAIT_OBJECT_0) {
                    TerminateProcess(process.hProcess, 124);
                    WaitForSingleObject(process.hProcess, 10000);
                    return -1;
                }
                uint exitCode;
                if (!GetExitCodeProcess(process.hProcess, out exitCode)) {
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed");
                }
                return unchecked((int)exitCode);
            } finally {
                CloseHandle(process.hThread);
                CloseHandle(process.hProcess);
            }
        } finally {
            CloseHandle(token);
        }
    }
}
"@

$exitCode = [CuaStandardUserToken]::Run($CommandLine, $WorkingDirectory, [uint32]($TimeoutSeconds * 1000))
if ($exitCode -eq -1) {
    throw "standard-user command timed out after $TimeoutSeconds seconds"
}
exit $exitCode
