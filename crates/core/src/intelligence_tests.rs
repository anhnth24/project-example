use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::intelligence::*;

static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

fn doc(name: &str, markdown: &str) -> CorpusDocument {
    CorpusDocument {
        source_rel: name.into(),
        md_rel: format!("{name}.md"),
        format: "markdown".into(),
        markdown: markdown.into(),
    }
}

fn requirements_doc() -> CorpusDocument {
    doc(
        "requirements.docx",
        "# Thanh toán\n\n\
         Hệ thống phải lưu nhật ký giao dịch trong 5 năm.\n\
         Doanh nghiệp cần giảm thời gian đối soát.\n\n\
         ## Story\n\n\
         Là nhân viên đối soát, tôi muốn xuất báo cáo, để gửi kế toán.\n\
         Given đã có dữ liệu, When xuất báo cáo, Then số liệu phải khớp.\n\n\
         ## Rủi ro\n\nGiả định đối tác gửi dữ liệu trước 08:00.\n\
         TBD: SLA cần làm rõ?\n",
    )
}

fn citation(id: &str, quote: &str) -> Citation {
    Citation {
        id: id.into(),
        source_rel: "source.md".into(),
        md_rel: "source.md".into(),
        heading: "Yêu cầu".into(),
        quote: quote.into(),
        start: 0,
        end: quote.len(),
        page: None,
        confidence: 1.0,
    }
}

fn item(id: &str, kind: HandoffItemKind, text: &str, citations: &[&str]) -> HandoffItem {
    HandoffItem {
        id: id.into(),
        kind,
        text: text.into(),
        citations: citations.iter().map(|value| value.to_string()).collect(),
        status: "draft".into(),
        parent_id: None,
    }
}

fn temp_zip() -> std::path::PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "markhand_intelligence_{}_{}.zip",
        std::process::id(),
        n
    ))
}

#[test]
fn corpus_ids_are_unique_across_documents() {
    let corpus = build_corpus(
        &[
            doc("a.md", "# A\n\nNội dung giống nhau."),
            doc("b.md", "# A\n\nNội dung giống nhau."),
        ],
        2_000,
    );
    let ids: HashSet<&str> = corpus.iter().map(|chunk| chunk.id.as_str()).collect();
    assert_eq!(corpus.len(), 2);
    assert_eq!(ids.len(), 2);
}

/// Consumer ID stability under `INTELLIGENCE_ID_SCHEME` (`sha256-v1`).
/// Fixture bodies are fixed; digests must not drift across toolchains/runs.
#[test]
fn sha256_v1_consumer_ids_are_stable() {
    assert_eq!(INTELLIGENCE_ID_SCHEME, "sha256-v1");
    assert_eq!(HANDOFF_SCHEMA_VERSION, 2);
    let markdown = "# Heading\n\nBody text for corpus.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
    let document = doc("doc.md", markdown);

    let corpus = build_corpus(&[document.clone()], 2_000);
    assert_eq!(corpus.len(), 1);
    assert_eq!(corpus[0].heading, "Heading");
    assert_eq!(corpus[0].start, 11);
    assert_eq!(
        corpus[0].id,
        "chunk-sha256-v1-5c9caedfab489c26935e66e1b4e06016a670e011b174d324e236c40a87ed21ee"
    );
    assert!(corpus[0].id.contains(INTELLIGENCE_ID_SCHEME));

    let tables = parse_markdown_tables(&document);
    assert_eq!(tables.len(), 1);
    assert_eq!(
        tables[0].id,
        "table-sha256-v1-7de19ebea10d6579f97772d6e4f8a6d75186198ce075d6e14a472beec81aa68a"
    );
    assert!(tables[0].id.contains(INTELLIGENCE_ID_SCHEME));

    // Pack id embeds a wall-clock nonce; trailing digest + schema encode the scheme.
    let pack = generate_handoff_pack(
        &[document],
        &HandoffOptions {
            product_slug: "probe".into(),
            ..Default::default()
        },
    );
    assert_eq!(pack.schema_version, HANDOFF_SCHEMA_VERSION);
    assert_eq!(pack.id_scheme, INTELLIGENCE_ID_SCHEME);
    let fingerprint = pack
        .pack_id
        .rsplit('-')
        .next()
        .expect("pack_id ends with fingerprint digest");
    assert_eq!(
        fingerprint,
        "a501848e6a9bdc8a6e9ecb767558726e845f10e185dcd6faa02220ef959e4e5d"
    );
    assert!(
        pack.pack_id
            .starts_with(&format!("handoff-{INTELLIGENCE_ID_SCHEME}-")),
        "pack_id={}",
        pack.pack_id
    );

    let again = build_corpus(&[doc("doc.md", markdown)], 2_000);
    assert_eq!(again[0].id, corpus[0].id);
}

#[test]
fn corpus_parses_page_anchor_comments() {
    let corpus = build_corpus(
        &[doc(
            "scan.pdf",
            "<!-- Trang 7 (OCR) -->\n\n# Điều 1\n\nNội dung.",
        )],
        2_000,
    );
    assert_eq!(corpus[0].page, Some(7));
}

#[test]
fn search_empty_query_returns_no_hits() {
    assert!(search_corpus(&[requirements_doc()], "   ", 10).is_empty());
}

#[test]
fn heading_match_ranks_above_body_only_match() {
    let documents = [
        doc("heading.md", "# Đối soát\n\nNội dung chung."),
        doc("body.md", "# Khác\n\nQuy trình đối soát."),
    ];
    let hits = search_corpus(&documents, "đối soát", 10);
    assert_eq!(hits[0].chunk.source_rel, "heading.md");
}

#[test]
fn search_honours_result_limit() {
    let documents = [
        doc("a.md", "# A\n\nYêu cầu hệ thống."),
        doc("b.md", "# B\n\nYêu cầu hệ thống."),
        doc("c.md", "# C\n\nYêu cầu hệ thống."),
    ];
    assert_eq!(search_corpus(&documents, "yêu cầu", 2).len(), 2);
}

#[test]
fn ask_without_matches_is_explicit() {
    let result = ask_corpus(&[requirements_doc()], "không-tồn-tại-xyz", 5);
    assert!(result.citations.is_empty());
    assert!(result.answer.contains("Không tìm thấy"));
}

#[test]
fn quality_flags_short_content() {
    let report = quality_report(&[doc("short.md", "ít")]);
    assert!(report.documents[0]
        .issues
        .iter()
        .any(|issue| issue.code == "SHORT_CONTENT"));
    assert!(report.documents[0].score < 1.0);
}

#[test]
fn quality_flags_ocr_content() {
    let report = quality_report(&[doc(
        "scan.pdf",
        "<!-- Trang 1 (OCR) -->\n\nNội dung OCR đủ dài để vượt ngưỡng tối thiểu của báo cáo chất lượng và cần rà soát.",
    )]);
    assert!(report.documents[0]
        .issues
        .iter()
        .any(|issue| issue.code == "OCR_CONTENT"));
}

#[test]
fn quality_flags_repeated_alphanumeric_runs() {
    let report = quality_report(&[doc(
        "bad.md",
        "# Nội dung\n\nVăn bản dài hợp lệ nhưng có nnnnnnnn do OCR bị lỗi và cần kiểm tra.",
    )]);
    assert!(report.documents[0]
        .issues
        .iter()
        .any(|issue| issue.code == "REPEATED_RUN"));
}

#[test]
fn quality_aggregates_multiple_documents() {
    let report = quality_report(&[
        doc(
            "good.md",
            "# Tốt\n\nNội dung đủ dài, có cấu trúc và không chứa lỗi OCR hay encoding bất thường.",
        ),
        doc("bad.md", "x"),
    ]);
    assert_eq!(report.documents.len(), 2);
    assert!(report.score > 0.0 && report.score < 1.0);
}

fn assert_single_span(report: &PiiReport, kind: PiiKind, exact: &str) {
    let hits: Vec<_> = report
        .findings
        .iter()
        .filter(|finding| finding.kind == kind)
        .collect();
    assert_eq!(hits.len(), 1, "{kind:?} hits: {:?}", report.findings);
    assert_eq!(hits[0].text, exact);
}

#[test]
fn pii_detects_plus84_phone() {
    let report = detect_pii(&[doc("phone.md", "Liên hệ +84912345678 ngay.")]);
    assert_single_span(&report, PiiKind::Phone, "+84912345678");
}

#[test]
fn pii_detects_leading_zero_vn_mobile() {
    let report = detect_pii(&[doc("phone0.md", "Gọi 0912345678 giúp tôi.")]);
    assert_single_span(&report, PiiKind::Phone, "0912345678");
}

#[test]
fn pii_rejects_price_at_numeric_as_email() {
    let report = detect_pii(&[doc("price.md", "Giá price@100.00 không phải email.")]);
    assert!(
        !report.counts.contains_key(&PiiKind::Email),
        "price@100.00 must not be email: {:?}",
        report.findings
    );
    let real = detect_pii(&[doc("mail.md", "Liên hệ lan@example.com ngay.")]);
    assert_single_span(&real, PiiKind::Email, "lan@example.com");
}

#[test]
fn pii_rejects_arbitrary_bare_ten_digit_numbers() {
    let bare = detect_pii(&[doc(
        "nums.md",
        "Mã đơn 1234567890 và chuỗi 0123456789 không phải SĐT.",
    )]);
    assert!(
        !bare.counts.contains_key(&PiiKind::Phone),
        "bare 10-digit runs must not be phone: {:?}",
        bare.findings
    );
    let mobile = detect_pii(&[doc("ok.md", "SĐT 0912345678")]);
    assert_eq!(mobile.counts.get(&PiiKind::Phone), Some(&1));
}

#[test]
fn pii_phone_vs_bank_prefers_bank_context() {
    let bankish = detect_pii(&[doc(
        "bank-phone.md",
        "Tài khoản ngân hàng: 0912345678 cần đối soát.",
    )]);
    assert_eq!(bankish.counts.get(&PiiKind::BankAccount), Some(&1));
    assert!(
        !bankish.counts.contains_key(&PiiKind::Phone),
        "bank label must win over phone shape: {:?}",
        bankish.findings
    );
    let phone = detect_pii(&[doc("phone-only.md", "Điện thoại: 0912345678")]);
    assert_eq!(phone.counts.get(&PiiKind::Phone), Some(&1));
    assert!(!phone.counts.contains_key(&PiiKind::BankAccount));
}

#[test]
fn pii_detects_identity_number_only_with_context() {
    let with_context = detect_pii(&[doc("id.md", "CCCD: 001234567890")]);
    let without_context = detect_pii(&[doc("number.md", "Mã: 001234567890")]);
    assert_eq!(with_context.counts.get(&PiiKind::NationalId), Some(&1));
    assert!(!without_context.counts.contains_key(&PiiKind::NationalId));
}

#[test]
fn pii_detects_bank_account_with_context() {
    let report = detect_pii(&[doc("bank.md", "Tài khoản ngân hàng: 1234567890123")]);
    assert_eq!(report.counts.get(&PiiKind::BankAccount), Some(&1));
}

#[test]
fn pii_email_exact_span_in_table_wrappers_labels_and_links() {
    let cases = [
        (
            "| lan@example.com |",
            "lan@example.com",
            "| [REDACTED_Email] |",
        ),
        (
            "Email:lan@example.com",
            "lan@example.com",
            "Email:[REDACTED_Email]",
        ),
        (
            "Liên hệ (lan@example.com).",
            "lan@example.com",
            "Liên hệ ([REDACTED_Email]).",
        ),
        ("<lan@example.com>", "lan@example.com", "<[REDACTED_Email]>"),
        (
            "[gửi](mailto:lan@example.com)",
            "lan@example.com",
            "[gửi](mailto:[REDACTED_Email])",
        ),
        (
            "\"lan@example.com\"",
            "lan@example.com",
            "\"[REDACTED_Email]\"",
        ),
    ];
    for (markdown, exact, redacted_shape) in cases {
        let report = detect_pii(&[doc("wrap.md", markdown)]);
        assert_single_span(&report, PiiKind::Email, exact);
        let redacted = redact_pii(markdown, &report.findings);
        assert_eq!(redacted, redacted_shape, "input={markdown:?}");
    }
}

#[test]
fn pii_email_accepts_numeric_local_and_rejects_dot_hyphen_rules() {
    let ok = detect_pii(&[doc("num-local.md", "Liên hệ 123@example.com")]);
    assert_single_span(&ok, PiiKind::Email, "123@example.com");

    for bad in [
        ".user@example.com",
        "user.@example.com",
        "us..er@example.com",
        "user@-example.com",
        "user@example-.com",
        "price@100.00",
    ] {
        let report = detect_pii(&[doc("bad-mail.md", bad)]);
        assert!(
            !report.counts.contains_key(&PiiKind::Email),
            "must reject {bad:?}: {:?}",
            report.findings
        );
    }
}

#[test]
fn pii_phone_scans_grouped_plus84_optional_trunk_and_landline() {
    let grouped = detect_pii(&[doc("g.md", "Gọi 0912 345 678 hoặc 0912-345-678")]);
    assert_eq!(grouped.counts.get(&PiiKind::Phone), Some(&2));
    assert!(grouped.findings.iter().any(|f| f.text == "0912 345 678"));
    assert!(grouped.findings.iter().any(|f| f.text == "0912-345-678"));

    let plus = detect_pii(&[doc("p84.md", "Hotline +84 912 345 678")]);
    assert_single_span(&plus, PiiKind::Phone, "+84 912 345 678");

    let trunk = detect_pii(&[doc("trunk.md", "Máy +84 (0) 912 345 678")]);
    assert_single_span(&trunk, PiiKind::Phone, "+84 (0) 912 345 678");

    let landline = detect_pii(&[doc("ll.md", "Bàn 024 3825 1234")]);
    assert_single_span(&landline, PiiKind::Phone, "024 3825 1234");

    let paren = detect_pii(&[doc("par.md", "Gọi (0912) 345-678")]);
    assert_single_span(&paren, PiiKind::Phone, "(0912) 345-678");
}

#[test]
fn pii_phone_rejects_invalid_030_prefix() {
    let report = detect_pii(&[doc("bad030.md", "Số 0301234567 và 030 123 4567")]);
    assert!(
        !report.counts.contains_key(&PiiKind::Phone),
        "030 must be rejected: {:?}",
        report.findings
    );
}

#[test]
fn pii_phone_exact_span_in_markdown_table() {
    let markdown = "| Liên hệ | 0912345678 |\n|---|---|\n";
    let report = detect_pii(&[doc("t.md", markdown)]);
    assert_single_span(&report, PiiKind::Phone, "0912345678");
    let redacted = redact_pii(markdown, &report.findings);
    assert!(redacted.contains("| Liên hệ | [REDACTED_Phone] |"));
    assert!(redacted.contains("|---|---|"));
}

#[test]
fn pii_bank_supports_stk_aliases_grouped_digits_and_nearest_label() {
    for (markdown, exact) in [
        ("STK: 123456789012", "123456789012"),
        ("Số TK 1234 5678 9012", "1234 5678 9012"),
        ("Tài khoản: 1234-5678-901234", "1234-5678-901234"),
    ] {
        let report = detect_pii(&[doc("stk.md", markdown)]);
        assert_single_span(&report, PiiKind::BankAccount, exact);
    }
}

#[test]
fn pii_explicit_phone_label_beats_generic_ngan_hang() {
    let report = detect_pii(&[doc(
        "mix.md",
        "Tại ngân hàng A, SĐT: 0912345678 vẫn là số điện thoại.",
    )]);
    assert_eq!(report.counts.get(&PiiKind::Phone), Some(&1));
    assert!(
        !report.counts.contains_key(&PiiKind::BankAccount),
        "SĐT must beat generic ngân hàng: {:?}",
        report.findings
    );
}

#[test]
fn pii_generic_ngan_hang_prose_does_not_classify_transaction_counts() {
    let report = detect_pii(&[doc(
        "prose.md",
        "Ngân hàng xử lý 1500000000 giao dịch trong quý.",
    )]);
    assert!(
        !report.counts.contains_key(&PiiKind::BankAccount),
        "transaction counts must not become STK: {:?}",
        report.findings
    );
    assert!(!report.counts.contains_key(&PiiKind::Phone));
}

#[test]
fn pii_phone_maintains_active_prefixes_055_087_and_rejects_plus_without_84() {
    let wintel = detect_pii(&[doc("055.md", "Gọi 0551234567")]);
    assert_single_span(&wintel, PiiKind::Phone, "0551234567");
    let itel = detect_pii(&[doc("087.md", "Gọi 0871234567")]);
    assert_single_span(&itel, PiiKind::Phone, "0871234567");

    let plus = detect_pii(&[doc("p.md", "Intl +85551234567")]);
    assert!(
        !plus.counts.contains_key(&PiiKind::Phone),
        "+ without 84 must reject: {:?}",
        plus.findings
    );
    let ok = detect_pii(&[doc("ok84.md", "Intl +84551234567")]);
    assert_single_span(&ok, PiiKind::Phone, "+84551234567");
}

#[test]
fn pii_phone_alphanumeric_boundaries_and_consecutive_separated() {
    let glued = detect_pii(&[doc("glue.md", "id0912345678 và 0912345678x")]);
    assert!(
        !glued.counts.contains_key(&PiiKind::Phone),
        "alphanumeric-adjacent must not match: {:?}",
        glued.findings
    );
    let consecutive = detect_pii(&[doc(
        "two.md",
        "Gọi 0912345678 / 0987654321 và 0912 345 678, 0978 654 321",
    )]);
    assert_eq!(consecutive.counts.get(&PiiKind::Phone), Some(&4));
    assert!(consecutive.findings.iter().any(|f| f.text == "0912345678"));
    assert!(consecutive.findings.iter().any(|f| f.text == "0987654321"));
}

#[test]
fn pii_phone_exact_landline_length_rules() {
    let ok = detect_pii(&[doc("ll.md", "HN 02438251234")]);
    assert_single_span(&ok, PiiKind::Phone, "02438251234");
    let short = detect_pii(&[doc("short.md", "HN 0243825123")]);
    assert!(
        !short.counts.contains_key(&PiiKind::Phone),
        "2-digit area requires 8 subscriber digits: {:?}",
        short.findings
    );
    let province = detect_pii(&[doc("dn.md", "Đà Nẵng 02363825123")]);
    assert_single_span(&province, PiiKind::Phone, "02363825123");
}

#[test]
fn pii_label_scope_does_not_cross_newline_pipe_or_sentence() {
    let newline = detect_pii(&[doc("nl.md", "Tài khoản: 123456789012\nGọi ngay 0912345678")]);
    assert_eq!(newline.counts.get(&PiiKind::BankAccount), Some(&1));
    assert_eq!(newline.counts.get(&PiiKind::Phone), Some(&1));

    let sentence = detect_pii(&[doc(
        "sent.md",
        "Tài khoản đã đóng. Mã tham chiếu 123456789012. Liên hệ 0912345678",
    )]);
    assert!(
        !sentence.counts.contains_key(&PiiKind::BankAccount),
        "label must not cross sentence: {:?}",
        sentence.findings
    );
    assert_eq!(sentence.counts.get(&PiiKind::Phone), Some(&1));

    let pipe = detect_pii(&[doc("pipe.md", "Tài khoản | 0912345678 còn lại")]);
    assert!(
        !pipe.counts.contains_key(&PiiKind::BankAccount),
        "label must not cross table pipe: {:?}",
        pipe.findings
    );
    assert_eq!(pipe.counts.get(&PiiKind::Phone), Some(&1));
}

#[test]
fn pii_table_column_headers_associate_bank_and_phone() {
    let markdown = "| Tài khoản | SĐT |\n|---|---|\n| 123456789012 | 0912345678 |\n";
    let report = detect_pii(&[doc("cols.md", markdown)]);
    let bank = report
        .findings
        .iter()
        .find(|f| f.kind == PiiKind::BankAccount)
        .expect("bank col");
    let phone = report
        .findings
        .iter()
        .find(|f| f.kind == PiiKind::Phone)
        .expect("phone col");
    assert_eq!(bank.text, "123456789012");
    assert_eq!(phone.text, "0912345678");
    let redacted = redact_pii(markdown, &report.findings);
    assert!(redacted.contains("| [REDACTED_BankAccount] | [REDACTED_Phone] |"));
}

#[test]
fn pii_email_apostrophe_dot_atom_and_strict_boundaries() {
    let ok = detect_pii(&[doc("apos.md", "Mail o'reilly@example.com please")]);
    assert_single_span(&ok, PiiKind::Email, "o'reilly@example.com");

    for (label, markdown) in [
        ("unicode-prefix", "xin chàoélan@example.com"),
        ("unsupported-adjacent", "token§lan@example.com"),
        ("right-glue", "lan@example.com§x"),
    ] {
        let report = detect_pii(&[doc("bad.md", markdown)]);
        assert!(
            !report.counts.contains_key(&PiiKind::Email),
            "{label} must not yield partial email from {markdown:?}: {:?}",
            report.findings
        );
    }
}

#[test]
fn redaction_ignores_out_of_range_findings() {
    let markdown = "Không đổi";
    let findings = [PiiFinding {
        kind: PiiKind::Email,
        text: "x".into(),
        source_rel: "a.md".into(),
        start: 100,
        end: 110,
        confidence: 1.0,
    }];
    assert_eq!(redact_pii(markdown, &findings), markdown);
}

#[test]
fn redaction_ignores_stale_finding_text() {
    let markdown = "Email: lan@example.com còn nguyên";
    let start = markdown.find("lan@example.com").unwrap();
    let end = start + "lan@example.com".len();
    let findings = [PiiFinding {
        kind: PiiKind::Email,
        // Offsets still point at the email, but text is stale after an edit.
        text: "old@example.com".into(),
        source_rel: "a.md".into(),
        start,
        end,
        confidence: 1.0,
    }];
    assert_eq!(redact_pii(markdown, &findings), markdown);
    assert!(markdown.contains("lan@example.com"));
}

#[test]
fn redaction_coalesces_crossing_and_nested_spans_in_public_api() {
    let markdown = "secret=ABCDEFGHtail and more";
    let findings = [
        PiiFinding {
            kind: PiiKind::Phone,
            text: "FGHtail".into(),
            source_rel: "a.md".into(),
            start: 12,
            end: 19,
            confidence: 1.0,
        },
        PiiFinding {
            kind: PiiKind::Email,
            text: "CDE".into(),
            source_rel: "a.md".into(),
            start: 9,
            end: 12,
            confidence: 1.0,
        },
        PiiFinding {
            kind: PiiKind::BankAccount,
            text: "ABCDEFGH".into(),
            source_rel: "a.md".into(),
            start: 7,
            end: 15,
            confidence: 1.0,
        },
    ];
    let redacted = redact_pii(markdown, &findings);
    assert!(!redacted.contains("ABCDEFGH"));
    assert!(
        !redacted.contains("tail"),
        "crossing suffix must redact: {redacted}"
    );
    assert_eq!(redacted.matches("[REDACTED_").count(), 1);
}

#[test]
fn redaction_ignores_non_utf8_boundary_offsets() {
    let markdown = "Liên hệ: a@b.co và ệ";
    let email_start = markdown.find("a@b.co").unwrap();
    let email_end = email_start + "a@b.co".len();
    let ye = markdown.find('ệ').unwrap();
    assert!(!markdown.is_char_boundary(ye + 1));
    let findings = [
        PiiFinding {
            kind: PiiKind::Email,
            text: "a@b.co".into(),
            source_rel: "a.md".into(),
            start: email_start,
            end: email_end,
            confidence: 1.0,
        },
        PiiFinding {
            kind: PiiKind::Phone,
            text: "x".into(),
            source_rel: "a.md".into(),
            start: ye + 1,
            end: ye + 2,
            confidence: 1.0,
        },
    ];
    let redacted = redact_pii(markdown, &findings);
    assert!(redacted.contains("[REDACTED_Email]"));
    assert!(redacted.contains('ệ'));
}

#[test]
fn malformed_markdown_table_is_ignored() {
    let document = doc("bad-table.md", "| A | B |\n|--|---|\n| 1 | 2 |\n");
    assert!(parse_markdown_tables(&document).is_empty());
}

#[test]
fn table_update_rejects_invalid_span() {
    let table = MarkdownTable {
        id: "x".into(),
        source_rel: "x.md".into(),
        index: 0,
        start: 100,
        end: 200,
        rows: vec![],
    };
    assert!(update_markdown_table("short", &table, &[]).is_err());
}

#[test]
fn table_update_rejects_start_not_less_than_end() {
    let markdown = "| A | B |\n|---|---|\n| 1 | 2 |\n";
    for (start, end) in [(0, 0), (5, 5), (8, 3)] {
        let table = MarkdownTable {
            id: "x".into(),
            source_rel: "x.md".into(),
            index: 0,
            start,
            end,
            rows: vec![],
        };
        assert!(
            update_markdown_table(markdown, &table, &[]).is_err(),
            "start={start} end={end}"
        );
    }
}

#[test]
fn table_update_rejects_stale_non_table_span() {
    let markdown = "không còn bảng ở đây nữa";
    let table = MarkdownTable {
        id: "x".into(),
        source_rel: "x.md".into(),
        index: 0,
        start: 0,
        end: markdown.len(),
        rows: vec![vec!["A".into(), "B".into()]],
    };
    let err = update_markdown_table(markdown, &table, &[]).unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
}

#[test]
fn table_update_requires_reparsed_id_span_and_original_rows() {
    let document = doc(
        "tbl.md",
        "| A | B |\n|---|---|\n| 1 | 2 |\n\n| X | Y |\n|---|---|\n| 9 | 8 |\n",
    );
    let tables = parse_markdown_tables(&document);
    assert_eq!(tables.len(), 2);
    let first = tables[0].clone();
    // Happy path: exact reparsed match.
    let updated = update_markdown_table(
        &document.markdown,
        &first,
        &[vec!["A".into(), "B".into()], vec!["3".into(), "4".into()]],
    )
    .unwrap();
    assert!(updated.contains("|3|4|"));

    // Stale rows (content edited) → conflict.
    let mut stale_rows = first.clone();
    stale_rows.rows[1][0] = "999".into();
    let err = update_markdown_table(&document.markdown, &stale_rows, &first.rows).unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");

    // Unrelated pipe span / wrong id+offsets → conflict.
    let mut unrelated = first.clone();
    unrelated.start = tables[1].start;
    unrelated.end = tables[1].end;
    let err = update_markdown_table(&document.markdown, &unrelated, &first.rows).unwrap_err();
    assert!(err.to_string().contains("conflict"), "{err}");
}

#[test]
fn table_update_rejects_non_utf8_boundary_offsets() {
    let markdown = "trước ệ | A | B |\n|---|---|\n| 1 | 2 |\n sau";
    let ye = markdown.find('ệ').unwrap();
    assert!(!markdown.is_char_boundary(ye + 1));
    let table = MarkdownTable {
        id: "x".into(),
        source_rel: "x.md".into(),
        index: 0,
        start: ye + 1,
        end: ye + 4,
        rows: vec![vec!["A".into(), "B".into()]],
    };
    assert!(update_markdown_table(markdown, &table, &[]).is_err());
}

#[test]
fn rendered_table_escapes_pipes_and_newlines() {
    let table = render_markdown_table(&[
        vec!["A".into(), "B".into()],
        vec!["x|y".into(), "line1\nline2".into()],
    ]);
    assert!(table.contains("x\\|y"));
    assert!(table.contains("line1<br>line2"));
}

#[test]
fn csv_neutralizes_all_formula_prefixes() {
    let csv = String::from_utf8(
        table_to_csv(&[vec![
            "=SUM(A1:A2)".into(),
            "+cmd".into(),
            "-1+2".into(),
            "@risk".into(),
        ]])
        .unwrap(),
    )
    .unwrap();
    for value in ["'=SUM", "'+cmd", "'-1+2", "'@risk"] {
        assert!(csv.contains(value));
    }
}

#[test]
fn schema_infers_number_date_boolean_and_string() {
    let document = doc(
        "schema.md",
        "| Count | Date | Enabled | Name |\n|---|---|---|---|\n\
         | 10 | 10/07/2026 | Có | CASAN |\n| 20 | 11/07/2026 | Không | Markhand |\n",
    );
    let schema = extract_schema(&document);
    let kinds: Vec<FieldType> = schema
        .fields
        .iter()
        .map(|field| field.field_type.clone())
        .collect();
    assert_eq!(
        kinds,
        vec![
            FieldType::Number,
            FieldType::Date,
            FieldType::Boolean,
            FieldType::String
        ]
    );
}

#[test]
fn schema_extracts_form_like_fields() {
    let schema = extract_schema(&doc(
        "form.md",
        "# Form\n\nSố hợp đồng: HD-001\nNgày ký: 10/07/2026\n",
    ));
    assert!(schema
        .fields
        .iter()
        .any(|field| field.name == "Số hợp đồng"));
    assert!(schema.fields.iter().any(|field| field.name == "Ngày ký"));
}

#[test]
fn diff_detects_added_and_removed_content() {
    assert_eq!(diff_markdown("a", "a\nb")[0].kind, DiffKind::Added);
    assert_eq!(diff_markdown("a\nb", "a")[0].kind, DiffKind::Removed);
}

#[test]
fn diff_identical_content_is_unchanged() {
    let hunks = diff_markdown("a\nb", "a\nb");
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].kind, DiffKind::Unchanged);
}

#[test]
fn merge_accepts_theirs_when_ours_is_base() {
    let merged = three_way_merge("base", "base", "theirs");
    assert_eq!(merged.markdown, "theirs");
    assert!(merged.conflicts.is_empty());
}

#[test]
fn merge_accepts_equal_changes() {
    let merged = three_way_merge("base", "same", "same");
    assert_eq!(merged.markdown, "same");
    assert!(merged.conflicts.is_empty());
}

#[test]
fn watch_glob_is_case_insensitive() {
    assert!(watch_pattern_matches("*.PDF", "report.pdf"));
    assert!(!watch_pattern_matches("*.docx", "report.pdf"));
}

#[test]
fn handoff_extracts_br_fr_assumption_question_and_explicit_story() {
    let pack = generate_handoff_pack(
        &[requirements_doc()],
        &HandoffOptions {
            product_name: "Đối soát".into(),
            product_slug: "doi-soat".into(),
            ..Default::default()
        },
    );
    for kind in [
        HandoffItemKind::BusinessRequirement,
        HandoffItemKind::FunctionalRequirement,
        HandoffItemKind::UserStory,
        HandoffItemKind::AcceptanceCriterion,
        HandoffItemKind::Assumption,
        HandoffItemKind::OpenQuestion,
    ] {
        assert!(pack.items.iter().any(|item| item.kind == kind), "{kind:?}");
    }
}

#[test]
fn handoff_artifact_set_is_complete() {
    let pack = generate_handoff_pack(&[requirements_doc()], &HandoffOptions::default());
    for name in [
        "00-README.md",
        "01-BRD.md",
        "02-PRD.md",
        "03-USER-STORIES.md",
        "04-ACCEPTANCE-CRITERIA.md",
        "05-GLOSSARY.md",
        "06-TEST-CASES.md",
        "07-TRACEABILITY.md",
        "08-ASSUMPTIONS-QUESTIONS.md",
        "09-JIRA-IMPORT.csv",
        "10-GITHUB-ISSUES.md",
        "11-CONFLUENCE.md",
        "12-OBSIDIAN-MOC.md",
    ] {
        assert!(pack.artifacts.contains_key(name), "{name}");
    }
}

#[test]
fn deterministic_item_ids_are_stable_across_runs() {
    let first = generate_handoff_pack(&[requirements_doc()], &HandoffOptions::default());
    let second = generate_handoff_pack(&[requirements_doc()], &HandoffOptions::default());
    let first_ids: Vec<&str> = first.items.iter().map(|item| item.id.as_str()).collect();
    let second_ids: Vec<&str> = second.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(first_ids, second_ids);
    assert_ne!(first.pack_id, second.pack_id);
}

#[test]
fn validation_rejects_duplicate_ids() {
    let items = vec![
        item(
            "BR-001",
            HandoffItemKind::BusinessRequirement,
            "Hệ thống phải lưu log.",
            &["CITE-0001"],
        ),
        item(
            "BR-001",
            HandoffItemKind::BusinessRequirement,
            "Hệ thống phải mã hóa.",
            &["CITE-0001"],
        ),
    ];
    let report = validate_handoff(
        &items,
        &[citation("CITE-0001", "Hệ thống phải lưu log và mã hóa.")],
        &[],
        true,
    );
    assert!(!report.ok);
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "DUPLICATE_ID"));
}

#[test]
fn validation_rejects_missing_citation_in_strict_mode() {
    let report = validate_handoff(
        &[item(
            "BR-001",
            HandoffItemKind::BusinessRequirement,
            "Hệ thống phải lưu log.",
            &["CITE-MISSING"],
        )],
        &[],
        &[],
        true,
    );
    assert!(!report.ok);
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "MISSING_CITATION"));
}

#[test]
fn validation_rejects_weak_citation_grounding() {
    let report = validate_handoff(
        &[item(
            "FR-001",
            HandoffItemKind::FunctionalRequirement,
            "Hệ thống phải phóng tên lửa lên sao Hỏa.",
            &["CITE-0001"],
        )],
        &[citation(
            "CITE-0001",
            "Ứng dụng chỉ hiển thị báo cáo giao dịch nội bộ.",
        )],
        &[],
        true,
    );
    assert!(!report.ok);
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "CITATION_GROUNDING_WEAK"));
}

#[test]
fn validation_rejects_pack_without_requirements() {
    let report = validate_handoff(&[], &[], &[], true);
    assert!(!report.ok);
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "NO_REQUIREMENTS"));
}

#[test]
fn zip_export_contains_manifest_and_artifacts() {
    let pack = generate_handoff_pack(&[requirements_doc()], &HandoffOptions::default());
    let path = temp_zip();
    fs::write(&path, b"old partial data").unwrap();
    export_handoff_zip(&pack, &path).unwrap();

    let file = fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    assert!(archive.by_name("manifest.json").is_ok());
    assert!(archive.by_name("01-BRD.md").is_ok());
    assert!(archive.by_name("09-JIRA-IMPORT.csv").is_ok());
    fs::remove_file(path).ok();
}
