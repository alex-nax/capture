//! capture-platform — Windows audio capture (#66 slice C): WASAPI **per-process loopback** (an app's
//! audio, folding in `helper/audiocap_win_rs`), **microphone** capture, and input-device enumeration.
//!
//! - App audio: `ActivateAudioInterfaceAsync` + `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`
//!   (`INCLUDE_TARGET_PROCESS_TREE`, Windows 10 2004+) — captures one app's audio tree, requesting our
//!   pipeline's 16 kHz mono s16le directly (the loopback path format-converts), so `source_rate = 16000`.
//! - Mic: a normal shared-mode capture client on the chosen (or default) input endpoint, captured at the
//!   device mix format and converted to mono `i16`, delivered with `source_rate = mix rate` (the session
//!   loop resamples to 16 kHz).
//!
//! Each stream runs on its own thread (Windows allows concurrent WASAPI clients, unlike macOS's single
//! SCStream); `WinAudioCapture` signals + joins them on stop/drop.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::core::{Interface, Result as WinResult, PCWSTR};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, ActivateAudioInterfaceAsync, IAudioCaptureClient, IAudioClient,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, DEVICE_STATE_ACTIVE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows_core::implement;

use crate::{AudioInputInfo, AudioTarget};

// Hand-defined WAVEFORMATEX tags + flag (windows-rs types these awkwardly; matches audiocap_win_rs).
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// AUDCLNT_BUFFERFLAGS_SILENT — the engine filled the packet with silence.
const SILENT_FLAG: u32 = 0x2;

/// A running set of WASAPI capture threads (one for app audio, one for mic — either may be absent).
pub struct WinAudioCapture {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl WinAudioCapture {
    pub fn stop(mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        Ok(())
    }
}

impl Drop for WinAudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Start a single audio capture (app loopback or mic) delivering mono `i16` batches to `on_samples`.
pub fn start_audio_capture(
    target: &AudioTarget,
    on_samples: impl Fn(&[i16], u32) + Send + Sync + 'static,
) -> Result<WinAudioCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let cb: Box<dyn Fn(&[i16], u32) + Send + Sync> = Box::new(on_samples);
    let thread = spawn_stream(target.clone_for_thread(), stop.clone(), cb)?;
    Ok(WinAudioCapture { stop, threads: vec![thread] })
}

/// Start app loopback AND mic in one capture, each to its own callback (concurrent WASAPI clients).
pub fn start_audio_capture_dual(
    app: Option<&AudioTarget>,
    mic_device: Option<&str>,
    on_audio: Box<dyn Fn(&[i16], u32) + Send + Sync>,
    on_mic: Box<dyn Fn(&[i16], u32) + Send + Sync>,
) -> Result<WinAudioCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    if let Some(app) = app {
        threads.push(spawn_stream(app.clone_for_thread(), stop.clone(), on_audio)?);
    }
    threads.push(spawn_stream(
        ThreadTarget::Mic { device_id: mic_device.map(|s| s.to_string()) },
        stop.clone(),
        on_mic,
    )?);
    Ok(WinAudioCapture { stop, threads })
}

/// A `Send` snapshot of an [`AudioTarget`] for moving into a capture thread.
enum ThreadTarget {
    AppLoopback { pid: u32 },
    Mic { device_id: Option<String> },
}

impl AudioTarget {
    fn clone_for_thread(&self) -> ThreadTarget {
        match self {
            AudioTarget::App { pid, .. } => ThreadTarget::AppLoopback { pid: pid.unwrap_or(0) as u32 },
            AudioTarget::Mic { device_id } => ThreadTarget::Mic { device_id: device_id.clone() },
        }
    }
}

fn spawn_stream(
    target: ThreadTarget,
    stop: Arc<AtomicBool>,
    cb: Box<dyn Fn(&[i16], u32) + Send + Sync>,
) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("wasapi-capture".into())
        .spawn(move || {
            let r = match target {
                ThreadTarget::AppLoopback { pid } => run_app_loopback(pid, &stop, cb.as_ref()),
                ThreadTarget::Mic { device_id } => run_mic(device_id.as_deref(), &stop, cb.as_ref()),
            };
            if let Err(e) = r {
                eprintln!("capture-platform: wasapi capture thread ended: {e:?}");
            }
        })
        .map_err(|e| format!("spawn capture thread: {e}"))
}

/// VT_BLOB PROPVARIANT pointing at `AUDIOCLIENT_ACTIVATION_PARAMS` (windows-rs exposes no BLOB ctor).
#[repr(C)]
struct BlobPropVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    cb_size: u32,
    _pad: u32,
    p_blob: *mut u8,
}
const VT_BLOB: u16 = 65;

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler {
    event: HANDLE,
}
impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler_Impl {
    fn ActivateCompleted(
        &self,
        _op: windows_core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> WinResult<()> {
        unsafe {
            let _ = SetEvent(self.event);
        }
        Ok(())
    }
}

/// Per-app loopback (folds in `helper/audiocap_win_rs`): request 16 kHz mono s16le directly — the
/// process-loopback virtual device format-converts, so samples arrive ready and `source_rate = 16000`.
fn run_app_loopback(pid: u32, stop: &AtomicBool, cb: &(dyn Fn(&[i16], u32) + Send + Sync)) -> WinResult<()> {
    if pid == 0 {
        eprintln!("capture-platform: app loopback needs a target pid");
        return Ok(());
    }
    let rate: u32 = 16_000;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let activate_event = CreateEventW(None, false, false, PCWSTR::null())?;

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let pv = BlobPropVariant {
            vt: VT_BLOB,
            r1: 0,
            r2: 0,
            r3: 0,
            cb_size: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            _pad: 0,
            p_blob: &mut params as *mut _ as *mut u8,
        };
        let handler: IActivateAudioInterfaceCompletionHandler =
            CompletionHandler { event: activate_event }.into();
        let op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&pv as *const _ as *const PROPVARIANT),
            &handler,
        )?;
        WaitForSingleObject(activate_event, 5_000);

        let mut hr = windows::core::HRESULT(0);
        let mut unknown: Option<windows::core::IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unknown)?;
        hr.ok()?;
        let client: IAudioClient = unknown.ok_or_else(windows::core::Error::from_win32)?.cast()?;

        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: 1,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            &fmt,
            None,
        )?;
        capture_loop(&client, stop, 1, 16, false, rate, cb)
    }
}

/// Microphone capture: shared-mode client at the device mix format, converted to mono `i16` and
/// delivered with `source_rate = mix rate` (the session loop resamples to 16 kHz).
fn run_mic(device_id: Option<&str>, stop: &AtomicBool, cb: &(dyn Fn(&[i16], u32) + Send + Sync)) -> WinResult<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = match device_id.map(str::trim).filter(|s| !s.is_empty() && *s != "default") {
            Some(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                enumerator.GetDevice(PCWSTR(wide.as_ptr()))?
            }
            None => enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?,
        };
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let mix = client.GetMixFormat()?;
        let (channels, rate, bits, is_float) = parse_format(mix);
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            mix,
            None,
        )?;
        let r = capture_loop(&client, stop, channels, bits, is_float, rate, cb);
        windows::Win32::System::Com::CoTaskMemFree(Some(mix as *const _ as *const core::ffi::c_void));
        r
    }
}

/// Shared WASAPI capture pump: wait on the buffer event, drain packets, convert each to mono `i16`, and
/// hand them to `cb` until `stop`. `bits`/`is_float`/`channels` describe the source frames.
unsafe fn capture_loop(
    client: &IAudioClient,
    stop: &AtomicBool,
    channels: u16,
    bits: u16,
    is_float: bool,
    rate: u32,
    cb: &(dyn Fn(&[i16], u32) + Send + Sync),
) -> WinResult<()> {
    let buffer_event = CreateEventW(None, false, false, PCWSTR::null())?;
    client.SetEventHandle(buffer_event)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let bytes_per_sample = (bits / 8) as usize;
    let frame_bytes = bytes_per_sample * channels as usize;

    while !stop.load(Ordering::SeqCst) {
        // Wake every 200 ms so a stop is noticed promptly even with no audio flowing.
        if WaitForSingleObject(buffer_event, 200) != WAIT_OBJECT_0 {
            continue;
        }
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let packet = capture.GetNextPacketSize()?;
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let n = frames as usize;
            let mono: Vec<i16> = if flags & SILENT_FLAG != 0 || data.is_null() {
                vec![0i16; n] // keep the timeline advancing through silence
            } else {
                let raw = std::slice::from_raw_parts(data, n * frame_bytes);
                frames_to_mono_i16(raw, channels, bits, is_float)
            };
            if !mono.is_empty() {
                cb(&mono, rate);
            }
            capture.ReleaseBuffer(frames)?;
        }
    }
    let _ = client.Stop();
    Ok(())
}

/// `(channels, sample_rate, bits, is_float)` from a `WAVEFORMATEX` (incl. EXTENSIBLE — float if 32-bit).
/// `WAVEFORMATEX` is packed, so each field is read via `addr_of!` + `read_unaligned` (no field reference).
unsafe fn parse_format(fmt: *const WAVEFORMATEX) -> (u16, u32, u16, bool) {
    use std::ptr::{addr_of, read_unaligned};
    let tag = read_unaligned(addr_of!((*fmt).wFormatTag));
    let channels = read_unaligned(addr_of!((*fmt).nChannels));
    let rate = read_unaligned(addr_of!((*fmt).nSamplesPerSec));
    let bits = read_unaligned(addr_of!((*fmt).wBitsPerSample));
    let is_float = match tag {
        x if x == WAVE_FORMAT_IEEE_FLOAT => true,
        x if x == WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_EXTENSIBLE => bits == 32, // mix formats are 32-bit float in practice
        _ => bits == 32,
    };
    (channels, rate, bits, is_float)
}

/// Convert interleaved source frames to mono `i16` by averaging channels. Handles 32-bit float and
/// 16-bit PCM (the WASAPI shared-mode formats we request/encounter). Pure — unit-tested.
fn frames_to_mono_i16(raw: &[u8], channels: u16, bits: u16, is_float: bool) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    let bps = (bits / 8) as usize;
    if bps == 0 {
        return Vec::new();
    }
    let frame = bps * ch;
    let mut out = Vec::with_capacity(raw.len() / frame.max(1));
    for f in raw.chunks_exact(frame) {
        let mut acc = 0f32;
        for c in 0..ch {
            let s = &f[c * bps..c * bps + bps];
            acc += match (is_float, bps) {
                (true, 4) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]).clamp(-1.0, 1.0) * 32767.0,
                (false, 2) => i16::from_le_bytes([s[0], s[1]]) as f32,
                (false, 4) => (i32::from_le_bytes([s[0], s[1], s[2], s[3]]) >> 16) as f32, // 32-bit PCM
                _ => 0.0,
            };
        }
        out.push((acc / ch as f32).round().clamp(-32768.0, 32767.0) as i16);
    }
    out
}

/// Available input devices (mics), the default flagged. Empty on error. Mirrors `audio_input_devices`.
pub fn audio_input_devices() -> Vec<AudioInputInfo> {
    match enumerate_inputs() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("capture-platform: audio_input_devices: {e:?}");
            Vec::new()
        }
    }
}

fn enumerate_inputs() -> WinResult<Vec<AudioInputInfo>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()
            .and_then(|d| d.GetId().ok())
            .map(|p| pwstr_to_string(p.as_ptr()))
            .unwrap_or_default();

        let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        let count = collection.GetCount()?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let device = collection.Item(i)?;
            let id = device.GetId().map(|p| pwstr_to_string(p.as_ptr())).unwrap_or_default();
            let name = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|pv| propvariant_to_string(&pv))
                .unwrap_or_else(|_| id.clone());
            out.push(AudioInputInfo { default: !id.is_empty() && id == default_id, id, name });
        }
        Ok(out)
    }
}

unsafe fn pwstr_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

fn propvariant_to_string(pv: &PROPVARIANT) -> String {
    // PROPVARIANT for a string property holds a wide-string pointer; windows-rs has no safe getter, so
    // read it via the documented layout (vt at offset 0, the union pointer at offset 8 on x64).
    unsafe {
        let bytes = pv as *const PROPVARIANT as *const u8;
        let p = *(bytes.add(8) as *const *const u16);
        pwstr_to_string(p)
    }
}

#[cfg(test)]
mod tests {
    use super::frames_to_mono_i16;

    #[test]
    fn float_stereo_downmix_to_mono_i16() {
        // Two frames, stereo f32: [1.0, -1.0] -> 0; [0.5, 0.5] -> ~16384.
        let mut raw = Vec::new();
        for v in [1.0f32, -1.0, 0.5, 0.5] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        let mono = frames_to_mono_i16(&raw, 2, 32, true);
        assert_eq!(mono.len(), 2);
        assert!(mono[0].abs() <= 1, "1.0/-1.0 averages to ~0, got {}", mono[0]);
        assert!((mono[1] - 16383).abs() <= 2, "0.5 -> ~16384, got {}", mono[1]);
    }

    #[test]
    fn pcm16_mono_passthrough() {
        let raw: Vec<u8> = [1000i16, -2000, 32767].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mono = frames_to_mono_i16(&raw, 1, 16, false);
        assert_eq!(mono, vec![1000, -2000, 32767]);
    }

    #[test]
    fn float_clamps_out_of_range() {
        let raw: Vec<u8> = 2.0f32.to_le_bytes().to_vec();
        let mono = frames_to_mono_i16(&raw, 1, 32, true);
        assert_eq!(mono, vec![32767]); // clamped, not wrapped
    }
}
