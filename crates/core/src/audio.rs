//! Audio → text (tiếng Việt) bằng whisper.cpp qua `whisper-rs`.
//!
//! - Decode mp3/wav/m4a/flac/ogg bằng `symphonia` (Rust thuần), downmix mono,
//!   resample về 16kHz bằng `rubato` FFT (windowed anti-alias) — Whisper cần PCM f32 16kHz mono.
//! - `WhisperContext` cache **process-wide**, LRU-bounded, keyed by immutable load identity;
//!   MCP/desktop tạo `Converter` mỗi request vẫn tái dùng model đã load.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
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

/// Default number of Ready Whisper contexts retained in the process cache.
const DEFAULT_WHISPER_CACHE_CAPACITY: usize = 2;

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
/// - New canonical path or different load knobs → new cache entry.
/// - Ready entries beyond the LRU capacity are evicted from the cache map; any
///   outstanding [`AudioEngine`] `Arc` keeps that context alive until dropped.
/// - Replacing bytes at the same path is **not** detected; restart or use a
///   different path to pick up a new file.
/// - Failed loads leave a stable `Failed` slot (no invalid `Ready` reference);
///   the next caller transitions `Failed → Loading` (single loader) and retries.
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

/// Per-key slot state. Transitions are serialized by the slot mutex + condvar.
enum SlotState<V> {
    Loading,
    Ready(Arc<V>),
    Failed,
}

struct Slot<V> {
    state: Mutex<SlotState<V>>,
    cv: Condvar,
}

impl<V> Slot<V> {
    fn loading() -> Self {
        Self {
            state: Mutex::new(SlotState::Loading),
            cv: Condvar::new(),
        }
    }
}

struct CacheInner<K, V> {
    entries: HashMap<K, Arc<Slot<V>>>,
    /// LRU order of Ready keys (front = oldest).
    lru: VecDeque<K>,
    capacity: usize,
    /// Concurrent in-flight loaders (all keys). Testable via [`LoadOnceCache::max_active_loaders`].
    active_loaders: usize,
    max_active_loaders: usize,
}

/// Process-wide load-once cache with stable Loading/Ready/Failed states.
///
/// - One loader per key at a time (condvar waiters never overlap loaders).
/// - Ready set is LRU-bounded; eviction drops the cache's `Arc` only.
/// - Failed slots stay until a caller claims `Failed → Loading` for retry.
struct LoadOnceCache<K, V> {
    inner: Mutex<CacheInner<K, V>>,
}

impl<K, V> LoadOnceCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Send + Sync,
{
    fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                entries: HashMap::new(),
                lru: VecDeque::new(),
                capacity: capacity.max(1),
                active_loaders: 0,
                max_active_loaders: 0,
            }),
        }
    }

    fn get_or_insert_with<E>(
        &self,
        key: K,
        loader: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        let slot = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = inner.entries.get(&key).cloned() {
                // Fast path: already Ready.
                {
                    let st = existing.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let SlotState::Ready(value) = &*st {
                        let out = Arc::clone(value);
                        drop(st);
                        touch_lru(&mut inner, &key);
                        return Ok(out);
                    }
                }
                existing
            } else {
                inner.evict_ready_to_capacity();
                let slot = Arc::new(Slot::loading());
                inner.entries.insert(key.clone(), Arc::clone(&slot));
                drop(inner);
                return self.run_loader(key, slot, loader);
            }
        };

        // Existing non-Ready slot: wait or claim Failed → Loading.
        let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match &*st {
                SlotState::Ready(value) => {
                    let out = Arc::clone(value);
                    drop(st);
                    let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    touch_lru(&mut inner, &key);
                    return Ok(out);
                }
                SlotState::Loading => {
                    st = slot.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                SlotState::Failed => {
                    *st = SlotState::Loading;
                    drop(st);
                    return self.run_loader(key, slot, loader);
                }
            }
        }
    }

    fn run_loader<E>(
        &self,
        key: K,
        slot: Arc<Slot<V>>,
        loader: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.active_loaders += 1;
            if inner.active_loaders > inner.max_active_loaders {
                inner.max_active_loaders = inner.active_loaders;
            }
        }

        let result = loader();

        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.active_loaders = inner.active_loaders.saturating_sub(1);
        }

        match result {
            Ok(value) => {
                let shared = Arc::new(value);
                {
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    *st = SlotState::Ready(Arc::clone(&shared));
                    slot.cv.notify_all();
                }
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                // Ensure entry still points at this slot (not evicted mid-load).
                inner
                    .entries
                    .entry(key.clone())
                    .and_modify(|s| *s = Arc::clone(&slot))
                    .or_insert_with(|| Arc::clone(&slot));
                touch_lru(&mut inner, &key);
                inner.evict_ready_to_capacity();
                Ok(shared)
            }
            Err(err) => {
                {
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    *st = SlotState::Failed;
                    slot.cv.notify_all();
                }
                Err(err)
            }
        }
    }

    #[cfg(test)]
    fn len_ready(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .values()
            .filter(|slot| {
                matches!(
                    *slot.state.lock().unwrap_or_else(|e| e.into_inner()),
                    SlotState::Ready(_)
                )
            })
            .count()
    }

    #[cfg(test)]
    fn len_entries(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }

    #[cfg(test)]
    fn max_active_loaders(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .max_active_loaders
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capacity
    }
}

impl<K, V> CacheInner<K, V>
where
    K: Eq + Hash + Clone,
{
    fn evict_ready_to_capacity(&mut self) {
        while self.ready_count() > self.capacity {
            let Some(old) = self.lru.pop_front() else {
                break;
            };
            let should_remove = self
                .entries
                .get(&old)
                .map(|slot| {
                    matches!(
                        *slot.state.lock().unwrap_or_else(|e| e.into_inner()),
                        SlotState::Ready(_)
                    )
                })
                .unwrap_or(false);
            if should_remove {
                self.entries.remove(&old);
            }
        }
    }

    fn ready_count(&self) -> usize {
        self.entries
            .values()
            .filter(|slot| {
                matches!(
                    *slot.state.lock().unwrap_or_else(|e| e.into_inner()),
                    SlotState::Ready(_)
                )
            })
            .count()
    }
}

fn touch_lru<K, V>(inner: &mut CacheInner<K, V>, key: &K)
where
    K: Eq + Hash + Clone,
{
    inner.lru.retain(|k| k != key);
    inner.lru.push_back(key.clone());
}

fn whisper_cache_capacity() -> usize {
    std::env::var("FILECONV_WHISPER_CACHE_CAPACITY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(DEFAULT_WHISPER_CACHE_CAPACITY)
}

fn whisper_model_cache() -> &'static LoadOnceCache<WhisperModelKey, WhisperContext> {
    static CACHE: OnceLock<LoadOnceCache<WhisperModelKey, WhisperContext>> = OnceLock::new();
    CACHE.get_or_init(|| LoadOnceCache::with_capacity(whisper_cache_capacity()))
}

/// Production loader: all Whisper load behavior is derived from the complete key.
fn load_whisper_from_key(key: &WhisperModelKey) -> Result<WhisperContext, ConvertError> {
    WhisperContext::new_with_params(&key.model_path, key.context_parameters())
        .map_err(|e| ConvertError::Failed(format!("load model whisper: {e}")))
}

/// Engine giữ `WhisperContext` đã load (chia sẻ process-wide qua cache).
pub struct AudioEngine {
    ctx: Arc<WhisperContext>,
    threads: i32,
    no_speech_threshold: f32,
}

impl AudioEngine {
    /// Load model GGML (vd models/ggml-base.bin) via the process-wide LRU cache.
    pub fn load(model_path: &Path) -> Result<Self, ConvertError> {
        let key = WhisperModelKey::from_path(model_path);
        let ctx = whisper_model_cache()
            .get_or_insert_with(key.clone(), || load_whisper_from_key(&key))?;
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

/// Target sample rate for Whisper ASR preprocess.
const WHISPER_RATE: u32 = 16_000;

/// Exact output length contract: `round(n_in * 16000 / from)`.
fn expected_resample_len(input_len: usize, from: u32) -> usize {
    if input_len == 0 {
        return 0;
    }
    let n = ((input_len as f64) * (WHISPER_RATE as f64 / from as f64)).round() as usize;
    n.max(1)
}

/// Resample mono PCM to 16 kHz with `rubato::Fft` (synchronous FFT + Blackman–Harris-2
/// anti-alias window — maintained polyphase/FFT resampler).
///
/// **Latency / edge policy**
/// - Uses [`Resampler::process_all`], which resets the resampler and **trims startup
///   delay** so the clip aligns with the input (no leading resampler silence).
/// - Output length is normalized to [`expected_resample_len`] (`round`). If rubato
///   returns fewer frames (short-clip edge after delay trim), pad with the last
///   sample (or `0.0` if empty). If more, truncate.
/// - Empty input → empty output. Non-empty input → non-empty output.
/// - Channel downmix happens **before** this function (average → mono); this path
///   is strictly mono.
///
/// Does **not** claim WER improvement without corpus evidence.
fn resample_to_16k(input: &[f32], from: u32) -> Vec<f32> {
    if from == WHISPER_RATE || input.is_empty() {
        return input.to_vec();
    }
    let expected = expected_resample_len(input.len(), from);
    match resample_rubato_fft(input, from, WHISPER_RATE) {
        Ok(mut out) => {
            normalize_resample_len(&mut out, expected);
            out
        }
        Err(_) => {
            // Construction/process failure is unexpected for valid rates; keep ASR
            // path alive with length-correct zeros rather than panicking decode.
            vec![0.0; expected]
        }
    }
}

fn normalize_resample_len(out: &mut Vec<f32>, expected: usize) {
    match out.len().cmp(&expected) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => {
            let pad = out.last().copied().unwrap_or(0.0);
            out.resize(expected, pad);
        }
        std::cmp::Ordering::Greater => {
            out.truncate(expected);
        }
    }
}

fn resample_rubato_fft(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>, ConvertError> {
    // Chunk size is a hint; FixedSync::Both snaps to an exact ratio-friendly size.
    // 1024 keeps delay modest for ASR offline clips.
    let mut resampler = Fft::<f32>::new(from as usize, to as usize, 1024, 1, FixedSync::Both)
        .map_err(|e| ConvertError::Failed(format!("rubato init: {e}")))?;

    let frames = input.len();
    let adapter = InterleavedSlice::new(input, 1, frames)
        .map_err(|e| ConvertError::Failed(format!("rubato input adapter: {e}")))?;
    let owned = resampler
        .process_all(&adapter, frames, None)
        .map_err(|e| ConvertError::Failed(format!("rubato process_all: {e}")))?;
    Ok(owned.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

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
        let cache = Arc::new(LoadOnceCache::<String, u64>::with_capacity(4));
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
                        thread::sleep(Duration::from_millis(30));
                        Ok::<u64, &'static str>(42)
                    })
                    .unwrap()
            }));
        }

        let values: Vec<Arc<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(cache.max_active_loaders(), 1);
        assert!(values.iter().all(|v| **v == 42));
        assert!(values.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
        assert_eq!(cache.len_ready(), 1);
    }

    #[test]
    fn load_once_cache_fail_retry_third_caller_never_overlaps_loaders() {
        let cache = Arc::new(LoadOnceCache::<&'static str, i32>::with_capacity(2));
        let loads = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        // First load fails.
        let err = cache.get_or_insert_with("k", || Err::<i32, _>("boom"));
        assert_eq!(err, Err("boom"));
        assert_eq!(cache.len_entries(), 1); // Failed slot retained

        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..3 {
            let cache = Arc::clone(&cache);
            let loads = Arc::clone(&loads);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                cache
                    .get_or_insert_with("k", || {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(now, Ordering::SeqCst);
                        loads.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(40));
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok::<i32, &str>(7)
                    })
                    .unwrap()
            }));
        }

        let values: Vec<Arc<i32>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(values.iter().all(|v| **v == 7));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(cache.max_active_loaders(), 1);
        assert_eq!(cache.len_ready(), 1);
    }

    #[test]
    fn load_once_cache_does_not_leak_failed_as_ready() {
        let cache = LoadOnceCache::<&'static str, i32>::with_capacity(2);
        let _ = cache.get_or_insert_with("k", || Err::<i32, _>("boom"));
        assert_eq!(cache.len_ready(), 0);

        let ok = cache
            .get_or_insert_with("k", || Ok::<i32, &str>(7))
            .unwrap();
        assert_eq!(*ok, 7);
        assert_eq!(cache.len_ready(), 1);
    }

    #[test]
    fn load_once_cache_keys_distinct_configs_separately() {
        let cache = LoadOnceCache::<(String, bool), i32>::with_capacity(4);
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
        assert_eq!(cache.len_ready(), 2);
    }

    #[test]
    fn load_once_cache_lru_evicts_and_reloads_while_arcs_keep_alive() {
        let cache = LoadOnceCache::<String, u64>::with_capacity(2);
        assert_eq!(cache.capacity(), 2);
        let loads = AtomicUsize::new(0);

        let a = cache
            .get_or_insert_with("a".into(), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<u64, &str>(1)
            })
            .unwrap();
        let b = cache
            .get_or_insert_with("b".into(), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<u64, &str>(2)
            })
            .unwrap();
        assert_eq!(cache.len_ready(), 2);

        // Insert c → evict oldest Ready (a) from the map.
        let c = cache
            .get_or_insert_with("c".into(), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<u64, &str>(3)
            })
            .unwrap();
        assert_eq!(*c, 3);
        assert!(cache.len_ready() <= 2);
        // Held Arc stays alive after eviction.
        assert_eq!(*a, 1);
        assert_eq!(Arc::strong_count(&a), 1);

        // Reload a → new load (evicted), not the same Arc.
        let a2 = cache
            .get_or_insert_with("a".into(), || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok::<u64, &str>(11)
            })
            .unwrap();
        assert_eq!(*a2, 11);
        assert!(!Arc::ptr_eq(&a, &a2));
        assert_eq!(loads.load(Ordering::SeqCst), 4);
        let _ = b;
    }

    #[test]
    fn load_once_cache_lru_concurrent_safety() {
        let cache = Arc::new(LoadOnceCache::<usize, usize>::with_capacity(2));
        let barrier = Arc::new(Barrier::new(6));
        let mut handles = Vec::new();
        for i in 0..6 {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                for k in 0..8 {
                    let key = (i + k) % 5;
                    let v = cache
                        .get_or_insert_with(key, || Ok::<usize, &str>(key))
                        .unwrap();
                    assert_eq!(*v, key);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(cache.len_ready() <= cache.capacity());
        assert!(cache.max_active_loaders() >= 1);
    }

    fn sine_wave(freq_hz: f32, sample_rate: u32, secs: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq_hz * (i as f32) / sample_rate as f32).sin())
            .collect()
    }

    fn goertzel_power(samples: &[f32], sample_rate: u32, freq_hz: f32) -> f32 {
        let n = samples.len() as f32;
        if n < 16.0 {
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
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    const RATES_TO_16K: &[u32] = &[22_050, 24_000, 32_000, 44_100, 48_000, 96_000];

    #[test]
    fn resample_exact_rounded_length_across_common_rates() {
        for &rate in RATES_TO_16K {
            let src = sine_wave(440.0, rate, 0.2);
            let out = resample_to_16k(&src, rate);
            assert_eq!(
                out.len(),
                expected_resample_len(src.len(), rate),
                "rate={rate}"
            );
        }
    }

    #[test]
    fn resample_short_input_is_nonempty() {
        for &rate in RATES_TO_16K {
            let src = vec![0.25f32; 3];
            let out = resample_to_16k(&src, rate);
            assert!(!out.is_empty(), "rate={rate}");
            assert_eq!(out.len(), expected_resample_len(src.len(), rate));
        }
        assert!(resample_to_16k(&[], 48_000).is_empty());
    }

    #[test]
    fn resample_dc_unity_gain() {
        for &rate in RATES_TO_16K {
            let src = vec![1.0f32; (rate as usize) / 5]; // 0.2 s
            let out = resample_to_16k(&src, rate);
            // Skip a few edge samples; center should sit near DC 1.0.
            let start = out.len() / 10;
            let end = out.len().saturating_sub(out.len() / 10).max(start + 1);
            let region = &out[start..end];
            let mean = region.iter().sum::<f32>() / region.len() as f32;
            assert!((mean - 1.0).abs() < 0.05, "DC mean={mean} at rate={rate}");
        }
    }

    #[test]
    fn resample_passband_gain_and_alias_attenuation_above_8k() {
        for &rate in RATES_TO_16K {
            // Passband: 1 kHz tone should retain most energy.
            let pass_src = sine_wave(1_000.0, rate, 0.25);
            let pass_out = resample_to_16k(&pass_src, rate);
            let pass_in_rms = rms(&pass_src);
            let pass_out_rms = rms(&pass_out);
            assert!(
                pass_out_rms > pass_in_rms * 0.7,
                "passband RMS dropped too far at {rate}: in={pass_in_rms} out={pass_out_rms}"
            );

            // Just above 8 kHz Nyquist of 16 kHz target → must be attenuated.
            let alias_freq = 8_500.0;
            if alias_freq >= (rate as f32) / 2.0 {
                continue;
            }
            let alias_src = sine_wave(alias_freq, rate, 0.25);
            let alias_out = resample_to_16k(&alias_src, rate);
            let alias_out_rms = rms(&alias_out);
            // Residual should be far below the passband tone level.
            assert!(
                alias_out_rms < pass_out_rms * 0.15,
                "alias leakage at {rate}: alias_rms={alias_out_rms} pass_rms={pass_out_rms}"
            );
            // Also check energy near the folded region is small vs passband bin.
            let pass_bin = goertzel_power(&pass_out, WHISPER_RATE, 1_000.0);
            let fold = (alias_freq - WHISPER_RATE as f32).abs(); // 8500 → ~7500
            let alias_bin = goertzel_power(&alias_out, WHISPER_RATE, fold);
            assert!(
                alias_bin < pass_bin * 0.05,
                "alias bin too strong at {rate}: alias_bin={alias_bin} pass_bin={pass_bin}"
            );
        }
    }

    #[test]
    fn upsample_from_8k_preserves_rounded_length() {
        let src = sine_wave(440.0, 8_000, 0.1);
        let up = resample_to_16k(&src, 8_000);
        assert_eq!(up.len(), expected_resample_len(src.len(), 8_000));
    }
}
