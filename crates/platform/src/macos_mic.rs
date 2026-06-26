//! Live microphone capture via **AVFoundation** (`AVCaptureSession` + `AVCaptureAudioDataOutput`) —
//! feature #88. ScreenCaptureKit's `captureMicrophone` delivers a Bluetooth-HFP headset at 8 kHz CVSD
//! narrowband (telephone grade); a direct AVCaptureSession on the SAME device negotiates 16 kHz mSBC
//! WIDEBAND. macOS lets an SCStream (app/system audio) and an AVCaptureSession (mic) run concurrently
//! in one process — the "no two concurrent audio SCStreams" limit does NOT cross frameworks — so the
//! engine keeps SCK for app audio and routes the mic through here.
//!
//! The delegate decodes each `CMSampleBuffer` to mono `i16` (the output is configured LinearPCM Int16
//! mono via `audioSettings`) and reads the buffer's TRUE rate from its ASBD, then calls `on_samples(&[i16],
//! rate)` — the same `(samples, rate)` contract the SCK `AudioSink` uses, so it feeds the existing
//! `sink_into` → buffer → audio_worker path unchanged (the worker's #87 empirical re-measurement stays
//! the safety net for any source still misreporting its rate).

use std::ffi::c_char;
use std::ptr;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureAudioDataOutput, AVCaptureAudioDataOutputSampleBufferDelegate, AVCaptureConnection,
    AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession, AVMediaTypeAudio,
};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer};
use objc2_foundation::NSObjectProtocol;

use crate::{AudioCallback, AUDIO_SAMPLE_RATE};

/// Append a diagnostic line to `~/.capture/mic-avf.log` (and stderr). The daemon's stderr is
/// `/dev/null` under the launched app, so this file is how #88 mic-resolution issues are diagnosed.
fn mic_log(msg: &str) {
    use std::io::Write;
    eprintln!("{msg}");
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join(".capture").join("mic-avf.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// `kAudioFormatFlagIsFloat` — set in an ASBD's `mFormatFlags` when samples are floating-point.
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;

/// Log the first delivered buffer's format once per process — confirms the delegate fires and reveals
/// the native sample format AVFoundation hands us (so the conversion below can be verified).
fn log_format_once(rate: u32, flags: u32, bits: u32, channels: u32) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        mic_log(&format!(
            "[mic-avf] delegate FIRED; native buffer format rate={rate} flags={flags:#x} bits={bits} channels={channels}"
        ));
    }
}

/// A running AVFoundation mic capture. Holds the `AVCaptureSession`, the delegate (whose ivar owns the
/// `on_samples` callback), and its serial GCD queue alive for the capture's lifetime. `Drop` stops the
/// session. Mirrors how [`crate::macos::MacAudioCapture`] keeps its SCStream + queue alive.
pub struct MicCapture {
    session: Retained<AVCaptureSession>,
    // The delegate is retained by the output (a weak-ish AV reference internally), but we keep our own
    // strong ref so the ivar-held callback can't be freed while buffers are still in flight.
    _delegate: Retained<MicDelegate>,
    // The GCD serial queue the delegate is called on; must outlive the session.
    _queue: dispatch2::DispatchRetained<dispatch2::DispatchQueue>,
}

// SAFETY: the contained Obj-C objects are thread-safe to hold + release (AVCaptureSession is used
// across threads by AVFoundation itself); we only move/drop the handle, never share &mut.
unsafe impl Send for MicCapture {}

impl MicCapture {
    /// Stop the capture (idempotent with drop).
    pub fn stop(self) -> Result<(), String> {
        unsafe { self.session.stopRunning() };
        Ok(())
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        unsafe { self.session.stopRunning() };
    }
}

/// The delegate's ivar: the boxed `(&[i16], rate)` sink. `Arc` so it's cheaply cloned into the class.
struct MicDelegateIvars {
    on_samples: Arc<AudioCallback>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - The class implements `Drop` only via its ivars (an `Arc`), which is fine.
    // - The delegate is called on the AV serial queue (an arbitrary thread), so it is NOT
    //   MainThreadOnly; `AllocAnyThread` is used to construct it.
    #[unsafe(super = objc2_foundation::NSObject)]
    #[ivars = MicDelegateIvars]
    struct MicDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for MicDelegate {}

    // SAFETY: the method signature matches `captureOutput:didOutputSampleBuffer:fromConnection:`.
    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for MicDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // SAFETY: `sample_buffer` is a valid CMSampleBuffer owned by AVFoundation for the call.
            let Some((samples, rate)) = (unsafe { decode_buffer(sample_buffer) }) else {
                return;
            };
            if samples.is_empty() {
                return;
            }
            (self.ivars().on_samples)(&samples, rate);
        }
    }
);

impl MicDelegate {
    fn new(on_samples: Arc<AudioCallback>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MicDelegateIvars { on_samples });
        // SAFETY: NSObject's `init` has the standard signature.
        unsafe { msg_send![super(this), init] }
    }
}

/// Decode one `CMSampleBuffer` (LinearPCM Int16 mono, per the output's `audioSettings`) to `Vec<i16>`
/// plus the buffer's TRUE sample rate read from its ASBD. `None` if the buffer has no data / no format
/// description. Contiguous-buffer fast path via `data_pointer`; falls back to copying via `copy_data_bytes`.
///
/// # Safety
/// `sample` must be a valid `CMSampleBuffer` (the AV callback's argument is).
unsafe fn decode_buffer(sample: &CMSampleBuffer) -> Option<(Vec<i16>, u32)> {
    // Read the buffer's audio format from its ASBD: the negotiated rate (e.g. 16000 for a wideband BT
    // headset), float-vs-int, bit depth, and channel count. AVCaptureAudioDataOutput delivers the
    // device's NATIVE format (we no longer pin Int16 via audioSettings — that silently blocked delivery
    // on macOS), which is typically interleaved Float32.
    let fd = unsafe { sample.format_description() }?;
    let asbd_ptr = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&fd) };
    if asbd_ptr.is_null() {
        return None;
    }
    let asbd = unsafe { *asbd_ptr };
    let rate = if asbd.mSampleRate > 0.0 { asbd.mSampleRate.round() as u32 } else { AUDIO_SAMPLE_RATE };
    let channels = (asbd.mChannelsPerFrame.max(1)) as usize;
    let bits = asbd.mBitsPerChannel;
    let is_float = asbd.mFormatFlags & K_AUDIO_FORMAT_FLAG_IS_FLOAT != 0;
    log_format_once(rate, asbd.mFormatFlags, bits, asbd.mChannelsPerFrame);

    let bbuf = unsafe { sample.data_buffer() }?;
    let len = unsafe { bbuf.data_length() };
    if len == 0 {
        return Some((Vec::new(), rate));
    }

    // Try the zero-copy contiguous pointer first; fall back to a safe copy.
    let mut length_at_offset: usize = 0;
    let mut total_length: usize = 0;
    let mut data_ptr: *mut c_char = ptr::null_mut();
    let st = unsafe {
        bbuf.data_pointer(0, &mut length_at_offset, &mut total_length, &mut data_ptr)
    };
    let bytes: Vec<u8> = if st == 0 && !data_ptr.is_null() && length_at_offset >= len {
        unsafe { std::slice::from_raw_parts(data_ptr as *const u8, len).to_vec() }
    } else {
        let mut buf = vec![0u8; len];
        let dst = std::ptr::NonNull::new(buf.as_mut_ptr() as *mut std::ffi::c_void)?;
        if unsafe { bbuf.copy_data_bytes(0, len, dst) } != 0 {
            return None;
        }
        buf
    };

    // Convert to MONO i16, taking channel 0 of each (interleaved) frame — for a mono mic that's every
    // sample. Float32 → scaled i16 (the SCK path's convention); native Int16 → straight reinterpret.
    let out: Vec<i16> = if is_float && bits == 32 {
        bytes
            .chunks_exact(4)
            .step_by(channels)
            .map(|b| (f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect()
    } else if bits == 16 {
        bytes
            .chunks_exact(2)
            .step_by(channels)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    } else {
        return None; // unsupported bit depth
    };
    Some((out, rate))
}

/// Start capturing `device_id`'s mic via AVFoundation, delivering mono s16le batches + their native rate
/// to `on_samples` until the returned [`MicCapture`] is stopped/dropped. `device_id` of `""`/`"default"`
/// uses the system default input. `Err` if no device resolves or the session won't start (usually a mic
/// TCC denial). See the module docs for why this exists.
pub fn start_mic_capture_avf(
    device_id: &str,
    on_samples: AudioCallback,
) -> Result<MicCapture, String> {
    unsafe {
        let media_audio = AVMediaTypeAudio.ok_or("AVMediaTypeAudio constant unavailable")?;

        // Resolve the requested id to a concrete AVCaptureDevice.
        let auth = AVCaptureDevice::authorizationStatusForMediaType(media_audio);
        mic_log(&format!("[mic-avf] audio authorization status = {}", auth.0));
        let device = resolve_device(device_id, media_audio)?;
        mic_log(&format!(
            "[mic-avf] CHOSEN device uniqueID={:?} name={:?}",
            device.uniqueID().to_string(),
            device.localizedName().to_string(),
        ));

        let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
            .map_err(|e| format!("AVCaptureDeviceInput init: {}", e.localizedDescription()))?;

        let session = AVCaptureSession::new();
        session.beginConfiguration();
        if !session.canAddInput(&input) {
            session.commitConfiguration();
            return Err("AVCaptureSession cannot add the mic input".into());
        }
        session.addInput(&input);

        let output = AVCaptureAudioDataOutput::new();
        // Deliver the device's NATIVE format (typically interleaved Float32 at the negotiated wideband
        // rate). Pinning Int16 via audioSettings silently stops AVCaptureAudioDataOutput from delivering
        // ANY buffers on macOS (confirmed live, #88), so we leave it native and convert in the delegate.
        output.setAudioSettings(None);

        let delegate = MicDelegate::new(Arc::new(on_samples));
        // A serial queue guarantees in-order delivery (the AV docs require it).
        // `DispatchQueueAttr::SERIAL` is already an `Option<&DispatchQueueAttr>` (a serial queue is the
        // dispatch default ⇒ `None`), so it's passed straight through.
        let queue =
            dispatch2::DispatchQueue::new("com.capture.mic.avf", dispatch2::DispatchQueueAttr::SERIAL);
        let proto: &ProtocolObject<dyn AVCaptureAudioDataOutputSampleBufferDelegate> =
            ProtocolObject::from_ref(&*delegate);
        output.setSampleBufferDelegate_queue(Some(proto), Some(&queue));

        if !session.canAddOutput(&output) {
            session.commitConfiguration();
            return Err("AVCaptureSession cannot add the audio output".into());
        }
        session.addOutput(&output);
        session.commitConfiguration();

        session.startRunning();
        mic_log(&format!(
            "[mic-avf] startRunning called; session.isRunning={}, inputs={}, outputs={}",
            session.isRunning(),
            session.inputs().count(),
            session.outputs().count(),
        ));
        Ok(MicCapture { session, _delegate: delegate, _queue: queue })
    }
}

/// Resolve a requested input-device id to a concrete `AVCaptureDevice`. Empty/`"default"` → the system
/// default audio input. Otherwise apply [`match_device_index`] over the enumerated devices' uniqueIDs +
/// localizedNames; fall back to the system default if nothing matches.
///
/// # Safety
/// `media_audio` must be the `AVMediaTypeAudio` constant.
unsafe fn resolve_device(
    device_id: &str,
    media_audio: &objc2_av_foundation::AVMediaType,
) -> Result<Retained<AVCaptureDevice>, String> {
    let want = device_id.trim();
    let default = || {
        AVCaptureDevice::defaultDeviceWithMediaType(media_audio)
            .ok_or_else(|| "no default audio input device available".to_string())
    };
    if want.is_empty() || want.eq_ignore_ascii_case("default") {
        return default();
    }

    // Enumerate the audio-input devices, collecting (uniqueID, localizedName) for the match rule.
    #[allow(deprecated)] // `devicesWithMediaType:` is deprecated but right for a one-shot lookup here.
    let devices = AVCaptureDevice::devicesWithMediaType(media_audio);
    let pairs: Vec<(String, String)> = (0..devices.count())
        .map(|i| {
            let d = devices.objectAtIndex(i);
            (d.uniqueID().to_string(), d.localizedName().to_string())
        })
        .collect();

    mic_log(&format!("[mic-avf] resolve {want:?} over {} AVCaptureDevice(s): {pairs:?}", pairs.len()));
    match match_device_index(want, &pairs) {
        Some(i) => {
            mic_log(&format!("[mic-avf] matched index {i} → {:?}", pairs[i]));
            Ok(devices.objectAtIndex(i))
        }
        // Nothing matched — fall back to the default rather than fail the whole capture.
        None => {
            mic_log(&format!("[mic-avf] NO match for {want:?}; falling back to the system default input"));
            default()
        }
    }
}

/// Pick the index of the device whose id best matches `want`, given each device's
/// `(unique_id, localized_name)`. The rule, in priority order:
/// 1. exact `unique_id == want`;
/// 2. `unique_id` is a prefix of `want` (handles `"<uid>:input"`-style composite ids from the device
///    list), or `want` is a prefix of `unique_id`;
/// 3. case-insensitive `localized_name == want`.
///
/// Returns `None` if nothing matches (caller falls back to the default device). Pure — unit-tested.
fn match_device_index(want: &str, pairs: &[(String, String)]) -> Option<usize> {
    let want = want.trim();
    if want.is_empty() {
        return None;
    }
    // 1. exact uniqueID.
    if let Some(i) = pairs.iter().position(|(uid, _)| uid == want) {
        return Some(i);
    }
    // 2. prefix either way (composite ids like "<uid>:input").
    if let Some(i) = pairs
        .iter()
        .position(|(uid, _)| !uid.is_empty() && (want.starts_with(uid.as_str()) || uid.starts_with(want)))
    {
        return Some(i);
    }
    // 3. localizedName (case-insensitive).
    pairs.iter().position(|(_, name)| name.eq_ignore_ascii_case(want))
}

/// Enumerate audio input devices via AVFoundation (`AVCaptureDevice`) — the SAME source #88 captures
/// from, so the list always includes devices ScreenCaptureKit's enumeration misses (e.g. a
/// Bluetooth-HFP mic whose HFP link is engaged). The `id` is the device's `uniqueID` (exactly what
/// `start_mic_capture_avf` resolves against); `default` flags the current system default input.
pub(crate) fn audio_input_devices_avf() -> Vec<crate::AudioInputInfo> {
    unsafe {
        let Some(media_audio) = AVMediaTypeAudio else {
            return Vec::new();
        };
        let default_uid =
            AVCaptureDevice::defaultDeviceWithMediaType(media_audio).map(|d| d.uniqueID().to_string());
        #[allow(deprecated)] // devicesWithMediaType: is deprecated but right for a one-shot enumeration.
        let devices = AVCaptureDevice::devicesWithMediaType(media_audio);
        (0..devices.count())
            .filter_map(|i| {
                let d = devices.objectAtIndex(i);
                // Only currently-CONNECTED devices — `devicesWithMediaType:` otherwise lingers a
                // known/paired device (e.g. a Bluetooth headset) after it disconnects, so the picker
                // would keep offering a mic that isn't there.
                if !d.isConnected() {
                    return None;
                }
                let id = d.uniqueID().to_string();
                let default = default_uid.as_deref() == Some(id.as_str());
                Some(crate::AudioInputInfo { name: d.localizedName().to_string(), id, default })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::match_device_index;

    fn pairs() -> Vec<(String, String)> {
        vec![
            ("BuiltInMicrophoneDevice".into(), "MacBook Pro Microphone".into()),
            ("14-0A-29-7C-08-42:input".into(), "Alex's AirPods Pro".into()),
            ("AppleUSBAudioEngine:Generic:USB Audio".into(), "USB Audio".into()),
        ]
    }

    #[test]
    fn exact_unique_id_wins() {
        assert_eq!(match_device_index("BuiltInMicrophoneDevice", &pairs()), Some(0));
        assert_eq!(match_device_index("14-0A-29-7C-08-42:input", &pairs()), Some(1));
    }

    #[test]
    fn prefix_match_handles_composite_ids() {
        // The device list may hand back the bare CoreAudio UID while AVFoundation's uniqueID carries a
        // ":input" suffix (or vice-versa); a prefix either direction still resolves it.
        assert_eq!(match_device_index("14-0A-29-7C-08-42", &pairs()), Some(1));
        assert_eq!(match_device_index("14-0A-29-7C-08-42:input:extra", &pairs()), Some(1));
    }

    #[test]
    fn localized_name_match_is_case_insensitive() {
        assert_eq!(match_device_index("usb audio", &pairs()), Some(2));
        assert_eq!(match_device_index("MACBOOK PRO MICROPHONE", &pairs()), Some(0));
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(match_device_index("does-not-exist", &pairs()), None);
        assert_eq!(match_device_index("", &pairs()), None);
    }

    #[test]
    fn exact_id_beats_a_weaker_prefix_collision() {
        // An exact uniqueID match must win even when another device's uniqueID is a prefix of `want`.
        let p = vec![
            ("AB".into(), "Short".into()),
            ("ABCD".into(), "Exact".into()),
        ];
        assert_eq!(match_device_index("ABCD", &p), Some(1));
    }
}
