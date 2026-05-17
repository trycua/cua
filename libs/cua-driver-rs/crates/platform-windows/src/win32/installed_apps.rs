//! Windows installed-app enumeration.
//!
//! Two sources:
//!
//! 1. **Start Menu shortcuts**: walks `[CSIDL_COMMON_PROGRAMS]\` and
//!    `[CSIDL_PROGRAMS]\` recursively, resolves each `.lnk` via
//!    `IShellLinkW::GetPath`, and emits one entry per `.exe` target.
//!
//! 2. **UWP / packaged apps**: queries WinRT
//!    `PackageManager::FindPackagesWithPackageTypes(Main)` for every
//!    `Main` package installed in the current context. Builds the
//!    `shell:appsFolder\{PackageFamilyName}!App` launch token so callers
//!    can hand it to `launch_app`.

use std::path::{Path, PathBuf};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::MAX_PATH;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

/// Parsed metadata for an installed Windows application.
#[derive(Debug, Clone)]
pub struct InstalledApp {
    /// Display name. For Start-Menu apps: the `.lnk` basename (sans extension).
    /// For UWP apps: the package `DisplayName` from the manifest, falling back
    /// to the package family name when not surfaced.
    pub name: String,
    /// Stable identifier. For Start-Menu apps: the resolved `.exe` path.
    /// For UWP apps: the package family name (e.g. `Microsoft.WindowsCalculator_8wekyb3d8bbwe`).
    pub bundle_id: String,
    /// `"desktop"` for `.lnk`-backed apps, `"uwp"` for packaged apps.
    pub kind: String,
    /// What `launch_app(launch_path=...)` would consume.
    ///   - Desktop: absolute `.exe` path.
    ///   - UWP: `shell:appsFolder\{PackageFamilyName}!App`.
    pub launch_path: String,
    /// RFC3339 mtime of the launcher (`.lnk` for desktop, package install
    /// path for UWP), when readable; `None` otherwise.
    pub last_used: Option<String>,
}

/// Aggregate Start-Menu shortcuts and UWP packages into one list.
///
/// Errors from either source are swallowed — the goal is best-effort
/// enumeration, not a guaranteed-complete catalog.
pub fn list_installed_apps() -> Vec<InstalledApp> {
    let mut out = Vec::new();
    out.extend(scan_start_menu());
    out.extend(scan_uwp_packages());

    // Dedupe by (kind, bundle_id) — Start Menu may list both a per-user and a
    // machine-wide shortcut for the same .exe target.
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert((a.kind.clone(), a.bundle_id.clone())));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

// ── Start Menu (.lnk) scan ───────────────────────────────────────────────────

fn scan_start_menu() -> Vec<InstalledApp> {
    let mut out = Vec::new();
    let roots = start_menu_roots();
    // COM init for IShellLinkW. Best-effort — if Apartment-threaded init
    // fails (already-initialized as MTA on this thread), the calls below
    // still succeed because IShellLinkW is registered for both apartments.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    for root in roots {
        let _ = walk_for_lnks(&root, &mut out);
    }
    if init.is_ok() {
        unsafe { CoUninitialize() };
    }
    out
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("ProgramData") {
        // CSIDL_COMMON_PROGRAMS — machine-wide Start Menu.
        out.push(PathBuf::from(format!("{p}\\Microsoft\\Windows\\Start Menu\\Programs")));
    }
    if let Ok(p) = std::env::var("APPDATA") {
        // CSIDL_PROGRAMS — per-user Start Menu.
        out.push(PathBuf::from(format!("{p}\\Microsoft\\Windows\\Start Menu\\Programs")));
    }
    out
}

fn walk_for_lnks(dir: &Path, sink: &mut Vec<InstalledApp>) -> std::io::Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = walk_for_lnks(&path, sink);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("lnk")) != Some(true) {
            continue;
        }
        if let Some(app) = resolve_lnk(&path) {
            sink.push(app);
        }
    }
    Ok(())
}

/// Resolve a `.lnk` to its target via IShellLinkW. Returns `None` if the
/// target isn't a `.exe` or the resolve failed.
fn resolve_lnk(lnk_path: &Path) -> Option<InstalledApp> {
    let target = unsafe { read_lnk_target(lnk_path) }.ok()?;
    if !target.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let name = lnk_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();
    if name.is_empty() { return None; }

    let last_used = std::fs::metadata(lnk_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| unix_secs_to_rfc3339(d.as_secs() as i64));

    Some(InstalledApp {
        name,
        bundle_id: target.clone(),
        kind: "desktop".to_owned(),
        launch_path: target,
        last_used,
    })
}

/// COM-call into IShellLinkW / IPersistFile to read the target `.exe` path.
unsafe fn read_lnk_target(lnk_path: &Path) -> windows::core::Result<String> {
    let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    let persist: IPersistFile = shell_link.cast()?;

    let wide_path = to_wide_nul(lnk_path.to_string_lossy().as_ref());
    persist.Load(PCWSTR(wide_path.as_ptr()), STGM(0))?;

    let mut buf = vec![0u16; MAX_PATH as usize + 1];
    shell_link.GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
    Ok(decode_wstr(&buf))
}

fn to_wide_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn decode_wstr(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

// ── UWP / packaged-app enumeration via WinRT PackageManager ──────────────────

fn scan_uwp_packages() -> Vec<InstalledApp> {
    // Wrapped in catch_unwind because the WinRT activation path can panic on
    // very old Windows builds (pre-1809) that lack PackageManager altogether.
    let result = std::panic::catch_unwind(|| -> windows::core::Result<Vec<InstalledApp>> {
        use windows::ApplicationModel::Package;
        use windows::Management::Deployment::{PackageManager, PackageTypes};

        let pm = PackageManager::new()?;
        let packages = pm.FindPackagesWithPackageTypes(PackageTypes::Main)?;

        let mut out = Vec::new();
        for pkg in packages {
            // `pkg: Package` — annotate locally to satisfy the type
            // inference paths through `id.FamilyName()` and friends.
            let pkg: Package = pkg;
            let id = match pkg.Id() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let family_name = match id.FamilyName() {
                Ok(h) => h.to_string(),
                Err(_) => continue,
            };
            if family_name.is_empty() { continue }

            let display = match pkg.DisplayName() {
                Ok(h) => {
                    let s = h.to_string();
                    if s.is_empty() { family_name.clone() } else { s }
                }
                Err(_) => family_name.clone(),
            };

            // ApplicationModel::Package doesn't surface the per-app
            // Application.Id list directly in the stable WinRT API; the
            // canonical shell launch token "shell:appsFolder\{family}!{appid}"
            // needs the AppId from the package manifest, which we don't
            // fetch here to keep this enumeration cheap.
            //
            // Falling back to "shell:appsFolder\{family}!App" works for the
            // overwhelming majority of single-entry packages (the manifest's
            // default Application Id is literally "App"); for multi-entry
            // packages the caller can read the manifest itself.
            let launch_path = format!("shell:appsFolder\\{family_name}!App");
            let last_used = read_install_mtime(&pkg);

            out.push(InstalledApp {
                name: display,
                bundle_id: family_name,
                kind: "uwp".to_owned(),
                launch_path,
                last_used,
            });
        }
        Ok(out)
    });

    match result {
        Ok(Ok(v)) => v,
        _ => Vec::new(),
    }
}

/// Resolve a UWP package's install-path mtime to an RFC3339 string.
fn read_install_mtime(pkg: &windows::ApplicationModel::Package) -> Option<String> {
    let path_hstring = pkg.InstalledPath().ok()?;
    let path = path_hstring.to_string();
    if path.is_empty() { return None }
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(unix_secs_to_rfc3339(duration.as_secs() as i64))
}

// ── Date helper ──────────────────────────────────────────────────────────────

/// Format Unix epoch seconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
fn unix_secs_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hour, minute, second)
}
