//! Audio → text (tiếng Việt) bằng whisper.cpp qua `whisper-rs`.
//!
//! - Decode mp3/wav/m4a/flac/ogg bằng `symphonia` (Rust thuần), downmix mono,
//!   resample về 16kHz (whisper yêu cầu PCM f32 16kHz mono).
//! - `WhisperContext` được cache **process-wide** theo identity model + cấu hình
//!   load bất biến; MCP/desktop tạo `Converter` mỗi request vẫn tái dùng model.

use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::ConvertError;

const WHISPER_MODEL_CANDIDATES: &[&str] = &[
    "ggml-PhoWhisper-small.bin",
    "ggml-small.bin",
    "ggml-base.bin",
    "ggml-tiny.bin",
];

fn discover_model_in_roots(roots: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    for root in roots {
        for candidate in WHISPER_MODEL_CANDIDATES {
            let path = root.join("models").join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Discover an installed Whisper model without bundling model weights.
/// PhoWhisper is preferred when users downloaded it explicitly.
pub fn discover_whisper_model() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("FILECONV_WHISPER_MODEL") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().take(4).map(Path::to_path_buf));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.extend(parent.ancestors().take(4).map(Path::to_path_buf));
        }
    }
    roots.extend(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .take(4)
            .map(Path::to_path_buf),
    );
    discover_model_in_roots(&roots)
}

/// Kết quả phiên âm kèm số liệu thời gian (phục vụ báo cáo RTF).
#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub audio_secs: f64,
    pub decode_ms: f64,
    pub infer_ms: f64,
    pub total_segments: usize,
    pub filtered_segments: usize,
}

/// Immutable Whisper load identity: canonical model path + load-time knobs.
///
/// Runtime knobs (`threads`, `no_speech_threshold`) are **not** part of this key —
/// they are applied per [`AudioEngine`] after the shared context is retrieved.
///
/// **Path/config change policy**
/// - New canonical path or different load knobs → new cache entry (old retained).
/// - Replacing bytes at the same path is **not** detected; restart the process or
///   point at a different path to pick up a new file.
/// - Failed loads are not retained; the next caller retries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WhisperModelKey {
    pub model_path: PathBuf,
    pub use_gpu: bool,
    pub flash_attn: bool,
    pub gpu_device: i32,
}

impl WhisperModelKey {
    /// Build a key from a model path using default load parameters
    /// (matches [`WhisperContextParameters::default`]).
    pub fn from_path(path: &Path) -> Self {
        let defaults = WhisperContextParameters::default();
        Self {
            model_path: canonical_model_path(path),
            use_gpu: defaults.use_gpu,
            flash_attn: defaults.flash_attn,
            gpu_device: defaults.gpu_device,
        }
    }

    fn context_parameters(&self) -> WhisperContextParameters<'static> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu = self.use_gpu;
        params.flash_attn = self.flash_attn;
        params.gpu_device = self.gpu_device;
        params
    }
}

fn canonical_model_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Process-wide load-once cache. Successful values are retained for the process
/// lifetime; failed loads leave no entry (retryable). Concurrent callers for the
/// same key share a single in-flight load.
struct LoadOnceCache<K, V> {
    map: Mutex<HashMap<K, Arc<CacheSlot<V>>>>,
}

struct CacheSlot<V> {
    value: OnceLock<Arc<V>>,
    /// Serializes the loader for this key so only one thread loads.
    gate: Mutex<()>,
}

impl<K, V> LoadOnceCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Send + Sync,
{
    fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Load `key` once. Successful values stay for process lifetime; failures remove
    /// the empty slot so the next caller retries without retaining an invalid entry.
    fn get_or_insert_with<E>(
        &self,
        key: K,
        loader: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        let slot = {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            map.entry(key.clone())
                .or_insert_with(|| {
                    Arc::new(CacheSlot {
                        value: OnceLock::new(),
                        gate: Mutex::new(()),
                    })
                })
                .clone()
        };

        if let Some(ready) = slot.value.get() {
            return Ok(Arc::clone(ready));
        }

        let gate = slot.gate.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ready) = slot.value.get() {
            return Ok(Arc::clone(ready));
        }

        match loader() {
            Ok(value) => {
                let shared = Arc::new(value);
                // Under `gate`, only one loader runs; `OnceLock` still guards races.
                let _ = slot.value.set(Arc::clone(&shared));
                drop(gate);
                Ok(shared)
            }
            Err(err) => {
                drop(gate);
                let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(existing) = map.get(&key) {
                    if existing.value.get().is_none() && Arc::ptr_eq(existing, &slot) {
                        map.remove(&key);
                    }
                }
                Err(err)
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

fn whisper_model_cache() -> &'static LoadOnceCache<WhisperModelKey, WhisperContext> {
    static CACHE: OnceLock<LoadOnceCache<WhisperModelKey, WhisperContext>> = OnceLock::new();
    CACHE.get_or_init(LoadOnceCache::new)
}

fn load_whisper_context_cached(
    key: &WhisperModelKey,
    loader: impl FnOnce(&WhisperModelKey) -> Result<WhisperContext, ConvertError>,
) -> Result<Arc<WhisperContext>, ConvertError> {
    whisper_model_cache().get_or_insert_with(key.clone(), || loader(key))
}

/// Engine giữ `WhisperContext` đã load (chia sẻ process-wide qua cache).
pub struct AudioEngine {
    ctx: Arc<WhisperContext>,
    threads: i32,
    no_speech_threshold: f32,
}

impl AudioEngine {
    /// Load model GGML (vd models/ggml-base.bin), using the process-wide cache.
    pub fn load(model_path: &Path) -> Result<Self, ConvertError> {
        Self::load_with(model_path, |key| {
            WhisperContext::new_with_params(&key.model_path, key.context_parameters())
                .map_err(|e| ConvertError::Failed(format!("load model whisper: {e}")))
        })
    }

    /// Load with an injectable loader (tests can count invocations without a real model
    /// by exercising [`LoadOnceCache`] directly; this path is for whisper-backed hooks).
    pub fn load_with(
        model_path: &Path,
        loader: impl FnOnce(&WhisperModelKey) -> Result<WhisperContext, ConvertError>,
    ) -> Result<Self, ConvertError> {
        let key = WhisperModelKey::from_path(model_path);
        let ctx = load_whisper_context_cached(&key, loader)?;
        Ok(Self {
            ctx,
            threads: 4,
            no_speech_threshold: 0.6,
        })
    }

    pub fn with_threads(mut self, n: i32) -> Self {
        self.threads = n;
        self
    }

    pub fn with_no_speech_threshold(mut self, threshold: f32) -> Self {
        if threshold.is_finite() && (0.0..=1.0).contains(&threshold) {
            self.no_speech_threshold = threshold;
        }
        self
    }

    /// Phiên âm một file audio. `lang` = Some("vi") cho tiếng Việt, None = tự nhận.
    pub fn transcribe_file(
        &self,
        path: &Path,
        lang: Option<&str>,
    ) -> Result<Transcript, ConvertError> {
        let t0 = Instant::now();
        let (pcm, audio_secs) = decode_to_pcm16k_mono(path)?;
        let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| ConvertError::Failed(format!("whisper state: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(lang.or(Some("vi")));
        params.set_n_threads(self.threads);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_no_speech_thold(self.no_speech_threshold);

        let t1 = Instant::now();
        state
            .full(params, &pcm)
            .map_err(|e| ConvertError::Failed(format!("whisper full: {e}")))?;
        let infer_ms = t1.elapsed().as_secs_f64() * 1000.0;

        let n = state.full_n_segments();
        let mut text = String::new();
        let mut filtered_segments = 0usize;
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    let segment = s.trim();
                    if !segment_is_speech(
                        segment,
                        seg.no_speech_probability(),
                        self.no_speech_threshold,
                    ) {
                        filtered_segments += 1;
                        continue;
                    }
                    text.push_str(segment);
                    text.push(' ');
                }
            }
        }
        Ok(Transcript {
            text: text.trim().to_string(),
            audio_secs,
            decode_ms,
            infer_ms,
            total_segments: n.max(0) as usize,
            filtered_segments,
        })
    }
}

fn segment_is_speech(text: &str, no_speech_probability: f32, threshold: f32) -> bool {
    if text.trim().is_empty() || no_speech_probability >= threshold {
        return false;
    }
    let trimmed = text.trim();
    let enclosed_marker = ((trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')')))
        && trimmed.chars().count() <= 120;
    if enclosed_marker {
        return false;
    }
    let marker = trimmed
        .trim_matches(|character| matches!(character, '[' | ']' | '(' | ')' | '♪' | ' '))
        .to_ascii_lowercase();
    !matches!(
        marker.as_str(),
        "music" | "silence" | "no speech" | "applause" | "background music" | "nhạc" | "im lặng"
    )
}

/// Decode file audio → (PCM f32 mono 16kHz, độ dài giây).
pub fn decode_to_pcm16k_mono(path: &Path) -> Result<(Vec<f32>, f64), ConvertError> {
    let file = std::fs::File::open(path).map_err(|e| ConvertError::Failed(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| ConvertError::Failed(format!("probe audio: {e}")))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| ConvertError::Failed("không tìm thấy track audio".into()))?;
    let track_id = track.id;
    let src_rate = track.codec_params.sample_rate.unwrap_or(16000);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1)
        .max(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| ConvertError::Failed(format!("decoder: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break, // hết packet
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_buf.is_none() {
                    let spec = *decoded.spec();
                    let dur = decoded.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(dur, spec));
                }
                let sb = sample_buf.as_mut().unwrap();
                sb.copy_interleaved_ref(decoded);
                for frame in sb.samples().chunks(channels) {
                    let sum: f32 = frame.iter().sum();
                    mono.push(sum / channels as f32);
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }

    let audio_secs = mono.len() as f64 / src_rate as f64;
    let pcm16k = resample_to_16k(&mono, src_rate);
    Ok((pcm16k, audio_secs))
}

/// Resample to 16 kHz with a cheap anti-alias path when downsampling.
///
/// Integer ratios (e.g. 48 kHz → 16 kHz) use a short triangular-FIR decimator.
/// Fractional downsampling (e.g. 44.1 kHz → 16 kHz) applies a short moving-average
/// prefilter then linear interpolation. Upsampling stays linear.
///
/// This targets alias energy reduction for ASR preprocessing; it does **not** claim
/// WER improvement without corpus measurement.
fn resample_to_16k(input: &[f32], from: u32) -> Vec<f32> {
    const TO: u32 = 16000;
    if from == TO || input.is_empty() {
        return input.to_vec();
    }
    if from > TO {
        if from % TO == 0 {
            return box_decimate(input, (from / TO) as usize);
        }
        let filtered = moving_average_prefilter(input, from, TO);
        return resample_linear(&filtered, from, TO);
    }
    resample_linear(input, from, TO)
}

/// Integer-factor downsample with a short triangular FIR (length `2*factor-1`), then
/// keep every `factor`-th filtered sample. Stronger stopband than a single box of
/// width `factor`, still O(n) and allocation-light for ASR preprocess.
fn box_decimate(input: &[f32], factor: usize) -> Vec<f32> {
    if factor <= 1 {
        return input.to_vec();
    }
    let taps = 2 * factor - 1;
    if input.len() < taps {
        return Vec::new();
    }
    let mid = factor - 1;
    let mut kernel = vec![0.0f32; taps];
    let mut kern_sum = 0.0f32;
    for (i, tap) in kernel.iter_mut().enumerate() {
        let dist = i.abs_diff(mid);
        *tap = (factor - dist) as f32;
        kern_sum += *tap;
    }
    let inv = 1.0 / kern_sum;
    for tap in &mut kernel {
        *tap *= inv;
    }

    let out_len = (input.len() - taps) / factor + 1;
    let mut out = Vec::with_capacity(out_len);
    let mut start = 0usize;
    while start + taps <= input.len() {
        let window = &input[start..start + taps];
        let mut acc = 0.0f32;
        for (s, k) in window.iter().zip(kernel.iter()) {
            acc += s * k;
        }
        out.push(acc);
        start += factor;
    }
    out
}

/// Causal moving-average prefilter; window ≈ from/to (≥ 2 when downsampling).
fn moving_average_prefilter(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let window = ((from as f64 / to as f64).round() as usize).max(2);
    if input.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(input.len());
    let mut run = 0.0f32;
    for i in 0..input.len() {
        run += input[i];
        if i >= window {
            run -= input[i - window];
            out.push(run / window as f32);
        } else {
            out.push(run / (i + 1) as f32);
        }
    }
    out
}

/// Resample tuyến tính (upsample / fractional stage after prefilter).
fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn no_speech_filter_rejects_probability_and_marker_hallucinations() {
        assert!(!segment_is_speech("văn bản bị bịa", 0.8, 0.6));
        assert!(!segment_is_speech("[Music]", 0.1, 0.6));
        assert!(!segment_is_speech("[Mà đồ]", 0.1, 0.6));
        assert!(!segment_is_speech("♪ nhạc ♪", 0.1, 0.6));
        assert!(segment_is_speech("Xin chào Việt Nam", 0.1, 0.6));
    }

    #[test]
    fn invalid_threshold_does_not_replace_safe_default() {
        assert!(!segment_is_speech("text", 0.7, 0.6));
        assert!(segment_is_speech("text", 0.59, 0.6));
    }

    #[test]
    fn model_discovery_prefers_phowhisper_then_standard_models() {
        let root =
            std::env::temp_dir().join(format!("fileconv_model_discovery_{}", std::process::id()));
        let models = root.join("models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("ggml-base.bin"), b"base").unwrap();
        assert!(discover_model_in_roots(std::slice::from_ref(&root))
            .unwrap()
            .ends_with("ggml-base.bin"));
        std::fs::write(models.join("ggml-PhoWhisper-small.bin"), b"pho").unwrap();
        assert!(discover_model_in_roots(std::slice::from_ref(&root))
            .unwrap()
            .ends_with("ggml-PhoWhisper-small.bin"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_key_uses_canonical_path_identity() {
        let dir = std::env::temp_dir().join(format!(
            "fileconv_model_key_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("ggml-tiny.bin");
        std::fs::write(&model, b"fake").unwrap();

        let key_a = WhisperModelKey::from_path(&model);
        let key_b = WhisperModelKey::from_path(&dir.join(".").join("ggml-tiny.bin"));
        assert_eq!(key_a.model_path, key_b.model_path);
        assert_eq!(key_a, key_b);

        let mut key_gpu = key_a.clone();
        key_gpu.use_gpu = !key_a.use_gpu;
        assert_ne!(key_a, key_gpu);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_once_cache_dedups_concurrent_loads_with_injectable_counter() {
        let cache = Arc::new(LoadOnceCache::<String, u64>::new());
        let loads = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let cache_loads = Arc::clone(&loads);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                cache
                    .get_or_insert_with("model-a".to_string(), || {
                        cache_loads.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(std::time::Duration::from_millis(20));
                        Ok::<u64, &'static str>(42)
                    })
                    .unwrap()
            }));
        }

        let values: Vec<Arc<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(values.iter().all(|v| **v == 42));
        assert!(values.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn load_once_cache_does_not_retain_failed_loads() {
        let cache = LoadOnceCache::<&'static str, i32>::new();
        let loads = AtomicUsize::new(0);

        let err = cache.get_or_insert_with("k", || {
            loads.fetch_add(1, Ordering::SeqCst);
            Err::<i32, _>("boom")
        });
        assert_eq!(err, Err("boom"));
        assert_eq!(cache.len(), 0);

        let ok = cache
            .get_or_insert_with("k", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<i32, &str>(7)
            })
            .unwrap();
        assert_eq!(*ok, 7);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(cache.len(), 1);

        let again = cache
            .get_or_insert_with("k", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<i32, &str>(99)
            })
            .unwrap();
        assert_eq!(*again, 7);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn load_once_cache_keys_distinct_configs_separately() {
        let cache = LoadOnceCache::<(String, bool), i32>::new();
        let loads = AtomicUsize::new(0);
        let a = cache
            .get_or_insert_with(("m".into(), false), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<i32, &str>(1)
            })
            .unwrap();
        let b = cache
            .get_or_insert_with(("m".into(), true), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<i32, &str>(2)
            })
            .unwrap();
        assert_eq!(*a, 1);
        assert_eq!(*b, 2);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(cache.len(), 2);
    }

    fn sine_wave(freq_hz: f32, sample_rate: u32, secs: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq_hz * (i as f32) / sample_rate as f32).sin())
            .collect()
    }

    /// Single-bin Goertzel power for numeric alias checks.
    fn goertzel_power(samples: &[f32], sample_rate: u32, freq_hz: f32) -> f32 {
        let n = samples.len() as f32;
        if n < 8.0 {
            return 0.0;
        }
        let k = (0.5 + (n * freq_hz / sample_rate as f32)).floor();
        let omega = 2.0 * PI * k / n;
        let coeff = 2.0 * omega.cos();
        let mut s1 = 0.0f32;
        let mut s2 = 0.0f32;
        for &x in samples {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        power.max(0.0)
    }

    #[test]
    fn downsample_48k_to_16k_suppresses_above_nyquist_alias() {
        // 10 kHz tone at 48 kHz folds near 6 kHz after naive 16 kHz resample (Nyquist 8 kHz).
        let src = sine_wave(10_000.0, 48_000, 0.25);
        let naive = resample_linear(&src, 48_000, 16_000);
        let filtered = resample_to_16k(&src, 48_000);
        let expected_len = (src.len() - (2 * 3 - 1)) / 3 + 1;
        assert_eq!(filtered.len(), expected_len);

        let alias_naive = goertzel_power(&naive, 16_000, 6_000.0);
        let alias_filtered = goertzel_power(&filtered, 16_000, 6_000.0);
        assert!(
            alias_filtered < alias_naive * 0.2,
            "triangular decimate should suppress 10k→6k alias: filtered={alias_filtered}, naive={alias_naive}"
        );
    }

    #[test]
    fn downsample_44k1_to_16k_suppresses_above_nyquist_alias() {
        // 9.5 kHz tone at 44.1 kHz aliases below 8 kHz Nyquist after 16 kHz resample.
        let src = sine_wave(9_500.0, 44_100, 0.25);
        let naive = resample_linear(&src, 44_100, 16_000);
        let filtered = resample_to_16k(&src, 44_100);

        let alias_naive = goertzel_power(&naive, 16_000, 6_500.0);
        let alias_filtered = goertzel_power(&filtered, 16_000, 6_500.0);
        assert!(
            alias_filtered < alias_naive * 0.55,
            "prefilter+linear should reduce 9.5k alias energy: filtered={alias_filtered}, naive={alias_naive}"
        );
    }

    #[test]
    fn upsample_linear_preserves_length_ratio() {
        let src = sine_wave(440.0, 8_000, 0.1);
        let up = resample_to_16k(&src, 8_000);
        let expected = ((src.len() as f64) * (16_000.0 / 8_000.0)).round() as usize;
        assert_eq!(up.len(), expected);
    }
}
