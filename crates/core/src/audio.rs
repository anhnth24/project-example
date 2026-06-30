//! Audio → text (tiếng Việt) bằng whisper.cpp qua `whisper-rs`.
//!
//! - Decode mp3/wav/m4a/flac/ogg bằng `symphonia` (Rust thuần), downmix mono,
//!   resample về 16kHz (whisper yêu cầu PCM f32 16kHz mono).
//! - `AudioEngine` load model một lần, tái dùng cho nhiều file (đo tốc độ chuẩn).

use std::path::Path;
use std::time::Instant;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::ConvertError;

/// Kết quả phiên âm kèm số liệu thời gian (phục vụ báo cáo RTF).
#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    pub audio_secs: f64,
    pub decode_ms: f64,
    pub infer_ms: f64,
}

/// Engine giữ một WhisperContext đã load (tái dùng cho nhiều file).
pub struct AudioEngine {
    ctx: WhisperContext,
    threads: i32,
}

impl AudioEngine {
    /// Load model GGML (vd models/ggml-base.bin).
    pub fn load(model_path: &Path) -> Result<Self, ConvertError> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| ConvertError::Failed(format!("load model whisper: {e}")))?;
        Ok(Self { ctx, threads: 4 })
    }

    pub fn with_threads(mut self, n: i32) -> Self {
        self.threads = n;
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

        let t1 = Instant::now();
        state
            .full(params, &pcm)
            .map_err(|e| ConvertError::Failed(format!("whisper full: {e}")))?;
        let infer_ms = t1.elapsed().as_secs_f64() * 1000.0;

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    text.push_str(s.trim());
                    text.push(' ');
                }
            }
        }
        Ok(Transcript {
            text: text.trim().to_string(),
            audio_secs,
            decode_ms,
            infer_ms,
        })
    }
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
    let pcm16k = resample_linear(&mono, src_rate, 16000);
    Ok((pcm16k, audio_secs))
}

/// Resample tuyến tính (đủ tốt cho ASR giọng nói).
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
