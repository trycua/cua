// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::backtrace::Backtrace;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str =
    "info,cua_spacesd_client::input=debug,cua_spacesd_client::host_input=debug,cua_spacesd_client::geometry=debug,cua_spacesd::quic=debug";
const LOG_RETENTION_DAYS: usize = 14;
const LOG_RETENTION: Duration = Duration::from_secs(LOG_RETENTION_DAYS as u64 * 24 * 60 * 60);
/// A panic log past this size keeps only its last half.
const MAX_PANIC_LOG_BYTES: u64 = 10 * 1024 * 1024;
const MAX_COMPONENT_LOG_BYTES: u64 = 100 * 1024 * 1024;

pub struct DiagnosticsGuard {
    _writer: tracing_appender::non_blocking::WorkerGuard,
    path: PathBuf,
}

impl DiagnosticsGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DiagnosticsGuard {
    fn drop(&mut self) {
        tracing::info!(component = "process", "RCDP process exiting");
    }
}

pub fn init(component: &str) -> Result<DiagnosticsGuard, Box<dyn Error + Send + Sync>> {
    let directory = log_directory();
    fs::create_dir_all(&directory)?;
    archive_legacy_log(&directory, component)?;
    prune_component_logs(&directory, component, SystemTime::now())?;
    let path = directory.join(format!("{component}.panic.log"));
    cap_panic_log(&path)?;
    // Daily files, and the appender itself drops the oldest past the
    // retention window, so a long-running process stays bounded between
    // restarts too (startup pruning also caps the total size).
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(format!("{component}.log"))
        .max_log_files(LOG_RETENTION_DAYS)
        .build(&directory)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter =
        EnvFilter::try_from_env("CUA_ENV_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(writer)
        .try_init()?;
    install_panic_hook(path.clone());
    tracing::info!(
        component,
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        log_path = %path.display(),
        "RCDP process started"
    );
    Ok(DiagnosticsGuard {
        _writer: guard,
        path,
    })
}

/// Keeps the panic log bounded: past the cap only its last half stays.
fn cap_panic_log(path: &Path) -> std::io::Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let len = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if len <= MAX_PANIC_LOG_BYTES {
        return Ok(());
    }
    let keep = MAX_PANIC_LOG_BYTES / 2;
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(len - keep))?;
    let mut tail = Vec::with_capacity(keep as usize);
    f.take(keep).read_to_end(&mut tail)?;
    let tmp = path.with_extension("log.tmp");
    fs::write(&tmp, &tail)?;
    fs::rename(tmp, path)
}

fn archive_legacy_log(directory: &Path, component: &str) -> std::io::Result<()> {
    let legacy = directory.join(format!("{component}.log"));
    if !legacy.is_file() {
        return Ok(());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    fs::rename(
        legacy,
        directory.join(format!("{component}.log.legacy-{timestamp}")),
    )
}

fn prune_component_logs(directory: &Path, component: &str, now: SystemTime) -> std::io::Result<()> {
    let prefix = format!("{component}.log.");
    let mut files = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((entry.path(), metadata.modified().ok()?, metadata.len()))
        })
        .collect::<Vec<_>>();
    for (path, modified, _) in &files {
        if now
            .duration_since(*modified)
            .is_ok_and(|age| age > LOG_RETENTION)
        {
            let _ = fs::remove_file(path);
        }
    }
    files.retain(|(path, _, _)| path.exists());
    files.sort_by_key(|(_, modified, _)| *modified);
    let mut total = files.iter().map(|(_, _, bytes)| *bytes).sum::<u64>();
    for (path, _, bytes) in files {
        if total <= MAX_COMPONENT_LOG_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
    }
    Ok(())
}

fn log_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("CUA_ENV_LOG_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home).join("Library/Logs/cua-spacesd");
    }
    #[cfg(target_os = "windows")]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|path| !path.is_empty()) {
        return PathBuf::from(local_app_data).join("cua-spacesd/Logs");
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME").filter(|path| !path.is_empty()) {
        return PathBuf::from(state).join("cua-spacesd/logs");
    }
    std::env::temp_dir().join("cua-spacesd/logs")
}

fn install_panic_hook(path: PathBuf) {
    std::panic::set_hook(Box::new(move |panic| {
        let payload = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        let location = panic
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown location".into());
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let backtrace = Backtrace::force_capture();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(
                file,
                "{timestamp_ms} ERROR panic pid={} location={location} payload={payload:?}\n{backtrace}",
                std::process::id()
            );
            let _ = file.flush();
        }
        tracing::error!(%location, %payload, %backtrace, "RCDP process panicked");
    }));
}

#[cfg(test)]
mod tests {
    use super::{log_directory, prune_component_logs};

    #[test]
    fn explicit_log_directory_wins() {
        let temporary = std::env::temp_dir().join(format!("rcdp-log-test-{}", std::process::id()));
        std::env::set_var("CUA_ENV_LOG_DIR", &temporary);
        assert_eq!(log_directory(), temporary);
        std::env::remove_var("CUA_ENV_LOG_DIR");
    }

    #[test]
    fn panic_log_is_capped() {
        let temporary =
            std::env::temp_dir().join(format!("rcdp-panic-cap-test-{}", std::process::id()));
        std::fs::create_dir_all(&temporary).unwrap();
        let p = temporary.join("host.panic.log");
        std::fs::write(&p, vec![b'p'; (super::MAX_PANIC_LOG_BYTES + 1) as usize]).unwrap();
        super::cap_panic_log(&p).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            super::MAX_PANIC_LOG_BYTES / 2
        );
        let _ = std::fs::remove_dir_all(&temporary);
    }

    #[test]
    fn component_log_pruning_ignores_unrelated_files() {
        let temporary =
            std::env::temp_dir().join(format!("rcdp-log-prune-test-{}", std::process::id()));
        std::fs::create_dir_all(&temporary).unwrap();
        let unrelated = temporary.join("keep.txt");
        std::fs::write(&unrelated, b"keep").unwrap();
        prune_component_logs(&temporary, "client", std::time::SystemTime::now()).unwrap();
        assert!(unrelated.exists());
    }
}
