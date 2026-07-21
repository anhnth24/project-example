//! CORE-T8: end-to-end real-file golden coverage for DOCX / PPTX / XLSX / HTML.
//!
//! Fixtures are project-owned under `bench/markhand_web/golden/documents`
//! (never `vendor/markitdown-rs`). Paths resolve from `CARGO_MANIFEST_DIR` so
//! the suite runs in CI and in git worktrees without depending on cwd.
//!
//! Assertions target semantic Vietnamese content and key Markdown structure
//! (headings, slides, tables) — not brittle whole-output snapshots.

use fileconv_core::{ConvertError, Converter, FormatKind};
use std::path::{Path, PathBuf};

fn fixture(relative_from_repo_root: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_from_repo_root)
}

fn golden_document(name: &str) -> PathBuf {
    let path = fixture(&format!("bench/markhand_web/golden/documents/{name}"));
    assert!(
        path.is_file(),
        "missing golden fixture {name} at {} (resolved from CARGO_MANIFEST_DIR)",
        path.display()
    );
    path
}

fn adversarial_file(name: &str) -> PathBuf {
    let path = fixture(&format!("bench/markhand_web/adversarial/files/{name}"));
    assert!(
        path.is_file(),
        "missing adversarial fixture {name} at {} (resolved from CARGO_MANIFEST_DIR)",
        path.display()
    );
    path
}

fn convert(path: &Path) -> fileconv_core::ConversionResult {
    Converter::new()
        .convert_path(path)
        .unwrap_or_else(|err| panic!("convert {} failed: {err}", path.display()))
}

fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected to contain {needle:?}\n--- markdown ---\n{haystack}\n--- end ---"
    );
}

fn assert_heading_line(md: &str, level: usize, title: &str) {
    let prefix = "#".repeat(level);
    let expected = format!("{prefix} {title}");
    assert!(
        md.lines().any(|line| line.trim() == expected),
        "missing heading line {expected:?}\n--- markdown ---\n{md}\n--- end ---"
    );
}

/// Shared Vietnamese “hồ sơ” semantic markers used across golden office/html docs.
fn assert_hoso_semantics(md: &str, code: &str) {
    assert_contains(md, code);
    assert_contains(md, "Ngân sách được phê duyệt");
    assert_contains(md, "triệu đồng");
    assert_contains(md, "Hạn hoàn tất là ngày");
    assert_contains(md, "Đơn vị phụ trách");
    // Accented Vietnamese must survive as NFC graphemes (not stripped/mojibake).
    assert!(
        md.contains('ệ') || md.contains('ễ') || md.contains('ả'),
        "expected Vietnamese diacritics in output:\n{md}"
    );
}

#[test]
fn gold_docx_preserves_vietnamese_headings_and_fields() {
    let path = golden_document("gold-006.docx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Docx);
    assert_eq!(
        result.title.as_deref(),
        Some("Hồ sơ bảo trì thiết bị số 06")
    );

    let md = &result.markdown;
    assert_heading_line(md, 1, "Hồ sơ bảo trì thiết bị số 06");
    assert_heading_line(md, 2, "Thông tin đã phê duyệt");
    assert_hoso_semantics(md, "HS-2028-006");
    assert_contains(md, "222 triệu đồng");
    assert_contains(md, "Phòng Tài chính (PTC)");
}

#[test]
fn gold_docx_versioned_budget_preserves_structure() {
    let path = golden_document("gold-budget-v1.docx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Docx);
    assert_eq!(
        result.title.as_deref(),
        Some("Quy định kinh phí chuyển đổi số — phiên bản 1")
    );

    let md = &result.markdown;
    assert_heading_line(md, 1, "Quy định kinh phí chuyển đổi số — phiên bản 1");
    assert_heading_line(md, 2, "Thông tin đã phê duyệt");
    assert_contains(md, "QD-KP-2026");
    assert_contains(md, "Kinh phí được phê duyệt là 10 triệu đồng");
    assert_contains(md, "Phòng Tài chính (PTC)");
    assert_contains(md, "phiên bản đầu tiên");
}

#[test]
fn gold_pptx_preserves_slide_structure_and_vietnamese() {
    let path = golden_document("gold-009.pptx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Pptx);
    assert_eq!(result.title.as_deref(), Some("Slide 1"));

    let md = &result.markdown;
    assert_heading_line(md, 2, "Slide 1");
    assert_contains(md, "Hồ sơ lưu trữ hồ sơ số 09");
    assert_hoso_semantics(md, "HS-2028-009");
    assert_contains(md, "273 triệu đồng");
    assert_contains(md, "Trung tâm Dữ liệu (TTDL)");
}

#[test]
fn gold_pptx_second_fixture_keeps_slide_and_owner() {
    let path = golden_document("gold-010.pptx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Pptx);

    let md = &result.markdown;
    assert_heading_line(md, 2, "Slide 1");
    assert_contains(md, "Hồ sơ quy trình mua sắm số 10");
    assert_hoso_semantics(md, "HS-2026-010");
    assert_contains(md, "Ban Quản lý dự án (BQLDA)");
}

#[test]
fn gold_xlsx_preserves_table_structure_and_vietnamese() {
    let path = golden_document("gold-011.xlsx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Xlsx);
    assert_eq!(result.title.as_deref(), Some("Thông tin"));

    let md = &result.markdown;
    assert_heading_line(md, 2, "Thông tin");
    // Key Markdown table shape without pinning every cell whitespace quirk.
    assert_contains(md, "| Mục | Nội dung |");
    assert_contains(md, "| --- | --- |");
    assert_contains(md, "| code |");
    assert_contains(md, "| budget |");
    assert_contains(md, "| deadline |");
    assert_contains(md, "| owner |");
    assert_hoso_semantics(md, "HS-2027-011");
    assert_contains(md, "307 triệu đồng");
    assert_contains(md, "Phòng Tài chính (PTC)");
}

#[test]
fn gold_xlsx_second_fixture_keeps_table_and_owner() {
    let path = golden_document("gold-012.xlsx");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Xlsx);

    let md = &result.markdown;
    assert_contains(md, "| Mục | Nội dung |");
    assert_contains(md, "| --- | --- |");
    assert_hoso_semantics(md, "HS-2028-012");
    assert_contains(md, "Ban An toàn thông tin (BATTT)");
}

#[test]
fn gold_html_preserves_headings_and_vietnamese() {
    let path = golden_document("gold-017.html");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Html);
    assert_eq!(
        result.title.as_deref(),
        Some("Hồ sơ đối soát thanh toán số 17")
    );

    let md = &result.markdown;
    assert_heading_line(md, 1, "Hồ sơ đối soát thanh toán số 17");
    assert_heading_line(md, 2, "Thông tin đã phê duyệt");
    assert_hoso_semantics(md, "HS-2027-017");
    assert_contains(md, "409 triệu đồng");
    assert_contains(md, "Ban An toàn thông tin (BATTT)");
}

#[test]
fn gold_html_second_fixture_keeps_owner_and_headings() {
    let path = golden_document("gold-018.html");
    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Html);

    let md = &result.markdown;
    assert_heading_line(md, 1, "Hồ sơ lưu trữ hồ sơ số 18");
    assert_heading_line(md, 2, "Thông tin đã phê duyệt");
    assert_hoso_semantics(md, "HS-2028-018");
    assert_contains(md, "Phòng Nhân sự (PNS)");
}

/// Compact shared probe: OOXML formats must return `ConvertError` (no panic);
/// HTML must not panic on binary garbage (htmd may still emit a short string).
#[test]
fn malformed_office_and_html_fail_gracefully() {
    let conv = Converter::new();

    // Project-owned adversarial malformed DOCX (invalid OOXML inside zip-ish bytes).
    let malformed_docx = adversarial_file("malformed.docx");
    match conv.convert_path(&malformed_docx) {
        Err(ConvertError::Failed(msg)) => {
            assert!(
                !msg.trim().is_empty(),
                "Failed error should carry a message"
            );
        }
        other => panic!("expected ConvertError::Failed for malformed.docx, got {other:?}"),
    }

    let dir = std::env::temp_dir().join(format!(
        "fileconv_core_t8_malformed_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();

    // Truncated / invalid ZIP headers — enough to exercise zip/calamine error paths.
    let bogus_zip = b"PK\x03\x04not-a-valid-ooxml-package";
    let cases = [
        ("probe.docx", bogus_zip.as_slice(), true),
        ("probe.pptx", bogus_zip.as_slice(), true),
        ("probe.xlsx", bogus_zip.as_slice(), true),
        // Binary garbage is not valid HTML; converter must not panic.
        (
            "probe.html",
            b"\x00\x01\xff\xfe<<<not html>>>".as_slice(),
            false,
        ),
    ];

    for (name, bytes, expect_err) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();

        let caught = std::panic::catch_unwind(|| conv.convert_path(&path));
        assert!(
            caught.is_ok(),
            "converter panicked on malformed {name} (must fail gracefully)"
        );
        let result = caught.unwrap();
        if expect_err {
            match result {
                Err(ConvertError::Failed(_)) => {}
                other => panic!("expected Failed for {name}, got {other:?}"),
            }
        } else {
            // HTML: either Err or a non-panicking Ok is acceptable fail-soft behavior.
            let _ = result;
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
