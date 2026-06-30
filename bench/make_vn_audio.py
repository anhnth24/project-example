#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Tạo audio tiếng Việt (gTTS) kèm ground-truth để đo WER của whisper.

Lưu ý: gTTS là giọng TTS tổng hợp (rõ ràng) — không phải giọng người tự nhiên.
Dùng làm baseline có ground-truth chính xác. Giọng người thật (vd Common Voice)
sẽ khó hơn; cần mẫu có nhãn để đo.
"""
import os
from gtts import gTTS

OUT = os.path.join(os.path.dirname(__file__), "vn_audio")
os.makedirs(OUT, exist_ok=True)

CLIPS = {
    "clip1": "Xin chào, hôm nay tôi thử nghiệm công cụ chuyển giọng nói thành văn bản bằng tiếng Việt.",
    "clip2": "Hà Nội là thủ đô của Việt Nam, một thành phố có lịch sử lâu đời và văn hóa phong phú.",
    "clip3": "Trí tuệ nhân tạo đang thay đổi cách chúng ta làm việc và học tập mỗi ngày.",
    "clip4": "Công cụ này được viết bằng ngôn ngữ Rust để đạt tốc độ xử lý cao và an toàn bộ nhớ.",
}

for name, text in CLIPS.items():
    mp3 = os.path.join(OUT, f"{name}.mp3")
    txt = os.path.join(OUT, f"{name}.txt")
    gTTS(text=text, lang="vi").save(mp3)
    with open(txt, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"  {name}: {os.path.getsize(mp3)} bytes")

print("Xong audio tiếng Việt.")
