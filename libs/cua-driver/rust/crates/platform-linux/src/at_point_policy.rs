//! Pure routing facts for Linux background pixel clicks.
//!
//! Kept free of X11, AT-SPI, and `/proc` I/O so the decisions are unit-tested
//! on every host, not only on Linux.

/// AT-SPI roles of an application's top-level shell: the application node,
/// its frames and windows. A pixel hit test that resolves only to one of
/// these did not find the control under the point. Chromium on Linux is the
/// common case: until its accessibility tree is populated, the page content
/// is a bare `frame` whose `doDefault` does nothing. Firing that action and
/// reporting a click is a false success, so the shell is never an at-point
/// click target; the caller falls through to its pointer route or refuses.
pub(crate) fn is_top_level_shell_role(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "application" | "frame" | "window" | "desktop frame"
    )
}

/// Whether one `/proc/<pid>/cmdline` belongs to a Chromium helper process
/// (renderer, zygote, or GPU process).
///
/// The kernel separates arguments with NUL, but Chromium rewrites its own
/// process title with `setproctitle`, which joins the whole command line
/// with spaces into a single NUL-terminated string. Matching only
/// NUL-separated arguments misses every Google Chrome and Chromium helper,
/// so both separators are accepted.
pub(crate) fn cmdline_is_chromium_helper(raw: &[u8]) -> bool {
    String::from_utf8_lossy(raw)
        .split(|ch: char| ch == '\0' || ch.is_whitespace())
        .any(|arg| {
            matches!(
                arg,
                "--type=renderer" | "--type=zygote" | "--type=gpu-process"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{cmdline_is_chromium_helper, is_top_level_shell_role};

    #[test]
    fn application_frames_and_windows_are_never_click_targets() {
        for role in ["frame", "window", "application", "desktop frame", " Frame "] {
            assert!(is_top_level_shell_role(role), "{role:?}");
        }
    }

    #[test]
    fn controls_and_documents_remain_click_targets() {
        for role in [
            "push button",
            "button",
            "link",
            "check box",
            "menu item",
            "document web",
            "internal frame",
            "dialog",
            "panel",
        ] {
            assert!(!is_top_level_shell_role(role), "{role:?}");
        }
    }

    #[test]
    fn nul_separated_chromium_helper_is_detected() {
        assert!(cmdline_is_chromium_helper(
            b"/usr/lib/chromium/chromium\0--type=renderer\0--lang=en-US\0"
        ));
        assert!(cmdline_is_chromium_helper(
            b"/opt/app/app\0--type=zygote\0--no-zygote-sandbox\0"
        ));
    }

    #[test]
    fn setproctitle_rewritten_chrome_helper_is_detected() {
        // Google Chrome 151 renderer as `/proc/<pid>/cmdline` shows it after
        // Chromium's setproctitle: one space-joined string.
        assert!(cmdline_is_chromium_helper(
            b"/opt/google/chrome/chrome --type=renderer --crashpad-handler-pid=7960 \
              --enable-crash-reporter --lang=en-US --num-raster-threads=4\0"
        ));
        assert!(cmdline_is_chromium_helper(
            b"/opt/google/chrome/chrome --type=gpu-process --ozone-platform=x11\0\0\0"
        ));
    }

    #[test]
    fn unrelated_processes_are_not_chromium_helpers() {
        assert!(!cmdline_is_chromium_helper(
            b"/usr/bin/gedit\0--new-window\0"
        ));
        assert!(!cmdline_is_chromium_helper(
            b"/usr/bin/python3 tool.py --type=renderer-like\0"
        ));
        assert!(!cmdline_is_chromium_helper(
            b"/usr/lib/webkit2gtk-4.1/WebKitWebProcess 7 12\0"
        ));
        assert!(!cmdline_is_chromium_helper(b""));
    }
}
