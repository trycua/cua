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
//!    `shell:appsFolder\{PackageFamilyName}!{AppId}` launch token so
//!    callers can hand it to `launch_app`; `{AppId}` falls back to `App`
//!    when the package manifest does not surface a specific
//!    Application.Id.

use std::path::{Path, PathBuf};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::MAX_PATH;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM,
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
    /// What `launch_app(path=...)` would consume.
    ///   - Desktop: absolute `.exe` path, plus any commandline arguments the
    ///     source `.lnk` carries (e.g.
    ///     `"C:\\Path\\chrome.exe" --profile-directory="Profile 2"`).
    ///     The exe path is quoted when it contains whitespace.
    ///   - UWP: `shell:appsFolder\{PackageFamilyName}!{AppId}` — `{AppId}`
    ///     falls back to `App` when the package manifest doesn't surface a
    ///     specific `Application.Id`.
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
    let t0 = std::time::Instant::now();
    let sm = scan_start_menu();
    tracing::debug!(target: "installed_apps", "scan_start_menu: {} entries ({}ms)", sm.len(), t0.elapsed().as_millis());
    out.extend(sm);
    let t1 = std::time::Instant::now();
    let uwp = scan_uwp_packages();
    tracing::debug!(target: "installed_apps", "scan_uwp_packages: {} entries ({}ms)", uwp.len(), t1.elapsed().as_millis());
    out.extend(uwp);

    // Dedupe by (kind, bundle_id) — Start Menu may list both a per-user and a
    // machine-wide shortcut for the same .exe target.
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert((a.kind.clone(), a.bundle_id.clone())));
    out.sort_by(app_sort_key);
    out
}

fn app_sort_key(a: &InstalledApp, b: &InstalledApp) -> std::cmp::Ordering {
    // Keep recently-used apps near the top when we can infer usage recency.
    // Interface-Agent's Windows adapter sorts by LastAccessTime; mirror that
    // behavior here, with deterministic name-based fallback.
    match (a.last_used.as_deref(), b.last_used.as_deref()) {
        (Some(ax), Some(bx)) => bx
            .cmp(ax)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    }
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
        out.push(PathBuf::from(format!(
            "{p}\\Microsoft\\Windows\\Start Menu\\Programs"
        )));
    }
    if let Ok(p) = std::env::var("APPDATA") {
        // CSIDL_PROGRAMS — per-user Start Menu.
        out.push(PathBuf::from(format!(
            "{p}\\Microsoft\\Windows\\Start Menu\\Programs"
        )));
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
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.eq_ignore_ascii_case("lnk"))
            != Some(true)
        {
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
///
/// The `launch_path` is the full commandline — the resolved executable path
/// followed by `IShellLinkW::GetArguments` when non-empty. Many real-world
/// shortcuts encode behavior in their arguments (Chrome profile launchers,
/// per-workspace VS Code shortcuts), so dropping the args would break
/// `launch_app(path=...)` round-trips. The `bundle_id` stays as the exe
/// path alone so the by-basename merge logic in list_apps still works.
fn resolve_lnk(lnk_path: &Path) -> Option<InstalledApp> {
    let (target, args) = unsafe { read_lnk_target(lnk_path) }.ok()?;
    if !target.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let name = lnk_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();
    if name.is_empty() {
        return None;
    }

    let last_used = std::fs::metadata(lnk_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| unix_secs_to_rfc3339(d.as_secs() as i64));

    let launch_path = if args.trim().is_empty() {
        target.clone()
    } else {
        // Quote the exe path if it contains whitespace so a shell-style
        // parse round-trips correctly. Arguments are passed through as
        // received from the shortcut — Windows preserves their own quoting.
        let exe_token = if target.contains(' ') {
            format!("\"{target}\"")
        } else {
            target.clone()
        };
        format!("{exe_token} {}", args.trim())
    };

    Some(InstalledApp {
        name,
        bundle_id: target,
        kind: "desktop".to_owned(),
        launch_path,
        last_used,
    })
}

/// COM-call into IShellLinkW / IPersistFile to read the target `.exe` path and
/// any commandline arguments encoded in the shortcut. Returns `(target, args)`
/// — `args` is the empty string when the shortcut carries no arguments.
unsafe fn read_lnk_target(lnk_path: &Path) -> windows::core::Result<(String, String)> {
    let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    let persist: IPersistFile = shell_link.cast()?;

    let wide_path = to_wide_nul(lnk_path.to_string_lossy().as_ref());
    persist.Load(PCWSTR(wide_path.as_ptr()), STGM(0))?;

    let mut path_buf = vec![0u16; MAX_PATH as usize + 1];
    shell_link.GetPath(&mut path_buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;

    // INFOTIPSIZE (1024) is the documented upper bound for the LNK
    // arguments field; allocate one extra slot for the NUL terminator.
    let mut args_buf = vec![0u16; 1025];
    // GetArguments succeeds with an empty string when no args are set —
    // we don't need to special-case that case.
    let args_res = shell_link.GetArguments(&mut args_buf);
    let args = if args_res.is_ok() {
        decode_wstr(&args_buf)
    } else {
        String::new()
    };

    Ok((decode_wstr(&path_buf), args))
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
            if family_name.is_empty() {
                continue;
            }

            // (Frameworks: Visual C++ runtime, .NET shared assemblies,
            // language packs etc.) — we'd ideally filter via
            // `pkg.IsFramework()`, but that WinRT call has been observed
            // to hang in Session 0 on some Win11 SKUs. Skipping for now;
            // the opaque-family-name filter below catches most of the
            // noise these would have contributed.

            let raw_display = pkg.DisplayName().ok().map(|h| h.to_string());
            let display = normalize_uwp_display_name(raw_display, &pkg, &family_name);

            // Filter out packages whose display name ended up identical
            // to the family name (e.g. `1527c705-839a-4832-...!App` for
            // an opaque system Component without a `<Properties><DisplayName>`
            // entry in its manifest). These show up as line noise in
            // list_apps and are typically unactivatable system pieces
            // the agent should never invoke. Real user-facing apps
            // almost always have a meaningful DisplayName.
            if display == family_name && looks_like_opaque_system_family(&family_name) {
                continue;
            }

            // Resolve manifest Application.Id when available so launch_path
            // works for multi-entry packages too; fallback to "App" for
            // compatibility with single-entry packages.
            let app_id = read_uwp_application_id(&pkg).unwrap_or_else(|| "App".to_owned());
            let launch_path = format!("shell:appsFolder\\{family_name}!{app_id}");
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
        Ok(Ok(v)) => {
            // #1636: when scan_uwp_packages returns 0 packages, log a warning
            // so users hitting the "Calculator isn't in list_apps" gap see a
            // hint immediately instead of silently empty output. The Windows
            // packaging stack reliably has Microsoft.WindowsCalculator and
            // similar OS-bundled UWP apps registered for every user that's
            // logged in interactively at least once, so a zero result usually
            // means PackageManager.FindPackagesWithPackageTypes failed
            // silently (rare COM hiccup, locale issue, etc.) rather than a
            // truly empty package set. Surfacing this in tracing lets users
            // and triage agents (#1636) diagnose without an extra Win32 round-
            // trip. Set RUST_LOG=installed_apps=warn to see the message.
            if v.is_empty() {
                tracing::warn!(
                    target: "installed_apps",
                    "scan_uwp_packages: WinRT PackageManager.FindPackagesWithPackageTypes(Main) \
                     returned 0 UWP packages. Expected at least the OS-bundled apps \
                     (Calculator, modern Settings, Photos, ...). See #1636 — usually means \
                     either a COM activation hiccup or a per-user package context the daemon \
                     can't see. Cross-check with PowerShell `Get-AppxPackage`."
                );
            }
            v
        }
        Ok(Err(e)) => {
            tracing::warn!(target: "installed_apps", "scan_uwp_packages: WinRT error: {e}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(target: "installed_apps", "scan_uwp_packages: panicked (very old Windows?)");
            Vec::new()
        }
    }
}

/// Best-effort parse of AppxManifest.xml to extract the first Application Id.
fn read_uwp_application_id(pkg: &windows::ApplicationModel::Package) -> Option<String> {
    let base = pkg.InstalledPath().ok()?.to_string();
    if base.is_empty() {
        return None;
    }
    let manifest = std::path::Path::new(&base).join("AppxManifest.xml");
    let xml = std::fs::read_to_string(manifest).ok()?;

    // Look for the first <Application ... Id="..."> start tag.
    let app_pos = xml.find("<Application")?;
    let tail = &xml[app_pos..];
    let tag_end = tail.find('>')?;
    let start_tag = &tail[..tag_end];

    let id_key = "Id=\"";
    let id_pos = start_tag.find(id_key)? + id_key.len();
    let rest = &start_tag[id_pos..];
    let id_end = rest.find('"')?;
    let app_id = rest[..id_end].trim();
    if app_id.is_empty() {
        None
    } else {
        Some(app_id.to_owned())
    }
}

fn normalize_uwp_display_name(
    raw_display: Option<String>,
    pkg: &windows::ApplicationModel::Package,
    family_name: &str,
) -> String {
    let trimmed = raw_display
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    if trimmed.is_empty() {
        return family_name.to_owned();
    }
    // Many packaged apps expose an unresolved "ms-resource:" token via
    // Package.DisplayName(). When that happens, prefer manifest
    // Properties/DisplayName if it is a concrete user-facing string.
    if trimmed.starts_with("ms-resource:") {
        if let Some(manifest_name) = read_uwp_manifest_display_name(pkg) {
            return manifest_name;
        }
    }
    trimmed.to_owned()
}

/// True when a UWP family name looks like an opaque system component
/// rather than a real user-facing app — GUID-prefixed packages
/// (`{8-4-4-4-12}_pubid`) or names that begin with `Windows.Internal`.
///
/// These packages routinely ship without a meaningful `DisplayName` /
/// `Properties/DisplayName` (e.g. `CredentialDialogHost`,
/// `CloudExperienceHost`, internal language shims), so they fall through
/// `normalize_uwp_display_name`'s fallbacks to the family name itself.
/// Reporting that in `list_apps` is just noise — agents can't usefully
/// launch them and they crowd out actual installed apps.
fn looks_like_opaque_system_family(family_name: &str) -> bool {
    // Strip the publisher suffix (`_pubid`) before testing the
    // package-name half — that's where the GUID lives.
    let pkg_name = family_name.split('_').next().unwrap_or(family_name);

    // `Windows.Internal.*` is Microsoft's own conventional namespace
    // for internal-only packages.
    if pkg_name.starts_with("Windows.Internal") {
        return true;
    }

    // GUID pattern: 8-4-4-4-12 hex with `-` separators.
    let mut parts = pkg_name.splitn(5, '-');
    let segs = [
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    ];
    let expected_lens = [8, 4, 4, 4, 12];
    let all_hex = segs
        .iter()
        .zip(expected_lens.iter())
        .all(|(s, &n)| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit()));
    all_hex
}

/// Best-effort parse of AppxManifest.xml to extract `<Properties><DisplayName>`.
fn read_uwp_manifest_display_name(pkg: &windows::ApplicationModel::Package) -> Option<String> {
    let base = pkg.InstalledPath().ok()?.to_string();
    if base.is_empty() {
        return None;
    }
    let manifest = std::path::Path::new(&base).join("AppxManifest.xml");
    let xml = std::fs::read_to_string(manifest).ok()?;
    let key = "<DisplayName>";
    let start = xml.find(key)? + key.len();
    let rest = &xml[start..];
    let end = rest.find("</DisplayName>")?;
    let value = rest[..end].trim();
    if value.is_empty() || value.starts_with("ms-resource:") {
        None
    } else {
        Some(value.to_owned())
    }
}
/// Resolve a UWP package's install-path mtime to an RFC3339 string.
fn read_install_mtime(pkg: &windows::ApplicationModel::Package) -> Option<String> {
    let path_hstring = pkg.InstalledPath().ok()?;
    let path = path_hstring.to_string();
    if path.is_empty() {
        return None;
    }
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

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_system_family_matches_guid_prefix() {
        // Both real-world examples seen on a stock Win11 image where
        // Package.DisplayName resolved to an unresolved ms-resource:
        // token and AppxManifest.xml didn't have a Properties/DisplayName.
        assert!(looks_like_opaque_system_family(
            "1527c705-839a-4832-9118-54d4Bd6a0c89_cw5n1h2txyewy"
        ));
        assert!(looks_like_opaque_system_family(
            "F46D4000-FD22-4DB4-AC8E-4E1DDDE828FE_cw5n1h2txyewy"
        ));
    }

    #[test]
    fn opaque_system_family_matches_internal_namespace() {
        assert!(looks_like_opaque_system_family(
            "Windows.Internal.ShellExperience_cw5n1h2txyewy"
        ));
        assert!(looks_like_opaque_system_family(
            "Windows.Internal.PrintingFramework_cw5n1h2txyewy"
        ));
    }

    #[test]
    fn opaque_system_family_keeps_real_apps() {
        // Familiar packaged apps must NOT be filtered out — their family
        // names don't look like GUIDs and don't start with the internal
        // prefix.
        assert!(!looks_like_opaque_system_family(
            "Microsoft.WindowsNotepad_8wekyb3d8bbwe"
        ));
        assert!(!looks_like_opaque_system_family(
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe"
        ));
        assert!(!looks_like_opaque_system_family(
            "Microsoft.MicrosoftEdge.Stable_8wekyb3d8bbwe"
        ));
        assert!(!looks_like_opaque_system_family(
            "SpotifyAB.SpotifyMusic_zpdnekdrzrea0"
        ));
    }

    #[test]
    fn opaque_system_family_handles_missing_publisher_suffix() {
        // No underscore = no publisher half. Still examine the whole
        // string for the GUID / internal-namespace shape.
        assert!(looks_like_opaque_system_family(
            "1527c705-839a-4832-9118-54d4Bd6a0c89"
        ));
        assert!(!looks_like_opaque_system_family("Microsoft.WindowsNotepad"));
        assert!(!looks_like_opaque_system_family(""));
    }
}
