//! App icons for a Space's windows, as the guest's own desktop shows them.
//!
//! No spacesd RPC carries icons, so [`Space::app_icons`] asks the guest with
//! the primitives every Space has: one shell script ([`Space::bash`]) looks
//! every requested app up and appends the icon files it finds to one bundle
//! file, and one spacesd download fetches the bundle. A window list's icons
//! cost two round trips however many apps it has.
//!
//! - macOS: the app bundle of the window's pid, rendered to a 64 px PNG by
//!   the guest's own `NSWorkspace.iconForFile` (Assets.car and `.icns`
//!   alike);
//! - Linux (any desktop, Omarchy included): the `.desktop` entry whose name,
//!   `StartupWMClass` or `Exec` matches the window's app (or its process),
//!   then that entry's `Icon=` in the icon theme: PNG sizes first (64, 48,
//!   128, 96, 256, 32), `pixmaps`, then scalable SVG, converted to PNG in the
//!   guest when `rsvg-convert` or ImageMagick is there;
//! - Windows: not supported; the result is `None`.
//!
//! Results go through the SDK's one icon cache ([`cua_icon_cache`]): keyed
//! by the app's id and the Space's image digest (or OS version), never the
//! pid, normalized to 32 and 64 px PNGs, in memory and under
//! `$CUA_HOME/cache/icons`. When the guest has no icon for an app (a bare
//! X11 client, a fixture) the result is `None` (remembered for a few
//! minutes): callers show no icon, never a stand-in glyph.

use crate::error::Result;
use crate::space::Space;
use cua_icon_cache::{IconCache, IconKey};
use cua_spacesd_client::pb;
use std::time::Duration;

/// Largest icon file accepted.
pub const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;

/// How long the guest script may run.
pub const ICON_SCRIPT_TIMEOUT: Duration = Duration::from_secs(15);

/// An app icon, normalized by the icon cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppIcon {
    /// The 64 px PNG (2x), or the SVG the guest could not rasterize.
    pub bytes: Vec<u8>,
    /// The 32 px PNG (1x); empty for an SVG.
    pub bytes_1x: Vec<u8>,
    /// `image/png` or `image/svg+xml`.
    pub content_type: &'static str,
}

impl From<&cua_icon_cache::Icon> for AppIcon {
    fn from(i: &cua_icon_cache::Icon) -> Self {
        AppIcon {
            bytes: i.bytes.clone(),
            bytes_1x: i.bytes_1x.clone(),
            content_type: i.content_type,
        }
    }
}

/// One app to look up: a window's app as the window list reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconRequest {
    /// App name.
    pub app_name: String,
    /// App id (bundle id, `.desktop` id).
    pub app_id: String,
    /// A process of the app (macOS finds the bundle by it); 0 when unknown.
    pub pid: u32,
}

/// Which lookup a guest gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconPlatform {
    Macos,
    Linux,
}

impl IconPlatform {
    /// The lookup for a guest OS family; `None` for Windows and unknown.
    pub fn of(os: pb::OsFamily) -> Option<IconPlatform> {
        match os {
            pb::OsFamily::Macos => Some(IconPlatform::Macos),
            pb::OsFamily::Linux => Some(IconPlatform::Linux),
            _ => None,
        }
    }
}

/// The app's stable identity in the icon cache: its id, else its name.
/// Never the pid: every window of an app shares one icon.
pub fn icon_identity(app_name: &str, app_id: &str) -> String {
    let id = app_id.trim();
    if id.is_empty() {
        app_name.trim().to_lowercase()
    } else {
        id.to_lowercase()
    }
}

/// A name safe to splice into the script unquoted: letters, digits and
/// `._+-` only (window lists come from the guest; nothing else is needed to
/// match a desktop entry).
pub fn shell_word(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        .collect()
}

/// The guest script: prints the path of the icon file, or nothing. `out` is
/// where a rendered or converted PNG is written.
pub fn icon_script(p: IconPlatform, app_name: &str, app_id: &str, pid: u32, out: &str) -> String {
    let out: String = out
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '/'))
        .collect();
    let out = if out.starts_with('/') {
        out
    } else {
        format!("/tmp/{out}")
    };
    match p {
        IconPlatform::Macos => {
            let name = shell_word(app_name);
            format!(
                r#"app=""
exe=$(ps -o comm= -p {pid} 2>/dev/null)
case "$exe" in *.app/*) app="${{exe%%.app/*}}.app";; esac
if [ ! -d "$app" ] && [ -n "{name}" ]; then
  for d in /Applications /System/Applications /System/Applications/Utilities /Applications/Utilities "$HOME/Applications"; do
    [ -d "$d/{name}.app" ] && {{ app="$d/{name}.app"; break; }}
  done
fi
[ -d "$app" ] || exit 0
APP="$app" /usr/bin/osascript -l JavaScript -e 'ObjC.import("AppKit");
var path=$.NSProcessInfo.processInfo.environment.objectForKey("APP").js;
var img=$.NSWorkspace.sharedWorkspace.iconForFile(path);
img.setSize($.NSMakeSize(64,64));
var rep=$.NSBitmapImageRep.imageRepWithData(img.TIFFRepresentation);
var png=rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $());
png.writeToFileAtomically("{out}", true);' >/dev/null 2>&1 && echo "{out}"
"#
            )
        }
        IconPlatform::Linux => {
            let names: Vec<String> = [app_id, app_name]
                .iter()
                .map(|n| shell_word(n))
                .filter(|n| !n.is_empty())
                .collect();
            let names = names.join(" ");
            format!(
                r#"want=""
for n in {names}; do want="$want $(printf %s "$n" | tr 'A-Z' 'a-z')"; done
if [ {pid} -gt 0 ] && [ -r /proc/{pid}/comm ]; then want="$want $(tr 'A-Z' 'a-z' < /proc/{pid}/comm)"; fi
[ -n "$want" ] || exit 0
icon=""
for f in /usr/share/applications/*.desktop /usr/local/share/applications/*.desktop "$HOME"/.local/share/applications/*.desktop /var/lib/flatpak/exports/share/applications/*.desktop; do
  [ -f "$f" ] || continue
  base=$(basename "$f" .desktop | tr 'A-Z' 'a-z')
  wm=$(grep -m1 '^StartupWMClass=' "$f" | cut -d= -f2- | tr 'A-Z' 'a-z')
  exe=$(grep -m1 '^Exec=' "$f" | cut -d= -f2- | awk '{{print $1}}' | xargs -r basename 2>/dev/null | tr 'A-Z' 'a-z')
  for w in $want; do
    case "$w" in "$base"|"$wm"|"$exe"|*."$base"|"$base".*) icon=$(grep -m1 '^Icon=' "$f" | cut -d= -f2-); break 2;; esac
  done
done
[ -n "$icon" ] || exit 0
found=""
case "$icon" in /*) [ -f "$icon" ] && found="$icon";; esac
if [ -z "$found" ]; then
  for s in 64 48 128 96 256 32; do
    for d in "$HOME/.local/share/icons/hicolor/${{s}}x$s/apps" "/usr/share/icons/hicolor/${{s}}x$s/apps" /usr/share/icons/*/"${{s}}x$s"/apps /usr/share/icons/*/apps/"$s"; do
      [ -f "$d/$icon.png" ] && {{ found="$d/$icon.png"; break 2; }}
    done
  done
fi
[ -z "$found" ] && [ -f "/usr/share/pixmaps/$icon.png" ] && found="/usr/share/pixmaps/$icon.png"
if [ -z "$found" ]; then
  for d in "$HOME/.local/share/icons/hicolor/scalable/apps" /usr/share/icons/hicolor/scalable/apps /usr/share/icons/*/scalable/apps /usr/share/pixmaps; do
    [ -f "$d/$icon.svg" ] && {{ found="$d/$icon.svg"; break; }}
  done
fi
[ -n "$found" ] || exit 0
case "$found" in *.svg)
  rm -f "{out}"
  if command -v rsvg-convert >/dev/null 2>&1; then rsvg-convert -w 64 -h 64 "$found" -o "{out}" 2>/dev/null
  elif command -v convert >/dev/null 2>&1; then convert -background none -density 384 "$found" -resize 64x64 "png:{out}" 2>/dev/null
  fi
  [ -s "{out}" ] && found="{out}";;
esac
echo "$found"
"#
            )
        }
    }
}

/// What the bytes are: a PNG or an SVG document; `None` for anything else
/// (XPM, ICO, a truncated file).
pub fn content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    let t = head.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with('<') && head.contains("<svg")).then_some("image/svg+xml")
}

/// The path the script printed: its last line, when absolute.
pub fn printed_path(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .filter(|l| l.starts_with('/'))
}

/// The guest script for a batch: looks each request up ([`icon_script`])
/// and appends every icon file found to `bundle`, framed as
/// `<index> <length>\n<bytes>`; prints `bundle` when it holds any. Bundles
/// older than five minutes (from runs that never downloaded theirs) go.
pub fn batch_script(p: IconPlatform, requests: &[IconRequest], bundle: &str) -> String {
    let bundle: String = bundle
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
        .collect();
    let mut s = format!(
        "find /tmp -maxdepth 1 -name 'cua-app-icons-*' -mmin +5 -exec rm -f {{}} + 2>/dev/null\n\
         b=\"{bundle}\"\n: > \"$b\" || exit 0\nn=0\n\
         emit() {{\n\
         \x20 if [ -n \"$2\" ] && [ -f \"$2\" ]; then\n\
         \x20   len=$(wc -c < \"$2\" | tr -d ' ')\n\
         \x20   printf '%s %s\\n' \"$1\" \"$len\" >> \"$b\"; cat \"$2\" >> \"$b\"; n=$((n+1))\n\
         \x20 fi\n\
         }}\n"
    );
    match p {
        IconPlatform::Macos => {
            for (i, r) in requests.iter().enumerate() {
                let out = format!("{bundle}.{i}.png");
                let lookup = icon_script(p, &r.app_name, &r.app_id, r.pid, &out);
                s.push_str(&format!(
                    "lookup_{i}() {{\n{lookup}\n}}\nemit {i} \"$(lookup_{i})\"\nrm -f \"{out}\"\n"
                ));
            }
        }
        IconPlatform::Linux => {
            s.push_str(LINUX_BATCH_PRELUDE);
            for (i, r) in requests.iter().enumerate() {
                let names: Vec<String> = [&r.app_id, &r.app_name]
                    .iter()
                    .map(|n| shell_word(n).to_lowercase())
                    .filter(|n| !n.is_empty())
                    .collect();
                let names = names.join(" ");
                let pid = r.pid;
                s.push_str(&format!(
                    "c=\"\"\n\
                     if [ {pid} -gt 0 ] && [ -r /proc/{pid}/comm ]; then read -r c < /proc/{pid}/comm; c=$(printf %s \"$c\" | tr 'A-Z' 'a-z'); fi\n\
                     icon=$(match {names} $c)\n\
                     [ -n \"$icon\" ] && emit {i} \"$(theme \"$icon\" \"$b.{i}.png\")\"\n\
                     rm -f \"$b.{i}.png\"\n"
                ));
            }
        }
    }
    s.push_str("if [ \"$n\" -gt 0 ]; then echo \"$b\"; else rm -f \"$b\"; fi\n");
    s
}

/// The Linux batch's shared part: one `awk` pass indexes every `.desktop`
/// entry (`base|StartupWMClass|Exec basename|Icon`, lowercased but the
/// icon), so a batch costs one scan however many apps it asks about;
/// `match` finds the first entry naming one of its arguments and `theme`
/// resolves its `Icon=` in the icon theme (PNG sizes first, `pixmaps`, then
/// scalable SVG converted in the guest when it can).
const LINUX_BATCH_PRELUDE: &str = r#"idx=$(awk '
function flush() { if (f != "") print base "|" wm "|" exe "|" icon }
FNR == 1 { flush(); f = FILENAME; k = split(FILENAME, a, "/"); base = tolower(a[k]); sub(/\.desktop$/, "", base); wm = ""; exe = ""; icon = "" }
/^StartupWMClass=/ && wm == "" { wm = tolower(substr($0, 16)) }
/^Exec=/ && exe == "" { split(substr($0, 6), p, " "); k = split(p[1], q, "/"); exe = tolower(q[k]) }
/^Icon=/ && icon == "" { icon = substr($0, 6) }
END { flush() }
' /usr/share/applications/*.desktop /usr/local/share/applications/*.desktop "$HOME"/.local/share/applications/*.desktop /var/lib/flatpak/exports/share/applications/*.desktop 2>/dev/null)
match() {
  printf '%s\n' "$idx" | while IFS='|' read -r base wm exe icon; do
    [ -n "$base" ] || continue
    for w in "$@"; do
      case "$w" in "$base"|"$wm"|"$exe"|*."$base"|"$base".*) echo "$icon"; exit 0;; esac
    done
  done
}
theme() {
  icon=$1; out=$2; found=""
  case "$icon" in /*) [ -f "$icon" ] && found="$icon";; esac
  if [ -z "$found" ]; then
    for s in 64 48 128 96 256 32; do
      for d in "$HOME/.local/share/icons/hicolor/${s}x$s/apps" "/usr/share/icons/hicolor/${s}x$s/apps" /usr/share/icons/*/"${s}x$s"/apps /usr/share/icons/*/apps/"$s"; do
        [ -f "$d/$icon.png" ] && { found="$d/$icon.png"; break 2; }
      done
    done
  fi
  [ -z "$found" ] && [ -f "/usr/share/pixmaps/$icon.png" ] && found="/usr/share/pixmaps/$icon.png"
  if [ -z "$found" ]; then
    for d in "$HOME/.local/share/icons/hicolor/scalable/apps" /usr/share/icons/hicolor/scalable/apps /usr/share/icons/*/scalable/apps /usr/share/pixmaps; do
      [ -f "$d/$icon.svg" ] && { found="$d/$icon.svg"; break; }
    done
  fi
  [ -n "$found" ] || return 0
  case "$found" in *.svg)
    rm -f "$out"
    if command -v rsvg-convert >/dev/null 2>&1; then rsvg-convert -w 64 -h 64 "$found" -o "$out" 2>/dev/null
    elif command -v convert >/dev/null 2>&1; then convert -background none -density 384 "$found" -resize 64x64 "png:$out" 2>/dev/null
    fi
    [ -s "$out" ] && found="$out";;
  esac
  echo "$found"
}
"#;

/// The files in a bundle [`batch_script`] wrote, as `(index, bytes)`; stops
/// at the first malformed frame.
pub fn parse_bundle(mut bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    while let Some(nl) = bytes.iter().position(|&b| b == b'\n') {
        let header = String::from_utf8_lossy(&bytes[..nl]);
        let mut parts = header.split_whitespace();
        let (Some(Ok(i)), Some(Ok(len))) = (
            parts.next().map(str::parse::<usize>),
            parts.next().map(str::parse::<usize>),
        ) else {
            break;
        };
        let body = &bytes[nl + 1..];
        if body.len() < len {
            break;
        }
        out.push((i, body[..len].to_vec()));
        bytes = &body[len..];
    }
    out
}

impl Space {
    /// The icon the guest desktop shows for a window's app (`app_name`,
    /// `app_id` and `pid` as the window list reports them), or `None` when
    /// the guest has none. See [`Space::app_icons`].
    pub async fn app_icon(
        &self,
        app_name: &str,
        app_id: &str,
        pid: u32,
    ) -> Result<Option<AppIcon>> {
        let request = IconRequest {
            app_name: app_name.into(),
            app_id: app_id.into(),
            pid,
        };
        Ok(self.app_icons(&[request]).await?.pop().flatten())
    }

    /// Icons for many windows' apps, in `requests` order: answered by the
    /// icon cache where it can, and every miss in one guest round trip
    /// (plus one download). Windows of one app share one lookup. A failed
    /// round trip is an error and is not remembered.
    pub async fn app_icons(&self, requests: &[IconRequest]) -> Result<Vec<Option<AppIcon>>> {
        let Some(p) = IconPlatform::of(self.os_family()) else {
            return Ok(vec![None; requests.len()]);
        };
        let scope = self.icon_scope(p);
        let keys: Vec<IconKey> = requests
            .iter()
            .map(|r| IconKey::new(&scope, &icon_identity(&r.app_name, &r.app_id), ""))
            .collect();
        let icons = IconCache::shared()
            .get_or_fetch_many(&keys, |indices| async move {
                let batch: Vec<IconRequest> =
                    indices.iter().map(|&i| requests[i].clone()).collect();
                self.fetch_icons(p, &batch).await
            })
            .await?;
        Ok(icons
            .iter()
            .map(|i| i.as_deref().map(AppIcon::from))
            .collect())
    }

    /// Where this Space's apps live, for the cache key: the image digest
    /// when the Space runs a known image, else the guest's OS version.
    fn icon_scope(&self, p: IconPlatform) -> String {
        let platform = match p {
            IconPlatform::Macos => "macos",
            IconPlatform::Linux => "linux",
        };
        let digest = self.image().map(|(_, d)| d).filter(|d| !d.is_empty());
        let os = self.capabilities().os.as_ref().map(|o| {
            if o.pretty_name.is_empty() {
                format!("{} {}", o.name, o.version)
            } else {
                o.pretty_name.clone()
            }
        });
        let at = digest
            .map(str::to_string)
            .or(os)
            .unwrap_or_else(|| self.id().to_string());
        format!("guest-{platform}:{at}")
    }

    /// One guest round trip for `batch`: the icon files it found, in order.
    async fn fetch_icons(
        &self,
        p: IconPlatform,
        batch: &[IconRequest],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        let mut found: Vec<Option<Vec<u8>>> = vec![None; batch.len()];
        let wanted: Vec<usize> = (0..batch.len())
            .filter(|&i| {
                let r = &batch[i];
                !(r.app_name.trim().is_empty() && r.app_id.trim().is_empty() && r.pid == 0)
            })
            .collect();
        if wanted.is_empty() {
            return Ok(found);
        }
        let asked: Vec<IconRequest> = wanted.iter().map(|&i| batch[i].clone()).collect();
        let bundle = format!("/tmp/cua-app-icons-{:016x}", rand_u64());
        let script = batch_script(p, &asked, &bundle);
        let timeout = ICON_SCRIPT_TIMEOUT + Duration::from_secs(2) * asked.len().min(30) as u32;
        let r = self.bash(&script, timeout).await?;
        let Some(path) = printed_path(&r.stdout) else {
            return Ok(found);
        };
        let bytes = self.spacesd()?.download(path).await?;
        let _ = self.spacesd()?.remove(path, false).await;
        for (i, file) in parse_bundle(&bytes) {
            if let Some(&slot) = wanted.get(i)
                && !file.is_empty()
                && file.len() <= MAX_ICON_BYTES
                && content_type(&file).is_some()
            {
                found[slot] = Some(file);
            }
        }
        Ok(found)
    }
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_of_an_app_share_one_identity() {
        assert_eq!(
            icon_identity("Firefox", "org.mozilla.firefox"),
            "org.mozilla.firefox"
        );
        assert_eq!(
            icon_identity("Firefox", " ORG.mozilla.Firefox "),
            "org.mozilla.firefox"
        );
        assert_eq!(icon_identity("XCalc", ""), "xcalc");
    }

    #[test]
    fn a_batch_is_one_script_and_one_bundle() {
        let reqs = [
            IconRequest {
                app_name: "Xfce4-terminal".into(),
                app_id: "xfce4-terminal".into(),
                pid: 7,
            },
            IconRequest {
                app_name: "Thunar".into(),
                app_id: "thunar".into(),
                pid: 0,
            },
        ];
        let s = batch_script(IconPlatform::Linux, &reqs, "/tmp/cua-app-icons-ab;rm");
        assert!(s.contains("b=\"/tmp/cua-app-icons-abrm\""), "{s}");
        // One index of the .desktop entries for the whole batch.
        assert_eq!(s.matches("idx=$(awk").count(), 1);
        assert!(
            s.contains("icon=$(match xfce4-terminal xfce4-terminal $c)"),
            "{s}"
        );
        assert!(s.contains("icon=$(match thunar thunar $c)"), "{s}");
        assert!(s.contains("emit 0 ") && s.contains("emit 1 "));
        assert!(s.contains("/proc/7/comm"));
        // macOS: a function per app (no `$(...)` around `case`: bash 3.2).
        let m = batch_script(IconPlatform::Macos, &reqs, "/tmp/cua-app-icons-ab");
        assert_eq!(m.matches("emit 0 \"$(lookup_0)\"").count(), 1, "{m}");
        let mut bundle = b"1 3\nabc0 2\nxy".to_vec();
        assert_eq!(
            parse_bundle(&bundle),
            vec![(1, b"abc".to_vec()), (0, b"xy".to_vec())]
        );
        bundle.extend_from_slice(b"2 99\nshort");
        assert_eq!(
            parse_bundle(&bundle).len(),
            2,
            "a truncated frame is dropped"
        );
    }

    /// Both platforms' batch scripts parse in the system shell (bash 3.2 on
    /// macOS, the oldest a guest runs). Syntax only: nothing is executed.
    #[cfg(unix)]
    #[test]
    fn batch_scripts_parse_in_the_system_bash() {
        let reqs = [
            IconRequest {
                app_name: "Calculator".into(),
                app_id: "com.apple.calculator".into(),
                pid: 42,
            },
            IconRequest {
                app_name: "Firefox".into(),
                app_id: "firefox".into(),
                pid: 0,
            },
        ];
        for p in [IconPlatform::Macos, IconPlatform::Linux] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("batch.sh");
            std::fs::write(&file, batch_script(p, &reqs, "/tmp/cua-app-icons-test")).unwrap();
            let out = std::process::Command::new("/bin/bash")
                .arg("-n")
                .arg(&file)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{p:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn names_cannot_break_out_of_the_script() {
        assert_eq!(shell_word("Foo'; rm -rf / #"), "Foorm-rf");
        assert_eq!(shell_word("org.xfce.Terminal+2_x"), "org.xfce.Terminal+2_x");
        let s = icon_script(
            IconPlatform::Linux,
            "a$(reboot)`x`",
            "b\"c",
            7,
            "/tmp/o.png",
        );
        assert!(!s.contains("$(reboot)") && !s.contains("`x`") && !s.contains("b\"c"));
        assert!(s.contains("for n in bc arebootx;"), "{s}");
        let out = icon_script(IconPlatform::Macos, "Calc", "", 1, "/tmp/x;rm -rf ~.png");
        assert!(
            out.contains("\"/tmp/xrm-rf.png\"") && !out.contains("~"),
            "{out}"
        );
    }

    #[test]
    fn macos_renders_the_bundle_of_the_pid() {
        let s = icon_script(
            IconPlatform::Macos,
            "Calculator",
            "com.apple.calculator",
            42,
            "/tmp/i.png",
        );
        assert!(s.contains("ps -o comm= -p 42"));
        assert!(s.contains("iconForFile"));
        assert!(
            s.contains("/System/Applications/Calculator.app") || s.contains("$d/Calculator.app")
        );
        assert!(s.contains("writeToFileAtomically(\"/tmp/i.png\""));
    }

    #[test]
    fn linux_prefers_png_sizes_before_scalable() {
        let s = icon_script(
            IconPlatform::Linux,
            "Xfce4-terminal",
            "xfce4-terminal",
            247,
            "/tmp/i.png",
        );
        assert!(s.contains("/proc/247/comm"));
        let png = s.find("for s in 64 48 128 96 256 32").unwrap();
        let pixmaps = s.find("/usr/share/pixmaps/$icon.png").unwrap();
        let svg = s.find("scalable/apps").unwrap();
        assert!(png < pixmaps && pixmaps < svg);
        assert!(s.contains("rsvg-convert -w 64 -h 64"));
    }

    #[test]
    fn only_png_and_svg_are_icons() {
        assert_eq!(content_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(
            content_type(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(content_type(b"/* XPM */\nstatic char *x[] = {"), None);
        assert_eq!(content_type(b""), None);
    }

    #[test]
    fn the_printed_path_is_the_last_absolute_line() {
        assert_eq!(
            printed_path("noise\n/usr/share/icons/a.png\n\n"),
            Some("/usr/share/icons/a.png")
        );
        assert_eq!(printed_path(""), None);
        assert_eq!(printed_path("relative.png"), None);
    }

    #[test]
    fn windows_and_unknown_guests_have_no_lookup() {
        assert_eq!(IconPlatform::of(pb::OsFamily::Windows), None);
        assert_eq!(IconPlatform::of(pb::OsFamily::Unspecified), None);
        assert_eq!(
            IconPlatform::of(pb::OsFamily::Linux),
            Some(IconPlatform::Linux)
        );
    }
}
