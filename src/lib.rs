//! Intelligence Workbench Phase 1: mnestic-backed research ingestion and
//! briefing synthesis.
//!
//! This crate owns the shared `ExtractionPayload` contract used to exchange
//! research extractions with the Python worker, the deterministic
//! feature-hashed text embedder, and the application error type shared by
//! the Axum service built on top of it.

pub mod embed;
pub mod error;
pub mod model;
pub mod schema;
pub mod store;
