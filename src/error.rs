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
    /// `Research` to 502, and `NotFound` to 404. Server-side errors (5xx)
    /// are logged with `tracing::error!` before the response is built.
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::InvalidPayload(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Research(_) => StatusCode::BAD_GATEWAY,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PayloadError;

    #[test]
    fn invalid_payload_maps_to_422() {
        let err = AppError::InvalidPayload(PayloadError::EmptyQuestion);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn store_maps_to_500() {
        let err = AppError::Store("mnestic script failed".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn research_maps_to_502() {
        let err = AppError::Research("worker exited with status 1".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn not_found_maps_to_404() {
        let err = AppError::NotFound("dos:missing-dossier".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
