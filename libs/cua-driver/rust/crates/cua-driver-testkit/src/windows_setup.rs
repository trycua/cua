//! Windows hosted-runner desktop cleanup performed before behavioral capture.

#[cfg(any(target_os = "windows", test))]
fn is_hosted_runner_console(title: &str, class_name: &str) -> bool {
    let title = title.to_ascii_lowercase();
    let console_class = matches!(
        class_name,
        "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS"
    );
    console_class
        && [
            "hostedcomputeagent",
            "hosted-compute-agent",
            "runner.worker",
            "github actions runner",
        ]
        .iter()
        .any(|marker| title.contains(marker))
}

#[cfg(target_os = "windows")]
pub fn minimize_hosted_runner_console() -> Result<&'static str, String> {
    if std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted") {
        return Ok("not_applicable");
    }

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, IsIconic,
        IsWindowVisible, ShowWindow, SW_MINIMIZE,
    };

    unsafe fn identity(hwnd: HWND) -> (String, String) {
        let title_len = GetWindowTextLengthW(hwnd);
        let mut title = vec![0u16; title_len.max(0) as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut title);
        let title = String::from_utf16_lossy(&title[..copied.max(0) as usize]);
        let mut class_name = [0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class_name);
        let class_name = String::from_utf16_lossy(&class_name[..class_len.max(0) as usize]);
        (title, class_name)
    }

    unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd).as_bool() {
            let (title, class_name) = identity(hwnd);
            if is_hosted_runner_console(&title, &class_name) {
                let windows = &mut *(state.0 as *mut Vec<HWND>);
                windows.push(hwnd);
            }
        }
        TRUE
    }

    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        );
    }
    windows.sort_by_key(|hwnd| hwnd.0 as usize);
    windows.dedup_by_key(|hwnd| hwnd.0 as usize);
    if windows.is_empty() {
        return Ok("not_present");
    }

    let mut changed = false;
    for &hwnd in &windows {
        if !unsafe { IsIconic(hwnd) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            changed = true;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    if windows
        .iter()
        .any(|&hwnd| !unsafe { IsIconic(hwnd) }.as_bool())
    {
        return Err("HostedComputeAgent console did not enter the minimized state".to_owned());
    }
    Ok(if changed {
        "minimized"
    } else {
        "already_minimized"
    })
}

#[cfg(not(target_os = "windows"))]
pub fn minimize_hosted_runner_console() -> Result<&'static str, String> {
    Ok("not_applicable")
}

#[cfg(test)]
mod tests {
    use super::is_hosted_runner_console;

    #[test]
    fn matches_only_named_runner_console_windows() {
        assert!(is_hosted_runner_console(
            "Administrator: HostedComputeAgent.exe",
            "ConsoleWindowClass"
        ));
        assert!(is_hosted_runner_console(
            "GitHub Actions Runner",
            "CASCADIA_HOSTING_WINDOW_CLASS"
        ));
        assert!(is_hosted_runner_console(
            r"C:\ProgramData\GitHub\HostedComputeAgent\hosted-compute-agent",
            "ConsoleWindowClass"
        ));
        assert!(!is_hosted_runner_console(
            "CuaTestHarness Sentinel",
            "Chrome_WidgetWin_1"
        ));
        assert!(!is_hosted_runner_console(
            "HostedComputeAgent status",
            "Chrome_WidgetWin_1"
        ));
    }
}
