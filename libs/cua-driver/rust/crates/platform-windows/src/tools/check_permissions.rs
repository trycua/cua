use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

// ── check_permissions ─────────────────────────────────────────────────────────

pub struct CheckPermissionsTool;
static PERMS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef {
        PERMS_DEF.get_or_init(|| ToolDef {
            name: "check_permissions".into(),
            description: "Check required permissions for cua-driver-rs on Windows.".into(),
            input_schema: json!({"type":"object","properties":{"prompt":{"type":"boolean","description":"Ignored: Windows has no permission prompts."}},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        // `elevated` reports the daemon's actual token integrity level, NOT the
        // legacy `TokenIsElevated` UAC-elevation flag. The two diverge in two
        // important cases that matter to cua-driver:
        //
        //   1. RID-500 built-in Administrator — runs at High IL all the time
        //      because RID 500 has no UAC split token (FilterAdministratorToken=0
        //      default). `TokenIsElevated` returns false (never UAC-prompted)
        //      but the IL is High and admin-level access works fine.
        //
        //   2. Task-Scheduler-spawned processes registered at RunLevel=Highest
        //      and triggered by AtLogon — Task Scheduler hands out the user's
        //      full admin token without going through the UAC prompt path, so
        //      `TokenIsElevated` returns false while IL is High. This is the
        //      common case for cua-driver's autostart task on non-RID-500
        //      admin users — the daemon IS at High IL (cross-AppContainer UIA
        //      works, Calculator-class UWP apps drive cleanly) but the legacy
        //      flag reports it as a "standard user", which confused everyone
        //      tracking #1602 / #1601. See issue #1640.
        //
        // #1646 / dogfood-iteration-3: PR #1647's in-process windows-rs
        // implementation of TokenIntegrityLevel STILL misreported the IL.
        // Verified externally on the cuademo dogfood VM: a daemon at
        // demonstrably High IL (RID 0x3000 by direct PowerShell-C# Win32
        // read against the daemon's pid) was reported by the daemon's own
        // check_permissions as Medium IL (RID 0x2000). Both code paths used
        // identical Win32 APIs (OpenProcess + OpenProcessToken +
        // GetTokenInformation(TokenIntegrityLevel=25)), yet they disagreed.
        // Root cause unclear — possibly a subtle interaction between the
        // windows-rs crate's bindings and the named-pipe server's thread
        // security context, possibly a Vec-buffer alignment issue that
        // doesn't reproduce in standalone testing. RevertToSelf +
        // OpenProcess(GetCurrentProcessId()) didn't help.
        //
        // Pragmatic workaround: shell out to PowerShell and run the same
        // C# Win32 helper that's verified to return the correct IL. Cost
        // is one PowerShell spawn (~100-200ms) per check_permissions call,
        // which is acceptable — check_permissions is a low-frequency,
        // user-initiated diagnostic, not a hot path.
        const HIGH_IL_RID: u32 = 0x3000;

        let il_rid: Option<u32> = read_self_integrity_level_via_powershell();

        let is_elevated = il_rid.map(|r| r >= HIGH_IL_RID).unwrap_or(false);
        let il_name = match il_rid {
            Some(0x0000) => "Untrusted",
            Some(0x1000) => "Low",
            Some(0x2000) => "Medium",
            Some(0x2100) => "Medium+",
            Some(0x3000) => "High",
            Some(0x4000) => "System",
            Some(_) => "Unknown",
            None => "Unavailable",
        };
        // Distinguish the unavailable case from a real measurement — don't
        // render a fake RID 0x0000 or a fake elevation status when the lookup
        // failed. See CodeRabbit review on PR #1641.
        let status_text = match il_rid {
            Some(rid) => format!(
                "Process integrity level: {il_name} (RID 0x{rid:04X}, {})\n\
                 UIA accessibility: available (no special permission needed)\n\
                 PostMessage injection: available",
                if is_elevated {
                    "elevated"
                } else {
                    "non-elevated"
                }
            ),
            None => format!(
                "Process integrity level: {il_name} (token query failed)\n\
                 UIA accessibility: available (no special permission needed)\n\
                 PostMessage injection: available"
            ),
        };
        ToolResult::text(status_text).with_structured(json!({
            "elevated": is_elevated,
            "integrity_level": il_name,
            "integrity_level_rid": il_rid,
            "uia": true,
            "post_message": true
        }))
    }
}

/// Read the current process's token integrity level (RID) by shelling out
/// to PowerShell and running a P/Invoke-based C# helper. See the long
/// comment in `CheckPermissionsTool::invoke` for why the in-process
/// windows-rs approach was abandoned (issue #1646 / dogfood iteration 3).
///
/// Returns `Some(rid)` on success (e.g. `0x3000` for High), `None` if
/// PowerShell exits non-zero or stdout isn't a u32. Spawns one short-lived
/// `powershell.exe` process per call; only used from `check_permissions`,
/// which is a low-frequency, user-initiated diagnostic.
fn read_self_integrity_level_via_powershell() -> Option<u32> {
    let pid = std::process::id();
    // Inline the same C# helper we verified externally on the dogfood VM.
    // Heredoc-wrap (`@" ... "@`) avoids shell-escape acrobatics around the
    // C# braces / double-quotes. The `Write-Output` at the end is parsed
    // as a plain integer.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class IL {{
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(int a, bool i, int p);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("advapi32.dll")] public static extern bool OpenProcessToken(IntPtr p, int a, out IntPtr t);
    [DllImport("advapi32.dll")] public static extern bool GetTokenInformation(IntPtr t, int c, IntPtr i, int l, out int n);
    [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthority(IntPtr s, int i);
    [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthorityCount(IntPtr s);
    public static uint? Rid(int pid) {{
        var h = OpenProcess(0x1000, false, pid); if (h == IntPtr.Zero) return null;
        try {{ IntPtr t; if (!OpenProcessToken(h, 0x8, out t)) return null;
            try {{ int n; GetTokenInformation(t, 25, IntPtr.Zero, 0, out n); if (n == 0) return null;
                var b = Marshal.AllocHGlobal(n); try {{ if (!GetTokenInformation(t, 25, b, n, out n)) return null;
                    var s = Marshal.ReadIntPtr(b); var c = Marshal.ReadByte(GetSidSubAuthorityCount(s));
                    return (uint)Marshal.ReadInt32(GetSidSubAuthority(s, c - 1)); }}
                finally {{ Marshal.FreeHGlobal(b); }} }}
            finally {{ CloseHandle(t); }} }}
        finally {{ CloseHandle(h); }}
    }}
}}
"@
$rid = [IL]::Rid({pid})
if ($null -ne $rid) {{ Write-Output ([int]$rid) }}
"#,
        pid = pid
    );
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.trim().parse::<u32>().ok()
}
