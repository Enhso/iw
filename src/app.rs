//! The Axum application: shared state, router, and HTTP handlers wiring the
//! mnestic graph store and the Python research worker into the Question ->
//! Extraction -> Ingestion -> Retrieval -> Briefing slice.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::briefing::{build_context, render, Briefing};
use crate::error::AppError;
use crate::model::{ExtractionPayload, PayloadError};
use crate::research::ResearchWorker;
use crate::store::{GraphStore, IngestReport, StoreError};

/// Shared state for every Axum handler in this service.
#[derive(Clone)]
pub struct AppState {
    /// The mnestic-backed graph store.
    pub store: Arc<GraphStore>,
    /// Launches the Python research worker subprocess.
    pub worker: Arc<ResearchWorker>,
}

/// Builds the Axum router for this service, wiring every route to
/// handlers that share `state`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/ingest", post(ingest))
        .route("/api/dossiers/{id}/briefing", get(dossier_briefing))
        .route("/api/questions", post(ask_question))
        .with_state(state)
}

/// `GET /health`: a liveness probe.
async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Response body for `POST /api/ingest`: the derived dossier id and
/// per-list ingestion counts.
#[derive(Debug, Serialize)]
struct IngestResponse {
    /// The dossier id the payload was ingested under.
    dossier_id: String,
    /// Per-list ingestion counts.
    counts: IngestReport,
}

/// `POST /api/ingest`: validates and ingests an [`ExtractionPayload`]
/// directly, without invoking the research worker.
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if the payload fails
/// [`ExtractionPayload::validate`], or [`AppError::Store`] (500) if
/// ingestion fails.
async fn ingest(
    State(state): State<AppState>,
    Json(payload): Json<ExtractionPayload>,
) -> Result<impl IntoResponse, AppError> {
    payload.validate()?;
    let report = ingest_payload(&state.store, payload).await?;
    let dossier_id = report.dossier_id.clone();
    Ok(Json(IngestResponse {
        dossier_id,
        counts: report,
    }))
}

/// `GET /api/dossiers/{id}/briefing`: renders the briefing for an already
/// ingested dossier.
///
/// # Errors
/// Returns [`AppError::NotFound`] (404) if no dossier with `id` has been
/// ingested, or [`AppError::Store`] (500) if a query fails.
async fn dossier_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    match render_briefing(&state.store, id.clone()).await? {
        Some(briefing) => Ok(Json(briefing)),
        None => Err(AppError::NotFound(format!("dossier {id}"))),
    }
}

/// Request body for `POST /api/questions`.
#[derive(Debug, Deserialize)]
struct QuestionRequest {
    /// The research question to answer.
    question: String,
}

/// Response body for `POST /api/questions`: the dossier id, ingestion
/// counts, and the rendered briefing, all from one round trip through the
/// research worker.
#[derive(Debug, Serialize)]
struct QuestionResponse {
    /// The dossier id the worker's output was ingested under.
    dossier_id: String,
    /// Per-list ingestion counts.
    counts: IngestReport,
    /// The rendered briefing for the newly ingested dossier.
    briefing: Briefing,
}

/// `POST /api/questions`: runs the research worker for `question`,
/// ingests its output, and returns the rendered briefing.
///
/// # Errors
/// Returns [`AppError::InvalidPayload`] (422) if `question` is empty or
/// whitespace-only, [`AppError::Research`] (502) if the worker fails or
/// returns an invalid payload, or [`AppError::Store`] (500) if ingestion or
/// briefing rendering fails.
async fn ask_question(
    State(state): State<AppState>,
    Json(request): Json<QuestionRequest>,
) -> Result<impl IntoResponse, AppError> {
    if request.question.trim().is_empty() {
        return Err(AppError::InvalidPayload(PayloadError::EmptyQuestion));
    }

    let payload = state.worker.run(&request.question).await?;
    let report = ingest_payload(&state.store, payload).await?;
    let dossier_id = report.dossier_id.clone();

    let briefing = render_briefing(&state.store, dossier_id.clone())
        .await?
        .ok_or_else(|| AppError::Store("dossier missing immediately after ingest".to_string()))?;

    Ok(Json(QuestionResponse {
        dossier_id,
        counts: report,
        briefing,
    }))
}

/// Ingests `payload` into `store` on a blocking task, with `created_at`
/// set to the current UTC time.
///
/// # Errors
/// Returns [`AppError::Store`] if the blocking task panics or ingestion
/// itself fails.
async fn ingest_payload(
    store: &Arc<GraphStore>,
    payload: ExtractionPayload,
) -> Result<IngestReport, AppError> {
    let store = Arc::clone(store);
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    tokio::task::spawn_blocking(move || store.ingest(&payload, &created_at))
        .await
        .map_err(|err| AppError::Store(err.to_string()))?
        .map_err(AppError::from)
}

/// Builds and renders the briefing for `dossier_id` on a blocking task.
///
/// # Returns
/// `Ok(None)` if no dossier with `dossier_id` has been ingested.
///
/// # Errors
/// Returns [`AppError::Store`] if the blocking task panics or a query
/// fails.
async fn render_briefing(
    store: &Arc<GraphStore>,
    dossier_id: String,
) -> Result<Option<Briefing>, AppError> {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || -> Result<Option<Briefing>, StoreError> {
        let context = build_context(&store, &dossier_id)?;
        Ok(context.map(|ctx| render(&ctx)))
    })
    .await
    .map_err(|err| AppError::Store(err.to_string()))?
    .map_err(AppError::from)
}
