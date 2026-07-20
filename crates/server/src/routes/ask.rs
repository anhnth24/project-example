//! Ask API routes backed by the grounded QA engine.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::sse::{
    envelope, event_from_envelope, parse_last_event_id, StreamCaller, StreamRegistryError,
};
use crate::api::SseEnvelope;
use crate::auth::context::OrgContext;
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::routes::common::{require_permission_or_403, RestError};
use crate::routes::search::{
    narrowed_context, validate_collection_ids_len, validate_limit, JSON_BODY_LIMIT, MAX_QUERY_CHARS,
};
use crate::services::qa::stream::{DEFAULT_QA_LIMIT, MAX_QA_LIMIT};
use crate::services::qa::{answer_question, QaAnswerMode, QaCitation, QaError, QaEvent, QaRequest};
use crate::services::retrieval::VersionMode;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ask", post(ask))
        .route("/api/v1/ask/stream", post(ask_stream))
        .route_layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskRequestBody {
    question: String,
    limit: Option<u32>,
    collection_ids: Option<Vec<uuid::Uuid>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AskResponse {
    answer: String,
    citations: Vec<QaCitation>,
    warnings: Vec<String>,
    mode: QaAnswerMode,
}

struct ValidAskRequest {
    question: String,
    limit: usize,
    ctx: OrgContext,
}

async fn ask(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    body: Result<Json<AskRequestBody>, JsonRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let Json(body) =
        body.map_err(|_| RestError::validation("request body is invalid", &request_id))?;
    require_permission_or_403(&auth.context, "qa.query", &request_id)?;
    let input = validate_ask_request(body, &auth.context, &request_id)?;
    let stream = answer_question(
        state.pool(),
        state.vector_store(),
        &input.ctx,
        QaRequest {
            question: input.question,
            limit: input.limit,
            mode: VersionMode::Current,
        },
    )
    .await
    .map_err(|error| map_qa_error(error, &request_id))?;
    let response = collect_answer(stream, &request_id).await?;
    Ok(Json(response).into_response())
}

async fn ask_stream(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    require_permission_or_403(&auth.context, "qa.query", &request_id)?;
    let caller = StreamCaller {
        org_id: auth.context.org_id(),
        user_id: auth.context.user_id(),
    };

    if let Some(last_event_id) = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
    {
        let (stream_id, sequence) =
            parse_last_event_id(last_event_id).ok_or_else(|| RestError::not_found(&request_id))?;
        let envelopes = state
            .ask_streams()
            .replay_after(
                stream_id,
                sequence,
                caller,
                auth.context.allowed_collection_ids().iter().copied(),
            )
            .ok_or_else(|| RestError::not_found(&request_id))?;
        let stream = stream::iter(
            envelopes
                .into_iter()
                .map(move |envelope| event_from_envelope(stream_id, &envelope)),
        );
        return Ok(sse_response(stream));
    }

    let body: AskRequestBody = serde_json::from_slice(&body)
        .map_err(|_| RestError::validation("request body is invalid", &request_id))?;
    let input = validate_ask_request(body, &auth.context, &request_id)?;
    let collection_scope = input.ctx.allowed_collection_ids().iter().copied();
    let stream_id = state
        .ask_streams()
        .start_stream(caller, collection_scope)
        .map_err(|error| map_registry_error(error, &request_id))?;
    let qa_stream = answer_question(
        state.pool(),
        state.vector_store(),
        &input.ctx,
        QaRequest {
            question: input.question,
            limit: input.limit,
            mode: VersionMode::Current,
        },
    )
    .await
    .map_err(|error| {
        state.ask_streams().remove(stream_id);
        map_qa_error(error, &request_id)
    })?;
    let stream = qa_sse_stream(
        stream_id,
        request_id.clone(),
        state.ask_streams(),
        qa_stream,
    );
    Ok(sse_response(stream))
}

fn sse_response<S>(stream: S) -> Response
where
    S: futures::Stream<Item = Result<axum::response::sse::Event, Infallible>> + Send + 'static,
{
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

fn qa_sse_stream(
    stream_id: uuid::Uuid,
    request_id: String,
    registry: crate::api::sse::AskStreamRegistry,
    qa_stream: BoxStream<'static, QaEvent>,
) -> BoxStream<'static, Result<axum::response::sse::Event, Infallible>> {
    struct State {
        stream_id: uuid::Uuid,
        request_id: String,
        registry: crate::api::sse::AskStreamRegistry,
        qa_stream: BoxStream<'static, QaEvent>,
        sequence: u64,
    }

    stream::unfold(
        State {
            stream_id,
            request_id,
            registry,
            qa_stream,
            sequence: 0,
        },
        |mut state| async move {
            let Some(event) = state.qa_stream.next().await else {
                state.registry.mark_done(state.stream_id);
                return None;
            };
            state.sequence += 1;
            let envelope = envelope_from_qa_event(state.sequence, &state.request_id, event);
            let is_done = envelope.event == "ask.done";
            let _ = state.registry.append(state.stream_id, envelope.clone());
            if is_done {
                state.registry.mark_done(state.stream_id);
            }
            Some((event_from_envelope(state.stream_id, &envelope), state))
        },
    )
    .boxed()
}

async fn collect_answer(
    mut stream: BoxStream<'static, QaEvent>,
    request_id: &str,
) -> Result<AskResponse, RestError> {
    let mut answer = String::new();
    let mut citations = Vec::new();
    let mut warnings = Vec::new();
    let mut mode = None;
    while let Some(event) = stream.next().await {
        match event {
            QaEvent::Token(token) => answer.push_str(&token),
            QaEvent::Citations(values) => citations.extend(values),
            QaEvent::Warning(warning) => warnings.push(warning),
            QaEvent::Done { mode: value } => mode = Some(value),
        }
    }
    let mode = mode.ok_or_else(|| RestError::internal(request_id))?;
    Ok(AskResponse {
        answer,
        citations,
        warnings,
        mode,
    })
}

fn validate_ask_request(
    body: AskRequestBody,
    ctx: &OrgContext,
    request_id: &str,
) -> Result<ValidAskRequest, RestError> {
    let question = body.question.trim().to_string();
    if question.is_empty() {
        return Err(RestError::validation(
            "question must not be empty",
            request_id,
        ));
    }
    if question.chars().count() > MAX_QUERY_CHARS {
        return Err(RestError::validation("question is too long", request_id));
    }
    validate_collection_ids_len(body.collection_ids.as_ref(), request_id)?;
    Ok(ValidAskRequest {
        question,
        limit: validate_limit(
            body.limit.or(Some(DEFAULT_QA_LIMIT as u32)),
            MAX_QA_LIMIT,
            request_id,
        )?,
        ctx: narrowed_context(ctx, body.collection_ids),
    })
}

fn envelope_from_qa_event(sequence: u64, request_id: &str, event: QaEvent) -> SseEnvelope {
    match event {
        QaEvent::Token(token) => {
            envelope(sequence, "ask.token", request_id, json!({ "token": token }))
        }
        QaEvent::Citations(citations) => envelope(
            sequence,
            "ask.citations",
            request_id,
            json!({ "citations": citations }),
        ),
        QaEvent::Warning(warning) => envelope(
            sequence,
            "ask.warning",
            request_id,
            json!({ "warning": warning }),
        ),
        QaEvent::Done { mode } => {
            envelope(sequence, "ask.done", request_id, json!({ "mode": mode }))
        }
    }
}

fn map_qa_error(error: QaError, request_id: &str) -> RestError {
    match error {
        QaError::EmptyScope => RestError::empty_scope(request_id),
        QaError::Retrieval(_) => RestError::internal(request_id),
    }
}

fn map_registry_error(error: StreamRegistryError, request_id: &str) -> RestError {
    match error {
        StreamRegistryError::TooManyStreams => {
            RestError::too_many_requests("too many active streams", request_id)
        }
        StreamRegistryError::BufferLimitExceeded => {
            RestError::too_many_requests("stream replay buffer limit exceeded", request_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_events_map_to_stable_sse_event_names() {
        let request_id = "request";
        assert_eq!(
            envelope_from_qa_event(1, request_id, QaEvent::Token("hello".into())).event,
            "ask.token"
        );
        assert_eq!(
            envelope_from_qa_event(2, request_id, QaEvent::Warning("warn".into())).event,
            "ask.warning"
        );
        assert_eq!(
            envelope_from_qa_event(
                3,
                request_id,
                QaEvent::Done {
                    mode: QaAnswerMode::OfflineExtractive
                }
            )
            .event,
            "ask.done"
        );
    }

    #[test]
    fn collection_ids_length_is_bounded() {
        let collection = uuid::Uuid::new_v4();
        let ctx = OrgContext::try_new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            ["qa.query"],
            [collection],
        )
        .unwrap();
        let body = AskRequestBody {
            question: "hello?".into(),
            limit: None,
            collection_ids: Some(vec![
                collection;
                crate::routes::search::MAX_COLLECTION_IDS + 1
            ]),
        };

        assert!(validate_ask_request(body, &ctx, "req").is_err());
    }

    #[test]
    fn ask_response_fixture_matches_wire_dto() {
        let response = AskResponse {
            answer: "Payment is due within 30 days. [1]".into(),
            citations: vec![QaCitation {
                document_id: uuid::Uuid::parse_str("d4010000-0000-4000-8000-000000000001").unwrap(),
                version_id: uuid::Uuid::parse_str("e5010000-0000-4000-8000-000000000001").unwrap(),
                version_number: 3,
                content_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
                chunk_id: uuid::Uuid::parse_str("c3010000-0000-4000-8000-000000000001").unwrap(),
                heading_path: vec!["Contract".into(), "Payment".into()],
                snippet: "Payment is due within 30 days.".into(),
                page: Some(4),
                slide: None,
                sheet: None,
                is_current: true,
            }],
            warnings: Vec::new(),
            mode: QaAnswerMode::OfflineExtractive,
        };
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../../openapi/fixtures/ask_response.json")).unwrap();

        assert_eq!(serde_json::to_value(response).unwrap(), expected);
    }
}
