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
//! qua font hoa riêng (TCVN3/VN3 regular font vs TCVN3/ABC all-capital H-font),
//! không phải digraph byte chắc chắn trong luồng không có metadata font/run.
//! Decode mặc định **không** đoán hoa bằng lookahead base+tone — chỉ map
//! single-byte (base hoa ĂÂÊÔƠƯĐ + chữ thường có dấu).
//!
//! **Opt-in font/run case hint (C11):** caller đã có metadata TCVN3/ABC
//! all-capital H-font (ví dụ `.Vn*H`, `w:rFonts`) thì dùng
//! [`Tcvn3CaseHint::UppercaseFont`] với [`decode_tcvn3_with_hint`]: decode bảng
//! single-byte hiện có, rồi áp dụng Unicode uppercase xác định. **Không** bịa
//! digraph byte hoa. [`decode_tcvn3`] / [`decode_text`] giữ semantics cũ
//! (an toàn khi thiếu metadata).
//!
//! **TXT/CSV thuần** không suy ra H-font đáng tin — cùng byte vừa có thể là chữ
//! thường vừa là glyph hoa tùy font đã mất; không đoán từ tên file hay nội dung.

use crate::viet_legacy_maps::{VNI_MAP, VPS_MAP};

/// Hint font/run cho decode TCVN3 sau bước map single-byte canonical.
///
/// TCVN3 không có bảng byte riêng cho nguyên âm hoa có dấu; TCVN3/ABC all-capital
/// H-font (`.Vn*H`) dùng cùng byte với chữ thường rồi vẽ glyph hoa. Chỉ bật
/// [`Tcvn3CaseHint::UppercaseFont`] khi caller **đã có** metadata tường minh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tcvn3CaseHint {
    /// TCVN3/VN3 regular font / mixed / không metadata: giữ map single-byte.
    #[default]
    AsMapped,
    /// TCVN3/ABC all-capital H-font run: Unicode uppercase sau decode.
    UppercaseFont,
}

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

/// Decode TCVN3 → String Unicode (single-byte map; [`Tcvn3CaseHint::AsMapped`]).
///
/// Không đổi case theo font. Xem [`decode_tcvn3_with_hint`] khi có metadata
/// TCVN3/ABC all-capital H-font.
pub fn decode_tcvn3(bytes: &[u8]) -> String {
    decode_tcvn3_with_hint(bytes, Tcvn3CaseHint::AsMapped)
}

/// Decode TCVN3 với hint font/run tường minh.
///
/// Luôn map single-byte canonical trước; với [`Tcvn3CaseHint::UppercaseFont`] áp dụng
/// Unicode uppercase trên chuỗi đã decode. Không lookahead base+tone, không digraph hoa.
pub fn decode_tcvn3_with_hint(bytes: &[u8], hint: Tcvn3CaseHint) -> String {
    let decoded: String = bytes
        .iter()
        .map(|&b| match tcvn3_char(b) {
            Some(c) => c,
            None => b as char, // ASCII + latin-1 passthrough
        })
        .collect();
    apply_tcvn3_case_hint(&decoded, hint)
}

/// Áp dụng [`Tcvn3CaseHint`] lên chuỗi **đã** decode TCVN3 canonical.
pub fn apply_tcvn3_case_hint(decoded: &str, hint: Tcvn3CaseHint) -> String {
    match hint {
        Tcvn3CaseHint::AsMapped => decoded.to_string(),
        Tcvn3CaseHint::UppercaseFont => decoded.to_uppercase(),
    }
}

/// Style-only tokens stripped from the **end** before H-font detection.
///
/// Width/weight tokens that commonly glue to terminal `H` (`HeavyH`, `NarrowH`,
/// `LightH`, …) are intentionally **not** listed — they stay on the last family
/// token so `… HeavyH Normal` still reads as all-capital H-font.
const ABC_VN_STYLE_SUFFIXES: &[&str] = &[
    "bold", "italic", "regular", "normal", "medium", "oblique", "thin", "semibold", "demibold",
    "black",
];

/// Family names that look like `.Vn*` but are not TCVN3/ABC text fonts.
const ABC_VN_REJECTED_FAMILIES: &[&str] = &[
    "post", // VNPost / .VnPost
    "vps",  // cross-encoding
    "vni",  // cross-encoding (also caught by `.vni` prefix guard)
];

/// Phân loại font TCVN3/ABC `.Vn*` → hint case (terminal `H` = all-capital H-font).
///
/// **Bắt buộc** prefix case-insensitive `.Vn` (có dấu chấm). Dotless `VnTimeH`,
/// VNI/VPS, `.VnTimes2`, `.VnPost` và tên cross-encoding → `None`.
/// CSS `font-family` lấy family đầu trước dấu phẩy.
///
/// H-font: sau khi bỏ style suffix cuối (`Bold`/`Italic`/`Normal`/…), token họ
/// cuối cùng kết thúc bằng `H` (ví dụ `.VNTimeH`, `.VnArial NarrowH`,
/// `.VnTifani HeavyH Normal`).
pub fn tcvn3_case_hint_from_font_name(font_name: &str) -> Option<Tcvn3CaseHint> {
    let primary = first_css_font_family(font_name)?;
    let lower = primary.to_ascii_lowercase();
    // Dot required; case-insensitive `.Vn`. Reject `.VNI…` (would be `.vn` + `i…`).
    let body = lower.strip_prefix(".vn")?;
    if body.is_empty() || body.starts_with('i') {
        return None;
    }

    let mut tokens: Vec<&str> = body.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return None;
    }
    for token in &tokens {
        if !token.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
            // Digits (`.VnTimes2`) and other punctuation → reject.
            return None;
        }
        let compact: String = token.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if compact.is_empty() {
            return None;
        }
        if ABC_VN_REJECTED_FAMILIES
            .iter()
            .any(|bad| compact.eq_ignore_ascii_case(bad))
        {
            return None;
        }
    }

    while let Some(last) = tokens.last() {
        if ABC_VN_STYLE_SUFFIXES
            .iter()
            .any(|style| last.eq_ignore_ascii_case(style))
        {
            tokens.pop();
        } else {
            break;
        }
    }
    if tokens.is_empty() {
        return None;
    }

    let last = *tokens.last()?;
    let last_alpha: String = last
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // Terminal H on the last remaining family/width token ⇒ all-capital H-font.
    if last_alpha.len() > 1 && last_alpha.ends_with('h') {
        Some(Tcvn3CaseHint::UppercaseFont)
    } else {
        Some(Tcvn3CaseHint::AsMapped)
    }
}

fn first_css_font_family(name: &str) -> Option<String> {
    let primary = name.split(',').next()?.trim();
    let unquoted = primary.trim_matches(|c| c == '"' || c == '\'').trim();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
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
///
/// Nhánh TCVN3 dùng [`decode_tcvn3`] ([`Tcvn3CaseHint::AsMapped`]). Không có
/// smart-decoder có hint: TXT/CSV không suy H-font đáng tin; caller có metadata
/// font/run gọi [`decode_tcvn3_with_hint`] trực tiếp.
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
        assert_eq!(
            decode_tcvn3_with_hint(&[0x41, 0xB8], Tcvn3CaseHint::AsMapped),
            "Aá"
        );
        // Hint hoa cũng không merge A+tone thành một grapheme — chỉ uppercase từng
        // char đã map: "Aá" → "AÁ".
        assert_eq!(
            decode_tcvn3_with_hint(&[0x41, 0xB8], Tcvn3CaseHint::UppercaseFont),
            "AÁ"
        );
    }

    /// Mọi chữ thường có dấu trong TCVN3_MAP (+ đ) — kỳ vọng Unicode uppercase đủ gồm Đ.
    const TCVN3_LOWER_ACCENTED: &str =
        "ăâêôơưđàảãáạằẳẵắặầẩẫấậèẻẽéẹềểễếệìỉĩíịòỏõóọồổỗốộờởỡớợùủũúụừửữứựỳỷỹýỵ";
    const TCVN3_UPPER_ACCENTED: &str =
        "ĂÂÊÔƠƯĐÀẢÃÁẠẰẲẴẮẶẦẨẪẤẬÈẺẼÉẸỀỂỄẾỆÌỈĨÍỊÒỎÕÓỌỒỔỖỐỘỜỞỠỚỢÙỦŨÚỤỪỬỮỨỰỲỶỸÝỴ";

    fn tcvn3_bytes_for_lower_accented() -> Vec<u8> {
        TCVN3_LOWER_ACCENTED
            .chars()
            .map(|ch| {
                TCVN3_MAP
                    .iter()
                    .find(|(_, mapped)| *mapped == ch)
                    .map(|(b, _)| *b)
                    .unwrap_or_else(|| panic!("missing TCVN3 byte for {ch}"))
            })
            .collect()
    }

    #[test]
    fn default_decode_preserves_lowercase_accented_without_hint() {
        let bytes = tcvn3_bytes_for_lower_accented();
        assert_eq!(decode_tcvn3(&bytes), TCVN3_LOWER_ACCENTED);
        assert_eq!(decode_text(&bytes), TCVN3_LOWER_ACCENTED);
        assert_eq!(
            decode_tcvn3_with_hint(&bytes, Tcvn3CaseHint::AsMapped),
            TCVN3_LOWER_ACCENTED
        );
        assert_ne!(decode_tcvn3(&bytes), TCVN3_UPPER_ACCENTED);
    }

    #[test]
    fn uppercase_font_hint_recovers_all_vietnamese_base_and_tone_combos() {
        let bytes = tcvn3_bytes_for_lower_accented();
        assert_eq!(
            decode_tcvn3_with_hint(&bytes, Tcvn3CaseHint::UppercaseFont),
            TCVN3_UPPER_ACCENTED
        );
        assert!(TCVN3_UPPER_ACCENTED.contains('Đ'));
        assert_eq!(
            apply_tcvn3_case_hint("đường", Tcvn3CaseHint::UppercaseFont),
            "ĐƯỜNG"
        );
        // Base hoa đã có trong map (ĂÂÊÔƠƯĐ) giữ nguyên khi uppercase.
        assert_eq!(
            decode_tcvn3_with_hint(&[0xA1, 0xA2, 0xA7], Tcvn3CaseHint::UppercaseFont),
            "ĂÂĐ"
        );
    }

    #[test]
    fn uppercase_font_hint_ascii_behavior() {
        assert_eq!(
            decode_tcvn3_with_hint(b"Abc-123", Tcvn3CaseHint::AsMapped),
            "Abc-123"
        );
        assert_eq!(
            decode_tcvn3_with_hint(b"Abc-123", Tcvn3CaseHint::UppercaseFont),
            "ABC-123"
        );
        // Soft-hyphen byte 0xAD là 'ư' trong TCVN3 — không phải ASCII '-'.
        assert_eq!(decode_tcvn3(b"a-b"), "a-b");
        assert_eq!(
            decode_tcvn3_with_hint(b"a-b", Tcvn3CaseHint::UppercaseFont),
            "A-B"
        );
    }

    #[test]
    fn font_name_helper_table_accepts_h_fonts_and_rejects_cross_encoding() {
        let cases: &[(&str, Option<Tcvn3CaseHint>)] = &[
            // Regular TCVN3/VN3 fonts (dot + .Vn required, case-insensitive).
            (".VnTime", Some(Tcvn3CaseHint::AsMapped)),
            (".vntime", Some(Tcvn3CaseHint::AsMapped)),
            (".VnTime Bold Italic", Some(Tcvn3CaseHint::AsMapped)),
            (".VnArial Narrow", Some(Tcvn3CaseHint::AsMapped)),
            // TCVN3/ABC all-capital H-fonts — terminal H before style suffixes.
            (".VnTimeH", Some(Tcvn3CaseHint::UppercaseFont)),
            (".VNTimeH", Some(Tcvn3CaseHint::UppercaseFont)),
            (".VnArialH Bold", Some(Tcvn3CaseHint::UppercaseFont)),
            (".VnArial NarrowH", Some(Tcvn3CaseHint::UppercaseFont)),
            (
                ".VnTifani HeavyH Normal",
                Some(Tcvn3CaseHint::UppercaseFont),
            ),
            (r#"".VnTimeH", serif"#, Some(Tcvn3CaseHint::UppercaseFont)),
            // Dot required — reject dotless / non-.Vn names.
            ("VnTimeH", None),
            ("VnTime", None),
            ("SomethingH", None),
            ("Arial", None),
            ("Times New Roman", None),
            // VNI / VPS / Post / digit / cross-encoding.
            ("VNI-Times", None),
            (".VNI-Times", None),
            (".VNI Times", None),
            ("VPS-Times", None),
            (".VPS-Times", None),
            ("VNPost", None),
            (".VnPost", None),
            (".VNPost", None),
            (".VnTimes2", None),
            (".VnTime2H", None),
        ];
        for &(name, expected) in cases {
            assert_eq!(
                tcvn3_case_hint_from_font_name(name),
                expected,
                "font name {name:?}"
            );
        }
    }

    #[test]
    fn apply_tcvn3_case_hint_nfc_nfd_equivalent() {
        use unicode_normalization::UnicodeNormalization;
        let nfc = "đường phố";
        let nfd: String = nfc.nfd().collect();
        assert_ne!(nfc.as_bytes(), nfd.as_bytes());
        let upper_nfc = apply_tcvn3_case_hint(nfc, Tcvn3CaseHint::UppercaseFont);
        let upper_nfd = apply_tcvn3_case_hint(&nfd, Tcvn3CaseHint::UppercaseFont);
        let norm = |s: &str| s.nfc().collect::<String>();
        assert_eq!(norm(&upper_nfc), norm(&upper_nfd));
        assert_eq!(norm(&upper_nfc), "ĐƯỜNG PHỐ");
        assert_eq!(apply_tcvn3_case_hint(nfc, Tcvn3CaseHint::AsMapped), nfc);
    }
}
