//! Foreground `type_text` through packaged Notepad, saved and compared (#4830).
//!
//! Manual. Arm A's failure rate was not measured, so this uses the floor of 5 trials.

#![cfg(target_os = "windows")]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_testkit::{spawn_in_job, ChildReaper, Driver, McpDriver};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, IsWindowVisible, SendMessageW,
};

#[path = "support/native_windows.rs"]
mod native_windows;
use native_windows::native_windows;

const TRIALS: usize = 5;
const T1: &str = "CH-RAW-20261006114218790-010444e4 \u{8f66}\u{6b21}1\u{ff1a}G4915 00:34 \u{4e0a}\u{6d77}\u{8679}\u{6865} \u{2192} 01:19 \u{676d}\u{5dde}\u{4e1c} \u{5386}\u{65f6}0\u{65f6}45\u{5206} \u{4e8c}\u{7b49}\u{5ea7}\u{6709}\u{7968}\u{ff08}\u{9875}\u{9762}\u{672a}\u{663e}\u{793a}\u{4ef7}\u{683c}\u{660e}\u{7ec6}\u{ff09}";
const WINUI3: &str = concat!(
    "\nCUA-MULTILINE-START\nASCII-LF-A-31\nASCII-LF-B-47\n",
    "ASCII-CRLF-A-52\r\nASCII-CRLF-B-68\nASCII-CR-A-13\rASCII-CR-B-24\n",
    "\u{8f66}\u{6b21} G7391\r\n\u{4e2d}\u{6587} 12345\nEMPTY-A-17\n\n\nEMPTY-B-29\n",
    "EMOJI-A \u{1f642} 123\r\nEMOJI-B \u{1f680} 456\nCUA-MULTILINE-END\n",
);

fn corpus_t2() -> String {
    let piece = format!("{T1}RAW-CUA-FT-CRLF-B-47数字 12345{WINUI3}");
    let mut out = String::new();
    while out.chars().count() < 390 {
        out.push_str(&piece);
    }
    out.chars().take(390).collect()
}

#[test]
#[ignore]
fn foreground_type_text_round_trips_through_notepad_save() {
    let t1: Vec<char> = T1.chars().collect();
    assert_eq!((t1.len(), &t1[75..78]), (91, &['二', '等', '座'][..]));
    let t2 = corpus_t2();
    assert_eq!(t2.chars().count(), 390);
    assert!(t2.contains("\r\n") && t2.contains('\n') && t2.contains("ASCII-CR-A-13\r"));
    assert!(t2.contains("二等座") && t2.contains('🙂') && t2.contains('🚀'));

    let mut driver = McpDriver::spawn_named("notepad-foreground-type-text")
        .expect("build cua-driver or set CUA_TEST_DRIVER_BIN");
    for trial in 0..TRIALS {
        round_trip(&mut driver, trial, "t1", T1);
        round_trip(&mut driver, trial, "t2", &t2);
    }
}

fn round_trip(driver: &mut McpDriver, trial: usize, label: &str, text: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("cua4830-{label}-{trial}.txt"));
    std::fs::write(&path, "").expect("empty file");
    let mut command = Command::new("notepad.exe");
    command
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    driver
        .reaper()
        .push(spawn_in_job(&mut command).expect("notepad"));
    let stem = path.file_name().unwrap().to_string_lossy().into_owned();
    let (pid, hwnd) = wait_for_title(&stem)
        .unwrap_or_else(|| panic!("no Notepad window for {stem}\n{}", note(0)));
    let mut kill = ChildReaper::new();
    kill.track_pid(pid);
    let typed = driver.call(
        "type_text",
        serde_json::json!({
            "pid": pid as i64, "window_id": hwnd, "text": text, "delivery_mode": "foreground"
        }),
    );
    assert!(
        !typed.is_error(),
        "{label} type_text: {}\n{}",
        typed.text(),
        note(hwnd)
    );
    // Notepad's Save accelerator is under a closed File menu, so hotkey's UIA walk misses it.
    let saved = driver.call(
        "press_key",
        serde_json::json!({
            "pid": pid as i64, "window_id": hwnd, "key": "s",
            "modifiers": ["ctrl"], "delivery_mode": "foreground"
        }),
    );
    assert!(
        !saved.is_error(),
        "{label} Ctrl+S: {}\n{}",
        saved.text(),
        note(hwnd)
    );
    assert_eq!(
        read_saved(&path, hwnd),
        normalize(text),
        "{label} trial {trial}\n{}",
        note(hwnd)
    );
}

fn read_saved(path: &Path, hwnd: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = 0u64;
    let mut stable: Option<Instant> = None;
    loop {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len > 0
            && len == last
            && stable.get_or_insert_with(Instant::now).elapsed() >= Duration::from_millis(500)
        {
            break;
        }
        if len != last {
            last = len;
            stable = None;
        }
        assert!(Instant::now() < deadline, "file not stable\n{}", note(hwnd));
        std::thread::sleep(Duration::from_millis(50));
    }
    let bytes = std::fs::read(path).expect("read save");
    normalize(
        &String::from_utf8(bytes).unwrap_or_else(|e| panic!("not utf-8: {e}\n{}", note(hwnd))),
    )
}

fn normalize(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn wait_for_title(prefix: &str) -> Option<(u32, u64)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some((pid, hwnd, _)) = native_windows().into_iter().find(|(_, hwnd, title)| {
            title.starts_with(prefix) && unsafe { IsWindowVisible(HWND(*hwnd as *mut _)) }.as_bool()
        }) {
            return Some((pid, hwnd));
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn note(hwnd: u64) -> String {
    let versions = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "$c=Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion'; $n=(Get-AppxPackage Microsoft.WindowsNotepad).Version; \"build=$($c.CurrentBuild).$($c.UBR) notepad=$n\""])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|e| e.to_string());
    if hwnd == 0 {
        return format!("{versions} hkl=unknown ime_open=unknown");
    }
    unsafe {
        let window = HWND(hwnd as *mut _);
        let hkl = format!(
            "{:?}",
            GetKeyboardLayout(GetWindowThreadProcessId(window, None))
        );
        let ime = ImmGetDefaultIMEWnd(window);
        let open = if ime.0.is_null() {
            "none".into()
        } else {
            format!(
                "{:?}",
                SendMessageW(ime, 0x0283, Some(WPARAM(5)), Some(LPARAM(0)))
            )
        };
        format!("{versions} hkl={hkl} ime_open={open}")
    }
}
