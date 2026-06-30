//! Bench harness cho backend fileconv-core.
//!
//! Lệnh:
//!   fileconv speed <corpus_dir> [report.md]   - đo tốc độ theo file & page
//!   fileconv accuracy <manifest> [report.md]  - đo độ chính xác CER/WER vs ground truth
//!   fileconv one <file>                        - convert 1 file, in markdown ra stdout
//!
//! Manifest accuracy: mỗi dòng "<đường_dẫn_file>\t<đường_dẫn_text_chuẩn>\t<nhãn_kịch_bản>".

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use fileconv_core::{Converter, FormatKind};
use walkdir::WalkDir;

mod metrics;

fn main() -> Result<()> {
    // Panic hook gọn: pdf-extract có thể panic; ta đã catch_unwind nên chỉ cần
    // một dòng ngắn thay vì backtrace dài.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        eprintln!("[panic đã bắt] {loc}");
    }));

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("dùng: fileconv <speed|accuracy|one> ...");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "speed" => {
            let dir = args.get(2).context("thiếu corpus_dir")?;
            let out = args.get(3).map(PathBuf::from);
            cmd_speed(Path::new(dir), out.as_deref())
        }
        "accuracy" => {
            let manifest = args.get(2).context("thiếu manifest")?;
            let out = args.get(3).map(PathBuf::from);
            cmd_accuracy(Path::new(manifest), out.as_deref())
        }
        "one" => {
            let f = args.get(2).context("thiếu file")?;
            let conv = Converter::new();
            let r = conv.convert_path(Path::new(f))?;
            println!("{}", r.markdown);
            Ok(())
        }
        other => bail!("lệnh không hợp lệ: {other}"),
    }
}

// ----------------------------- SPEED -----------------------------

struct SpeedRow {
    file: String,
    format: FormatKind,
    bytes: u64,
    pages: Option<u32>,
    ms: f64,
    out_chars: usize,
    ok: bool,
    err: Option<String>,
}

fn cmd_speed(dir: &Path, out: Option<&Path>) -> Result<()> {
    let conv = Converter::new();
    let mut rows: Vec<SpeedRow> = Vec::new();

    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();

    for f in &files {
        let fmt = FormatKind::from_path(f);
        if fmt == FormatKind::Unknown {
            continue;
        }
        let bytes = fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        let pages = count_pages(f, fmt);

        let t0 = Instant::now();
        let res = conv.convert_path(f);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;

        let (ok, out_chars, err) = match res {
            Ok(r) => (true, r.markdown.chars().count(), None),
            Err(e) => (false, 0, Some(e.to_string())),
        };
        rows.push(SpeedRow {
            file: rel(dir, f),
            format: fmt,
            bytes,
            pages,
            ms,
            out_chars,
            ok,
            err,
        });
    }

    let report = render_speed_report(&rows);
    print!("{report}");
    if let Some(p) = out {
        fs::write(p, &report).with_context(|| format!("ghi {p:?}"))?;
        eprintln!("\n[đã ghi báo cáo: {}]", p.display());
    }
    Ok(())
}

fn render_speed_report(rows: &[SpeedRow]) -> String {
    let mut s = String::new();
    s.push_str("# Báo cáo TỐC ĐỘ — fileconv-core backend\n\n");

    s.push_str("## Chi tiết từng file\n\n");
    s.push_str("| File | Loại | KB | Pages | Thời gian (ms) | ms/page | KB/s | Ký tự MD | OK |\n");
    s.push_str("|---|---|--:|--:|--:|--:|--:|--:|:-:|\n");
    for r in rows {
        let kb = r.bytes as f64 / 1024.0;
        let kbs = if r.ms > 0.0 { kb / (r.ms / 1000.0) } else { 0.0 };
        let mspp = match r.pages {
            Some(p) if p > 0 => format!("{:.2}", r.ms / p as f64),
            _ => "—".into(),
        };
        let pages = r.pages.map(|p| p.to_string()).unwrap_or_else(|| "—".into());
        s.push_str(&format!(
            "| {} | {} | {:.0} | {} | {:.2} | {} | {:.0} | {} | {} |\n",
            r.file,
            r.format.as_str(),
            kb,
            pages,
            r.ms,
            mspp,
            kbs,
            r.out_chars,
            if r.ok { "✓" } else { "✗" }
        ));
    }

    s.push_str("\n## Tổng hợp theo định dạng\n\n");
    s.push_str("| Loại | Số file | Thành công | Σ KB | Σ pages | Σ ms | ms/file (TB) | ms/page (TB) | KB/s (TB) |\n");
    s.push_str("|---|--:|--:|--:|--:|--:|--:|--:|--:|\n");

    let mut by_fmt: BTreeMap<&str, Vec<&SpeedRow>> = BTreeMap::new();
    for r in rows {
        by_fmt.entry(r.format.as_str()).or_default().push(r);
    }
    for (fmt, rs) in &by_fmt {
        let n = rs.len();
        let ok = rs.iter().filter(|r| r.ok).count();
        let sum_kb: f64 = rs.iter().map(|r| r.bytes as f64 / 1024.0).sum();
        let sum_pages: u32 = rs.iter().filter_map(|r| r.pages).sum();
        let sum_ms: f64 = rs.iter().filter(|r| r.ok).map(|r| r.ms).sum();
        let ms_per_file = if ok > 0 { sum_ms / ok as f64 } else { 0.0 };
        let ms_per_page = if sum_pages > 0 {
            format!("{:.2}", sum_ms / sum_pages as f64)
        } else {
            "—".into()
        };
        let kbs = if sum_ms > 0.0 { sum_kb / (sum_ms / 1000.0) } else { 0.0 };
        s.push_str(&format!(
            "| {} | {} | {} | {:.0} | {} | {:.1} | {:.2} | {} | {:.0} |\n",
            fmt,
            n,
            ok,
            sum_kb,
            if sum_pages > 0 { sum_pages.to_string() } else { "—".into() },
            sum_ms,
            ms_per_file,
            ms_per_page,
            kbs
        ));
    }

    let errs: Vec<&SpeedRow> = rows.iter().filter(|r| !r.ok).collect();
    if !errs.is_empty() {
        s.push_str("\n## File lỗi\n\n");
        for r in errs {
            s.push_str(&format!(
                "- `{}` ({}): {}\n",
                r.file,
                r.format.as_str(),
                r.err.as_deref().unwrap_or("?")
            ));
        }
    }
    s.push('\n');
    s
}

// ----------------------------- ACCURACY -----------------------------

struct AccRow {
    file: String,
    label: String,
    cer: f64,
    wer: f64,
    acc: f64,
    ref_chars: usize,
    hyp_chars: usize,
    ms: f64,
}

fn cmd_accuracy(manifest: &Path, out: Option<&Path>) -> Result<()> {
    let conv = Converter::new();
    let text = fs::read_to_string(manifest).with_context(|| format!("đọc {manifest:?}"))?;
    let base = manifest.parent().unwrap_or(Path::new("."));
    let mut rows: Vec<AccRow> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            eprintln!("bỏ qua dòng sai định dạng: {line}");
            continue;
        }
        let file = resolve(base, parts[0]);
        let gt_path = resolve(base, parts[1]);
        let label = parts.get(2).copied().unwrap_or("").to_string();

        let reference = fs::read_to_string(&gt_path)
            .with_context(|| format!("đọc ground-truth {gt_path:?}"))?;
        let t0 = Instant::now();
        let hyp = match conv.convert_path(&file) {
            Ok(r) => r.markdown,
            Err(e) => {
                eprintln!("convert lỗi {}: {e}", file.display());
                String::new()
            }
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;

        let r_norm = metrics::normalize(&reference);
        let h_norm = metrics::normalize(&hyp);
        let cer = metrics::cer(&r_norm, &h_norm);
        let wer = metrics::wer(&r_norm, &h_norm);
        rows.push(AccRow {
            file: file.file_name().unwrap().to_string_lossy().into_owned(),
            label,
            cer,
            wer,
            acc: (1.0 - cer).max(0.0) * 100.0,
            ref_chars: r_norm.chars().count(),
            hyp_chars: h_norm.chars().count(),
            ms,
        });
    }

    let report = render_accuracy_report(&rows);
    print!("{report}");
    if let Some(p) = out {
        fs::write(p, &report)?;
        eprintln!("\n[đã ghi báo cáo: {}]", p.display());
    }
    Ok(())
}

fn render_accuracy_report(rows: &[AccRow]) -> String {
    let mut s = String::new();
    s.push_str("# Báo cáo ĐỘ CHÍNH XÁC (tiếng Việt) — fileconv-core\n\n");
    s.push_str("Độ chính xác ký tự = (1 − CER)×100. CER/WER tính bằng khoảng cách Levenshtein \
                trên text đã chuẩn hoá khoảng trắng.\n\n");
    s.push_str("| File | Kịch bản | Ref ký tự | Hyp ký tự | CER | WER | Độ chính xác % | ms |\n");
    s.push_str("|---|---|--:|--:|--:|--:|--:|--:|\n");
    for r in rows {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.3} | {:.3} | **{:.1}%** | {:.1} |\n",
            r.file, r.label, r.ref_chars, r.hyp_chars, r.cer, r.wer, r.acc, r.ms
        ));
    }
    s.push_str("\n## Trung bình theo kịch bản\n\n");
    s.push_str("| Kịch bản | Số mẫu | Độ chính xác TB % | CER TB | WER TB |\n");
    s.push_str("|---|--:|--:|--:|--:|\n");
    let mut by_label: BTreeMap<&str, Vec<&AccRow>> = BTreeMap::new();
    for r in rows {
        by_label.entry(r.label.as_str()).or_default().push(r);
    }
    for (label, rs) in &by_label {
        let n = rs.len() as f64;
        let acc = rs.iter().map(|r| r.acc).sum::<f64>() / n;
        let cer = rs.iter().map(|r| r.cer).sum::<f64>() / n;
        let wer = rs.iter().map(|r| r.wer).sum::<f64>() / n;
        s.push_str(&format!(
            "| {} | {} | **{:.1}%** | {:.3} | {:.3} |\n",
            if label.is_empty() { "(none)" } else { label },
            rs.len(),
            acc,
            cer,
            wer
        ));
    }
    s.push('\n');
    s
}

// ----------------------------- helpers -----------------------------

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// Giải đường dẫn trong manifest: tuyệt đối giữ nguyên, tương đối nối với thư mục manifest.
fn resolve(base: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        base.join(pb)
    }
}

/// Đếm "page": pdf→số trang (pdfinfo), pptx→số slide (zip), còn lại None.
fn count_pages(path: &Path, fmt: FormatKind) -> Option<u32> {
    match fmt {
        FormatKind::Pdf => {
            let out = Command::new("pdfinfo").arg(path).output().ok()?;
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines() {
                if let Some(rest) = line.strip_prefix("Pages:") {
                    return rest.trim().parse().ok();
                }
            }
            None
        }
        FormatKind::Pptx => {
            let out = Command::new("python3")
                .arg("-c")
                .arg(
                    "import zipfile,sys,re;\
                     z=zipfile.ZipFile(sys.argv[1]);\
                     print(sum(1 for n in z.namelist() if re.match(r'ppt/slides/slide[0-9]+\\.xml$',n)))",
                )
                .arg(path)
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        }
        _ => None,
    }
}
