// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! WASAPI capture (Windows): loopback of the default render endpoint for the
//! desktop mix, and process loopback (Windows 10 2004+) for per-app audio.
//!
//! Written from the public Windows Core Audio documentation. Not exercised
//! on this project's CI hosts yet (no Windows runner with audio); the
//! gated test is `CUA_CODEC_TEST_WASAPI=1`.
//!
//! Both paths ask the shared-mode engine for 16-bit PCM at the requested
//! rate with `AUTOCONVERTPCM`, so no resampler is needed on our side.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{IUnknown, Interface, GUID, HRESULT, PROPVARIANT};
use windows::Win32::Media::Audio::{
    eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

use super::{
    AudioBackend, AudioCapture, AudioCaptureRequest, AudioCaptureState, AudioFormat, AudioFrame,
    AudioFrameSink, AudioSourceInfo, AudioSourceKind, AudioStream, DESKTOP_SOURCE_ID,
};
use crate::error::{CodecError, Result};

const WAVE_FORMAT_PCM: u16 = 1;
const VT_BLOB: u16 = 65;
const IID_IAGILE_OBJECT: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);

fn werr(what: &str, e: windows::core::Error) -> CodecError {
    CodecError::Audio(format!("{what}: {e}"))
}

fn wave_format(format: AudioFormat) -> WAVEFORMATEX {
    let block = format.channels * 2;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM,
        nChannels: format.channels,
        nSamplesPerSec: format.sample_rate,
        nAvgBytesPerSec: format.sample_rate * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

/// WASAPI capture backend.
#[derive(Debug, Default)]
pub struct WasapiCapture;

impl WasapiCapture {
    /// New backend.
    pub fn new() -> Self {
        Self
    }
}

struct WasapiStream {
    source: AudioSourceInfo,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioStream for WasapiStream {
    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }
    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Process loopback needs Windows 10 build 19041 (2004).
fn process_loopback_supported() -> bool {
    // Activation simply fails on older builds; we report the limitation
    // there. Assume support and let `start` fall back.
    true
}

impl AudioCapture for WasapiCapture {
    fn backend(&self) -> AudioBackend {
        AudioBackend::Wasapi
    }

    fn sources(&self) -> Result<Vec<AudioSourceInfo>> {
        // Per-app sources are addressed by pid ("app:<pid>"); enumerating
        // audio sessions is left to the window layer, which knows the pids
        // of the windows it streams.
        Ok(vec![AudioSourceInfo::desktop("Desktop audio")])
    }

    fn start(
        &self,
        request: &AudioCaptureRequest,
        sink: Arc<dyn AudioFrameSink>,
    ) -> Result<Box<dyn AudioStream>> {
        let per_app_pid: Option<u32> = if request.source_id == DESKTOP_SOURCE_ID {
            None
        } else {
            Some(
                request
                    .source_id
                    .strip_prefix("app:")
                    .and_then(|p| p.parse().ok())
                    .ok_or_else(|| {
                        CodecError::InvalidArgument(format!(
                            "unknown audio source {}",
                            request.source_id
                        ))
                    })?,
            )
        };
        let format = request.format;
        let buffer_ms = request.buffer_ms.max(5);
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let (ready_tx, ready_rx) =
            std::sync::mpsc::channel::<std::result::Result<Option<String>, String>>();
        let thread = std::thread::Builder::new()
            .name("cua-wasapi-capture".into())
            .spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let mut fallback = None;
                let client = match per_app_pid {
                    Some(pid) if process_loopback_supported() => {
                        match activate_process_loopback(pid) {
                            Ok(c) => Ok(c),
                            Err(e) => {
                                fallback = Some(e.to_string());
                                default_render_client()
                            }
                        }
                    }
                    _ => default_render_client(),
                };
                let result = client.and_then(|client| {
                    let wfx = wave_format(format);
                    let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                    client
                        .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &wfx, None)
                        .map_err(|e| werr("IAudioClient::Initialize", e))?;
                    let capture: IAudioCaptureClient = client
                        .GetService()
                        .map_err(|e| werr("GetService(IAudioCaptureClient)", e))?;
                    client.Start().map_err(|e| werr("IAudioClient::Start", e))?;
                    Ok((client, capture))
                });
                let (client, capture) = match result {
                    Ok(v) => {
                        let _ = ready_tx.send(Ok(fallback.clone()));
                        v
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if let Some(reason) = fallback {
                    sink.on_state(AudioCaptureState::Fallback(reason));
                }
                sink.on_state(AudioCaptureState::Active);
                let ch = usize::from(format.channels);
                while flag.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(u64::from(buffer_ms / 2)));
                    loop {
                        let next = match capture.GetNextPacketSize() {
                            Ok(n) => n,
                            Err(e) => {
                                sink.on_state(AudioCaptureState::Suspended(e.to_string()));
                                let _ = client.Stop();
                                return;
                            }
                        };
                        if next == 0 {
                            break;
                        }
                        let mut data = std::ptr::null_mut();
                        let mut frames = 0u32;
                        let mut flags = 0u32;
                        if capture
                            .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                            .is_err()
                        {
                            break;
                        }
                        let n = frames as usize * ch;
                        let samples = if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0
                            || data.is_null()
                        {
                            vec![0i16; n]
                        } else {
                            std::slice::from_raw_parts(data.cast::<i16>(), n).to_vec()
                        };
                        let _ = capture.ReleaseBuffer(frames);
                        sink.on_audio(AudioFrame {
                            pts_us: crate::types::media_clock_us()
                                .saturating_sub(format.duration_us(frames as usize)),
                            format,
                            samples,
                        });
                    }
                }
                let _ = client.Stop();
            })
            .map_err(|e| CodecError::Audio(format!("spawning capture thread: {e}")))?;
        let fallback = match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(fallback)) => fallback,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(CodecError::Audio(e));
            }
            Err(_) => {
                running.store(false, Ordering::Release);
                return Err(CodecError::Audio(
                    "WASAPI capture did not start within 10 s".into(),
                ));
            }
        };
        let source = match (per_app_pid, fallback) {
            (None, _) => AudioSourceInfo::desktop("Desktop audio"),
            (Some(pid), fb) => AudioSourceInfo {
                source_id: request.source_id.clone(),
                kind: AudioSourceKind::Application,
                name: format!("pid {pid}"),
                pid: Some(pid),
                app_id: None,
                available: true,
                desktop_fallback: fb.is_some(),
                limitation: fb.map(|e| {
                    format!("process loopback unavailable ({e}); capturing the desktop mix")
                }),
            },
        };
        Ok(Box::new(WasapiStream {
            source,
            running,
            thread: Some(thread),
        }))
    }
}

unsafe fn default_render_client() -> Result<IAudioClient> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| werr("MMDeviceEnumerator", e))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| werr("GetDefaultAudioEndpoint", e))?;
        device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| werr("IMMDevice::Activate", e))
    }
}

// ---- process loopback: a minimal agile completion handler (hand-rolled COM)

#[repr(C)]
struct HandlerVtbl {
    query_interface:
        unsafe extern "system" fn(*mut Handler, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Handler) -> u32,
    release: unsafe extern "system" fn(*mut Handler) -> u32,
    activate_completed: unsafe extern "system" fn(*mut Handler, *mut c_void) -> HRESULT,
}

#[repr(C)]
struct Handler {
    vtbl: *const HandlerVtbl,
    refs: AtomicU32,
    done: (Mutex<bool>, Condvar),
}

static HANDLER_VTBL: HandlerVtbl = HandlerVtbl {
    query_interface: handler_qi,
    add_ref: handler_add_ref,
    release: handler_release,
    activate_completed: handler_completed,
};

unsafe extern "system" fn handler_qi(
    this: *mut Handler,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let iid = *iid;
        if iid == IUnknown::IID
            || iid == IActivateAudioInterfaceCompletionHandler::IID
            || iid == IID_IAGILE_OBJECT
        {
            handler_add_ref(this);
            *out = this.cast();
            HRESULT(0)
        } else {
            *out = std::ptr::null_mut();
            HRESULT(0x8000_4002_u32 as i32) // E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn handler_add_ref(this: *mut Handler) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::AcqRel) + 1 }
}

unsafe extern "system" fn handler_release(this: *mut Handler) -> u32 {
    unsafe {
        let left = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(this));
        }
        left
    }
}

unsafe extern "system" fn handler_completed(this: *mut Handler, _op: *mut c_void) -> HRESULT {
    unsafe {
        let (lock, cv) = &(*this).done;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        cv.notify_all();
    }
    HRESULT(0)
}

#[repr(C)]
struct PropVariantBlob {
    vt: u16,
    reserved: [u16; 3],
    cb_size: u32,
    blob: *const u8,
}

unsafe fn activate_process_loopback(pid: u32) -> Result<IAudioClient> {
    unsafe {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let prop = PropVariantBlob {
            vt: VT_BLOB,
            reserved: [0; 3],
            cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            blob: (&params as *const AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
        };
        let raw = Box::into_raw(Box::new(Handler {
            vtbl: &HANDLER_VTBL,
            refs: AtomicU32::new(1),
            done: (Mutex::new(false), Condvar::new()),
        }));
        // Takes over our reference.
        let handler = IActivateAudioInterfaceCompletionHandler::from_raw(raw.cast());
        handler_add_ref(raw); // keep one for waiting below
        let op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some((&prop as *const PropVariantBlob).cast::<PROPVARIANT>()),
            &handler,
        )
        .map_err(|e| {
            handler_release(raw);
            werr("ActivateAudioInterfaceAsync", e)
        })?;
        let completed = {
            let (lock, cv) = &(*raw).done;
            let guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (guard, _) = cv
                .wait_timeout_while(guard, Duration::from_secs(5), |done| !*done)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
        };
        handler_release(raw);
        if !completed {
            return Err(CodecError::Audio(
                "process loopback activation timed out".into(),
            ));
        }
        let mut hr = HRESULT(0);
        let mut unknown: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unknown)
            .map_err(|e| werr("GetActivateResult", e))?;
        hr.ok()
            .map_err(|e| werr("process loopback activation", e))?;
        unknown
            .ok_or_else(|| CodecError::Audio("activation returned no interface".into()))?
            .cast::<IAudioClient>()
            .map_err(|e| werr("IAudioClient cast", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propvariant_blob_matches_the_native_layout() {
        assert_eq!(
            std::mem::size_of::<PropVariantBlob>(),
            std::mem::size_of::<PROPVARIANT>()
        );
    }

    #[test]
    fn wasapi_real_loopback() {
        if std::env::var_os("CUA_CODEC_TEST_WASAPI").is_none() {
            eprintln!(
                "skipped: set CUA_CODEC_TEST_WASAPI=1 on a Windows host with an audio endpoint"
            );
            return;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<usize>(16);
        let sink: Arc<dyn AudioFrameSink> = Arc::new(move |f: AudioFrame| {
            let _ = tx.try_send(f.samples.len());
        });
        let mut s = WasapiCapture::new()
            .start(&AudioCaptureRequest::desktop(), sink)
            .unwrap();
        let got = rx.recv_timeout(Duration::from_secs(3));
        s.stop();
        assert!(got.is_ok());
    }
}
