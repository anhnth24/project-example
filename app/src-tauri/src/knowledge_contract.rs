use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use fileconv_knowledge::types::{
    GroundedAnswer, HybridAskRequest, HybridSearchRequest, HybridSearchResponse, IndexBuildResult,
    IndexRequest, IndexStats,
};

fn value(source: &str) -> Value {
    serde_json::from_str(source).unwrap()
}

fn assert_round_trip<T>(source: &str)
where
    T: DeserializeOwned + Serialize,
{
    let expected = value(source);
    let decoded: T = serde_json::from_value(expected.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

fn assert_request<T>(source: &str, command: &str)
where
    T: DeserializeOwned + Serialize,
{
    let fixture = value(source);
    assert_eq!(fixture["command"], command);
    let expected = fixture["args"]["req"].clone();
    let decoded: T = serde_json::from_value(expected.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn freezes_four_command_request_contracts() {
    assert_request::<IndexRequest>(
        include_str!("../fixtures/knowledge/v1/requests/rebuild.json"),
        "rebuild_knowledge_index",
    );
    let stats = value(include_str!("../fixtures/knowledge/v1/requests/stats.json"));
    assert_eq!(stats["command"], "knowledge_index_stats");
    assert_eq!(stats["args"], serde_json::json!({}));
    assert_request::<HybridSearchRequest>(
        include_str!("../fixtures/knowledge/v1/requests/search.json"),
        "hybrid_search",
    );
    assert_request::<HybridAskRequest>(
        include_str!("../fixtures/knowledge/v1/requests/ask.json"),
        "hybrid_ask",
    );
}

#[test]
fn freezes_response_casing_modes_anchors_warnings_and_stats() {
    assert_round_trip::<IndexBuildResult>(include_str!(
        "../fixtures/knowledge/v1/responses/rebuild.json"
    ));
    assert_round_trip::<IndexBuildResult>(include_str!(
        "../fixtures/knowledge/v1/responses/rebuild-incremental.json"
    ));
    assert_round_trip::<IndexStats>(include_str!(
        "../fixtures/knowledge/v1/responses/stats.json"
    ));
    assert_round_trip::<HybridSearchResponse>(include_str!(
        "../fixtures/knowledge/v1/responses/search.json"
    ));
    assert_round_trip::<GroundedAnswer>(include_str!(
        "../fixtures/knowledge/v1/responses/ask.json"
    ));
    assert_round_trip::<GroundedAnswer>(include_str!(
        "../fixtures/knowledge/v1/responses/ask-fallback.json"
    ));
}

#[test]
fn score_contract_uses_epsilon_and_exact_hit_order() {
    let response: HybridSearchResponse = serde_json::from_str(include_str!(
        "../fixtures/knowledge/v1/responses/search.json"
    ))
    .unwrap();
    assert_eq!(response.hits[0].source_rel, "payments.pdf");
    assert!((response.hits[0].rerank_score - 1.875).abs() <= 0.0001);
    assert_eq!(response.hits[0].anchor.page, Some(7));
}

#[test]
fn backend_handler_registers_every_frozen_knowledge_command() {
    let handler_source = include_str!("lib.rs");
    let expected = [
        "rebuild_knowledge_index",
        "knowledge_index_stats",
        "hybrid_search",
        "hybrid_ask",
    ];
    assert_eq!(crate::knowledge::KNOWLEDGE_COMMAND_NAMES, expected);
    for command in expected {
        assert!(
            handler_source.contains(&format!("knowledge::{command}")),
            "missing backend registration for {command}"
        );
    }
}
