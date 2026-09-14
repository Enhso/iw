//! End-to-end integration tests for the Axum service in `src/app.rs`:
//! Question -> Extraction -> Ingestion -> Retrieval -> Briefing, including
//! a round trip across the real Rust -> Python process boundary.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use iw_server::app::{router, AppState};
use iw_server::briefing::SECTION_TITLES;
use iw_server::model::{dossier_id_for, ExtractionPayload};
use iw_server::research::ResearchWorker;
use iw_server::store::GraphStore;

/// The exact fixture question, matching `fixtures/payload/semiconductor.json`
/// and the Python worker's fixture-mode output for it.
const FIXTURE_QUESTION: &str = "Can export controls durably slow China's access to advanced \
semiconductor manufacturing capability?";

const FIXTURE_PAYLOAD_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/payload/semiconductor.json"
));

/// Returns this crate's manifest directory, the repository root.
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Builds a fresh in-memory-backed [`AppState`] whose research worker is
/// never expected to run (tests that only hit `/api/ingest` and
/// `/api/dossiers/{id}/briefing`).
fn test_state() -> AppState {
    let store = GraphStore::open_memory().expect("open in-memory store");
    store.init_schema().expect("init schema");
    AppState {
        store: Arc::new(store),
        worker: Arc::new(ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(300),
        }),
    }
}

/// Builds a fresh in-memory-backed [`AppState`] whose worker runs the real
/// Python worker with `--fixture-dir fixture_dir`.
fn test_state_with_worker(fixture_dir: PathBuf) -> AppState {
    let store = GraphStore::open_memory().expect("open in-memory store");
    store.init_schema().expect("init schema");
    AppState {
        store: Arc::new(store),
        worker: Arc::new(ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(fixture_dir),
            timeout: Duration::from_secs(300),
        }),
    }
}

/// Parses the committed fixture payload.
fn fixture_payload() -> ExtractionPayload {
    serde_json::from_str(FIXTURE_PAYLOAD_JSON).expect("fixture payload parses")
}

/// Manual scanner for the Global Constraint 1 regex `\d+\s*%`: a run of
/// ASCII digits followed by optional whitespace then a percent sign. No
/// regex crate per the task brief.
fn contains_percent_after_digits(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let mut idx = start + 1;
        while idx < chars.len() && chars[idx].is_ascii_digit() {
            idx += 1;
        }
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx < chars.len() && chars[idx] == '%' {
            return true;
        }
    }
    false
}

/// Sends `request` through `router(state)` via `oneshot` and returns the
/// response's status and decoded JSON body.
async fn send(state: AppState, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router(state)
        .oneshot(request)
        .await
        .expect("router handled request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response body is valid JSON")
    };
    (status, body)
}

#[tokio::test]
async fn health_ok() {
    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .expect("build request");

    let (status, body) = send(test_state(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn ingest_then_briefing_from_fixture_payload() {
    let state = test_state();

    let ingest_request = Request::builder()
        .method("POST")
        .uri("/api/ingest")
        .header("content-type", "application/json")
        .body(Body::from(FIXTURE_PAYLOAD_JSON))
        .expect("build ingest request");
    let (status, body) = send(state.clone(), ingest_request).await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");

    let payload = fixture_payload();
    let dossier_id = body["dossier_id"]
        .as_str()
        .expect("dossier_id string")
        .to_string();
    assert_eq!(dossier_id, dossier_id_for(&payload.question));
    assert_eq!(body["counts"]["entities"], payload.entities.len());
    assert_eq!(body["counts"]["events"], payload.events.len());
    assert_eq!(body["counts"]["sources"], payload.sources.len());
    assert_eq!(body["counts"]["claims"], payload.claims.len());
    assert_eq!(body["counts"]["evidence"], payload.evidence.len());
    assert_eq!(body["counts"]["causal_links"], payload.causal_links.len());
    assert_eq!(
        body["counts"]["temporal_relations"],
        payload.temporal_relations.len()
    );

    // Read a real crux's text straight from the graph store the request
    // just wrote into, so the assertion below is not tied to which claim
    // happens to be contested in the fixture.
    let crux_text = state
        .store
        .cruxes(&dossier_id)
        .expect("cruxes")
        .first()
        .expect("at least one crux in the fixture dossier")
        .text
        .clone();

    let briefing_request = Request::builder()
        .method("GET")
        .uri(format!("/api/dossiers/{dossier_id}/briefing"))
        .body(Body::empty())
        .expect("build briefing request");
    let (status, body) = send(state, briefing_request).await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");

    let sections = body["sections"].as_array().expect("sections array");
    assert_eq!(sections.len(), 11);
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section["title"].as_str().expect("title string"))
        .collect();
    assert_eq!(titles, SECTION_TITLES.to_vec());

    let markdown = body["markdown"].as_str().expect("markdown string");
    assert!(
        markdown.contains(&crux_text),
        "markdown missing crux text: {crux_text}"
    );
    assert!(
        !contains_percent_after_digits(markdown),
        "markdown matched the forbidden \\d+\\s*% pattern"
    );
    let lower = markdown.to_lowercase();
    assert!(!lower.contains("probability"), "markdown has 'probability'");
    assert!(!lower.contains("likelihood"), "markdown has 'likelihood'");

    let source_appendix = sections
        .iter()
        .find(|section| section["title"] == "Source Appendix")
        .expect("source appendix section");
    let appendix_body = source_appendix["body"].as_str().expect("body string");
    for source in &payload.sources {
        assert!(
            appendix_body.contains(&source.url),
            "Source Appendix missing url: {}",
            source.url
        );
    }
}

#[tokio::test]
async fn ingest_rejects_dangling_reference() {
    let mut payload = fixture_payload();
    payload.evidence[0].claim_id = "clm:does-not-exist".to_string();
    let body = serde_json::to_string(&payload).expect("serialize payload");

    let request = Request::builder()
        .method("POST")
        .uri("/api/ingest")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("build request");
    let (status, body) = send(test_state(), request).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let error = body["error"].as_str().expect("error string");
    assert!(error.contains("clm:does-not-exist"), "error: {error}");
}

#[tokio::test]
async fn briefing_unknown_dossier_404() {
    let request = Request::builder()
        .method("GET")
        .uri("/api/dossiers/dos:does-not-exist/briefing")
        .body(Body::empty())
        .expect("build request");

    let (status, body) = send(test_state(), request).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    let error = body["error"].as_str().expect("error string");
    assert_eq!(error, "not found: dossier dos:does-not-exist");
}

/// Proves the end-to-end slice across the real Rust -> Python process
/// boundary: spawns the actual `uv`-managed worker in fixture mode, feeds
/// its output through ingestion and briefing rendering, and checks the
/// result equals what ingesting the committed payload fixture directly
/// produces. This test spawns a real subprocess and is intentionally not
/// `#[ignore]`d.
#[tokio::test]
async fn question_roundtrip_across_python_boundary() {
    let fixture_dir = manifest_dir().join("fixtures/offline");
    let state = test_state_with_worker(fixture_dir);

    let request = Request::builder()
        .method("POST")
        .uri("/api/questions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "question": FIXTURE_QUESTION }).to_string(),
        ))
        .expect("build request");
    let (status, body) = send(state, request).await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");

    let expected_dossier_id = dossier_id_for(FIXTURE_QUESTION);
    assert_eq!(body["dossier_id"], expected_dossier_id);

    let payload = fixture_payload();
    assert_eq!(body["counts"]["entities"], payload.entities.len());
    assert_eq!(body["counts"]["events"], payload.events.len());
    assert_eq!(body["counts"]["sources"], payload.sources.len());
    assert_eq!(body["counts"]["claims"], payload.claims.len());
    assert_eq!(body["counts"]["evidence"], payload.evidence.len());
    assert_eq!(body["counts"]["causal_links"], payload.causal_links.len());
    assert_eq!(
        body["counts"]["temporal_relations"],
        payload.temporal_relations.len()
    );

    let sections = body["briefing"]["sections"]
        .as_array()
        .expect("sections array");
    assert_eq!(sections.len(), 11);

    // Prove the worker's output equals the committed payload fixture: the
    // markdown from this Rust -> Python -> Rust round trip must equal the
    // markdown produced by ingesting the fixture payload directly (the
    // same path as `ingest_then_briefing_from_fixture_payload`).
    let direct_state = test_state();
    let ingest_request = Request::builder()
        .method("POST")
        .uri("/api/ingest")
        .header("content-type", "application/json")
        .body(Body::from(FIXTURE_PAYLOAD_JSON))
        .expect("build ingest request");
    let (ingest_status, ingest_body) = send(direct_state.clone(), ingest_request).await;
    assert_eq!(
        ingest_status,
        StatusCode::OK,
        "response body: {ingest_body}"
    );
    let direct_dossier_id = ingest_body["dossier_id"]
        .as_str()
        .expect("dossier_id string")
        .to_string();

    let briefing_request = Request::builder()
        .method("GET")
        .uri(format!("/api/dossiers/{direct_dossier_id}/briefing"))
        .body(Body::empty())
        .expect("build briefing request");
    let (briefing_status, direct_briefing) = send(direct_state, briefing_request).await;
    assert_eq!(
        briefing_status,
        StatusCode::OK,
        "response body: {direct_briefing}"
    );

    assert_eq!(body["briefing"]["markdown"], direct_briefing["markdown"]);
}

#[tokio::test]
async fn question_worker_failure_is_502() {
    let missing_fixture_dir = manifest_dir().join("fixtures/does-not-exist");
    let state = test_state_with_worker(missing_fixture_dir);

    let request = Request::builder()
        .method("POST")
        .uri("/api/questions")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "question": FIXTURE_QUESTION }).to_string(),
        ))
        .expect("build request");
    let (status, body) = send(state, request).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let error = body["error"].as_str().expect("error string");
    assert!(error.starts_with("research worker error"), "error: {error}");
}
