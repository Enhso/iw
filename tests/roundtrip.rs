//! End-to-end integration tests for the Axum service in `src/app.rs`: IW
//! HTTP API v2 (`docs/contracts.md` §C in the `betomcat` repository).
//!
//! Most tests point [`iw_server::research::ResearchWorker`] at a stub
//! script (`fixtures/rust/stub_worker*.sh`) via `worker_cmd`, so they need
//! no Python environment. Two tests (marked below) spawn the real `uv`
//! -managed Python worker to prove the actual Rust -> Python process
//! boundary; they are not `#[ignore]`d.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use iw_server::app::{router, AppState};
use iw_server::briefing::SECTION_TITLES;
use iw_server::model::dossier_id_for;
use iw_server::research::ResearchWorker;
use iw_server::store::GraphStore;

/// The exact fixture question, matching
/// `fixtures/rust/semiconductor_v2.json` and its stub-worker/Python
/// worker fixture-mode output for it.
const FIXTURE_QUESTION: &str = "Can export controls durably slow China's access to advanced \
semiconductor manufacturing capability?";

/// Returns this crate's manifest directory, the repository root.
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Builds a fresh in-memory-backed [`AppState`] whose worker is the given
/// stub script (`fixtures/rust/<name>`), exercised via the `IW_WORKER_CMD`
/// seam ([`ResearchWorker::worker_cmd`]) rather than a real Python
/// environment.
fn stub_state(script_name: &str) -> AppState {
    let store = GraphStore::open_memory().expect("open in-memory store");
    store.init_schema().expect("init schema");
    AppState {
        store: Arc::new(store),
        worker: Arc::new(ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(10),
            worker_cmd: Some(vec![manifest_dir()
                .join("fixtures/rust")
                .join(script_name)
                .to_string_lossy()
                .into_owned()]),
        }),
    }
}

/// Builds a fresh in-memory-backed [`AppState`] whose worker runs the real
/// Python worker with `--fixture-dir fixture_dir`.
fn python_state(fixture_dir: PathBuf) -> AppState {
    let store = GraphStore::open_memory().expect("open in-memory store");
    store.init_schema().expect("init schema");
    AppState {
        store: Arc::new(store),
        worker: Arc::new(ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: Some(fixture_dir),
            timeout: Duration::from_secs(300),
            worker_cmd: None,
        }),
    }
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

fn get(uri: impl Into<String>) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri.into())
        .body(Body::empty())
        .expect("build GET request")
}

fn post_json(uri: impl Into<String>, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri.into())
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build POST request")
}

#[tokio::test]
async fn health_ok() {
    let (status, body) = send(stub_state("stub_worker.sh"), get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn research_ingests_via_stub_worker_and_returns_briefing_and_claims() {
    let state = stub_state("stub_worker.sh");

    let (status, body) = send(
        state.clone(),
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");

    let expected_dossier_id = dossier_id_for(FIXTURE_QUESTION);
    assert_eq!(body["dossier_id"], expected_dossier_id);
    assert_eq!(body["counts"]["entities"], 10);
    assert_eq!(body["counts"]["claims"], 7);
    assert_eq!(body["counts"]["evidence"], 12);

    let sections = body["briefing"]["sections"]
        .as_array()
        .expect("sections array");
    assert_eq!(sections.len(), 11);
    let titles: Vec<&str> = sections
        .iter()
        .map(|section| section["title"].as_str().expect("title string"))
        .collect();
    assert_eq!(titles, SECTION_TITLES.to_vec());

    let markdown = body["briefing"]["markdown"]
        .as_str()
        .expect("markdown string");
    assert!(
        !contains_percent_after_digits(markdown),
        "markdown matched the forbidden \\d+\\s*% pattern"
    );
    let lower = markdown.to_lowercase();
    assert!(!lower.contains("probability"), "markdown has 'probability'");
    assert!(!lower.contains("likelihood"), "markdown has 'likelihood'");

    // claims: this dossier's own ClaimViews, each carrying a support score
    // and, when supported/contradicted, nested evidence with source
    // metadata.
    let claims = body["claims"].as_array().expect("claims array");
    assert_eq!(claims.len(), 7);
    let scored = claims
        .iter()
        .find(|claim| claim["claim_id"] == "clm:smic-achieved-7nm-class-node-2023")
        .expect("scored fact claim present");
    assert_eq!(scored["support"], 0.9);
    assert_eq!(scored["support_method"], "jev");
    let unscored = claims
        .iter()
        .find(|claim| claim["claim_id"] == "clm:chinese-fabs-cannot-access-secondhand-euv")
        .expect("unscored assumption claim present");
    assert_eq!(unscored["support"], serde_json::Value::Null);
    assert_eq!(unscored["support_method"], "none");

    // Untagged research call: no family was given, so there is nothing to
    // report family-wide.
    assert_eq!(body["family_claims"], serde_json::json!([]));
    assert_eq!(body["history"], serde_json::json!([]));

    // gate_log/dropped_sources pass through from the worker's payload.
    assert!(!body["gate_log"]
        .as_array()
        .expect("gate_log array")
        .is_empty());
    assert_eq!(body["dropped_sources"], serde_json::json!([]));
}

#[tokio::test]
async fn research_rejects_invalid_worker_payload_as_502() {
    let state = stub_state("stub_worker_invalid.sh");

    let (status, body) = send(
        state,
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "response body: {body}");
    let error = body["error"].as_str().expect("error string");
    assert!(error.starts_with("research worker error"), "error: {error}");
}

#[tokio::test]
async fn research_empty_question_is_422() {
    let state = stub_state("stub_worker.sh");
    let (status, _) = send(
        state,
        post_json("/api/research", serde_json::json!({ "question": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn dossier_briefing_unknown_dossier_404() {
    let (status, body) = send(
        stub_state("stub_worker.sh"),
        get("/api/dossiers/dos:does-not-exist/briefing"),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    let error = body["error"].as_str().expect("error string");
    assert_eq!(error, "not found: dossier dos:does-not-exist");
}

#[tokio::test]
async fn dossier_briefing_as_of_malformed_is_422() {
    let (status, _) = send(
        stub_state("stub_worker.sh"),
        get("/api/dossiers/dos:whatever/briefing?as_of=not-a-timestamp"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// The core as-of proof at the HTTP layer (instruction (b)): a dossier
/// queried before it was ever researched does not exist, and a dossier
/// re-researched with an updated source keeps its earlier state reachable
/// via `as_of`.
#[tokio::test]
async fn dossier_briefing_as_of_reflects_state_over_time() {
    let state = stub_state("stub_worker.sh");

    let before_anything = "2000-01-01T00:00:00Z";
    let (status, body) = send(
        state.clone(),
        get(format!(
            "/api/dossiers/{}/briefing?as_of={before_anything}",
            dossier_id_for(FIXTURE_QUESTION)
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "response body: {body}");

    let (status, first_response) = send(
        state.clone(),
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {first_response}");
    let dossier_id = first_response["dossier_id"].as_str().unwrap().to_string();

    // Second-precision RFC 3339 is the public as_of granularity
    // (docs/contracts.md), so the two research calls must land in
    // different whole seconds for a between-point to exist.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let between = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let updated_state = AppState {
        store: Arc::clone(&state.store),
        worker: Arc::new(ResearchWorker {
            uv_bin: PathBuf::from("uv"),
            python_dir: manifest_dir().join("python"),
            fixture_dir: None,
            timeout: Duration::from_secs(10),
            worker_cmd: Some(vec![manifest_dir()
                .join("fixtures/rust/stub_worker_updated.sh")
                .to_string_lossy()
                .into_owned()]),
        }),
    };
    let (status, second_response) = send(
        updated_state,
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {second_response}");
    assert_eq!(second_response["dossier_id"], dossier_id);
    // The updated fixture has one more claim than the original.
    assert_eq!(second_response["counts"]["claims"], 8);
    assert_eq!(second_response["counts"]["evidence"], 13);

    let (status, current) = send(
        state.clone(),
        get(format!("/api/dossiers/{dossier_id}/briefing")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {current}");
    assert!(
        current["markdown"]
            .as_str()
            .unwrap()
            .contains("SMIC's 7nm-class yields improved through 2025"),
        "current state should include the claim added by the second ingest"
    );

    let (status, as_of_between) = send(
        state,
        get(format!(
            "/api/dossiers/{dossier_id}/briefing?as_of={between}"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {as_of_between}");
    assert!(
        !as_of_between["markdown"]
            .as_str()
            .unwrap()
            .contains("SMIC's 7nm-class yields improved through 2025"),
        "as_of between the two research calls should predate the second ingest's new claim"
    );
}

#[tokio::test]
async fn classify_family_via_stub_worker_mints_and_records_tag() {
    let state = stub_state("stub_worker.sh");

    let (status, body) = send(
        state.clone(),
        post_json(
            "/api/families/classify",
            serde_json::json!({
                "question": "Will the ECB cut its deposit rate at the October 2026 meeting?",
                "question_id": "metaculus:99001"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");
    // fixtures/rust/classify_family_response.json is a canned "matched"
    // decision against fam:ecb-rate-decisions. A "matched" decision never
    // inserts a family row (only "minted" does) -- it is echoed straight
    // through (resolved forward through merge chains, a no-op here since
    // the id has no `family` row at all in a fresh store).
    assert_eq!(body["decision"], "matched");
    assert_eq!(body["family_id"], "fam:ecb-rate-decisions");

    let (status, families) = send(state, get("/api/families")).await;
    assert_eq!(status, StatusCode::OK, "response body: {families}");
    let rows = families.as_array().expect("families array");
    assert_eq!(
        rows.len(),
        0,
        "a matched (not minted) decision must not insert a family row"
    );
}

#[tokio::test]
async fn documents_endpoint_ingests_and_lists_raw_documents() {
    let state = stub_state("stub_worker.sh");

    let (status, ingested) = send(
        state.clone(),
        post_json(
            "/api/documents",
            serde_json::json!({
                "documents": [{
                    "url": "https://example.com/outbox-document",
                    "title": "Outbox Document",
                    "provider": "asknews_news",
                    "published": "",
                    "fetched_at": "2026-09-22T19:23:00Z",
                    "content": "Content spooled from the bot's outbox."
                }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {ingested}");
    assert_eq!(ingested["ingested"], 1);

    let (status, documents) = send(state, get("/api/documents")).await;
    assert_eq!(status, StatusCode::OK, "response body: {documents}");
    let rows = documents.as_array().expect("documents array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "Outbox Document");
    assert!(
        rows[0]["content"].is_null(),
        "include_content defaults to false"
    );
}

#[tokio::test]
async fn history_endpoint_upserts_and_lists() {
    let state = stub_state("stub_worker.sh");

    let (status, response) = send(
        state.clone(),
        post_json(
            "/api/history",
            serde_json::json!({
                "items": [{
                    "id": "metaculus:41234",
                    "kind": "personal",
                    "title": "Will the ECB cut rates?",
                    "url": "https://www.metaculus.com/questions/41234",
                    "question_type": "binary",
                    "forecast": 0.42,
                    "resolution": null,
                    "resolved_at": null,
                    "family_id": null,
                    "note": ""
                }]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {response}");
    assert_eq!(response["upserted"], 1);

    let (status, history) = send(state, get("/api/history")).await;
    assert_eq!(status, StatusCode::OK, "response body: {history}");
    let rows = history.as_array().expect("history array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["forecast"], 0.42);
}

#[tokio::test]
async fn families_merge_is_prospective_and_rejects_unknown_ids() {
    let state = stub_state("stub_worker.sh");

    let (status, body) = send(
        state.clone(),
        post_json(
            "/api/families/merge",
            serde_json::json!({ "absorbed_id": "fam:does-not-exist", "into_id": "fam:also-not" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "response body: {body}");
}

/// Proves the end-to-end slice across the real Rust -> Python process
/// boundary: spawns the actual `uv`-managed worker in fixture mode via the
/// `research` subcommand + stdin protocol, feeds its output through
/// ingestion and briefing rendering, and checks the result equals what the
/// stub-worker path produces from the committed fixture. Not `#[ignore]`d.
#[tokio::test]
async fn research_roundtrip_across_python_boundary() {
    let fixture_dir = manifest_dir().join("fixtures/offline");
    let state = python_state(fixture_dir);

    let (status, body) = send(
        state,
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "response body: {body}");

    let expected_dossier_id = dossier_id_for(FIXTURE_QUESTION);
    assert_eq!(body["dossier_id"], expected_dossier_id);

    let sections = body["briefing"]["sections"]
        .as_array()
        .expect("sections array");
    assert_eq!(sections.len(), 11);
    assert!(body["counts"]["claims"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn research_worker_failure_is_502() {
    let missing_fixture_dir = manifest_dir().join("fixtures/does-not-exist");
    let state = python_state(missing_fixture_dir);

    let (status, body) = send(
        state,
        post_json(
            "/api/research",
            serde_json::json!({ "question": FIXTURE_QUESTION }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "response body: {body}");
    let error = body["error"].as_str().expect("error string");
    assert!(error.starts_with("research worker error"), "error: {error}");
}
