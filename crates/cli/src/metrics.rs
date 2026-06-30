//! Đo độ chính xác: chuẩn hoá text + CER/WER bằng khoảng cách Levenshtein.

/// Chuẩn hoá: gộp mọi khoảng trắng/xuống dòng thành 1 space, trim.
/// Giữ nguyên chữ hoa/thường và dấu tiếng Việt (để phản ánh độ chính xác thật).
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Khoảng cách Levenshtein tổng quát trên slice (dùng cho cả char và word).
fn levenshtein<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Character Error Rate = lev(ref_chars, hyp_chars) / len(ref_chars).
pub fn cer(reference: &str, hyp: &str) -> f64 {
    let r: Vec<char> = reference.chars().collect();
    let h: Vec<char> = hyp.chars().collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    levenshtein(&r, &h) as f64 / r.len() as f64
}

/// Word Error Rate = lev(ref_words, hyp_words) / len(ref_words).
pub fn wer(reference: &str, hyp: &str) -> f64 {
    let r: Vec<&str> = reference.split(' ').filter(|w| !w.is_empty()).collect();
    let h: Vec<&str> = hyp.split(' ').filter(|w| !w.is_empty()).collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    levenshtein(&r, &h) as f64 / r.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cer_perfect() {
        assert_eq!(cer("xin chào", "xin chào"), 0.0);
    }

    #[test]
    fn cer_vietnamese_diacritics() {
        // mất dấu: "chào" -> "chao" = 1 ký tự sai / 8
        let c = cer("xin chào", "xin chao");
        assert!((c - 1.0 / 8.0).abs() < 1e-9);
    }

    #[test]
    fn wer_one_wrong() {
        assert!((wer("a b c d", "a b x d") - 0.25).abs() < 1e-9);
    }
}
