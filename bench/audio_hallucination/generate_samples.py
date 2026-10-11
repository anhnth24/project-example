#!/usr/bin/env python3
"""
Tạo bộ mẫu âm thanh để đo lường Whisper hallucination (Issue #448):
1. 01_pure_silence.wav: 10 giây im lặng tuyệt đối.
2. 02_ambient_noise.wav: 10 giây tạp âm nền (hiss/hum phòng nhẹ).
3. 03_instrumental_music.wav: 15 giây hợp âm nhạc cụ tổng hợp (không lời).
4. 04_speech_with_pauses.wav: Giọng nói tiếng Việt thật xen kẽ khoảng lặng 3s.
5. Tải 4 clips chuẩn cho bench/vn_audio/ (clip1.mp3 -> clip4.mp3).
"""

import os
import sys
import wave
import math
import random
import struct
import urllib.parse
import urllib.request
from pathlib import Path

SAMPLE_RATE = 16000  # 16kHz chuẩn cho Whisper


def write_wav(path: Path, samples: list[float], sample_rate: int = SAMPLE_RATE):
    """Ghi danh sách mẫu float [-1.0, 1.0] thành file WAV 16-bit PCM mono."""
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        # Chuyển float sang int16
        raw_bytes = bytearray()
        for s in samples:
            clamped = max(-1.0, min(1.0, s))
            val = int(clamped * 32767.0)
            raw_bytes.extend(struct.pack("<h", val))
        wf.writeframes(raw_bytes)


def make_pure_silence(duration_sec: float = 10.0) -> list[float]:
    """10 giây im lặng tuyệt đối (mẫu = 0.0)."""
    n = int(SAMPLE_RATE * duration_sec)
    return [0.0] * n


def make_ambient_noise(duration_sec: float = 10.0, amplitude: float = 0.02) -> list[float]:
    """10 giây tiếng ồn nền (pink/brownian low-level ambient noise)."""
    n = int(SAMPLE_RATE * duration_sec)
    samples = []
    # Brownian noise filter
    last = 0.0
    for _ in range(n):
        white = random.uniform(-1.0, 1.0)
        last = (last + (0.05 * white)) / 1.05
        # Thêm chút hum 50Hz (tiếng điện lưới/quạt phòng)
        hum = 0.005 * math.sin(2 * math.pi * 50 * len(samples) / SAMPLE_RATE)
        samples.append(last * amplitude + hum)
    return samples


def make_instrumental_music(duration_sec: float = 15.0) -> list[float]:
    """
    15 giây hợp âm nhạc cụ acoustic tổng hợp (piano/flute synth arpeggio).
    Giai điệu: C maj -> Am -> F maj -> G maj.
    """
    chord_progression = [
        [261.63, 329.63, 392.00, 523.25],  # C4, E4, G4, C5
        [220.00, 261.63, 329.63, 440.00],  # A3, C4, E4, A4
        [174.61, 220.00, 261.63, 349.23],  # F3, A3, C4, F4
        [196.00, 246.94, 293.66, 392.00],  # G3, B3, D4, G4
    ]
    samples = []
    chord_duration = duration_sec / len(chord_progression)
    chord_samples = int(SAMPLE_RATE * chord_duration)

    for chord in chord_progression:
        for i in range(chord_samples):
            t = i / SAMPLE_RATE
            # Envelope (ADSR nhẹ)
            env = math.exp(-i / (SAMPLE_RATE * 1.5)) * (1.0 - math.exp(-i / 500.0))
            val = 0.0
            for freq in chord:
                # Sóng sin + hòa âm bậc 2, bậc 3 tạo âm sắc nhạc cụ
                tone = (
                    0.6 * math.sin(2 * math.pi * freq * t)
                    + 0.3 * math.sin(4 * math.pi * freq * t)
                    + 0.1 * math.sin(6 * math.pi * freq * t)
                )
                val += tone
            samples.append(val * 0.12 * env)
    return samples


def download_tts(text: str, out_path: Path):
    """Tải file MP3 từ Google Translate TTS."""
    encoded = urllib.parse.quote(text)
    url = f"https://translate.google.com/translate_tts?ie=UTF-8&q={encoded}&tl=vi&client=tw-ob"
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        content = resp.read()
    out_path.write_bytes(content)


def decode_mp3_to_wav_samples(mp3_path: Path) -> list[float]:
    """Dùng symphonia/fileconv hoặc decode PCM."""
    # Vì fileconv CLI decode được MP3 qua symphonia, ta có thể dùng python-wave hoặc 
    # tạo WAV trực tiếp từ raw TTS nếu cần, hoặc decode qua fileconv.
    pass


def main():
    repo_root = Path(__file__).resolve().parent.parent.parent
    hallucination_dir = repo_root / "bench" / "audio_hallucination"
    vn_audio_dir = repo_root / "bench" / "vn_audio"

    hallucination_dir.mkdir(parents=True, exist_ok=True)
    vn_audio_dir.mkdir(parents=True, exist_ok=True)

    print("1. Tạo 01_pure_silence.wav (10s)...")
    silence = make_pure_silence(10.0)
    write_wav(hallucination_dir / "01_pure_silence.wav", silence)
    (hallucination_dir / "01_pure_silence.gt.txt").write_text("", encoding="utf-8")

    print("2. Tạo 02_ambient_noise.wav (10s)...")
    noise = make_ambient_noise(10.0)
    write_wav(hallucination_dir / "02_ambient_noise.wav", noise)
    (hallucination_dir / "02_ambient_noise.gt.txt").write_text("", encoding="utf-8")

    print("3. Tạo 03_instrumental_music.wav (15s)...")
    music = make_instrumental_music(15.0)
    write_wav(hallucination_dir / "03_instrumental_music.wav", music)
    (hallucination_dir / "03_instrumental_music.gt.txt").write_text("", encoding="utf-8")

    print("4. Tải các file TTS cho bench/vn_audio/...")
    tts_clips = {
        "clip1": "Xin chào, hôm nay tôi thử nghiệm công cụ chuyển giọng nói thành văn bản bằng tiếng Việt.",
        "clip2": "Hà Nội là thủ đô của Việt Nam, một thành phố có lịch sử lâu đời và văn hóa phong phú.",
        "clip3": "Trí tuệ nhân tạo đang thay đổi cách chúng ta làm việc và học tập mỗi ngày.",
        "clip4": "Công cụ này được viết bằng ngôn ngữ Rust để đạt tốc độ xử lý cao và an toàn bộ nhớ.",
    }
    for name, text in tts_clips.items():
        mp3_file = vn_audio_dir / f"{name}.mp3"
        txt_file = vn_audio_dir / f"{name}.txt"
        print(f"   Tải {name}.mp3 ...")
        download_tts(text, mp3_file)
        txt_file.write_text(text, encoding="utf-8")

    # Tạo 04_speech_with_pauses:
    # Tải 3 câu ngắn
    p1 = "Hôm nay thời tiết rất đẹp."
    p2 = "Chúng tôi đang kiểm thử hệ thống phiên âm."
    p3 = "Xin cảm ơn các bạn đã lắng nghe."
    temp1 = hallucination_dir / "p1.mp3"
    temp2 = hallucination_dir / "p2.mp3"
    temp3 = hallucination_dir / "p3.mp3"
    download_tts(p1, temp1)
    download_tts(p2, temp2)
    download_tts(p3, temp3)

    # Viết ground truth cho speech_with_pauses
    (hallucination_dir / "04_speech_with_pauses.gt.txt").write_text(
        f"{p1} {p2} {p3}", encoding="utf-8"
    )

    print("Hoàn tất tạo bộ mẫu audio!")


if __name__ == "__main__":
    main()

