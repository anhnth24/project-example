use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use fileconv_core::intelligence::CorpusDocument;
use fileconv_core::llm::{embed_batch, embed_query, EmbeddingConfig, Provider};
use fileconv_core::Converter;
use fileconv_knowledge::desktop::service::{
    hybrid_search, index_stats, rebuild_index, DesktopEmbeddingPlan, KnowledgePaths,
};
use fileconv_knowledge::types::{HybridSearchHit, IndexBuildResult, IndexStats};
use fileconv_knowledge::KnowledgeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8088/v1";
const DEFAULT_API_KEY: &str = "dev-embedding-key";
const DEFAULT_MODEL: &str = "AITeamVN/Vietnamese_Embedding";
const DEFAULT_REVISION: &str = "dea33aa1ab339f38d66ae0a40e6c40e0a9249568";
const DEFAULT_DIMENSIONS: usize = 1024;
const DEFAULT_RUNTIME_PATH: &str = "local-neural";
const QUERY_LIMIT: usize = 10;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceLock {
    documents: usize,
    sources: Vec<Source>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    id: String,
    title: String,
    filename: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Clone)]
struct Query {
    id: String,
    category: String,
    text: String,
    relevant_source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct BlindQuery {
    question_id: String,
    topic_index: usize,
    intent: String,
    question: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    schema_version: u32,
    kind: &'static str,
    non_gating: bool,
    generated_at_epoch_seconds: u64,
    source_lock_sha256: String,
    documents: usize,
    chunks: usize,
    queries: usize,
    conversion: ConversionSummary,
    embedding: EmbeddingSummary,
    index: IndexBuildResult,
    index_stats: IndexStats,
    retrieval: RetrievalSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionSummary {
    successful: usize,
    non_empty: usize,
    cached: usize,
    success_rate: f64,
    non_empty_rate: f64,
    median_elapsed_ms: f64,
    p95_elapsed_ms: f64,
    median_markdown_chars: f64,
    rows: Vec<ConversionRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionRow {
    document_id: String,
    filename: String,
    success: bool,
    cached: bool,
    elapsed_ms: f64,
    markdown_chars: usize,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingSummary {
    provider: &'static str,
    base_url: String,
    model: String,
    revision: String,
    dimensions: usize,
    runtime_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RetrievalSummary {
    ranking_path: &'static str,
    query_provenance: String,
    overall: Metrics,
    by_category: BTreeMap<String, Metrics>,
    rows: Vec<QueryRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryRow {
    query_id: String,
    category: String,
    query: String,
    relevant_source: String,
    first_relevant_rank: Option<usize>,
    recall_at_5: f64,
    recall_at_10: f64,
    hit_at_5: f64,
    mrr: f64,
    ndcg_at_10: f64,
    hits: Vec<HybridSearchHit>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Metrics {
    queries: usize,
    recall_at_5: f64,
    recall_at_10: f64,
    hit_at_5: f64,
    mrr: f64,
    ndcg_at_10: f64,
}

struct Args {
    lock: PathBuf,
    originals: PathBuf,
    work: PathBuf,
    output: PathBuf,
    query_set: Option<PathBuf>,
    limit: Option<usize>,
}

fn parse_args() -> Args {
    let mut args = env::args_os().skip(1);
    let lock = args.next().map(PathBuf::from);
    let originals = args.next().map(PathBuf::from);
    let work = args.next().map(PathBuf::from);
    let output = args.next().map(PathBuf::from);
    let query_set = args
        .next()
        .map(PathBuf::from)
        .filter(|value| value.as_os_str() != "-");
    let limit = args
        .next()
        .map(|value| {
            value
                .to_string_lossy()
                .parse::<usize>()
                .expect("limit must be a positive integer")
        })
        .filter(|value| *value > 0);
    if args.next().is_some()
        || lock.is_none()
        || originals.is_none()
        || work.is_none()
        || output.is_none()
    {
        panic!(
            "usage: external_rag_pilot <sources.lock.json> <originals-dir> \
             <work-dir> <output.json> [queries.json|-] [limit]"
        );
    }
    Args {
        lock: lock.unwrap(),
        originals: originals.unwrap(),
        work: work.unwrap(),
        output: output.unwrap(),
        query_set,
        limit,
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn embedding_config() -> Result<(EmbeddingConfig, String), Box<dyn Error>> {
    let dimensions = env_value(
        "MARKHAND_EMBEDDING_DIMENSIONS",
        &DEFAULT_DIMENSIONS.to_string(),
    )
    .parse::<usize>()?;
    let revision = env_value("MARKHAND_EMBEDDING_REVISION", DEFAULT_REVISION);
    // Env/config boundary: validate runtime_path via EmbeddingConfig::new.
    let config = EmbeddingConfig::new(
        Provider::OpenAiCompatible,
        env_value("MARKHAND_EMBEDDING_SERVER_API_KEY", DEFAULT_API_KEY),
        env_value("MARKHAND_EMBEDDING_MODEL", DEFAULT_MODEL),
        Some(env_value("MARKHAND_EMBEDDING_BASE_URL", DEFAULT_BASE_URL)),
        Some(dimensions),
        env_value("MARKHAND_EMBEDDING_RUNTIME_PATH", DEFAULT_RUNTIME_PATH),
    )?;
    Ok((config, revision))
}

fn sha256(payload: &[u8]) -> String {
    Sha256::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn checked_source_path(root: &Path, source: &Source) -> Result<PathBuf, Box<dyn Error>> {
    let relative = Path::new(&source.filename);
    if relative.is_absolute()
        || relative.components().count() != 1
        || relative.file_name().and_then(|name| name.to_str()) != Some(source.filename.as_str())
    {
        return Err(format!("unsafe source filename: {}", source.filename).into());
    }
    let path = root.join(relative);
    let payload = fs::read(&path)?;
    if payload.len() as u64 != source.bytes {
        return Err(format!("size mismatch for {}", source.filename).into());
    }
    let digest = sha256(&payload);
    if digest != source.sha256 {
        return Err(format!(
            "SHA-256 mismatch for {}: got {digest}, expected {}",
            source.filename, source.sha256
        )
        .into());
    }
    Ok(path)
}

fn convert_source(
    source: &Source,
    originals: &Path,
    markdown_root: &Path,
    converter: &Converter,
    reuse_markdown: bool,
) -> Result<(Option<CorpusDocument>, ConversionRow), String> {
    let started = Instant::now();
    let source_path = checked_source_path(originals, source).map_err(|error| error.to_string())?;
    let md_name = format!("{}.md", source.id);
    let md_path = markdown_root.join(&md_name);
    if reuse_markdown && md_path.is_file() {
        let markdown = fs::read_to_string(&md_path).map_err(|error| error.to_string())?;
        let markdown_chars = markdown.chars().count();
        return Ok((
            Some(CorpusDocument {
                source_rel: source.filename.clone(),
                md_rel: format!("markdown/{md_name}"),
                format: source_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown")
                    .to_ascii_lowercase(),
                markdown,
            }),
            ConversionRow {
                document_id: source.id.clone(),
                filename: source.filename.clone(),
                success: true,
                cached: true,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                markdown_chars,
                error: None,
            },
        ));
    }
    match converter.convert_path(&source_path) {
        Ok(result) => {
            let markdown_chars = result.markdown.chars().count();
            fs::write(&md_path, &result.markdown).map_err(|error| error.to_string())?;
            Ok((
                Some(CorpusDocument {
                    source_rel: source.filename.clone(),
                    md_rel: format!("markdown/{md_name}"),
                    format: source_path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown")
                        .to_ascii_lowercase(),
                    markdown: result.markdown,
                }),
                ConversionRow {
                    document_id: source.id.clone(),
                    filename: source.filename.clone(),
                    success: true,
                    cached: false,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    markdown_chars,
                    error: None,
                },
            ))
        }
        Err(error) => {
            eprintln!("conversion failed for {}: {error}", source.id);
            Ok((
                None,
                ConversionRow {
                    document_id: source.id.clone(),
                    filename: source.filename.clone(),
                    success: false,
                    cached: false,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    markdown_chars: 0,
                    error: Some(error.to_string()),
                },
            ))
        }
    }
}

fn convert_sources(
    sources: &[Source],
    originals: &Path,
    markdown_root: &Path,
) -> Result<(Vec<CorpusDocument>, Vec<ConversionRow>), Box<dyn Error>> {
    fs::create_dir_all(markdown_root)?;
    let reuse_markdown = env::var("FILECONV_EXTERNAL_REUSE_MARKDOWN")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let workers = env_value("FILECONV_EXTERNAL_CONVERSION_WORKERS", "1")
        .parse::<usize>()?
        .clamp(1, sources.len());
    let mut converted = if workers == 1 {
        let converter = Converter::new();
        sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                (
                    index,
                    convert_source(source, originals, markdown_root, &converter, reuse_markdown),
                )
            })
            .collect::<Vec<_>>()
    } else {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            for _ in 0..workers {
                let sender = sender.clone();
                let next = &next;
                scope.spawn(move || {
                    let converter = Converter::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = sources.get(index) else {
                            break;
                        };
                        let result = convert_source(
                            source,
                            originals,
                            markdown_root,
                            &converter,
                            reuse_markdown,
                        );
                        if sender.send((index, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            receiver.into_iter().collect::<Vec<_>>()
        })
    };
    converted.sort_by_key(|(index, _)| *index);
    let mut documents = Vec::new();
    let mut rows = Vec::with_capacity(sources.len());
    for (index, result) in converted {
        let (document, row) = result.map_err(|error| -> Box<dyn Error> { error.into() })?;
        eprintln!(
            "converted {}/{} {} success={} cached={} chars={}",
            index + 1,
            sources.len(),
            row.document_id,
            row.success,
            row.cached,
            row.markdown_chars
        );
        if let Some(document) = document {
            documents.push(document);
        }
        rows.push(row);
    }
    Ok((documents, rows))
}

fn query_subject(title: &str) -> String {
    let subject = title
        .split_once(':')
        .map(|(_, subject)| subject)
        .unwrap_or(title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .to_string();
    format!("Văn bản nào {}?", lowercase_first(&subject))
}

fn query_identifier(title: &str) -> String {
    let prefix = title
        .split_once(':')
        .map(|(prefix, _)| prefix)
        .unwrap_or(title);
    let prefix = prefix
        .split_once(" của ")
        .map(|(prefix, _)| prefix)
        .unwrap_or(prefix)
        .trim();
    format!("Nội dung chính của {prefix} là gì?")
}

fn lowercase_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn build_metadata_queries(sources: &[Source]) -> Vec<Query> {
    sources
        .iter()
        .flat_map(|source| {
            [
                Query {
                    id: format!("{}-official-subject", source.id),
                    category: "official_subject".into(),
                    text: query_subject(&source.title),
                    relevant_source: source.filename.clone(),
                },
                Query {
                    id: format!("{}-identifier", source.id),
                    category: "identifier".into(),
                    text: query_identifier(&source.title),
                    relevant_source: source.filename.clone(),
                },
            ]
        })
        .collect()
}

fn load_queries(
    sources: &[Source],
    query_set: Option<&Path>,
) -> Result<(Vec<Query>, String), Box<dyn Error>> {
    let Some(path) = query_set else {
        return Ok((
            build_metadata_queries(sources),
            "official-detail-metadata-derived".into(),
        ));
    };
    let specs: Vec<BlindQuery> = serde_json::from_slice(&fs::read(path)?)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut queries = Vec::new();
    for spec in specs {
        if spec.topic_index == 0 {
            return Err(format!("{} has zero topic_index", spec.question_id).into());
        }
        if spec.topic_index > sources.len() {
            continue;
        }
        if !ids.insert(spec.question_id.clone()) {
            return Err(format!("duplicate blind query ID: {}", spec.question_id).into());
        }
        let question = spec.question.trim();
        if question.is_empty() {
            return Err(format!("{} has an empty question", spec.question_id).into());
        }
        queries.push(Query {
            id: spec.question_id,
            category: spec.intent,
            text: question.to_string(),
            relevant_source: sources[spec.topic_index - 1].filename.clone(),
        });
    }
    if queries.is_empty() {
        return Err("blind query set contains no query for selected sources".into());
    }
    Ok((queries, "independent-agent-overview-only".into()))
}

fn embedding_failure(error: impl std::fmt::Display) -> KnowledgeError {
    eprintln!("embedding provider failure: {error}");
    KnowledgeError::EmbeddingProviderFailure
}

fn evaluate_query(query: &Query, hits: Vec<HybridSearchHit>, warnings: Vec<String>) -> QueryRow {
    let first_relevant_rank = hits
        .iter()
        .position(|hit| hit.source_rel == query.relevant_source)
        .map(|index| index + 1);
    let recall_at_5 = first_relevant_rank.is_some_and(|rank| rank <= 5) as u8 as f64;
    let recall_at_10 = first_relevant_rank.is_some_and(|rank| rank <= 10) as u8 as f64;
    let mrr = first_relevant_rank
        .map(|rank| 1.0 / rank as f64)
        .unwrap_or(0.0);
    let ndcg_at_10 = first_relevant_rank
        .filter(|rank| *rank <= 10)
        .map(|rank| 1.0 / (rank as f64 + 1.0).log2())
        .unwrap_or(0.0);
    QueryRow {
        query_id: query.id.clone(),
        category: query.category.clone(),
        query: query.text.clone(),
        relevant_source: query.relevant_source.clone(),
        first_relevant_rank,
        recall_at_5,
        recall_at_10,
        hit_at_5: recall_at_5,
        mrr,
        ndcg_at_10,
        hits,
        warnings,
    }
}

fn summarize(rows: &[&QueryRow]) -> Metrics {
    let count = rows.len();
    let divisor = count.max(1) as f64;
    Metrics {
        queries: count,
        recall_at_5: rows.iter().map(|row| row.recall_at_5).sum::<f64>() / divisor,
        recall_at_10: rows.iter().map(|row| row.recall_at_10).sum::<f64>() / divisor,
        hit_at_5: rows.iter().map(|row| row.hit_at_5).sum::<f64>() / divisor,
        mrr: rows.iter().map(|row| row.mrr).sum::<f64>() / divisor,
        ndcg_at_10: rows.iter().map(|row| row.ndcg_at_10).sum::<f64>() / divisor,
    }
}

fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let index = ((ordered.len() - 1) as f64 * fraction).round() as usize;
    ordered[index]
}

fn run(args: Args) -> Result<Summary, Box<dyn Error>> {
    let lock_payload = fs::read(&args.lock)?;
    let lock: SourceLock = serde_json::from_slice(&lock_payload)?;
    if lock.documents != lock.sources.len() {
        return Err("source lock document count does not match sources".into());
    }
    let take = args.limit.unwrap_or(lock.sources.len());
    if take == 0 || take > lock.sources.len() {
        return Err(format!("limit must be between 1 and {}", lock.sources.len()).into());
    }
    let sources = &lock.sources[..take];
    let markdown_root = args.work.join("markdown");
    let index_root = args.work.join("rust-index");
    if index_root.exists() {
        fs::remove_dir_all(&index_root)?;
    }
    fs::create_dir_all(&index_root)?;
    let (documents, conversion_rows) = convert_sources(sources, &args.originals, &markdown_root)?;
    if documents.is_empty() {
        return Err("all document conversions failed".into());
    }

    let (config, revision) = embedding_config()?;
    let base_url = config.base_url.clone().unwrap_or_default();
    let dimensions = config.dimensions.unwrap_or_default();
    let plan = DesktopEmbeddingPlan::provider_with_revision(
        "openaicompatible",
        config.model.clone(),
        revision.clone(),
        Some(&base_url),
        Some(dimensions),
        Some(&config.runtime_path),
    )?;
    let paths = KnowledgePaths::new(index_root.join("knowledge.sqlite"), &index_root);
    let index = rebuild_index(&paths, &documents, &plan, false, |inputs| {
        embed_batch(&config, inputs).map_err(embedding_failure)
    })?;
    let stats = index_stats(&paths)?;

    let (queries, query_provenance) = load_queries(sources, args.query_set.as_deref())?;
    let mut query_rows = Vec::with_capacity(queries.len());
    for (index, query) in queries.iter().enumerate() {
        let response = hybrid_search(
            &paths,
            &[],
            &[],
            &query.text,
            QUERY_LIMIT,
            &plan,
            false,
            |_| Err(KnowledgeError::EmbeddingProviderFailure),
            |text| embed_query(&config, text).map_err(embedding_failure),
        )?;
        query_rows.push(evaluate_query(query, response.hits, response.warnings));
        eprintln!("queried {}/{} {}", index + 1, queries.len(), query.id);
    }
    let overall_refs = query_rows.iter().collect::<Vec<_>>();
    let categories = query_rows
        .iter()
        .map(|row| row.category.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let by_category = categories
        .into_iter()
        .map(|category| {
            let rows = query_rows
                .iter()
                .filter(|row| row.category == category)
                .collect::<Vec<_>>();
            (category, summarize(&rows))
        })
        .collect();
    let elapsed = conversion_rows
        .iter()
        .map(|row| row.elapsed_ms)
        .collect::<Vec<_>>();
    let markdown_chars = conversion_rows
        .iter()
        .map(|row| row.markdown_chars as f64)
        .collect::<Vec<_>>();
    let successful = conversion_rows.iter().filter(|row| row.success).count();
    let cached = conversion_rows.iter().filter(|row| row.cached).count();
    let non_empty = conversion_rows
        .iter()
        .filter(|row| row.success && row.markdown_chars >= 80)
        .count();

    Ok(Summary {
        schema_version: 1,
        kind: "external-public-document-pilot-rust",
        non_gating: true,
        generated_at_epoch_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        source_lock_sha256: sha256(&lock_payload),
        documents: sources.len(),
        chunks: stats.chunks,
        queries: queries.len(),
        conversion: ConversionSummary {
            successful,
            non_empty,
            cached,
            success_rate: successful as f64 / sources.len() as f64,
            non_empty_rate: non_empty as f64 / sources.len() as f64,
            median_elapsed_ms: median(&elapsed),
            p95_elapsed_ms: percentile(&elapsed, 0.95),
            median_markdown_chars: median(&markdown_chars),
            rows: conversion_rows,
        },
        embedding: EmbeddingSummary {
            provider: "openaicompatible",
            base_url,
            model: config.model,
            revision,
            dimensions,
            runtime_path: config.runtime_path,
        },
        index,
        index_stats: stats,
        retrieval: RetrievalSummary {
            ranking_path: "fileconv-knowledge/desktop::service::hybrid_search",
            query_provenance,
            overall: summarize(&overall_refs),
            by_category,
            rows: query_rows,
        },
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();
    let output_path = args.output.clone();
    let summary = run(args)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, serde_json::to_vec_pretty(&summary)?)?;
    Ok(())
}
