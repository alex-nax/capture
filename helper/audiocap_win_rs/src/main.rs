//! audiocap_win — Windows **per-process** audio loopback helper.
//!
//! Captures a target process's audio (and its process tree, so Chromium-family apps whose audio
//! renders in a child process are covered) via WASAPI process loopback
//! (`ActivateAudioInterfaceAsync` + `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` with
//! `INCLUDE_TARGET_PROCESS_TREE`, Windows 10 2004+). It writes **16 kHz mono s16le PCM** to stdout
//! and a single `READY ...` line to stderr — the frozen helper contract (docs/specs/helper-contract.md):
//! PCM-only on stdout, status/errors on stderr.
//!
//! Usage: `audiocap_win --pid <PID> [--rate 16000] [--no-tree]`
//!
//! This is the native sibling of `helper/audiocap_win.py` (which only does **system** loopback) and
//! of macOS `helper/audiocap.swift` (ScreenCaptureKit per-app). It isolates ONE app's audio —
//! parity with macOS — instead of the whole output mix.

use std::io::Write;
use std::mem::size_of;

use windows::core::{Interface, Result, PCWSTR};
use windows_core::implement;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};

const WAVE_FORMAT_PCM: u16 = 1;
/// AUDCLNT_BUFFERFLAGS_SILENT — the engine filled the packet with silence.
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;

/// A PROPVARIANT carrying a VT_BLOB that points at `AUDIOCLIENT_ACTIVATION_PARAMS`. windows-rs's
/// `PROPVARIANT` doesn't expose a BLOB constructor, so we build the 24-byte layout explicitly and
/// cast it for `ActivateAudioInterfaceAsync`.
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

/// Completion handler for the async activation — just signals an event so `run` can proceed.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler {
    event: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler_Impl {
    fn ActivateCompleted(
        &self,
        _op: windows_core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> Result<()> {
        unsafe {
            let _ = SetEvent(self.event);
        }
        Ok(())
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn main() {
    let mut pid: u32 = 0;
    let mut rate: u32 = 16000;
    let mut tree = true; // Chromium-family apps render audio in a child process
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pid" => pid = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--rate" => rate = args.next().and_then(|v| v.parse().ok()).unwrap_or(16000),
            "--no-tree" => tree = false,
            "--tree" => tree = true,
            _ => {}
        }
    }
    if pid == 0 {
        die("usage: audiocap_win --pid <PID> [--rate 16000] [--no-tree]");
    }
    if let Err(e) = run(pid, rate, tree) {
        die(&format!("audiocap_win error: {e:?}"));
    }
}

fn run(pid: u32, rate: u32, _tree: bool) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let activate_event = CreateEventW(None, false, false, PCWSTR::null())?;

        // Process-loopback activation params (include the whole target process tree).
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
        WaitForSingleObject(activate_event, INFINITE);

        let mut activate_hr = windows::core::HRESULT(0);
        let mut unknown: Option<windows::core::IUnknown> = None;
        op.GetActivateResult(&mut activate_hr, &mut unknown)?;
        activate_hr.ok()?;
        let audio_client: IAudioClient =
            unknown.ok_or_else(windows::core::Error::from_win32)?.cast()?;

        // Desired capture format: our pipeline's 16 kHz mono s16le. The loopback path
        // format-converts, so no resampling is needed on our side.
        let block_align: u16 = 2; // 1 channel * 16-bit
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: 1,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: 16,
            cbSize: 0,
        };

        // Process loopback REQUIRES shared + LOOPBACK + EVENTCALLBACK. 0.2 s buffer.
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            &fmt,
            None,
        )?;

        let buffer_event = CreateEventW(None, false, false, PCWSTR::null())?;
        audio_client.SetEventHandle(buffer_event)?;
        let capture: IAudioCaptureClient = audio_client.GetService()?;
        audio_client.Start()?;

        eprintln!("READY rate={rate} channels=1 fmt=s16le target=pid:{pid}");
        let _ = std::io::stderr().flush();

        let stdout = std::io::stdout();
        let mut out = stdout.lock();

        loop {
            WaitForSingleObject(buffer_event, INFINITE);
            loop {
                let packet = capture.GetNextPacketSize()?;
                if packet == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let nbytes = frames as usize * block_align as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 || data.is_null() {
                    // Keep the timeline advancing with zeros on a silent packet.
                    let _ = out.write_all(&vec![0u8; nbytes]);
                } else {
                    let slice = std::slice::from_raw_parts(data, nbytes);
                    let _ = out.write_all(slice);
                }
                let _ = out.flush();
                capture.ReleaseBuffer(frames)?;
            }
        }
    }
}
