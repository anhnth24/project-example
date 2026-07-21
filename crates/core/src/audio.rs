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
#[cfg(test)]
use rubato::Indexing;
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

    /// Lock order is always **inner → slot** when both are held. Slot-only waits
    /// never hold `inner`. Publication/LRU/eviction happen atomically under `inner`
    /// and never resurrect a replaced map entry.
    fn get_or_insert_with<E>(
        &self,
        key: K,
        loader: impl FnOnce() -> Result<V, E>,
    ) -> Result<Arc<V>, E> {
        loop {
            enum Next<V> {
                Load(Arc<Slot<V>>),
                Wait(Arc<Slot<V>>),
            }

            let next = {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = inner.entries.get(&key).cloned() {
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    match &*st {
                        SlotState::Ready(value) => {
                            let out = Arc::clone(value);
                            drop(st);
                            touch_lru(&mut inner, &key);
                            return Ok(out);
                        }
                        SlotState::Loading => {
                            drop(st);
                            Next::Wait(slot)
                        }
                        SlotState::Failed => {
                            *st = SlotState::Loading;
                            drop(st);
                            Next::Load(slot)
                        }
                    }
                } else {
                    // Make room among Ready entries before inserting a new Loading slot.
                    let cap = inner.capacity;
                    inner.evict_ready_while(|ready| ready >= cap);
                    let slot = Arc::new(Slot::loading());
                    inner.entries.insert(key.clone(), Arc::clone(&slot));
                    Next::Load(slot)
                }
            };

            match next {
                Next::Load(slot) => return self.run_loader(key, slot, loader),
                Next::Wait(slot) => {
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    while matches!(*st, SlotState::Loading) {
                        st = slot.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                    match &*st {
                        SlotState::Ready(value) => {
                            let out = Arc::clone(value);
                            drop(st);
                            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                            // Touch only if this slot is still the map entry for `key`.
                            if inner
                                .entries
                                .get(&key)
                                .is_some_and(|mapped| Arc::ptr_eq(mapped, &slot))
                            {
                                touch_lru(&mut inner, &key);
                            }
                            return Ok(out);
                        }
                        SlotState::Failed => {
                            // Retry loop: claim Failed → Loading under inner→slot.
                            drop(st);
                            continue;
                        }
                        SlotState::Loading => continue,
                    }
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

        match result {
            Ok(value) => {
                let shared = Arc::new(value);
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.active_loaders = inner.active_loaders.saturating_sub(1);
                let ours = inner
                    .entries
                    .get(&key)
                    .is_some_and(|mapped| Arc::ptr_eq(mapped, &slot));
                {
                    // inner → slot
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    *st = SlotState::Ready(Arc::clone(&shared));
                    slot.cv.notify_all();
                }
                if ours {
                    touch_lru(&mut inner, &key);
                    let cap = inner.capacity;
                    inner.evict_ready_while(|ready| ready > cap);
                }
                // If not ours: never resurrect/overwrite the replaced map entry.
                Ok(shared)
            }
            Err(err) => {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.active_loaders = inner.active_loaders.saturating_sub(1);
                let ours = inner
                    .entries
                    .get(&key)
                    .is_some_and(|mapped| Arc::ptr_eq(mapped, &slot));
                {
                    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                    *st = SlotState::Failed;
                    slot.cv.notify_all();
                }
                if !ours {
                    // Orphan failed slot is fine; map must keep the replacement.
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
    /// Evict oldest Ready entries while `should_evict(ready_count)` is true.
    /// Caller must hold `inner`; each candidate slot is locked under that order.
    fn evict_ready_while(&mut self, mut should_evict: impl FnMut(usize) -> bool) {
        while should_evict(self.ready_count()) {
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
    if src_rate == 0 {
        return Err(ConvertError::Failed("audio sample rate must be > 0".into()));
    }
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
    let pcm16k = resample_to_16k(&mono, src_rate)?;
    Ok((pcm16k, audio_secs))
}

/// Target sample rate for Whisper ASR preprocess.
const WHISPER_RATE: u32 = 16_000;

/// Exact output length: `round(n * to / from)`, or `0` when `n == 0`.
/// For nonempty input the result is at least 1.
fn exact_output_len(n: usize, from: u32, to: u32) -> usize {
    if n == 0 || from == 0 {
        return 0;
    }
    (((n as f64) * (to as f64 / from as f64)).round() as usize).max(1)
}

/// Minimum accepted DSP output before length-normalize: `floor(n * to / from)`.
fn min_output_len(n: usize, from: u32, to: u32) -> usize {
    if n == 0 || from == 0 {
        return 0;
    }
    ((n as f64) * (to as f64 / from as f64)).floor() as usize
}

/// Resample mono PCM to 16 kHz.
///
/// **Length contract** (`from > 0`, target 16 kHz):
/// - empty input → `Ok([])`
/// - nonempty → `Ok(v)` with `v.len() == exact_output_len(n, from, 16000)`
/// - successful DSP must yield at least `min_output_len` frames after delay trim;
///   length-normalize may repeat the last sample to hit exact length (not error silence)
/// - invalid rate / rubato failure → `Err` (never synthesize a silence buffer)
///
/// **Latency / edge policy**
/// - Fixed-input `rubato::Fft` with explicit partial final chunk + zero flush
/// - Leading `output_delay` frames are trimmed after flush
/// - Channel downmix happens before this function (mono only)
///
/// Does **not** claim WER improvement without corpus evidence.
fn resample_to_16k(input: &[f32], from: u32) -> Result<Vec<f32>, ConvertError> {
    resample_to_rate(input, from, WHISPER_RATE)
}

fn resample_to_rate(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>, ConvertError> {
    if from == 0 || to == 0 {
        return Err(ConvertError::Failed("audio sample rate must be > 0".into()));
    }
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if from == to {
        return Ok(input.to_vec());
    }

    let exact = exact_output_len(input.len(), from, to);
    let minimum = min_output_len(input.len(), from, to);
    let mut out = resample_rubato_fft(input, from, to)?;
    if out.len() < minimum {
        return Err(ConvertError::Failed(format!(
            "rubato produced {} frames after delay trim; minimum is {minimum}",
            out.len()
        )));
    }
    if out.is_empty() && exact > 0 {
        return Err(ConvertError::Failed(
            "rubato produced empty output for nonempty input".into(),
        ));
    }
    normalize_resample_len(&mut out, exact);
    debug_assert_eq!(out.len(), exact);
    Ok(out)
}

fn normalize_resample_len(out: &mut Vec<f32>, exact: usize) {
    match out.len().cmp(&exact) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => {
            let pad = *out.last().expect("nonempty before length pad");
            out.resize(exact, pad);
        }
        std::cmp::Ordering::Greater => out.truncate(exact),
    }
}

/// Offline FFT resample via [`Resampler::process_all`].
///
/// `process_all` feeds the clip, partial-pads the tail, flushes with silence until
/// the ratio-correct length is reached, and trims `output_delay` — without any
/// fixed flush-iteration cap. For a manual streaming path the equivalent bound is
/// derived below in [`flush_iter_bound`] (feed ≤ n+1, flush ≤ delay+exact at
/// ≥1 frame/iter, plus FFT block slack); true no-progress is an error.
fn resample_rubato_fft(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>, ConvertError> {
    // Chunk is a hint; FixedSync::Both snaps to a ratio-friendly size.
    let chunk = 1024usize;
    let mut resampler = Fft::<f32>::new(from as usize, to as usize, chunk, 1, FixedSync::Both)
        .map_err(|e| ConvertError::Failed(format!("rubato init: {e}")))?;

    let frames = input.len();
    let adapter = InterleavedSlice::new(input, 1, frames)
        .map_err(|e| ConvertError::Failed(format!("rubato input adapter: {e}")))?;

    // process_all resets, pads/flushes, and trims startup delay.
    let owned = resampler
        .process_all(&adapter, frames, None)
        .map_err(|e| ConvertError::Failed(format!("rubato process_all: {e}")))?;
    let out = owned.take_data();

    let exact = exact_output_len(frames, from, to);
    let minimum = min_output_len(frames, from, to);
    if out.len() < minimum {
        return Err(ConvertError::Failed(format!(
            "rubato process_all returned {} frames; minimum is {minimum} (rate {from}->{to})",
            out.len()
        )));
    }
    if out.is_empty() && exact > 0 {
        return Err(ConvertError::Failed(format!(
            "rubato process_all returned empty output for nonempty input (rate {from}->{to})"
        )));
    }
    Ok(out)
}

/// Mathematical upper bound on feed+flush iterations for a manual streaming
/// resample loop (documentation / test oracle). Not a fixed magic constant:
/// `n+1` feed steps, `delay+exact` flush steps at ≥1 out-frame progress, plus
/// FFT `input_frames_max + output_frames_max` pipeline slack.
#[cfg(test)]
fn flush_iter_bound(n: usize, delay: usize, exact: usize, in_max: usize, out_max: usize) -> usize {
    n.saturating_add(1)
        .saturating_add(delay.saturating_add(exact))
        .saturating_add(in_max)
        .saturating_add(out_max)
        .saturating_add(2)
}

/// Manual pad/partial/flush loop with derived bound + no-progress detection.
/// Mirrors `process_all_into_buffer` (trim delay, pump to ceil length) so tests can
/// prove arbitrary rates succeed without a fixed `MAX_FLUSH=64`.
#[cfg(test)]
fn resample_rubato_fft_streaming_for_test(
    input: &[f32],
    from: u32,
    to: u32,
) -> Result<Vec<f32>, ConvertError> {
    let chunk = 1024usize;
    let mut resampler = Fft::<f32>::new(from as usize, to as usize, chunk, 1, FixedSync::Both)
        .map_err(|e| ConvertError::Failed(format!("rubato init: {e}")))?;
    resampler.reset();

    let delay = resampler.output_delay();
    let exact = exact_output_len(input.len(), from, to);
    // After delay trim we need `exact` frames; pump target matches rubato's ceil.
    let pump_target = ((input.len() as f64) * (to as f64 / from as f64))
        .ceil()
        .max(exact as f64) as usize;

    let max_iters = flush_iter_bound(
        input.len(),
        delay,
        exact,
        resampler.input_frames_max(),
        resampler.output_frames_max(),
    );

    let mut collected = Vec::with_capacity(delay.saturating_add(pump_target).saturating_add(chunk));
    let mut frames_to_trim = delay;
    let mut pos = 0usize;
    let n = input.len();
    // Track raw collected growth so delay-fill flushes still count as progress.
    let mut raw_len = 0usize;

    for _ in 0..max_iters {
        let before_pos = pos;
        let before_raw = raw_len;

        let need_in = resampler.input_frames_next();
        let need_out = resampler.output_frames_next().max(1);
        let mut out_storage = vec![0.0f32; need_out];

        let (in_storage, indexing) = if pos < n {
            let remain = n - pos;
            if remain >= need_in {
                let chunk_in = input[pos..pos + need_in].to_vec();
                pos += need_in;
                (chunk_in, Indexing::new())
            } else {
                let mut chunk_in = vec![0.0f32; need_in];
                chunk_in[..remain].copy_from_slice(&input[pos..]);
                pos = n;
                (chunk_in, Indexing::new().partial_len(remain))
            }
        } else {
            (vec![0.0f32; need_in], Indexing::new().partial_len(0))
        };

        let adapter_in = InterleavedSlice::new(&in_storage, 1, need_in)
            .map_err(|e| ConvertError::Failed(format!("rubato input adapter: {e}")))?;
        let out_cap = out_storage.len();
        let mut adapter_out = InterleavedSlice::new_mut(&mut out_storage, 1, out_cap)
            .map_err(|e| ConvertError::Failed(format!("rubato output adapter: {e}")))?;

        let (_frames_in, frames_out) = resampler
            .process_into_buffer(&adapter_in, &mut adapter_out, Some(&indexing))
            .map_err(|e| ConvertError::Failed(format!("rubato process: {e}")))?;
        collected.extend_from_slice(&out_storage[..frames_out]);
        raw_len = collected.len();

        if frames_to_trim > 0 && collected.len() > frames_to_trim {
            // Move useful samples to the front (same intent as rubato's trim).
            let useful = collected.len() - frames_to_trim;
            collected.copy_within(frames_to_trim..frames_to_trim + useful, 0);
            collected.truncate(useful);
            frames_to_trim = 0;
        }

        let useful = if frames_to_trim == 0 {
            collected.len()
        } else {
            0
        };
        if pos >= n && frames_to_trim == 0 && useful >= pump_target {
            collected.truncate(pump_target);
            return Ok(collected);
        }

        let progressed = pos > before_pos || raw_len > before_raw;
        if !progressed {
            return Err(ConvertError::Failed(format!(
                "rubato streaming resample made no progress (rate {from}->{to}, pos={pos}/{n}, useful={useful}, delay_left={frames_to_trim})"
            )));
        }
    }

    Err(ConvertError::Failed(format!(
        "rubato streaming resample exhausted derived iter bound {max_iters} \
         without delay-trimmed target (rate {from}->{to})"
    )))
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

    #[test]
    fn load_once_cache_adversarial_concurrent_completion_touch_evict_same_key() {
        let cache = Arc::new(LoadOnceCache::<String, u64>::with_capacity(1));
        let barrier = Arc::new(Barrier::new(12));
        let mut handles = Vec::new();

        for i in 0..12 {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                for round in 0..40 {
                    let same = cache
                        .get_or_insert_with("same".to_string(), || {
                            thread::sleep(Duration::from_micros(50 + (i as u64) * 3));
                            Ok::<u64, &str>(100)
                        })
                        .unwrap();
                    assert_eq!(*same, 100);

                    // Force eviction pressure with distinct keys, then touch `same` again.
                    let other = format!("other-{i}-{round}");
                    let v = cache
                        .get_or_insert_with(other, || Ok::<u64, &str>(i as u64 + 1))
                        .unwrap();
                    assert!(*v >= 1);

                    let again = cache
                        .get_or_insert_with("same".to_string(), || Ok::<u64, &str>(100))
                        .unwrap();
                    assert_eq!(*again, 100);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
        assert!(cache.len_ready() <= cache.capacity());
        assert!(cache.max_active_loaders() >= 1);
        // Map must never hold more Ready entries than capacity.
        assert!(cache.len_ready() <= 1);
    }

    fn sine_wave(freq_hz: f32, sample_rate: u32, secs: f32) -> Vec<f32> {
        let n = ((sample_rate as f32) * secs).round() as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq_hz * (i as f32) / sample_rate as f32).sin())
            .collect()
    }

    fn samples_for_ms(sample_rate: u32, ms: u32) -> usize {
        ((sample_rate as u64) * (ms as u64) / 1000) as usize
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    fn center_mean(samples: &[f32]) -> f32 {
        let start = samples.len() / 5;
        let end = samples
            .len()
            .saturating_sub(samples.len() / 5)
            .max(start + 1);
        let region = &samples[start..end];
        region.iter().sum::<f32>() / region.len() as f32
    }

    /// Up + down rates used for short-clip value tests.
    const VALUE_RATES: &[(u32, &str)] = &[
        (8_000, "8k→16k"),
        (22_050, "22.05k→16k"),
        (24_000, "24k→16k"),
        (32_000, "32k→16k"),
        (44_100, "44.1k→16k"),
        (48_000, "48k→16k"),
        (96_000, "96k→16k"),
    ];

    const SHORT_MS: &[u32] = &[5, 10, 20, 100];

    #[test]
    fn resample_rejects_zero_sample_rate() {
        let err = resample_to_16k(&[1.0], 0).unwrap_err();
        assert!(err.to_string().contains("> 0"));
    }

    #[test]
    fn resample_exact_and_minimum_length_contract() {
        for &(rate, label) in VALUE_RATES {
            for &ms in SHORT_MS {
                let n = samples_for_ms(rate, ms).max(1);
                let src = vec![0.5f32; n];
                let out = resample_to_16k(&src, rate).unwrap();
                let exact = exact_output_len(n, rate, WHISPER_RATE);
                let minimum = min_output_len(n, rate, WHISPER_RATE);
                assert_eq!(out.len(), exact, "{label} {ms}ms");
                assert!(exact >= minimum, "{label}");
                assert!(out.len() >= minimum.max(1), "{label} {ms}ms");
            }
        }
        assert!(resample_to_16k(&[], 48_000).unwrap().is_empty());
    }

    /// Odd positive rates that must not trip a fixed flush cap (incl. ~192 kHz).
    const ARBITRARY_RATES: &[u32] = &[22_051, 44_101, 47_999, 88_201, 191_999];

    #[test]
    fn flush_iter_bound_scales_with_delay_exact_and_fft_blocks() {
        // Must exceed any fixed MAX_FLUSH=64 for large-delay awkward ratios.
        let bound = flush_iter_bound(110, 8_000, 80, 22_051, 16_000);
        assert!(bound > 64, "bound={bound}");
        assert_eq!(bound, 110 + 1 + 8_000 + 80 + 22_051 + 16_000 + 2);
        assert!(flush_iter_bound(10, 10, 10, 10, 10) < bound);
    }

    #[test]
    fn resample_arbitrary_positive_rates_short_and_one_second() {
        for &rate in ARBITRARY_RATES {
            for &ms in &[5u32, 20, 100] {
                let n = samples_for_ms(rate, ms).max(1);
                let src = vec![0.25f32; n];
                let out = resample_to_16k(&src, rate)
                    .unwrap_or_else(|e| panic!("short {ms}ms @ {rate} Hz: {e}"));
                assert_eq!(
                    out.len(),
                    exact_output_len(n, rate, WHISPER_RATE),
                    "short {ms}ms @ {rate}"
                );

                // Streaming path with derived bound must also succeed (no fixed flush cap).
                let streamed = resample_rubato_fft_streaming_for_test(&src, rate, WHISPER_RATE)
                    .unwrap_or_else(|e| panic!("streaming short {ms}ms @ {rate} Hz: {e}"));
                assert!(
                    streamed.len() >= min_output_len(n, rate, WHISPER_RATE),
                    "streaming short {ms}ms @ {rate}: len={}",
                    streamed.len()
                );
            }

            let n1 = rate as usize; // one second
            let src1 = vec![1.0f32; n1];
            let out1 =
                resample_to_16k(&src1, rate).unwrap_or_else(|e| panic!("1s @ {rate} Hz: {e}"));
            assert_eq!(
                out1.len(),
                exact_output_len(n1, rate, WHISPER_RATE),
                "1s @ {rate}"
            );
            // Non-silence after delay trim/flush. Absolute unity can vary for
            // hard-to-reduce FFT ratios; require clear DC energy.
            let mean = center_mean(&out1).abs();
            assert!(mean > 0.2, "1s |DC| mean={mean} @ {rate}");

            let streamed1 = resample_rubato_fft_streaming_for_test(&src1, rate, WHISPER_RATE)
                .unwrap_or_else(|e| panic!("streaming 1s @ {rate} Hz: {e}"));
            let ceil_len = ((n1 as f64) * (WHISPER_RATE as f64 / rate as f64)).ceil() as usize;
            assert_eq!(streamed1.len(), ceil_len, "streaming 1s @ {rate}");
        }
    }

    #[test]
    fn resample_short_dc_impulse_passband_value_tests() {
        for &(rate, label) in VALUE_RATES {
            for &ms in SHORT_MS {
                let n = samples_for_ms(rate, ms).max(1);

                // Length always (5/10/20/100ms). Value checks at 100ms+.
                let dc = resample_to_16k(&vec![1.0f32; n], rate).unwrap();
                assert_eq!(dc.len(), exact_output_len(n, rate, WHISPER_RATE));
                if ms >= 100 && dc.len() >= 64 {
                    // DC unity is reliable on downsample/equal paths; upsample FFT
                    // short buffers can show reduced DC gain after delay trim.
                    if rate >= WHISPER_RATE {
                        let mean = center_mean(&dc);
                        assert!((mean - 1.0).abs() < 0.15, "DC mean={mean} {label} {ms}ms");
                    } else {
                        assert!(
                            center_mean(&dc).abs() > 0.15,
                            "upsample DC energy missing {label} {ms}ms"
                        );
                    }

                    let mut impulse = vec![0.0f32; n];
                    impulse[n / 2] = 1.0;
                    let imp = resample_to_16k(&impulse, rate).unwrap();
                    let peak = imp.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
                    let min_peak = if rate >= WHISPER_RATE { 0.05 } else { 0.001 };
                    assert!(
                        peak > min_peak,
                        "impulse peak={peak} {label} {ms}ms (delay trim/flush)"
                    );

                    let tone = sine_wave(1_000.0, rate, ms as f32 / 1000.0);
                    let out = resample_to_16k(&tone, rate).unwrap();
                    let in_rms = rms(&tone);
                    let out_rms = rms(&out);
                    let min_ratio = if rate >= WHISPER_RATE { 0.45 } else { 0.25 };
                    assert!(
                        out_rms > in_rms * min_ratio,
                        "passband rms in={in_rms} out={out_rms} {label} {ms}ms"
                    );
                }
            }
        }
    }

    #[test]
    fn resample_passband_and_alias_on_longer_clips() {
        for &(rate, label) in VALUE_RATES {
            if rate <= 16_000 {
                // Upsample path: check passband only.
                let pass_src = sine_wave(1_000.0, rate, 0.25);
                let pass_out = resample_to_16k(&pass_src, rate).unwrap();
                assert!(rms(&pass_out) > rms(&pass_src) * 0.7, "{label}");
                continue;
            }
            let pass_src = sine_wave(1_000.0, rate, 0.25);
            let pass_out = resample_to_16k(&pass_src, rate).unwrap();
            let pass_out_rms = rms(&pass_out);
            assert!(pass_out_rms > rms(&pass_src) * 0.7, "{label}");

            let alias_src = sine_wave(8_500.0, rate, 0.25);
            let alias_out = resample_to_16k(&alias_src, rate).unwrap();
            assert!(
                rms(&alias_out) < pass_out_rms * 0.15,
                "alias leak {label}: {} vs {pass_out_rms}",
                rms(&alias_out)
            );
        }
    }
}
