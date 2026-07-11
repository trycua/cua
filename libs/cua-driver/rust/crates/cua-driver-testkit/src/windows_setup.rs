//! Windows hosted-runner desktop cleanup performed before behavioral capture.

#[cfg(any(target_os = "windows", test))]
fn is_hosted_runner_console(title: &str, class_name: &str) -> bool {
    let title = title.to_ascii_lowercase();
    let console_class = matches!(
        class_name,
        "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS"
    );
    console_class
        && ["hostedcomputeagent", "runner.worker", "github actions runner"]
            .iter()
            .any(|marker| title.contains(marker))
}

#[cfg(target_os = "windows")]
pub fn minimize_hosted_runner_console() -> Result<&'static str, String> {
    if std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted") {
        return Ok("not_applicable");
    }

    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowTextLengthW, GetWindowTextW, IsIconic, ShowWindow, SW_MINIMIZE,
    };

    let hwnd = unsafe { GetConsoleWindow() };
    if hwnd.0.is_null() {
        return Err(
            "hosted runner test process has no inherited top-level console window".to_owned(),
        );
    }
    let title_len = unsafe { GetWindowTextLengthW(hwnd) };
    let mut title = vec![0u16; title_len.max(0) as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut title) };
    let title = String::from_utf16_lossy(&title[..copied.max(0) as usize]);
    let mut class_name = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class_name) };
    let class_name = String::from_utf16_lossy(&class_name[..class_len.max(0) as usize]);
    if !is_hosted_runner_console(&title, &class_name) {
        return Err(
            "inherited console window is not the HostedComputeAgent/runner console".to_owned(),
        );
    }

    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Ok("already_minimized");
    }
    let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
    std::thread::sleep(std::time::Duration::from_millis(100));
    if !unsafe { IsIconic(hwnd) }.as_bool() {
        return Err("HostedComputeAgent console did not enter the minimized state".to_owned());
    }
    Ok("minimized")
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
