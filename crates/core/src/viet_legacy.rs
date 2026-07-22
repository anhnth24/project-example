//! Giải mã bảng mã tiếng Việt CŨ (pre-Unicode) → Unicode.
//!
//! Hỗ trợ **TCVN3 (ABC)**, **VNI-Windows** và **VPS** — các bảng mã phổ biến
//! trong văn bản Việt Nam trước Unicode. File .csv/.txt legacy mở bằng tool hiện đại sẽ ra "rác"
//! (vd "Céng hßa" thay vì "Cộng hòa") — không đối thủ nào xử lý
//! (xem bench/RESEARCH_COMPETITORS.md, mục khoảng trống tiếng Việt).
//!
//! Bảng mã đối chiếu từ các converter cộng đồng (gist congkhoa, anhskohbo/u-convert)
//! và cross-check với vietunicode.sourceforge.net (á=0xB8, ă=0xA8, đ=0xAE khớp).
//! Lưu ý: 'ư' là 0xAD (soft-hyphen) — nhiều bảng copy trên web hiển thị sai thành '-'.
//! VNI/VPS maps được sinh từ bảng VietUnicode bằng `bench/generate_viet_legacy_maps.py`.
//!
//! **Hạn chế TCVN3 chữ hoa có dấu:** TCVN 5712-3 / VietUnicode mô tả capital vowels
//! qua font hoa riêng (TCVN-3-1 thường / TCVN-3-2 hoa), không phải digraph byte chắc chắn
//! trong luồng không có metadata font/run. Decode ở đây **không** đoán hoa bằng
//! lookahead base+tone — chỉ map single-byte (base hoa ĂÂÊÔƠƯĐ + chữ thường có dấu).
//! Cần font/run metadata (hoặc bảng TCVN-3-2 tường minh) mới nâng case đúng.

use crate::viet_legacy_maps::{VNI_MAP, VPS_MAP};

/// TCVN3 byte → ký tự Unicode. Byte không có trong bảng: <0x80 giữ ASCII,
/// còn lại decode theo latin-1 (giữ nguyên hình).
const TCVN3_MAP: &[(u8, char)] = &[
    (0xA1, 'Ă'),
    (0xA2, 'Â'),
    (0xA3, 'Ê'),
    (0xA4, 'Ô'),
    (0xA5, 'Ơ'),
    (0xA6, 'Ư'),
    (0xA7, 'Đ'),
    (0xA8, 'ă'),
    (0xA9, 'â'),
    (0xAA, 'ê'),
    (0xAB, 'ô'),
    (0xAC, 'ơ'),
    (0xAD, 'ư'),
    (0xAE, 'đ'),
    (0xB5, 'à'),
    (0xB6, 'ả'),
    (0xB7, 'ã'),
    (0xB8, 'á'),
    (0xB9, 'ạ'),
    (0xBB, 'ằ'),
    (0xBC, 'ẳ'),
    (0xBD, 'ẵ'),
    (0xBE, 'ắ'),
    (0xC6, 'ặ'),
    (0xC7, 'ầ'),
    (0xC8, 'ẩ'),
    (0xC9, 'ẫ'),
    (0xCA, 'ấ'),
    (0xCB, 'ậ'),
    (0xCC, 'è'),
    (0xCE, 'ẻ'),
    (0xCF, 'ẽ'),
    (0xD0, 'é'),
    (0xD1, 'ẹ'),
    (0xD2, 'ề'),
    (0xD3, 'ể'),
    (0xD4, 'ễ'),
    (0xD5, 'ế'),
    (0xD6, 'ệ'),
    (0xD7, 'ì'),
    (0xD8, 'ỉ'),
    (0xDC, 'ĩ'),
    (0xDD, 'í'),
    (0xDE, 'ị'),
    (0xDF, 'ò'),
    (0xE1, 'ỏ'),
    (0xE2, 'õ'),
    (0xE3, 'ó'),
    (0xE4, 'ọ'),
    (0xE5, 'ồ'),
    (0xE6, 'ổ'),
    (0xE7, 'ỗ'),
    (0xE8, 'ố'),
    (0xE9, 'ộ'),
    (0xEA, 'ờ'),
    (0xEB, 'ở'),
    (0xEC, 'ỡ'),
    (0xED, 'ớ'),
    (0xEE, 'ợ'),
    (0xEF, 'ù'),
    (0xF1, 'ủ'),
    (0xF2, 'ũ'),
    (0xF3, 'ú'),
    (0xF4, 'ụ'),
    (0xF5, 'ừ'),
    (0xF6, 'ử'),
    (0xF7, 'ữ'),
    (0xF8, 'ứ'),
    (0xF9, 'ự'),
    (0xFA, 'ỳ'),
    (0xFB, 'ỷ'),
    (0xFC, 'ỹ'),
    (0xFD, 'ý'),
    (0xFE, 'ỵ'),
];

fn tcvn3_char(b: u8) -> Option<char> {
    TCVN3_MAP
        .binary_search_by_key(&b, |&(k, _)| k)
        .ok()
        .map(|i| TCVN3_MAP[i].1)
}

/// Đoán dữ liệu có phải TCVN3 không.
/// Điều kiện: KHÔNG phải UTF-8 hợp lệ, và phần lớn (≥70%) byte >0x7F nằm trong
/// bảng TCVN3, với ít nhất 3 byte như vậy (tránh nhận nhầm nhiễu ngắn).
pub fn looks_like_tcvn3(bytes: &[u8]) -> bool {
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let (mut high, mut hit) = (0usize, 0usize);
    for &b in bytes {
        if b > 0x7F {
            high += 1;
            if tcvn3_char(b).is_some() {
                hit += 1;
            }
        }
    }
    high >= 3 && hit * 10 >= high * 7
}

/// Decode TCVN3 → String Unicode (single-byte map; xem hạn chế chữ hoa có dấu ở đầu module).
pub fn decode_tcvn3(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match tcvn3_char(b) {
            Some(c) => c,
            None => b as char, // ASCII + latin-1 passthrough
        })
        .collect()
}

fn vni_match(bytes: &[u8]) -> Option<(char, usize)> {
    VNI_MAP
        .iter()
        .find(|(encoded, _)| bytes.starts_with(encoded))
        .map(|(encoded, character)| (*character, encoded.len()))
}

fn vps_char(byte: u8) -> Option<char> {
    VPS_MAP
        .binary_search_by_key(&byte, |&(key, _)| key)
        .ok()
        .map(|index| VPS_MAP[index].1)
}

/// VNI-Windows dùng cả sequence hai byte và một byte.
pub fn decode_vni(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some((character, consumed)) = vni_match(&bytes[index..]) {
            output.push(character);
            index += consumed;
        } else {
            output.push(bytes[index] as char);
            index += 1;
        }
    }
    output
}

/// VPS là bảng mã một byte; một số chữ hoa dùng cả vùng control 0x02–0x1D.
pub fn decode_vps(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| vps_char(byte).unwrap_or(byte as char))
        .collect()
}

fn vni_score(bytes: &[u8]) -> (usize, usize, usize) {
    let mut index = 0usize;
    let mut hits = 0usize;
    let mut digraphs = 0usize;
    let suspicious = bytes.iter().filter(|byte| **byte >= 0x80).count();
    while index < bytes.len() {
        if let Some((_, consumed)) = vni_match(&bytes[index..]) {
            hits += 1;
            digraphs += usize::from(consumed == 2);
            index += consumed;
        } else {
            index += 1;
        }
    }
    (hits, suspicious, digraphs)
}

fn vps_score(bytes: &[u8]) -> (usize, usize, usize) {
    let mut hits = 0usize;
    let mut controls = 0usize;
    let mut suspicious = 0usize;
    for &byte in bytes {
        let is_control_letter = byte < 0x20 && vps_char(byte).is_some();
        if byte >= 0x80 || is_control_letter {
            suspicious += 1;
            if vps_char(byte).is_some() {
                hits += 1;
                controls += usize::from(is_control_letter || (0x80..=0x9F).contains(&byte));
            }
        }
    }
    (hits, suspicious, controls)
}

pub fn looks_like_vni(bytes: &[u8]) -> bool {
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let (hits, suspicious, digraphs) = vni_score(bytes);
    hits >= 3 && digraphs >= 2 && hits * 10 >= suspicious.max(1) * 7
}

pub fn looks_like_vps(bytes: &[u8]) -> bool {
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let (hits, suspicious, distinctive) = vps_score(bytes);
    hits >= 3 && hits * 10 >= suspicious.max(1) * 7 && (distinctive > 0 || !looks_like_tcvn3(bytes))
}

/// Decode text bytes "thông minh": UTF-8 → giữ nguyên; VNI/VPS/TCVN3 → chuyển;
/// còn lại → lossy (giữ hành vi cũ).
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) if looks_like_vni(bytes) => decode_vni(bytes),
        Err(_) if vps_score(bytes).2 > 0 && looks_like_vps(bytes) => decode_vps(bytes),
        Err(_) if looks_like_tcvn3(bytes) => decode_tcvn3(bytes),
        Err(_) if looks_like_vps(bytes) => decode_vps(bytes),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // "Cộng hòa xã hội chủ nghĩa Việt Nam" trong TCVN3 (sinh từ bảng, đối chiếu ví dụ
    // gist "hép møt tÕt" → "hộp mứt tết").
    const T1: &[u8] = &[
        0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7, 0x20, 0x68, 0xE9, 0x69,
        0x20, 0x63, 0x68, 0xF1, 0x20, 0x6E, 0x67, 0x68, 0xDC, 0x61, 0x20, 0x56, 0x69, 0xD6, 0x74,
        0x20, 0x4E, 0x61, 0x6D,
    ];

    const LEGACY_SENTENCE: &str = "Cộng hòa xã hội chủ nghĩa Việt Nam";

    fn encode_vni(text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for character in text.chars() {
            if character.is_ascii() {
                bytes.push(character as u8);
            } else {
                let (encoded, _) = VNI_MAP
                    .iter()
                    .find(|(_, mapped)| *mapped == character)
                    .unwrap_or_else(|| panic!("missing VNI mapping for {character}"));
                bytes.extend_from_slice(encoded);
            }
        }
        bytes
    }

    fn encode_vps(text: &str) -> Vec<u8> {
        text.chars()
            .map(|character| {
                if character.is_ascii() {
                    character as u8
                } else {
                    VPS_MAP
                        .iter()
                        .find(|(_, mapped)| *mapped == character)
                        .map(|(encoded, _)| *encoded)
                        .unwrap_or_else(|| panic!("missing VPS mapping for {character}"))
                }
            })
            .collect()
    }

    #[test]
    fn decodes_tcvn3_sentence() {
        assert!(looks_like_tcvn3(T1));
        assert_eq!(decode_tcvn3(T1), "Cộng hòa xã hội chủ nghĩa Việt Nam");
    }

    #[test]
    fn decodes_gist_example() {
        // 'hép møt tÕt' → hộp mứt tết
        let bytes = &[
            0x68, 0xE9, 0x70, 0x20, 0x6D, 0xF8, 0x74, 0x20, 0x74, 0xD5, 0x74,
        ];
        assert_eq!(decode_tcvn3(bytes), "hộp mứt tết");
    }

    #[test]
    fn ascii_hyphen_not_mapped() {
        // 0x2D ('-') phải giữ nguyên — không phải 'ư' (lỗi bảng copy trên web).
        assert_eq!(decode_tcvn3(b"a-b"), "a-b");
    }

    #[test]
    fn utf8_not_flagged() {
        assert!(!looks_like_tcvn3("tiếng Việt UTF-8 bình thường".as_bytes()));
    }

    #[test]
    fn decode_text_routes() {
        assert_eq!(decode_text(T1), "Cộng hòa xã hội chủ nghĩa Việt Nam");
        assert_eq!(decode_text("đã là utf8".as_bytes()), "đã là utf8");
    }

    #[test]
    fn decodes_and_detects_vni_windows_digraphs() {
        let bytes = encode_vni(LEGACY_SENTENCE);
        assert!(looks_like_vni(&bytes));
        assert_eq!(decode_vni(&bytes), LEGACY_SENTENCE);
        assert_eq!(decode_text(&bytes), LEGACY_SENTENCE);
    }

    #[test]
    fn decodes_and_detects_vps_control_and_high_bytes() {
        let bytes = encode_vps(LEGACY_SENTENCE);
        assert!(looks_like_vps(&bytes));
        assert_eq!(decode_vps(&bytes), LEGACY_SENTENCE);
        assert_eq!(decode_text(&bytes), LEGACY_SENTENCE);
    }

    #[test]
    fn detectors_do_not_claim_utf8_or_cross_route_vni() {
        let utf8 = LEGACY_SENTENCE.as_bytes();
        assert!(!looks_like_vni(utf8));
        assert!(!looks_like_vps(utf8));
        let vni = encode_vni(LEGACY_SENTENCE);
        assert_eq!(decode_text(&vni), LEGACY_SENTENCE);
    }

    #[test]
    fn converter_routes_legacy_txt_through_decoder() {
        let path =
            std::env::temp_dir().join(format!("fileconv_vni_legacy_{}.txt", std::process::id()));
        std::fs::write(&path, encode_vni(LEGACY_SENTENCE)).unwrap();
        let result = crate::Converter::new().convert_path(&path).unwrap();
        assert_eq!(result.format, crate::FormatKind::Text);
        assert_eq!(result.markdown, LEGACY_SENTENCE);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn does_not_guess_tcvn3_uppercase_via_base_tone_lookahead() {
        // Không có font metadata: A + 0xB8 phải ra "Aá", không bị ghép thành "Á".
        assert_eq!(decode_tcvn3(&[0x41, 0xB8]), "Aá");
        assert_eq!(decode_tcvn3(&[0xA1, 0xA2, 0xA7]), "ĂÂĐ");
    }
}
