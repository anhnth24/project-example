//! Issue #369 / intern-31: convert → chunk → search workflow (happy path).
//!
//! Fixture: `tests/fixtures/heading-hierarchy.docx` (synthetic, no customer data).

use fileconv_core::chunk::chunk_markdown;
use fileconv_core::intelligence::{build_corpus, search_corpus, CorpusDocument};
use fileconv_core::{Converter, FormatKind};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn convert(path: &Path) -> fileconv_core::ConversionResult {
    Converter::new()
        .convert_path(path)
        .unwrap_or_else(|err| panic!("convert {} failed: {err}", path.display()))
}

#[test]
fn integration_convert_chunk_then_query_happy_path() {
    let path = fixture("heading-hierarchy.docx");
    assert!(
        path.is_file(),
        "missing fixture at {} — add heading-hierarchy.docx under crates/core/tests/fixtures/",
        path.display()
    );

    let result = convert(&path);
    assert_eq!(result.format, FormatKind::Docx);
    let md = &result.markdown;
    assert!(
        md.contains("# Phần I"),
        "converter must emit H1 heading\n---\n{md}\n---"
    );

    const MAX_CHARS: usize = 2_000;
    let chunks = chunk_markdown(md, MAX_CHARS);

    assert_eq!(
        chunks.len(),
        4,
        "expected 4 chunks (H1 intro + 3× H2); got {}",
        chunks.len()
    );

    let expected_headings = [
        "Phần I",
        "Phần I > Mục 1",
        "Phần I > Mục 2",
        "Phần I > Mục 3",
    ];
    for (i, want) in expected_headings.iter().enumerate() {
        assert_eq!(chunks[i].index, i, "chunk index must be monotonic");
        assert_eq!(chunks[i].heading, *want, "chunk {i} heading");
    }

    assert!(chunks[0].text.contains("Giới thiệu"));
    assert!(chunks[2].text.contains("REF-M2"));

    let doc = CorpusDocument {
        source_rel: "heading-hierarchy.docx".into(),
        md_rel: "heading-hierarchy.md".into(),
        format: "docx".into(),
        markdown: md.clone(),
    };

    let corpus = build_corpus(&[doc.clone()], MAX_CHARS);
    assert_eq!(corpus.len(), chunks.len(), "corpus mirrors chunk count");

    let hits = search_corpus(&[doc], "REF-M2", 5);
    assert!(!hits.is_empty(), "query must match at least one chunk");
    assert!(
        hits[0].chunk.heading.contains("Mục 2"),
        "top hit should be Mục 2, got {:?}",
        hits[0].chunk.heading
    );
}
