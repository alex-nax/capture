//! AVFoundation-backed media import — decode an audio/video file's audio track to s16le PCM and
//! sample video frames to PNG. The v3, in-process replacement for the Swift `audiocap` helper's
//! `--extract-audio` / `--extract-frames` modes (a port of `core/import_media.py`'s extraction).
//! macOS-only; the public wrappers in `lib.rs` gate other platforms with a clear error.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::AllocAnyThread;
use objc2_av_foundation::{
    AVAssetImageGenerator, AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput,
    AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset,
};
use objc2_avf_audio::{
    AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey,
    AVLinearPCMIsNonInterleaved, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_core_foundation::{CFMutableData, CFString, CGSize};
use objc2_core_graphics::CGImage;
use objc2_core_media::{kCMTimeZero, CMTime};
use objc2_foundation::{NSMutableDictionary, NSNumber, NSString, NSURL};
use objc2_image_io::CGImageDestination;

use crate::ImportedFrame;

/// `kAudioFormatLinearPCM` — the four-char-code `'lpcm'` (avoids an objc2-core-audio-types dep).
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6c70_636d;

/// A file URL for `src` (absolute string path → `NSURL`).
fn file_url(src: &Path) -> Result<Retained<NSURL>, String> {
    let path = src.to_str().ok_or("path is not valid UTF-8")?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(path)))
}

/// Decode the first audio track of `src` to 16-bit little-endian mono PCM at `rate` Hz. `Ok(None)`
/// when the file has no audio track (a video-only import); `Err` on a real decode failure.
// The synchronous `tracksWithMediaType:` is deprecated in favour of an async block-based load, but for
// an offline file import the sync read is exactly right (and matches the Python's helper).
#[allow(deprecated)]
pub fn extract_audio_s16le(src: &Path, rate: u32) -> Result<Option<Vec<u8>>, String> {
    let url = file_url(src)?;
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let media_audio = AVMediaTypeAudio.ok_or("AVMediaTypeAudio constant unavailable")?;
        let tracks = asset.tracksWithMediaType(media_audio);
        let Some(track) = tracks.firstObject() else {
            return Ok(None); // no audio track — caller treats like the helper's "exit 3"
        };

        // LinearPCM 16-bit mono little-endian output — AVAssetReader decodes + resamples to `rate`.
        let settings = NSMutableDictionary::<NSString, AnyObject>::new();
        let put = |key: Option<&NSString>, val: &NSNumber| -> Result<(), String> {
            let key = key.ok_or("an AV audio-settings key constant is unavailable")?;
            settings.setObject_forKey(val, ProtocolObject::from_ref(key));
            Ok(())
        };
        put(AVFormatIDKey, &NSNumber::numberWithUnsignedInt(K_AUDIO_FORMAT_LINEAR_PCM))?;
        put(AVSampleRateKey, &NSNumber::numberWithDouble(rate as f64))?;
        put(AVNumberOfChannelsKey, &NSNumber::numberWithInt(1))?;
        put(AVLinearPCMBitDepthKey, &NSNumber::numberWithInt(16))?;
        put(AVLinearPCMIsFloatKey, &NSNumber::numberWithBool(false))?;
        put(AVLinearPCMIsBigEndianKey, &NSNumber::numberWithBool(false))?;
        put(AVLinearPCMIsNonInterleaved, &NSNumber::numberWithBool(false))?;

        let reader = AVAssetReader::initWithAsset_error(AVAssetReader::alloc(), &asset)
            .map_err(|e| format!("AVAssetReader init: {}", e.localizedDescription()))?;
        let output = AVAssetReaderTrackOutput::initWithTrack_outputSettings(
            AVAssetReaderTrackOutput::alloc(),
            &track,
            Some(&settings),
        );
        if !reader.canAddOutput(&output) {
            return Err("AVAssetReader cannot add the audio output".into());
        }
        reader.addOutput(&output);
        if !reader.startReading() {
            let detail = reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
            return Err(format!("startReading failed: {detail}"));
        }

        let mut pcm: Vec<u8> = Vec::new();
        while let Some(sbuf) = output.copyNextSampleBuffer() {
            if let Some(bbuf) = sbuf.data_buffer() {
                let len = bbuf.data_length();
                if len > 0 {
                    let start = pcm.len();
                    pcm.resize(start + len, 0);
                    let dst = NonNull::new(pcm.as_mut_ptr().add(start) as *mut c_void)
                        .ok_or("null PCM destination")?;
                    let st = bbuf.copy_data_bytes(0, len, dst);
                    if st != 0 {
                        return Err(format!("CMBlockBufferCopyDataBytes failed: {st}"));
                    }
                }
            }
        }
        if reader.status() == AVAssetReaderStatus::Failed {
            let detail = reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
            return Err(format!("read failed: {detail}"));
        }
        Ok(Some(pcm))
    }
}

/// The CMTime timescale for the sample instants (600 is the conventional video timescale).
const TIMESCALE: i32 = 600;

/// Sample frames from `src`'s video track at `interval` seconds (`0, interval, … ≤ duration`), each
/// encoded PNG with its millisecond offset. Empty when the file has no video track (an audio-only
/// import). `max_width` optionally caps the frame width (aspect-preserved). `Err` on a decode failure.
#[allow(deprecated)] // sync `tracksWithMediaType:` is right for an offline import (see above).
pub fn extract_frames(
    src: &Path,
    interval: f64,
    max_width: Option<u32>,
) -> Result<Vec<ImportedFrame>, String> {
    let url = file_url(src)?;
    let interval = interval.max(0.1);
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let media_video = AVMediaTypeVideo.ok_or("AVMediaTypeVideo constant unavailable")?;
        if asset.tracksWithMediaType(media_video).firstObject().is_none() {
            return Ok(Vec::new()); // no video track — an audio-only import has no frames
        }
        let duration = asset.duration().seconds();
        if !duration.is_finite() || duration <= 0.0 {
            return Ok(Vec::new());
        }

        let gen = AVAssetImageGenerator::initWithAsset(AVAssetImageGenerator::alloc(), &asset);
        gen.setAppliesPreferredTrackTransform(true);
        // Exact frames (no snapping to nearby keyframes) so a frame's offset matches its real time.
        gen.setRequestedTimeToleranceBefore(kCMTimeZero);
        gen.setRequestedTimeToleranceAfter(kCMTimeZero);
        if let Some(w) = max_width {
            // maximumSize scales to FIT the box preserving aspect; a huge height lets width bind.
            gen.setMaximumSize(CGSize { width: w as f64, height: 1.0e6 });
        }

        let png_type = CFString::from_str("public.png");
        let mut frames = Vec::new();
        let mut t = 0.0_f64;
        while t <= duration + 1e-6 {
            let time = CMTime::with_seconds(t, TIMESCALE);
            match gen.copyCGImageAtTime_actualTime_error(time, std::ptr::null_mut()) {
                Ok(image) => {
                    let png = cgimage_to_png(&image, &png_type)?;
                    frames.push(ImportedFrame { offset_ms: (t * 1000.0).round() as u64, png });
                }
                // A time past the last decodable frame can fail — stop rather than spin to the end.
                Err(_) => break,
            }
            t += interval;
        }
        Ok(frames)
    }
}

/// Encode a CGImage to PNG bytes via ImageIO (`CGImageDestination`). `png_type` is the `public.png` UTI.
///
/// # Safety
/// `image` must be a valid CGImage; called only from [`extract_frames`] with a freshly-generated one.
unsafe fn cgimage_to_png(image: &CGImage, png_type: &CFString) -> Result<Vec<u8>, String> {
    let data = CFMutableData::new(None, 0).ok_or("CFMutableData allocation failed")?;
    let dest = CGImageDestination::with_data(&data, png_type, 1, None)
        .ok_or("CGImageDestinationCreateWithData failed")?;
    dest.add_image(image, None);
    if !dest.finalize() {
        return Err("CGImageDestinationFinalize failed".into());
    }
    let len = data.length();
    if len <= 0 {
        return Err("PNG encode produced no bytes".into());
    }
    let bytes = std::slice::from_raw_parts(data.byte_ptr(), len as usize);
    Ok(bytes.to_vec())
}
