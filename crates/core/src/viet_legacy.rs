//! Giải mã bảng mã tiếng Việt CŨ (pre-Unicode) → Unicode.
//!
//! Hiện hỗ trợ **TCVN3 (ABC)** — bảng mã 1-byte phổ biến nhất trong văn bản hành chính
//! miền Bắc trước ~2005. File .csv/.txt lưu TCVN3 mở bằng tool hiện đại sẽ ra "rác"
//! (vd "Céng hßa" thay vì "Cộng hòa") — không đối thủ nào xử lý
//! (xem bench/RESEARCH_COMPETITORS.md, mục khoảng trống tiếng Việt).
//!
//! Bảng mã đối chiếu từ các converter cộng đồng (gist congkhoa, anhskohbo/u-convert)
//! và cross-check với vietunicode.sourceforge.net (á=0xB8, ă=0xA8, đ=0xAE khớp).
//! Lưu ý: 'ư' là 0xAD (soft-hyphen) — nhiều bảng copy trên web hiển thị sai thành '-'.
//! VNI-Windows/VPS/VISCII: chưa hỗ trợ (backlog — thiếu bảng nguồn tin cậy).

/// TCVN3 byte → ký tự Unicode. Byte không có trong bảng: <0x80 giữ ASCII,
/// còn lại decode theo latin-1 (giữ nguyên hình).
const TCVN3_MAP: &[(u8, char)] = &[
    (0xA1, 'Ă'), (0xA2, 'Â'), (0xA3, 'Ê'), (0xA4, 'Ô'), (0xA5, 'Ơ'), (0xA6, 'Ư'),
    (0xA7, 'Đ'), (0xA8, 'ă'), (0xA9, 'â'), (0xAA, 'ê'), (0xAB, 'ô'), (0xAC, 'ơ'),
    (0xAD, 'ư'), (0xAE, 'đ'), (0xB5, 'à'), (0xB6, 'ả'), (0xB7, 'ã'), (0xB8, 'á'),
    (0xB9, 'ạ'), (0xBB, 'ằ'), (0xBC, 'ẳ'), (0xBD, 'ẵ'), (0xBE, 'ắ'), (0xC6, 'ặ'),
    (0xC7, 'ầ'), (0xC8, 'ẩ'), (0xC9, 'ẫ'), (0xCA, 'ấ'), (0xCB, 'ậ'), (0xCC, 'è'),
    (0xCE, 'ẻ'), (0xCF, 'ẽ'), (0xD0, 'é'), (0xD1, 'ẹ'), (0xD2, 'ề'), (0xD3, 'ể'),
    (0xD4, 'ễ'), (0xD5, 'ế'), (0xD6, 'ệ'), (0xD7, 'ì'), (0xD8, 'ỉ'), (0xDC, 'ĩ'),
    (0xDD, 'í'), (0xDE, 'ị'), (0xDF, 'ò'), (0xE1, 'ỏ'), (0xE2, 'õ'), (0xE3, 'ó'),
    (0xE4, 'ọ'), (0xE5, 'ồ'), (0xE6, 'ổ'), (0xE7, 'ỗ'), (0xE8, 'ố'), (0xE9, 'ộ'),
    (0xEA, 'ờ'), (0xEB, 'ở'), (0xEC, 'ỡ'), (0xED, 'ớ'), (0xEE, 'ợ'), (0xEF, 'ù'),
    (0xF1, 'ủ'), (0xF2, 'ũ'), (0xF3, 'ú'), (0xF4, 'ụ'), (0xF5, 'ừ'), (0xF6, 'ử'),
    (0xF7, 'ữ'), (0xF8, 'ứ'), (0xF9, 'ự'), (0xFA, 'ỳ'), (0xFB, 'ỷ'), (0xFC, 'ỹ'),
    (0xFD, 'ý'), (0xFE, 'ỵ'),
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

/// Decode TCVN3 → String Unicode.
pub fn decode_tcvn3(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match tcvn3_char(b) {
            Some(c) => c,
            None => b as char, // ASCII + latin-1 passthrough
        })
        .collect()
}

/// Decode text bytes "thông minh": UTF-8 → giữ nguyên; TCVN3 → chuyển;
/// còn lại → lossy (giữ hành vi cũ).
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) if looks_like_tcvn3(bytes) => decode_tcvn3(bytes),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // "Cộng hòa xã hội chủ nghĩa Việt Nam" trong TCVN3 (sinh từ bảng, đối chiếu ví dụ
    // gist "hép møt tÕt" → "hộp mứt tết").
    const T1: &[u8] = &[
        0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7, 0x20, 0x68, 0xE9,
        0x69, 0x20, 0x63, 0x68, 0xF1, 0x20, 0x6E, 0x67, 0x68, 0xDC, 0x61, 0x20, 0x56, 0x69,
        0xD6, 0x74, 0x20, 0x4E, 0x61, 0x6D,
    ];

    #[test]
    fn decodes_tcvn3_sentence() {
        assert!(looks_like_tcvn3(T1));
        assert_eq!(decode_tcvn3(T1), "Cộng hòa xã hội chủ nghĩa Việt Nam");
    }

    #[test]
    fn decodes_gist_example() {
        // 'hép møt tÕt' → hộp mứt tết
        let bytes = &[0x68, 0xE9, 0x70, 0x20, 0x6D, 0xF8, 0x74, 0x20, 0x74, 0xD5, 0x74];
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
}
