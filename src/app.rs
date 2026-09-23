//! The Axum application: shared state, router, and HTTP handlers for IW
//! HTTP API v2 (`docs/contracts.md` §C in the `betomcat` repository).
//!
//! Every handler that touches the mnestic store runs it on a blocking task
//! via [`with_store`]/[`with_store_mut`], since `GraphStore`'s calls are
//! synchronous. Read endpoints accept an `as_of` query parameter (an RFC
//! 3339 UTC string, or the literal `"NOW"`/omitted for current state),
//! validated by [`parse_as_of`] before it ever reaches mnestic.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::briefing::{build_context, render, Briefing};
use crate::error::AppError;
use crate::model::{DroppedSource, ExtractionPayload, GateLogEntry, HistoryItem, PayloadError};
use crate::research::{
    ClassifyFamilyCandidate, ClassifyFamilyRequest, ResearchContext, ResearchFamily,
    ResearchRequest, ResearchWorker,
};
use crate::store::{
    ClaimView, DocumentRow, FamilyRow, GraphStore, IngestReport, RawDocument, StoreError, AS_OF_NOW,
};

/// Shared state for every Axum handler in this service.
#[derive(Clone)]
pub struct AppState {
    /// The mnestic-backed graph store.
    pub store: Arc<GraphStore>,
    /// Launches the Python research worker subprocess.
    pub worker: Arc<ResearchWorker>,
}

/// Builds the Axum router for this service, wiring every IW HTTP API v2
/// route (`docs/contracts.md` §C) to handlers that share `state`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/families/classify", post(classify_family))
        .route("/api/research", post(research))
        .route("/api/families", get(list_families))
        .route("/api/families/{id}/claims", get(family_claims))
        .route("/api/documents", get(list_documents).post(ingest_documents))
        .route("/api/dossiers/{id}/briefing", get(dossier_briefing))
        .route("/api/history", get(list_history).post(post_history))
        .route("/api/families/merge", post(merge_families))
        .with_state(state)
}

/// `GET /health`: a liveness probe.
async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Runs a read-only `GraphStore` closure on a blocking task.
///
/// # Errors
/// Returns [`AppError::Store`] if the blocking task panics or the closure
/// itself returns a [`StoreError`].
async fn with_store<T, F>(store: &Arc<GraphStore>, f: F) -> Result<T, AppError>
where
    F: FnOnce(&GraphStore) -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|err| AppError::Store(err.to_string()))?
        .map_err(AppError::from)
}

/// The current UTC time as a second-precision RFC 3339 string
/// (`docs/contracts.md`'s timestamp convention).
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Validates and normalizes an `as_of` query parameter: `None` becomes
/// [`AS_OF_NOW`]; `Some("NOW")` passes through; anything else must parse as
/// RFC 3339.
///
/// # Errors
/// Returns [`AppError::InvalidQuery`] (422) if `as_of` is set to something
/// that is neither `"NOW"` nor a valid RFC 3339 timestamp.
fn parse_as_of(as_of: Option<String>) -> Result<String, AppError> {
    match as_of {
        None => Ok(AS_OF_NOW.to_string()),
        Some(value) if value == AS_OF_NOW => Ok(value),
        Some(value) => {
            chrono::DateTime::parse_from_rfc3339(&value).map_err(|_| {
                AppError::InvalidQuery(format!(
                    "as_of must be \"NOW\" or an RFC 3339 timestamp, got: {value}"
                ))
            })?;
            Ok(value)
        }
    }
}

// --- POST /api/families/classify (docs/contracts.md §C1) -----------------

/// Request body for `POST /api/families/classify`.
#[derive(Debug, Deserialize)]
struct ClassifyRequestBody {
    question: String,
    #[serde(default)]
    question_id: Option<String>,
    #[serde(default)]
    context: Option<ResearchContext>,
}

/// Response body for `POST /api/families/classify`.
#[derive(Debug, Serialize)]
struct ClassifyResponseBody {
    family_id: Option<String>,
    label: Option<String>,
    decision: String,
    probability: Option<f64>,
    method: String,
    gate_log: Vec<GateLogEntry>,
}

/// `POST /api/families/classify`: routes `question` to a live family (or
/// mints a new one), follows `merged_into` chains, and records the
/// question's tag when `question_id` is given.
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if `question` is empty,
/// [`AppError::Research`] (502) if the worker fails, or
/// [`AppError::Store`] (500) if a store operation fails.
async fn classify_family(
    State(state): State<AppState>,
    Json(body): Json<ClassifyRequestBody>,
) -> Result<impl IntoResponse, AppError> {
    if body.question.trim().is_empty() {
        return Err(AppError::InvalidPayload(PayloadError::EmptyQuestion));
    }

    let families = with_store(&state.store, |store| store.all_families(AS_OF_NOW)).await?;
    let candidates: Vec<ClassifyFamilyCandidate> = families
        .iter()
        .filter(|family| family.merged_into.is_none())
        .map(|family| ClassifyFamilyCandidate {
            id: family.id.clone(),
            label: family.label.clone(),
            description: family.description.clone(),
        })
        .collect();

    let worker_response = state
        .worker
        .run_classify_family(&ClassifyFamilyRequest {
            question: body.question.clone(),
            context: body.context.clone(),
            families: candidates,
        })
        .await?;

    let (family_id, label) = match worker_response.decision.as_str() {
        "matched" => {
            let matched_id = worker_response.family_id.clone().ok_or_else(|| {
                AppError::Research("classify-family matched with no family_id".to_string())
            })?;
            let canonical = GraphStore::resolve_family_id(&families, &matched_id);
            let label = families
                .iter()
                .find(|family| family.id == canonical)
                .map(|family| family.label.clone());
            (Some(canonical), label)
        }
        "minted" => {
            let label = worker_response.label.clone().ok_or_else(|| {
                AppError::Research("classify-family minted with no label".to_string())
            })?;
            let description = worker_response.description.clone().unwrap_or_default();
            let created_at = now_rfc3339();
            let label_for_store = label.clone();
            let new_id = with_store(&state.store, move |store| {
                let id = store.mint_family_id(&label_for_store)?;
                store.insert_family(&id, &label_for_store, &description, &created_at)?;
                Ok(id)
            })
            .await?;
            (Some(new_id), Some(label))
        }
        _ => (None, None),
    };

    if let (Some(question_id), Some(family_id)) = (&body.question_id, &family_id) {
        let question_id = question_id.clone();
        let family_id = family_id.clone();
        let method = worker_response.method.clone();
        let question = body.question.clone();
        let probability = worker_response.probability;
        with_store(&state.store, move |store| {
            store.record_question_family(&question_id, &family_id, probability, &method, &question)
        })
        .await?;
    }

    Ok(Json(ClassifyResponseBody {
        family_id,
        label,
        decision: worker_response.decision,
        probability: worker_response.probability,
        method: worker_response.method,
        gate_log: worker_response.gate_log,
    }))
}

// --- POST /api/research (docs/contracts.md §C2) --------------------------

/// Request body for `POST /api/research`.
#[derive(Debug, Deserialize)]
struct ResearchRequestBody {
    question: String,
    #[serde(default)]
    question_id: Option<String>,
    #[serde(default)]
    context: Option<ResearchContext>,
    #[serde(default)]
    providers: Option<Vec<String>>,
    #[serde(default)]
    news_since: Option<String>,
    #[serde(default)]
    max_news: Option<u32>,
    #[serde(default)]
    max_wiki: Option<u32>,
    #[serde(default)]
    family_id: Option<String>,
}

/// Response body for `POST /api/research`.
#[derive(Debug, Serialize)]
struct ResearchResponseBody {
    dossier_id: String,
    as_of: String,
    counts: IngestReport,
    briefing: Briefing,
    claims: Vec<ClaimView>,
    family_claims: Vec<ClaimView>,
    history: Vec<HistoryItem>,
    gate_log: Vec<GateLogEntry>,
    dropped_sources: Vec<DroppedSource>,
}

/// The maximum number of `family_claims` rows returned by `POST
/// /api/research` (`docs/contracts.md` §C2: "up to 30 claims").
const MAX_FAMILY_CLAIMS: usize = 30;

/// `POST /api/research`: resolves `family_id` (and its research history for
/// gap-fill), runs the `research` worker, validates and ingests its output
/// atomically, updates the family's `last_seen`, and returns the dossier's
/// briefing alongside its claims and family/history context.
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if `question` is empty,
/// [`AppError::Research`] (502) if the worker fails or returns an invalid
/// payload, or [`AppError::Store`] (500) if a store operation fails.
async fn research(
    State(state): State<AppState>,
    Json(body): Json<ResearchRequestBody>,
) -> Result<impl IntoResponse, AppError> {
    if body.question.trim().is_empty() {
        return Err(AppError::InvalidPayload(PayloadError::EmptyQuestion));
    }

    // Resolve the family (if any) and its prior last_seen, for the worker
    // request's gap-fill `news_since` (spec s1).
    let resolved_family = match &body.family_id {
        None => None,
        Some(family_id) => {
            let family_id = family_id.clone();
            with_store(&state.store, move |store| {
                let families = store.all_families(AS_OF_NOW)?;
                let canonical = GraphStore::resolve_family_id(&families, &family_id);
                let label = families
                    .iter()
                    .find(|family| family.id == canonical)
                    .map(|family| family.label.clone())
                    .unwrap_or_default();
                let last_seen = store
                    .all_family_seen(AS_OF_NOW)?
                    .into_iter()
                    .find(|(id, _)| *id == canonical)
                    .map(|(_, last_seen)| last_seen);
                Ok((canonical, label, last_seen))
            })
            .await
            .map(Some)?
        }
    };

    let news_since = resolved_family
        .as_ref()
        .and_then(|(_, _, last_seen)| last_seen.clone())
        .or_else(|| body.news_since.clone());

    let worker_request = ResearchRequest {
        question: body.question.clone(),
        question_id: body.question_id.clone(),
        context: body.context.clone(),
        family: resolved_family
            .as_ref()
            .map(|(id, label, last_seen)| ResearchFamily {
                id: id.clone(),
                label: label.clone(),
                last_seen: last_seen.clone(),
            }),
        providers: body.providers.clone(),
        news_since,
        max_news: body.max_news,
        max_wiki: body.max_wiki,
    };

    let payload: ExtractionPayload = state.worker.run_research(&worker_request).await?;
    let gate_log = payload.gate_log.clone();
    let dropped_sources = payload.dropped_sources.clone();

    let created_at = now_rfc3339();
    let question_id = body.question_id.clone();
    let created_at_for_ingest = created_at.clone();
    let counts = with_store(&state.store, move |store| {
        store.ingest(&payload, question_id.as_deref(), &created_at_for_ingest)
    })
    .await?;
    let dossier_id = counts.dossier_id.clone();

    if let Some((canonical, _, _)) = &resolved_family {
        let canonical = canonical.clone();
        let created_at = created_at.clone();
        with_store(&state.store, move |store| {
            store.record_family_seen(&canonical, &created_at)
        })
        .await?;
    }

    let dossier_id_for_briefing = dossier_id.clone();
    let briefing = with_store(&state.store, move |store| {
        let context = build_context(store, &dossier_id_for_briefing, AS_OF_NOW)?;
        Ok(context.map(|ctx| render(&ctx)))
    })
    .await?
    .ok_or_else(|| AppError::Store("dossier missing immediately after ingest".to_string()))?;

    let dossier_id_for_claims = dossier_id.clone();
    let claims = with_store(&state.store, move |store| {
        store.claim_views(&dossier_id_for_claims, AS_OF_NOW)
    })
    .await?;

    let (family_claims, history) = match &resolved_family {
        None => (vec![], vec![]),
        Some((canonical, _, _)) => {
            let canonical = canonical.clone();
            let current_dossier_id = dossier_id.clone();
            let family_claims = with_store(&state.store, move |store| {
                let mut dossier_ids = store.dossiers_for_family(&canonical, AS_OF_NOW)?;
                dossier_ids.retain(|id| *id != current_dossier_id);
                store.claim_views_for_dossiers(&dossier_ids, AS_OF_NOW, MAX_FAMILY_CLAIMS)
            })
            .await?;

            let canonical = resolved_family.as_ref().unwrap().0.clone();
            let history = with_store(&state.store, move |store| {
                let items = store.all_history(AS_OF_NOW)?;
                let families = store.all_families(AS_OF_NOW)?;
                Ok(items
                    .into_iter()
                    .filter(|item| {
                        item.family_id.as_deref().is_some_and(|item_family| {
                            GraphStore::resolve_family_id(&families, item_family) == canonical
                        })
                    })
                    .collect::<Vec<_>>())
            })
            .await?;

            (family_claims, history)
        }
    };

    Ok(Json(ResearchResponseBody {
        dossier_id,
        as_of: created_at,
        counts,
        briefing,
        claims,
        family_claims,
        history,
        gate_log,
        dropped_sources,
    }))
}

// --- GET /api/families (docs/contracts.md §C3) ----------------------------

/// One row of `GET /api/families`.
#[derive(Debug, Serialize)]
struct FamilyView {
    id: String,
    label: String,
    description: String,
    created_at: String,
    merged_into: Option<String>,
    last_seen: Option<String>,
    /// The number of `question_family` tags directly naming this family id
    /// (not resolved through `merged_into`, so a merged-away family's
    /// historical tag count stays visible rather than collapsing to 0).
    question_count: i64,
}

/// `GET /api/families`: every family, live and merged-away alike, as of
/// current state.
///
/// # Errors
/// Returns [`AppError::Store`] (500) if a store operation fails.
async fn list_families(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let views = with_store(&state.store, |store| {
        let families = store.all_families(AS_OF_NOW)?;
        let seen: std::collections::HashMap<String, String> =
            store.all_family_seen(AS_OF_NOW)?.into_iter().collect();
        let tags = store.all_question_families(AS_OF_NOW)?;
        Ok(families
            .into_iter()
            .map(|family| {
                let question_count =
                    tags.iter().filter(|tag| tag.family_id == family.id).count() as i64;
                FamilyView {
                    last_seen: seen.get(&family.id).cloned(),
                    question_count,
                    id: family.id,
                    label: family.label,
                    description: family.description,
                    created_at: family.created_at,
                    merged_into: family.merged_into,
                }
            })
            .collect::<Vec<_>>())
    })
    .await?;
    Ok(Json(views))
}

// --- GET /api/families/{id}/claims (docs/contracts.md §C3) ----------------

/// Query parameters for `GET /api/families/{id}/claims`.
#[derive(Debug, Deserialize)]
struct FamilyClaimsQuery {
    as_of: Option<String>,
    limit: Option<usize>,
}

/// `GET /api/families/{id}/claims?as_of&limit`: claims across every dossier
/// tagged (through `merged_into` chains) to family `id`, as of `as_of`,
/// newest dossier first, default limit 50.
///
/// # Errors
/// Returns [`AppError::InvalidQuery`] (422) if `as_of` is malformed, or
/// [`AppError::Store`] (500) if a store operation fails.
async fn family_claims(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FamilyClaimsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let as_of = parse_as_of(query.as_of)?;
    let limit = query.limit.unwrap_or(50);
    let claims = with_store(&state.store, move |store| {
        let families = store.all_families(&as_of)?;
        let canonical = GraphStore::resolve_family_id(&families, &id);
        let dossier_ids = store.dossiers_for_family(&canonical, &as_of)?;
        store.claim_views_for_dossiers(&dossier_ids, &as_of, limit)
    })
    .await?;
    Ok(Json(claims))
}

// --- GET/POST /api/documents (docs/contracts.md §C3/§C4) -----------------

/// Query parameters for `GET /api/documents`.
#[derive(Debug, Deserialize)]
struct DocumentsQuery {
    as_of: Option<String>,
    family_id: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    include_content: bool,
}

/// `GET /api/documents?as_of&family_id&limit&include_content`: the latest
/// version of each document as of `as_of`, optionally scoped to a family,
/// default limit 50.
///
/// # Errors
/// Returns [`AppError::InvalidQuery`] (422) if `as_of` is malformed, or
/// [`AppError::Store`] (500) if a store operation fails.
async fn list_documents(
    State(state): State<AppState>,
    Query(query): Query<DocumentsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let as_of = parse_as_of(query.as_of)?;
    let limit = query.limit.unwrap_or(50);
    let include_content = query.include_content;
    let documents: Vec<DocumentRow> = with_store(&state.store, move |store| {
        store.documents(&as_of, query.family_id.as_deref(), limit, include_content)
    })
    .await?;
    Ok(Json(documents))
}

/// One raw document in a `POST /api/documents` request body.
#[derive(Debug, Deserialize)]
struct RawDocumentBody {
    url: String,
    title: String,
    provider: String,
    #[serde(default)]
    published: String,
    fetched_at: String,
    content: String,
}

/// Request body for `POST /api/documents`.
#[derive(Debug, Deserialize)]
struct IngestDocumentsRequest {
    documents: Vec<RawDocumentBody>,
}

/// `POST /api/documents`: ingests raw documents without extraction (bot
/// outbox replay / degraded mode); Rust computes each document's
/// content-addressed id and hash.
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if any document's `url` is
/// empty, or [`AppError::Store`] (500) if the write fails.
async fn ingest_documents(
    State(state): State<AppState>,
    Json(body): Json<IngestDocumentsRequest>,
) -> Result<impl IntoResponse, AppError> {
    for document in &body.documents {
        if document.url.trim().is_empty() {
            return Err(AppError::InvalidPayload(PayloadError::EmptyQuestion));
        }
    }
    let documents: Vec<RawDocument> = body
        .documents
        .into_iter()
        .map(|document| RawDocument {
            url: document.url,
            title: document.title,
            provider: document.provider,
            published: document.published,
            fetched_at: document.fetched_at,
            content: document.content,
        })
        .collect();
    let ingested = with_store(&state.store, move |store| {
        store.ingest_raw_documents(&documents)
    })
    .await?;
    Ok(Json(json!({ "ingested": ingested })))
}

// --- GET /api/dossiers/{id}/briefing (docs/contracts.md §C3) -------------

/// Query parameters for `GET /api/dossiers/{id}/briefing`.
#[derive(Debug, Deserialize)]
struct BriefingQuery {
    as_of: Option<String>,
}

/// `GET /api/dossiers/{id}/briefing?as_of`: the briefing as it would have
/// rendered as of `as_of`.
///
/// # Errors
/// Returns [`AppError::InvalidQuery`] (422) if `as_of` is malformed,
/// [`AppError::NotFound`] (404) if the dossier did not exist yet as of
/// `as_of`, or [`AppError::Store`] (500) if a query fails.
async fn dossier_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<BriefingQuery>,
) -> Result<impl IntoResponse, AppError> {
    let as_of = parse_as_of(query.as_of)?;
    let dossier_id = id.clone();
    let briefing = with_store(&state.store, move |store| {
        let context = build_context(store, &dossier_id, &as_of)?;
        Ok(context.map(|ctx| render(&ctx)))
    })
    .await?;
    match briefing {
        Some(briefing) => Ok(Json(briefing)),
        None => Err(AppError::NotFound(format!("dossier {id}"))),
    }
}

// --- GET/POST /api/history (docs/contracts.md §C4) ------------------------

/// Query parameters for `GET /api/history`.
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    family_id: Option<String>,
    kind: Option<String>,
}

/// `GET /api/history?family_id&kind`: personal/bot forecast history,
/// current state, optionally filtered to a family (resolved through
/// `merged_into` chains, both sides) and/or a `kind`.
///
/// # Errors
/// Returns [`AppError::Store`] (500) if a store operation fails.
async fn list_history(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    let items = with_store(&state.store, move |store| {
        let mut items = store.all_history(AS_OF_NOW)?;
        if let Some(kind) = &query.kind {
            items.retain(|item| &item.kind == kind);
        }
        if let Some(family_id) = &query.family_id {
            let families = store.all_families(AS_OF_NOW)?;
            let canonical = GraphStore::resolve_family_id(&families, family_id);
            items.retain(|item| {
                item.family_id.as_deref().is_some_and(|item_family| {
                    GraphStore::resolve_family_id(&families, item_family) == canonical
                })
            });
        }
        Ok(items)
    })
    .await?;
    Ok(Json(items))
}

/// Request body for `POST /api/history`.
#[derive(Debug, Deserialize)]
struct PostHistoryRequest {
    items: Vec<HistoryItem>,
}

/// `POST /api/history`: upserts history items (each a new version).
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if any item fails
/// [`HistoryItem::validate`], or [`AppError::Store`] (500) if the write
/// fails.
async fn post_history(
    State(state): State<AppState>,
    Json(body): Json<PostHistoryRequest>,
) -> Result<impl IntoResponse, AppError> {
    for item in &body.items {
        item.validate()?;
    }
    let count = body.items.len();
    with_store(&state.store, move |store| store.upsert_history(&body.items)).await?;
    Ok(Json(json!({ "upserted": count })))
}

// --- POST /api/families/merge (docs/contracts.md §C4) --------------------

/// Request body for `POST /api/families/merge`.
#[derive(Debug, Deserialize)]
struct MergeFamiliesRequest {
    absorbed_id: String,
    into_id: String,
}

/// `POST /api/families/merge`: sets `absorbed_id`'s `merged_into` to
/// `into_id` (prospective only; existing tags are untouched and resolved
/// forward at read time).
///
/// # Errors
/// Returns [`AppError::NotFound`] (404) if either id does not currently
/// exist, or [`AppError::Store`] (500) if the write fails.
async fn merge_families(
    State(state): State<AppState>,
    Json(body): Json<MergeFamiliesRequest>,
) -> Result<impl IntoResponse, AppError> {
    let MergeFamiliesRequest {
        absorbed_id,
        into_id,
    } = body;
    let absorbed_id_check = absorbed_id.clone();
    let into_id_check = into_id.clone();
    let existing: Vec<FamilyRow> =
        with_store(&state.store, |store| store.all_families(AS_OF_NOW)).await?;
    if !existing.iter().any(|family| family.id == absorbed_id_check) {
        return Err(AppError::NotFound(format!("family {absorbed_id_check}")));
    }
    if !existing.iter().any(|family| family.id == into_id_check) {
        return Err(AppError::NotFound(format!("family {into_id_check}")));
    }

    let absorbed_for_store = absorbed_id.clone();
    let into_for_store = into_id.clone();
    with_store(&state.store, move |store| {
        store.merge_family(&absorbed_for_store, &into_for_store)
    })
    .await?;

    Ok(Json(
        json!({ "absorbed_id": absorbed_id, "into_id": into_id }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_as_of_defaults_to_now() {
        assert_eq!(parse_as_of(None).expect("default"), AS_OF_NOW);
    }

    #[test]
    fn parse_as_of_accepts_now_literal() {
        assert_eq!(
            parse_as_of(Some("NOW".to_string())).expect("NOW literal"),
            "NOW"
        );
    }

    #[test]
    fn parse_as_of_accepts_rfc3339() {
        let value = "2026-09-22T19:23:00Z".to_string();
        assert_eq!(
            parse_as_of(Some(value.clone())).expect("valid rfc3339"),
            value
        );
    }

    #[test]
    fn parse_as_of_rejects_malformed_timestamp() {
        let result = parse_as_of(Some("not-a-timestamp".to_string()));
        assert!(matches!(result, Err(AppError::InvalidQuery(_))));
    }
}
