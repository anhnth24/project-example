use std::path::PathBuf;

use fileconv_core::intelligence::CorpusDocument;
use fileconv_knowledge::desktop::service::{
    grounded_answer, hybrid_search, rebuild_index, DesktopEmbeddingPlan, KnowledgePaths,
};
use fileconv_knowledge::types::{HybridAskRequest, HybridSearchHit};
use fileconv_knowledge::{KnowledgeError, Result};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    database: PathBuf,
    ann_root: PathBuf,
    documents: Vec<CorpusDocument>,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Query {
    query_id: String,
    text: String,
    answer_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Output {
    index: fileconv_knowledge::types::IndexBuildResult,
    queries: Vec<QueryResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResult {
    query_id: String,
    expected_answer_mode: String,
    hits: Vec<HybridSearchHit>,
    answer: String,
    actual_answer_mode: String,
    grounded: bool,
    warnings: Vec<String>,
}

fn unavailable<T>() -> Result<T> {
    Err(KnowledgeError::AdapterUnavailable(
        "provider must not be called in local baseline",
    ))
}

fn run(input: Input) -> Result<Output> {
    let paths = KnowledgePaths::new(input.database, input.ann_root);
    let plan = DesktopEmbeddingPlan::local();
    let index = rebuild_index(&paths, &input.documents, &plan, false, |_| unavailable())?;
    let mut query_results = Vec::with_capacity(input.queries.len());
    for query in input.queries {
        let search = hybrid_search(
            &paths,
            &[],
            &[],
            &query.text,
            10,
            &plan,
            false,
            |_| unavailable(),
            |_| unavailable(),
        )?;
        let request = HybridAskRequest {
            source_rels: Vec::new(),
            question: query.text,
            top_k: Some(10),
            use_llm: Some(false),
        };
        let answer = grounded_answer(&request, search.clone(), None, None, None, |_, _| {
            unavailable()
        })?;
        query_results.push(QueryResult {
            query_id: query.query_id,
            expected_answer_mode: query.answer_mode,
            hits: search.hits,
            answer: answer.answer,
            actual_answer_mode: answer.mode,
            grounded: answer.grounded,
            warnings: answer.warnings,
        });
    }
    Ok(Output {
        index,
        queries: query_results,
    })
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let input_path = args
        .next()
        .map(PathBuf::from)
        .expect("usage: p0_desktop_baseline <input.json> <output.json>");
    let output_path = args
        .next()
        .map(PathBuf::from)
        .expect("usage: p0_desktop_baseline <input.json> <output.json>");
    if args.next().is_some() {
        panic!("usage: p0_desktop_baseline <input.json> <output.json>");
    }
    let input: Input =
        serde_json::from_slice(&std::fs::read(input_path).expect("read baseline input"))
            .expect("parse baseline input");
    let output = run(input).expect("run local desktop baseline");
    let encoded = serde_json::to_vec_pretty(&output).expect("encode baseline output");
    std::fs::write(output_path, encoded).expect("write baseline output");
}
