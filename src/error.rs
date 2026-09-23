//! Application-wide error type and its Axum response mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Top-level error type returned by every Axum handler in this service.
///
/// Task 3 and Task 5 add `From` impls that convert their own error types
/// into the appropriate `AppError` variant; this module defines only the
/// variants and the response mapping.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The request payload failed `ExtractionPayload::validate`.
    #[error("invalid payload: {0}")]
    InvalidPayload(#[from] crate::model::PayloadError),
    /// A query parameter (e.g. `as_of`) was malformed.
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    /// The mnestic-backed graph store returned an error.
    #[error("graph store error: {0}")]
    Store(String),
    /// The Python research worker failed or returned malformed output.
    #[error("research worker error: {0}")]
    Research(String),
    /// The requested resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<crate::store::StoreError> for AppError {
    /// Wraps a graph store failure as `AppError::Store`, carrying the
    /// store error's formatted message.
    fn from(err: crate::store::StoreError) -> Self {
        AppError::Store(err.to_string())
    }
}

impl From<crate::research::ResearchError> for AppError {
    /// Wraps a research worker failure as `AppError::Research`, carrying
    /// the underlying error's formatted message.
    fn from(err: crate::research::ResearchError) -> Self {
        AppError::Research(err.to_string())
    }
}

impl IntoResponse for AppError {
    /// Maps each variant to an HTTP status code and a JSON body of the form
    /// `{"error": "<message>"}`: `InvalidPayload` to 422, `Store` to 500,
    /// `Research` to 502, and `NotFound` to 404.
    ///
    /// Server-side errors (5xx) are logged with `tracing::error!` (the full
    /// detail, e.g. a Python traceback or a mnestic `Debug` report), but the
    /// response body carries only a short, generic message: 4xx bodies stay
    /// detailed because they are user-facing.
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::InvalidPayload(_) | AppError::InvalidQuery(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            AppError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Research(_) => StatusCode::BAD_GATEWAY,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let message = match &self {
            AppError::InvalidPayload(_) | AppError::InvalidQuery(_) | AppError::NotFound(_) => {
                self.to_string()
            }
            AppError::Store(_) => "graph store error".to_string(),
            AppError::Research(_) => "research worker error".to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;
    use crate::model::PayloadError;

    /// Collects `response`'s body and decodes it as JSON.
    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect response body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("response body is valid JSON")
    }

    #[test]
    fn invalid_payload_maps_to_422() {
        let err = AppError::InvalidPayload(PayloadError::EmptyQuestion);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn store_maps_to_500_with_a_generic_body() {
        let detail = "mnestic script failed: very specific miette report";
        let err = AppError::Store(detail.to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = body_json(response).await;
        let message = body["error"].as_str().expect("error string");
        assert_eq!(message, "graph store error");
        assert!(
            !message.contains(detail),
            "500 body should not contain the store detail: {message}"
        );
    }

    #[tokio::test]
    async fn research_maps_to_502_with_a_generic_body() {
        let detail = "worker exited with status 1: some python traceback";
        let err = AppError::Research(detail.to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let body = body_json(response).await;
        let message = body["error"].as_str().expect("error string");
        assert_eq!(message, "research worker error");
        assert!(
            !message.contains(detail),
            "502 body should not contain the research detail: {message}"
        );
    }

    #[test]
    fn not_found_maps_to_404() {
        let err = AppError::NotFound("dos:missing-dossier".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
