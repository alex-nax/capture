//! The ASR backend interface — a port of `core/asr/base.py` (`Segment` + `ASRBackend`) plus the
//! `asr.is_silent` silence gate from `core/asr/__init__.py`.
//!
//! A backend receives mono float32 PCM (range `[-1, 1]`) at a known sample rate and returns
//! recognized [`Segment`]s with timestamps **relative to the start of the chunk**. The caller adds
//! the chunk's absolute offset to place each segment on the capture timeline.

/// A recognized speech span. `start`/`end` are seconds **relative to the chunk** passed to
/// [`AsrBackend::transcribe`], not the capture timeline (mirrors `base.Segment`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The rate every backend wants its PCM resampled to (mirrors `ASRBackend.target_sample_rate`).
/// The capture pipeline is 16 kHz mono s16le end-to-end.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// A swappable speech-to-text engine. The single interface every ASR backend implements; nothing
/// else constructs a concrete engine directly (mirrors the architecture's dependency rule).
pub trait AsrBackend: Send + Sync {
    /// A short identifier for diagnostics (`"whisper-rs"`, `"openai-compat"`, …).
    fn name(&self) -> &str;

    /// The rate this backend wants `pcm` resampled to before [`transcribe`](AsrBackend::transcribe)
    /// (default 16 kHz). The backend itself resamples if it's handed another rate.
    fn target_sample_rate(&self) -> u32 {
        TARGET_SAMPLE_RATE
    }

    /// Transcribe one chunk: `pcm` is mono float32 in `[-1, 1]`, `sample_rate` is its rate. Returns
    /// the recognized segments (empty for silence/garbage). `Err` is a transcribe failure the caller
    /// records as an `asr_error` (capture continues).
    fn transcribe(&self, pcm: &[f32], sample_rate: u32) -> Result<Vec<Segment>, String>;
}

/// int16-scale RMS below which a chunk is treated as silence and SKIPPED — Whisper hallucinates
/// phantom phrases / token loops on silence (a dead mic at rms ~40 looped "Thank you."). Default 70,
/// overridable via `CAPTURE_ASR_SILENCE_RMS`. Mirrors `asr.SILENCE_RMS16`.
pub fn silence_rms16() -> f32 {
    std::env::var("CAPTURE_ASR_SILENCE_RMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(70.0)
}

/// True if `pcm` (float32 in `[-1, 1]`) is near-silent — its RMS, rescaled to the int16 range, is
/// below `threshold16`. The caller skips transcribing silent chunks (the offset still advances so the
/// timeline holds). Mirrors `asr.is_silent`.
pub fn is_silent(pcm: &[f32], threshold16: f32) -> bool {
    if pcm.is_empty() {
        return true;
    }
    // RMS in f64 to match the Python (np.square + mean in float64), then rescale to int16.
    let sum_sq: f64 = pcm.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum_sq / pcm.len() as f64).sqrt() * 32768.0;
    (rms as f32) < threshold16
}

/// Linear-interpolation resample of mono float32 PCM from `from_rate` to `to_rate`. The pipeline is
/// already 16 kHz end-to-end (so `transcribe` usually skips this), but a backend that strictly needs
/// 16 kHz (whisper.cpp) uses it as a safety net for any other input rate.
pub fn resample_linear(pcm: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || pcm.is_empty() {
        return pcm.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((pcm.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        // Source position in the input for output sample i.
        let src = i as f64 / ratio;
        let lo = src.floor() as usize;
        let frac = (src - lo as f64) as f32;
        let a = pcm[lo.min(pcm.len() - 1)];
        let b = pcm[(lo + 1).min(pcm.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_detects_empty_and_quiet() {
        assert!(is_silent(&[], 70.0), "empty is silent");
        // A tiny-amplitude tone: rms ~ 0.0005 * 32768 ≈ 16 < 70 → silent.
        let quiet: Vec<f32> = (0..16000).map(|i| 0.0005 * ((i as f32) * 0.1).sin()).collect();
        assert!(is_silent(&quiet, 70.0), "near-silent tone is silent");
    }

    #[test]
    fn silence_passes_loud_audio() {
        // A 0.2-amplitude tone: rms ~ 0.14 * 32768 ≈ 4600 >> 70 → not silent.
        let loud: Vec<f32> = (0..16000).map(|i| 0.2 * ((i as f32) * 0.3).sin()).collect();
        assert!(!is_silent(&loud, 70.0), "loud audio is not silent");
    }

    #[test]
    fn resample_is_noop_at_same_rate() {
        let pcm = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_linear(&pcm, 16000, 16000), pcm);
    }

    #[test]
    fn resample_halves_length_when_downsampling_2x() {
        // 48k → 16k is a 1/3 ratio; length ≈ input/3.
        let pcm: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let out = resample_linear(&pcm, 48000, 16000);
        assert!((out.len() as i64 - 100).abs() <= 1, "len {} ~ 100", out.len());
        // First sample preserved.
        assert_eq!(out[0], 0.0);
    }
}
