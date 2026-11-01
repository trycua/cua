//! Read the host task's numeric state, not schtasks' localized display text.

use super::{ServiceState, WINDOWS_TASK_NAME};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{ITaskService, TASK_STATE_RUNNING, TaskScheduler};
use windows::core::{BSTR, HRESULT, VARIANT};

pub(super) fn state() -> ServiceState {
    service_state(query())
}

struct QueryError {
    task_found: bool,
    detail: String,
}

impl QueryError {
    fn new(operation: &str, error: windows::core::Error, task_found: bool) -> Self {
        Self {
            task_found,
            detail: format!(
                "Task Scheduler {operation} failed (HRESULT 0x{:08X}): {error}",
                error.code().0 as u32
            ),
        }
    }
}

fn service_state(result: Result<Option<i32>, QueryError>) -> ServiceState {
    let (installed, running, detail) = match result {
        Ok(Some(state)) => (
            true,
            state == TASK_STATE_RUNNING.0,
            format!("{WINDOWS_TASK_NAME}; state={state}"),
        ),
        Ok(None) => (false, false, format!("{WINDOWS_TASK_NAME}; task not found")),
        Err(error) => (
            error.task_found,
            false,
            format!("{WINDOWS_TASK_NAME}; query unavailable: {}", error.detail),
        ),
    };
    ServiceState {
        installed,
        running,
        kind: "windows-task".into(),
        detail,
    }
}

struct Apartment(bool);

impl Apartment {
    fn initialize() -> windows::core::Result<Self> {
        // The SDK caller can already own an STA. Use it without balancing an
        // initialization we did not perform; S_OK and S_FALSE both need a drop.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            result.ok()?;
            Ok(Self(true))
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // All interfaces in query() have already been dropped on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn query() -> Result<Option<i32>, QueryError> {
    let _apartment = Apartment::initialize()
        .map_err(|error| QueryError::new("COM initialization", error, false))?;
    // Keep every interface within this call and apartment. This is a local,
    // read-only query; do not change the embedding process's COM security.
    unsafe {
        let scheduler: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| QueryError::new("activation", error, false))?;
        let empty = VARIANT::default();
        scheduler
            .Connect(&empty, &empty, &empty, &empty)
            .map_err(|error| QueryError::new("connection", error, false))?;
        let folder = scheduler
            .GetFolder(&BSTR::from("\\"))
            .map_err(|error| QueryError::new("root folder", error, false))?;
        let task = match folder.GetTask(&BSTR::from(WINDOWS_TASK_NAME)) {
            Ok(task) => task,
            Err(error) if task_missing(error.code()) => return Ok(None),
            Err(error) => return Err(QueryError::new("task lookup", error, false)),
        };
        task.State()
            .map(|state| Some(state.0))
            .map_err(|error| QueryError::new("state", error, true))
    }
}

fn task_missing(code: HRESULT) -> bool {
    // Only GetTask's file/path-not-found results establish absence. A COM
    // activation or connection failure cannot tell us whether the task exists.
    code == HRESULT::from_win32(2) || code == HRESULT::from_win32(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_task_state_does_not_depend_on_display_language() {
        // Enabled controls future launches, not whether an instance is running.
        // Do not add it as a second condition for TASK_STATE_RUNNING.
        for state in 0..=4 {
            let actual = service_state(Ok(Some(state)));
            assert!(actual.installed);
            assert_eq!(actual.running, state == 4, "state={state}");
            assert_eq!(actual.kind, "windows-task");
        }
    }

    #[test]
    fn com_initialization_is_balanced_on_a_fresh_thread() {
        std::thread::spawn(|| {
            use windows::Win32::Foundation::CO_E_NOTINITIALIZED;
            use windows::Win32::System::Com::CoGetApartmentType;

            let mut apartment = Default::default();
            let mut qualifier = Default::default();
            let mut current = || unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) };
            assert_eq!(current().unwrap_err().code(), CO_E_NOTINITIALIZED);
            {
                let _first = Apartment::initialize().unwrap();
                current().unwrap();
                let _nested = Apartment::initialize().unwrap();
                current().unwrap();
            }
            assert_eq!(current().unwrap_err().code(), CO_E_NOTINITIALIZED);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn an_existing_sta_is_not_uninitialized() {
        std::thread::spawn(|| {
            use windows::Win32::System::Com::COINIT_APARTMENTTHREADED;

            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .unwrap();
            let outer = Apartment(true);
            {
                let _existing = Apartment::initialize().unwrap();
            }
            assert_eq!(
                unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) },
                RPC_E_CHANGED_MODE
            );
            drop(outer);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn only_task_lookup_not_found_means_missing() {
        assert!(task_missing(HRESULT::from_win32(2)));
        assert!(task_missing(HRESULT::from_win32(3)));
        for code in [0x80070005_u32, 0x80041315, 0x80040154, 0x80010106] {
            assert!(!task_missing(HRESULT(code as i32)));
        }
        let missing = service_state(Ok(None));
        assert!(!missing.installed);
        assert!(!missing.running);
        assert!(missing.detail.ends_with("task not found"));
    }

    #[test]
    fn query_failures_preserve_the_operation_and_hresult() {
        for (operation, task_found) in [
            ("COM initialization", false),
            ("activation", false),
            ("connection", false),
            ("root folder", false),
            ("task lookup", false),
            ("state", true),
        ] {
            let error = windows::core::Error::from_hresult(HRESULT::from_win32(5));
            let actual = service_state(Err(QueryError::new(operation, error, task_found)));
            assert_eq!(actual.installed, task_found);
            assert!(!actual.running);
            assert!(actual.detail.contains("query unavailable"));
            assert!(actual.detail.contains(operation));
            assert!(actual.detail.contains("0x80070005"));
        }
    }
}
