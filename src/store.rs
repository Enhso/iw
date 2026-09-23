//! The mnestic-backed graph store: schema initialization, atomic dossier
//! ingestion, typed query methods for every named query in
//! [`crate::schema`], and the as-of-aware family/document/history reads
//! layered on top of them (`docs/contracts.md` §C/§D in the `betomcat`
//! repository).
//!
//! Every read method here takes an `as_of: &str` argument: either an RFC
//! 3339 UTC string or the literal `"NOW"` ([`AS_OF_NOW`]), passed straight
//! through to mnestic's `:as_of` block option. A handful of read paths
//! (family/document/history listings, family resolution) pull an entire
//! small relation into Rust and filter/join it there rather than writing a
//! single Datalog query for it; these relations are expected to stay small
//! at Phase-1/personal-forecasting-bot scale, and doing the join in Rust
//! made the merge-chain-following and cross-relation logic easier to get
//! right than encoding it as recursive Datalog under a deadline. This is a
//! deliberate scale/complexity tradeoff, not an oversight.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

use crate::embed::embed;
use crate::model::{dossier_id_for, sha256_hex, ExtractionPayload, HistoryItem};
use crate::schema;

/// The `:as_of` value meaning "current state" (mnestic's own `'NOW'`
/// literal).
pub const AS_OF_NOW: &str = "NOW";

/// A handle to an open mnestic database, with schema initialization,
/// atomic ingestion, and typed query methods.
pub struct GraphStore {
    db: DbInstance,
}

impl std::fmt::Debug for GraphStore {
    /// Prints `GraphStore { .. }` without dumping database contents.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStore").finish_non_exhaustive()
    }
}

/// An error returned by a [`GraphStore`] operation.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The underlying mnestic script failed. The message is the formatted
    /// `miette::Report` from mnestic.
    #[error("database error: {0}")]
    Db(String),
    /// A query returned a row that did not match the shape its typed
    /// accessor expected.
    #[error("unexpected row shape in {query}: {detail}")]
    RowShape { query: &'static str, detail: String },
    /// [`GraphStore::mint_family_id`] could not find an unused id after a
    /// generous number of `-N` suffixes; treated as an invariant violation
    /// rather than a normal error path.
    #[error("could not mint a unique family id for label {label:?} after {attempts} attempts")]
    FamilyIdExhausted { label: String, attempts: u32 },
}

/// A named entity row returned by [`GraphStore::entities`].
#[derive(Debug, Clone, PartialEq)]
pub struct EntityRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
}

/// A dated event row returned by [`GraphStore::events`].
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: String,
    pub name: String,
    pub occurred_at: String,
    pub description: String,
}

/// A citable source row returned by [`GraphStore::sources`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRow {
    pub id: String,
    pub title: String,
    pub url: String,
    pub provider: String,
    pub published: String,
    pub retrieved_at: String,
    pub content_hash: String,
}

/// An evidence row returned by [`GraphStore::evidence`].
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceRow {
    pub id: String,
    pub claim_id: String,
    pub source_id: String,
    pub stance: String,
    pub excerpt: String,
    pub quality: f64,
}

/// A claim row with zero-filled support/contradict evidence counts,
/// returned by [`GraphStore::claims_with_stance`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimStanceRow {
    pub id: String,
    pub text: String,
    pub kind: String,
    pub supports: i64,
    pub contradicts: i64,
}

/// A claim's support score, returned by [`GraphStore::claim_support`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimSupportRow {
    pub claim_id: String,
    pub support: Option<f64>,
    pub method: String,
}

/// A causal link row returned by [`GraphStore::causal_links`].
#[derive(Debug, Clone, PartialEq)]
pub struct CausalLinkRow {
    pub cause_id: String,
    pub effect_id: String,
    pub mechanism: String,
    pub confidence: String,
}

/// A causal chain row returned by [`GraphStore::causal_chains`].
#[derive(Debug, Clone, PartialEq)]
pub struct CausalChainRow {
    pub start: String,
    pub end: String,
    pub path: Vec<String>,
}

/// A crux claim row returned by [`GraphStore::cruxes`].
#[derive(Debug, Clone, PartialEq)]
pub struct CruxRow {
    pub id: String,
    pub text: String,
    pub supports: i64,
    pub contradicts: i64,
    pub downstream: i64,
    pub score: i64,
}

/// A consensus claim row returned by [`GraphStore::consensus`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConsensusRow {
    pub id: String,
    pub text: String,
    pub sources: i64,
}

/// A temporal relation row returned by [`GraphStore::temporal_relations`].
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalRow {
    pub before_id: String,
    pub after_id: String,
    pub relation: String,
}

/// A vector-similarity evidence row returned by
/// [`GraphStore::similar_evidence`].
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarEvidenceRow {
    pub id: String,
    pub claim_id: String,
    pub source_id: String,
    pub source_title: String,
    pub stance: String,
    pub excerpt: String,
    pub quality: f64,
    pub distance: f64,
}

/// A vector-similarity claim row returned by
/// [`GraphStore::similar_claims`].
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarClaimRow {
    pub id: String,
    pub text: String,
    pub distance: f64,
}

/// A summary of what [`GraphStore::ingest`] wrote, one count per payload
/// list plus the derived dossier id.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IngestReport {
    pub dossier_id: String,
    pub entities: usize,
    pub events: usize,
    pub sources: usize,
    pub claims: usize,
    pub evidence: usize,
    pub causal_links: usize,
    pub temporal_relations: usize,
}

/// A family record, returned by [`GraphStore::all_families`] and rendered
/// as `GET /api/families` rows by the caller (`docs/contracts.md` §C3).
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyRow {
    pub id: String,
    pub label: String,
    pub description: String,
    pub created_at: String,
    pub merged_into: Option<String>,
}

/// A `(dossier_id, created_at)` pair, returned by [`GraphStore::all_dossiers`].
#[derive(Debug, Clone, PartialEq)]
pub struct DossierRow {
    pub id: String,
    pub created_at: String,
}

/// A `(question_id, family_id, probability, method)` tag, returned by
/// [`GraphStore::all_question_families`].
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionFamilyRow {
    pub question_id: String,
    pub family_id: String,
    pub probability: Option<f64>,
    pub method: String,
}

/// A document row returned by [`GraphStore::documents`]
/// (`docs/contracts.md` §C3).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DocumentRow {
    pub source_id: String,
    pub url: String,
    pub title: String,
    pub provider: String,
    pub published: String,
    pub fetched_at: String,
    pub content_hash: String,
    /// Populated only when the caller asked for `include_content=true`.
    pub content: Option<String>,
}

/// One evidence item nested under a [`ClaimView`] (`docs/contracts.md`
/// §C2 `ClaimView`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClaimEvidenceView {
    pub source_id: String,
    pub title: String,
    pub url: String,
    pub provider: String,
    pub published: String,
    pub fetched_at: String,
    pub content_hash: String,
    pub stance: String,
    pub excerpt: String,
}

/// A claim with its support score, owning dossier, and evidence, returned
/// by [`GraphStore::claim_views`] and [`GraphStore::claim_views_for_dossiers`]
/// (`docs/contracts.md` §C2 `ClaimView`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClaimView {
    pub claim_id: String,
    pub text: String,
    pub kind: String,
    pub support: Option<f64>,
    pub support_method: String,
    pub dossier_id: String,
    pub evidence: Vec<ClaimEvidenceView>,
}

/// A raw document to ingest without extraction, for `POST /api/documents`
/// (`docs/contracts.md` §C4).
#[derive(Debug, Clone, PartialEq)]
pub struct RawDocument {
    pub url: String,
    pub title: String,
    pub provider: String,
    pub published: String,
    pub fetched_at: String,
    pub content: String,
}

impl GraphStore {
    /// Opens an in-memory mnestic database. Used by tests and by any run
    /// that does not need to persist across process restarts.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to open the database.
    pub fn open_memory() -> Result<Self, StoreError> {
        let db = DbInstance::new("mem", "", "").map_err(db_error)?;
        Ok(Self { db })
    }

    /// Opens (or creates) a mnestic database persisted at `path` using the
    /// `sqlite` storage engine.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to open the database.
    pub fn open_sqlite(path: &Path) -> Result<Self, StoreError> {
        let db = DbInstance::new("sqlite", path, "").map_err(db_error)?;
        Ok(Self { db })
    }

    /// Initializes the schema if it is not already present.
    ///
    /// Runs `::relations` and checks whether any relation is named
    /// `claim`; if so, the schema is assumed already initialized and this
    /// is a no-op. Otherwise runs [`schema::SCHEMA_DDL`]. This guard makes
    /// the method safe to call on every startup, since re-running
    /// `::hnsw create` on an existing index would otherwise fail.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if either script fails.
    pub fn init_schema(&self) -> Result<(), StoreError> {
        let existing = self.run("::relations", BTreeMap::new())?;
        let already_initialized = existing
            .rows
            .iter()
            .any(|row| row.first().and_then(DataValue::get_str) == Some("claim"));
        if already_initialized {
            return Ok(());
        }
        self.db
            .run_script(
                schema::SCHEMA_DDL,
                BTreeMap::new(),
                ScriptMutability::Mutable,
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Ingests `payload` as one dossier, atomically, via
    /// [`schema::INGEST_SCRIPT`]. `question_id`, when given, tags the
    /// dossier with the question it answers (`dossier_question`).
    ///
    /// Every relation `:put` here appends a new `tt` version rather than
    /// overwriting, so re-ingesting the same dossier (e.g. a source's
    /// content changed since the last research run) keeps every prior
    /// version reachable via `:as_of`.
    ///
    /// # Preconditions
    /// The caller must have already called `payload.validate()`
    /// successfully; this method does not re-validate.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the ingestion script fails; because
    /// the script is one chained set of blocks, mnestic rolls back every
    /// block and no partial write is left behind.
    pub fn ingest(
        &self,
        payload: &ExtractionPayload,
        question_id: Option<&str>,
        created_at: &str,
    ) -> Result<IngestReport, StoreError> {
        let dossier_id = dossier_id_for(&payload.question);
        let params = ingest_params(payload, &dossier_id, question_id, created_at);

        self.db
            .run_script(schema::INGEST_SCRIPT, params, ScriptMutability::Mutable)
            .map_err(db_error)?;

        Ok(IngestReport {
            dossier_id,
            entities: payload.entities.len(),
            events: payload.events.len(),
            sources: payload.sources.len(),
            claims: payload.claims.len(),
            evidence: payload.evidence.len(),
            causal_links: payload.causal_links.len(),
            temporal_relations: payload.temporal_relations.len(),
        })
    }

    /// Returns every entity in `dossier_id` as of `as_of`, ordered by kind
    /// then name.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn entities(&self, dossier_id: &str, as_of: &str) -> Result<Vec<EntityRow>, StoreError> {
        let rows = self.run(schema::ENTITIES, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EntityRow {
                    id: str_at(row, 0, "entities")?,
                    name: str_at(row, 1, "entities")?,
                    kind: str_at(row, 2, "entities")?,
                    description: str_at(row, 3, "entities")?,
                })
            })
            .collect()
    }

    /// Returns every event in `dossier_id` as of `as_of`, ordered by
    /// occurrence date then name.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn events(&self, dossier_id: &str, as_of: &str) -> Result<Vec<EventRow>, StoreError> {
        let rows = self.run(schema::EVENTS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EventRow {
                    id: str_at(row, 0, "events")?,
                    name: str_at(row, 1, "events")?,
                    occurred_at: str_at(row, 2, "events")?,
                    description: str_at(row, 3, "events")?,
                })
            })
            .collect()
    }

    /// Returns every `(event_id, entity_id)` participation pair for events
    /// in `dossier_id` as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn event_actors(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::EVENT_ACTORS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "event_actors")?,
                    str_at(row, 1, "event_actors")?,
                ))
            })
            .collect()
    }

    /// Returns every source in `dossier_id` as of `as_of`, ordered by
    /// provider then title.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn sources(&self, dossier_id: &str, as_of: &str) -> Result<Vec<SourceRow>, StoreError> {
        let rows = self.run(schema::SOURCES, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| source_row(row, "sources"))
            .collect()
    }

    /// Returns every source globally as of `as_of` (no dossier scoping),
    /// ordered by provider then title, capped at `limit`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_sources(&self, as_of: &str, limit: usize) -> Result<Vec<SourceRow>, StoreError> {
        let mut params = as_of_param(as_of);
        params.insert("limit".to_string(), DataValue::from(limit as i64));
        let rows = self.run(schema::ALL_SOURCES, params)?;
        rows.rows
            .iter()
            .map(|row| source_row(row, "all_sources"))
            .collect()
    }

    /// Looks up a blob's content by its `content_hash`. `blob` is plain
    /// (content-addressed, immutable), so there is no `as_of` parameter.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if the row does not match the expected
    /// shape.
    pub fn blob_content(&self, content_hash: &str) -> Result<Option<String>, StoreError> {
        let mut params = BTreeMap::new();
        params.insert("content_hash".to_string(), DataValue::from(content_hash));
        let rows = self.run(schema::BLOB_CONTENT, params)?;
        match rows.rows.first() {
            Some(row) => Ok(Some(str_at(row, 0, "blob_content")?)),
            None => Ok(None),
        }
    }

    /// Returns every `(claim_id, subject_id)` pair for claims in
    /// `dossier_id` as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn claim_subjects(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::CLAIM_SUBJECTS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "claim_subjects")?,
                    str_at(row, 1, "claim_subjects")?,
                ))
            })
            .collect()
    }

    /// Returns every evidence row in `dossier_id` as of `as_of`, ordered by
    /// claim then id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn evidence(&self, dossier_id: &str, as_of: &str) -> Result<Vec<EvidenceRow>, StoreError> {
        let rows = self.run(schema::EVIDENCE, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(EvidenceRow {
                    id: str_at(row, 0, "evidence")?,
                    claim_id: str_at(row, 1, "evidence")?,
                    source_id: str_at(row, 2, "evidence")?,
                    stance: str_at(row, 3, "evidence")?,
                    excerpt: str_at(row, 4, "evidence")?,
                    quality: float_at(row, 5, "evidence")?,
                })
            })
            .collect()
    }

    /// Returns every claim in `dossier_id` as of `as_of` with zero-filled
    /// support/contradict evidence counts, ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn claims_with_stance(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<ClaimStanceRow>, StoreError> {
        let rows = self.run(schema::CLAIMS_WITH_STANCE, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(ClaimStanceRow {
                    id: str_at(row, 0, "claims_with_stance")?,
                    text: str_at(row, 1, "claims_with_stance")?,
                    kind: str_at(row, 2, "claims_with_stance")?,
                    supports: int_at(row, 3, "claims_with_stance")?,
                    contradicts: int_at(row, 4, "claims_with_stance")?,
                })
            })
            .collect()
    }

    /// Returns every claim's support score in `dossier_id` as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn claim_support(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<ClaimSupportRow>, StoreError> {
        let rows = self.run(schema::CLAIM_SUPPORT, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(ClaimSupportRow {
                    claim_id: str_at(row, 0, "claim_support")?,
                    support: opt_float_at(row, 1, "claim_support")?,
                    method: str_at(row, 2, "claim_support")?,
                })
            })
            .collect()
    }

    /// Returns every causal link in `dossier_id` as of `as_of`, ordered by
    /// cause then effect.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn causal_links(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<CausalLinkRow>, StoreError> {
        let rows = self.run(schema::CAUSAL_LINKS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CausalLinkRow {
                    cause_id: str_at(row, 0, "causal_links")?,
                    effect_id: str_at(row, 1, "causal_links")?,
                    mechanism: str_at(row, 2, "causal_links")?,
                    confidence: str_at(row, 3, "causal_links")?,
                })
            })
            .collect()
    }

    /// Returns every causal chain in `dossier_id` as of `as_of`, longest
    /// first, then ordered by start then end.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn causal_chains(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<CausalChainRow>, StoreError> {
        let rows = self.run(schema::CAUSAL_CHAINS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CausalChainRow {
                    start: str_at(row, 0, "causal_chains")?,
                    end: str_at(row, 1, "causal_chains")?,
                    path: str_list_at(row, 2, "causal_chains")?,
                })
            })
            .collect()
    }

    /// Returns up to 3 crux claims in `dossier_id` as of `as_of`, highest
    /// score first, then ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn cruxes(&self, dossier_id: &str, as_of: &str) -> Result<Vec<CruxRow>, StoreError> {
        let rows = self.run(schema::CRUXES, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(CruxRow {
                    id: str_at(row, 0, "cruxes")?,
                    text: str_at(row, 1, "cruxes")?,
                    supports: int_at(row, 2, "cruxes")?,
                    contradicts: int_at(row, 3, "cruxes")?,
                    downstream: int_at(row, 4, "cruxes")?,
                    score: int_at(row, 5, "cruxes")?,
                })
            })
            .collect()
    }

    /// Returns claims in `dossier_id` as of `as_of` with at least 2
    /// distinct supporting sources and zero contradictions, most-sourced
    /// first, then ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn consensus(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<ConsensusRow>, StoreError> {
        let rows = self.run(schema::CONSENSUS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(ConsensusRow {
                    id: str_at(row, 0, "consensus")?,
                    text: str_at(row, 1, "consensus")?,
                    sources: int_at(row, 2, "consensus")?,
                })
            })
            .collect()
    }

    /// Returns every temporal relation in `dossier_id` as of `as_of`,
    /// ordered by before then after.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn temporal_relations(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Vec<TemporalRow>, StoreError> {
        let rows = self.run(schema::TEMPORAL_RELATIONS, dossier_param(dossier_id, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(TemporalRow {
                    before_id: str_at(row, 0, "temporal_relations")?,
                    after_id: str_at(row, 1, "temporal_relations")?,
                    relation: str_at(row, 2, "temporal_relations")?,
                })
            })
            .collect()
    }

    /// Returns the 8 evidence rows globally nearest to `query_text`'s
    /// embedding as of `as_of`, nearest first, then ordered by id. See
    /// [`schema::SIMILAR_EVIDENCE`] for why this stays leak-free under
    /// `as_of` despite the HNSW index itself carrying no history.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn similar_evidence(
        &self,
        query_text: &str,
        as_of: &str,
    ) -> Result<Vec<SimilarEvidenceRow>, StoreError> {
        let rows = self.run(schema::SIMILAR_EVIDENCE, query_param(query_text, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(SimilarEvidenceRow {
                    id: str_at(row, 0, "similar_evidence")?,
                    claim_id: str_at(row, 1, "similar_evidence")?,
                    source_id: str_at(row, 2, "similar_evidence")?,
                    source_title: str_at(row, 3, "similar_evidence")?,
                    stance: str_at(row, 4, "similar_evidence")?,
                    excerpt: str_at(row, 5, "similar_evidence")?,
                    quality: float_at(row, 6, "similar_evidence")?,
                    distance: float_at(row, 7, "similar_evidence")?,
                })
            })
            .collect()
    }

    /// Returns the 5 claims globally nearest to `query_text`'s embedding as
    /// of `as_of`, nearest first, then ordered by id. See
    /// [`schema::SIMILAR_CLAIMS`] for the same leak-free reasoning.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn similar_claims(
        &self,
        query_text: &str,
        as_of: &str,
    ) -> Result<Vec<SimilarClaimRow>, StoreError> {
        let rows = self.run(schema::SIMILAR_CLAIMS, query_param(query_text, as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(SimilarClaimRow {
                    id: str_at(row, 0, "similar_claims")?,
                    text: str_at(row, 1, "similar_claims")?,
                    distance: float_at(row, 2, "similar_claims")?,
                })
            })
            .collect()
    }

    /// Returns `(question, created_at)` for `dossier_id` as of `as_of`, or
    /// `None` if that dossier did not exist yet as of `as_of` (either it
    /// has never been ingested, or `as_of` predates its first ingest).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if the row does not match the expected
    /// shape.
    pub fn dossier_meta(
        &self,
        dossier_id: &str,
        as_of: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        const QUERY: &str = r#"
?[question, created_at] := *dossier{id: $dossier_id, question, created_at}
:as_of $as_of
"#;
        let rows = self.run(QUERY, dossier_param(dossier_id, as_of))?;
        match rows.rows.first() {
            Some(row) => Ok(Some((
                str_at(row, 0, "dossier_meta")?,
                str_at(row, 1, "dossier_meta")?,
            ))),
            None => Ok(None),
        }
    }

    /// Returns every dossier's `(id, created_at)` globally as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_dossiers(&self, as_of: &str) -> Result<Vec<DossierRow>, StoreError> {
        let rows = self.run(schema::ALL_DOSSIERS, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(DossierRow {
                    id: str_at(row, 0, "all_dossiers")?,
                    created_at: str_at(row, 1, "all_dossiers")?,
                })
            })
            .collect()
    }

    /// Returns every `(dossier_id, question_id)` tag globally as of
    /// `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_dossier_questions(&self, as_of: &str) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::ALL_DOSSIER_QUESTIONS, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "all_dossier_questions")?,
                    str_at(row, 1, "all_dossier_questions")?,
                ))
            })
            .collect()
    }

    /// Returns every family-routing tag globally as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_question_families(&self, as_of: &str) -> Result<Vec<QuestionFamilyRow>, StoreError> {
        let rows = self.run(schema::ALL_QUESTION_FAMILIES, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(QuestionFamilyRow {
                    question_id: str_at(row, 0, "all_question_families")?,
                    family_id: str_at(row, 1, "all_question_families")?,
                    probability: opt_float_at(row, 2, "all_question_families")?,
                    method: str_at(row, 3, "all_question_families")?,
                })
            })
            .collect()
    }

    /// Returns every family globally as of `as_of`, live and merged-away
    /// alike.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_families(&self, as_of: &str) -> Result<Vec<FamilyRow>, StoreError> {
        let rows = self.run(schema::ALL_FAMILIES, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(FamilyRow {
                    id: str_at(row, 0, "all_families")?,
                    label: str_at(row, 1, "all_families")?,
                    description: str_at(row, 2, "all_families")?,
                    created_at: str_at(row, 3, "all_families")?,
                    merged_into: opt_str_at(row, 4, "all_families")?,
                })
            })
            .collect()
    }

    /// Returns every `(family_id, last_seen)` pair globally as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_family_seen(&self, as_of: &str) -> Result<Vec<(String, String)>, StoreError> {
        let rows = self.run(schema::ALL_FAMILY_SEEN, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok((
                    str_at(row, 0, "all_family_seen")?,
                    str_at(row, 1, "all_family_seen")?,
                ))
            })
            .collect()
    }

    /// Returns every history item globally as of `as_of`.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the query fails, or
    /// [`StoreError::RowShape`] if a row does not match the expected shape.
    pub fn all_history(&self, as_of: &str) -> Result<Vec<HistoryItem>, StoreError> {
        let rows = self.run(schema::ALL_HISTORY, as_of_param(as_of))?;
        rows.rows
            .iter()
            .map(|row| {
                Ok(HistoryItem {
                    id: str_at(row, 0, "all_history")?,
                    kind: str_at(row, 1, "all_history")?,
                    title: str_at(row, 2, "all_history")?,
                    url: str_at(row, 3, "all_history")?,
                    question_type: str_at(row, 4, "all_history")?,
                    forecast: json_at(row, 5, "all_history")?,
                    resolution: opt_str_at(row, 6, "all_history")?,
                    resolved_at: opt_str_at(row, 7, "all_history")?,
                    family_id: opt_str_at(row, 8, "all_history")?,
                    note: str_at(row, 9, "all_history")?,
                })
            })
            .collect()
    }

    /// Resolves `family_id` forward through `merged_into` chains to the
    /// live (root) family, as reflected by `families`. Caps at 20 hops
    /// (generous for any real merge chain) and stops early on a cycle,
    /// returning the id at which the cycle was detected rather than
    /// looping forever; a merge cycle is a data-integrity bug elsewhere,
    /// not a case this method silently accepts.
    pub fn resolve_family_id(families: &[FamilyRow], family_id: &str) -> String {
        let by_id: HashMap<&str, &FamilyRow> =
            families.iter().map(|f| (f.id.as_str(), f)).collect();
        let mut current = family_id.to_string();
        let mut seen = HashSet::new();
        for _ in 0..20 {
            if !seen.insert(current.clone()) {
                break;
            }
            match by_id
                .get(current.as_str())
                .and_then(|f| f.merged_into.as_deref())
            {
                Some(next) => current = next.to_string(),
                None => break,
            }
        }
        current
    }

    /// Mints a fresh family id for `label`: `fam:<slug(label)>`, suffixed
    /// `-2`, `-3`, ... on collision with an existing id (checked against
    /// current state).
    ///
    /// # Errors
    /// Returns [`StoreError::FamilyIdExhausted`] if no unused id was found
    /// within 1000 attempts (a data-integrity red flag, not a normal
    /// outcome at this system's scale), or [`StoreError::Db`]/
    /// [`StoreError::RowShape`] if the existence check fails.
    pub fn mint_family_id(&self, label: &str) -> Result<String, StoreError> {
        let existing = self.all_families(AS_OF_NOW)?;
        let existing_ids: HashSet<&str> = existing.iter().map(|f| f.id.as_str()).collect();
        let base = format!("fam:{}", crate::model::slugify(label));
        if !existing_ids.contains(base.as_str()) {
            return Ok(base);
        }
        for n in 2..1000u32 {
            let candidate = format!("{base}-{n}");
            if !existing_ids.contains(candidate.as_str()) {
                return Ok(candidate);
            }
        }
        Err(StoreError::FamilyIdExhausted {
            label: label.to_string(),
            attempts: 1000,
        })
    }

    /// Inserts a new family row (current state; the row has no prior
    /// version).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the write fails.
    pub fn insert_family(
        &self,
        id: &str,
        label: &str,
        description: &str,
        created_at: &str,
    ) -> Result<(), StoreError> {
        const QUERY: &str = r#"
?[id, label, description, created_at, merged_into] <- $rows
:put family {id => label, description, created_at, merged_into}
"#;
        let mut params = BTreeMap::new();
        params.insert(
            "rows".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from(id),
                DataValue::from(label),
                DataValue::from(description),
                DataValue::from(created_at),
                DataValue::Null,
            ])]),
        );
        self.run_mut(QUERY, params)?;
        Ok(())
    }

    /// Sets `absorbed_id`'s `merged_into` to `into_id` (a new `family`
    /// version; `label`/`description`/`created_at` are carried over
    /// unchanged from the absorbed family's current state, since `:put`
    /// replaces the whole row rather than patching one column). Per
    /// `docs/contracts.md` §C4/spec s2, this is prospective only: existing
    /// `question_family` tags referencing `absorbed_id` are left as-is and
    /// are resolved forward at read time via [`Self::resolve_family_id`].
    ///
    /// # Errors
    /// Returns [`StoreError::Db`]/[`StoreError::RowShape`] if reading the
    /// absorbed family fails, or a [`StoreError`] from a plain
    /// `StoreError::Db("family not found: ...")` if `absorbed_id` does not
    /// currently exist.
    pub fn merge_family(&self, absorbed_id: &str, into_id: &str) -> Result<(), StoreError> {
        let existing = self.all_families(AS_OF_NOW)?;
        let absorbed = existing
            .iter()
            .find(|f| f.id == absorbed_id)
            .ok_or_else(|| StoreError::Db(format!("family not found: {absorbed_id}")))?;

        const QUERY: &str = r#"
?[id, label, description, created_at, merged_into] <- $rows
:put family {id => label, description, created_at, merged_into}
"#;
        let mut params = BTreeMap::new();
        params.insert(
            "rows".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from(absorbed.id.as_str()),
                DataValue::from(absorbed.label.as_str()),
                DataValue::from(absorbed.description.as_str()),
                DataValue::from(absorbed.created_at.as_str()),
                DataValue::from(into_id),
            ])]),
        );
        self.run_mut(QUERY, params)?;
        Ok(())
    }

    /// Records `last_seen` for `family_id` (a new `family_seen` version).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the write fails.
    pub fn record_family_seen(&self, family_id: &str, last_seen: &str) -> Result<(), StoreError> {
        const QUERY: &str = r#"
?[family_id, last_seen] <- $rows
:put family_seen {family_id => last_seen}
"#;
        let mut params = BTreeMap::new();
        params.insert(
            "rows".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from(family_id),
                DataValue::from(last_seen),
            ])]),
        );
        self.run_mut(QUERY, params)?;
        Ok(())
    }

    /// Records a `question_family` routing tag (a new version; prior tags
    /// for the same `question_id` remain reachable via `:as_of`).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the write fails.
    pub fn record_question_family(
        &self,
        question_id: &str,
        family_id: &str,
        probability: Option<f64>,
        method: &str,
        question: &str,
    ) -> Result<(), StoreError> {
        const QUERY: &str = r#"
?[question_id, family_id, probability, method, question] <- $rows
:put question_family {question_id => family_id, probability, method, question}
"#;
        let mut params = BTreeMap::new();
        params.insert(
            "rows".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from(question_id),
                DataValue::from(family_id),
                opt_float(probability),
                DataValue::from(method),
                DataValue::from(question),
            ])]),
        );
        self.run_mut(QUERY, params)?;
        Ok(())
    }

    /// Upserts every item in `items` (each a new `history_item` version).
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the write fails.
    pub fn upsert_history(&self, items: &[HistoryItem]) -> Result<(), StoreError> {
        if items.is_empty() {
            return Ok(());
        }
        const QUERY: &str = r#"
?[id, kind, title, url, question_type, forecast, resolution, resolved_at, family_id, note]
  <- $rows
:put history_item {id => kind, title, url, question_type, forecast, resolution, resolved_at,
  family_id, note}
"#;
        let rows = items
            .iter()
            .map(|item| {
                DataValue::List(vec![
                    DataValue::from(item.id.as_str()),
                    DataValue::from(item.kind.as_str()),
                    DataValue::from(item.title.as_str()),
                    DataValue::from(item.url.as_str()),
                    DataValue::from(item.question_type.as_str()),
                    DataValue::from(item.forecast.clone()),
                    opt_str(item.resolution.as_deref()),
                    opt_str(item.resolved_at.as_deref()),
                    opt_str(item.family_id.as_deref()),
                    DataValue::from(item.note.as_str()),
                ])
            })
            .collect();
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));
        self.run_mut(QUERY, params)?;
        Ok(())
    }

    /// Ingests `documents` without extraction (bot outbox replay / degraded
    /// mode, `docs/contracts.md` §C4): computes each document's
    /// content-addressed id and hash and writes `source` + `blob` rows.
    /// Returns the number of documents ingested.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if the write fails.
    pub fn ingest_raw_documents(&self, documents: &[RawDocument]) -> Result<usize, StoreError> {
        if documents.is_empty() {
            return Ok(0);
        }
        const QUERY: &str = r#"
{ ?[id, title, url, provider, published, retrieved_at, content_hash] <- $sources
  :put source {id => title, url, provider, published, retrieved_at, content_hash} }
{ ?[content_hash, content] <- $blobs
  :put blob {content_hash => content} }
"#;
        let mut sources = Vec::with_capacity(documents.len());
        let mut blobs = Vec::with_capacity(documents.len());
        let mut seen_hashes = HashSet::new();
        for document in documents {
            let content_hash = sha256_hex(&document.content);
            let url_hash = &sha256_hex(&document.url)[..16];
            let id = format!("src:{}:{url_hash}", document.provider);
            sources.push(DataValue::List(vec![
                DataValue::from(id.as_str()),
                DataValue::from(document.title.as_str()),
                DataValue::from(document.url.as_str()),
                DataValue::from(document.provider.as_str()),
                DataValue::from(document.published.as_str()),
                DataValue::from(document.fetched_at.as_str()),
                DataValue::from(content_hash.as_str()),
            ]));
            if seen_hashes.insert(content_hash.clone()) {
                blobs.push(DataValue::List(vec![
                    DataValue::from(content_hash.as_str()),
                    DataValue::from(document.content.as_str()),
                ]));
            }
        }
        let mut params = BTreeMap::new();
        params.insert("sources".to_string(), DataValue::List(sources));
        params.insert("blobs".to_string(), DataValue::List(blobs));
        self.db
            .run_script(QUERY, params, ScriptMutability::Mutable)
            .map_err(db_error)?;
        Ok(documents.len())
    }

    /// Returns the dossier ids whose tagged question resolves (through
    /// `merged_into` chains) to `canonical_family_id`, as of `as_of`,
    /// newest dossier first (`docs/contracts.md` §C2 "newest first").
    ///
    /// # Errors
    /// Returns [`StoreError`] if any underlying query fails.
    pub fn dossiers_for_family(
        &self,
        canonical_family_id: &str,
        as_of: &str,
    ) -> Result<Vec<String>, StoreError> {
        let families = self.all_families(as_of)?;
        let question_families = self.all_question_families(as_of)?;
        let dossier_questions = self.all_dossier_questions(as_of)?;
        let dossiers = self.all_dossiers(as_of)?;

        let matching_questions: HashSet<&str> = question_families
            .iter()
            .filter(|tag| Self::resolve_family_id(&families, &tag.family_id) == canonical_family_id)
            .map(|tag| tag.question_id.as_str())
            .collect();

        let mut matching_dossiers: HashSet<String> = dossier_questions
            .iter()
            .filter(|(_, question_id)| matching_questions.contains(question_id.as_str()))
            .map(|(dossier_id, _)| dossier_id.clone())
            .collect();

        let created_at: HashMap<&str, &str> = dossiers
            .iter()
            .map(|d| (d.id.as_str(), d.created_at.as_str()))
            .collect();
        let mut ordered: Vec<String> = matching_dossiers.drain().collect();
        ordered.sort_by(|a, b| {
            let a_time = created_at.get(a.as_str()).copied().unwrap_or("");
            let b_time = created_at.get(b.as_str()).copied().unwrap_or("");
            b_time.cmp(a_time).then_with(|| a.cmp(b))
        });
        Ok(ordered)
    }

    /// Builds this dossier's claims as [`ClaimView`]s as of `as_of`,
    /// ordered by support descending (nulls last), ties broken by claim id
    /// (`docs/contracts.md` §C2 `claims`).
    ///
    /// # Errors
    /// Returns [`StoreError`] if any underlying query fails.
    pub fn claim_views(&self, dossier_id: &str, as_of: &str) -> Result<Vec<ClaimView>, StoreError> {
        let claims = self.claims_with_stance(dossier_id, as_of)?;
        let support = self.claim_support(dossier_id, as_of)?;
        let evidence = self.evidence(dossier_id, as_of)?;
        let sources = self.sources(dossier_id, as_of)?;
        Ok(build_claim_views(
            dossier_id, &claims, &support, &evidence, &sources,
        ))
    }

    /// Builds [`ClaimView`]s across `dossier_ids` (already ordered by the
    /// caller, e.g. newest dossier first) as of `as_of`, concatenating each
    /// dossier's own support-ranked claims in that dossier order and
    /// truncating to `limit` (`docs/contracts.md` §C2 `family_claims`).
    ///
    /// # Errors
    /// Returns [`StoreError`] if any underlying query fails.
    pub fn claim_views_for_dossiers(
        &self,
        dossier_ids: &[String],
        as_of: &str,
        limit: usize,
    ) -> Result<Vec<ClaimView>, StoreError> {
        let mut out = Vec::new();
        for dossier_id in dossier_ids {
            if out.len() >= limit {
                break;
            }
            out.extend(self.claim_views(dossier_id, as_of)?);
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Returns documents as of `as_of`, optionally filtered to those
    /// belonging to a dossier tagged (through merge chains) to
    /// `family_id`, ordered by provider then title, capped at `limit`
    /// (`docs/contracts.md` §C3 `GET /api/documents`).
    ///
    /// # Errors
    /// Returns [`StoreError`] if any underlying query fails.
    pub fn documents(
        &self,
        as_of: &str,
        family_id: Option<&str>,
        limit: usize,
        include_content: bool,
    ) -> Result<Vec<DocumentRow>, StoreError> {
        let sources = match family_id {
            None => self.all_sources(as_of, limit)?,
            Some(family_id) => {
                let families = self.all_families(as_of)?;
                let canonical = Self::resolve_family_id(&families, family_id);
                let dossier_ids = self.dossiers_for_family(&canonical, as_of)?;
                let mut by_id: BTreeMap<String, SourceRow> = BTreeMap::new();
                for dossier_id in &dossier_ids {
                    for source in self.sources(dossier_id, as_of)? {
                        by_id.entry(source.id.clone()).or_insert(source);
                    }
                }
                let mut merged: Vec<SourceRow> = by_id.into_values().collect();
                merged.sort_by(|a, b| (&a.provider, &a.title).cmp(&(&b.provider, &b.title)));
                merged.truncate(limit);
                merged
            }
        };

        sources
            .into_iter()
            .map(|source| {
                let content = if include_content {
                    self.blob_content(&source.content_hash)?
                } else {
                    None
                };
                Ok(DocumentRow {
                    source_id: source.id,
                    url: source.url,
                    title: source.title,
                    provider: source.provider,
                    published: source.published,
                    fetched_at: source.retrieved_at,
                    content_hash: source.content_hash,
                    content,
                })
            })
            .collect()
    }

    /// Runs `query` as an immutable (read-only) script against this
    /// database.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to run the script.
    fn run(
        &self,
        query: &'static str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(query, params, ScriptMutability::Immutable)
            .map_err(db_error)
    }

    /// Runs `query` as a mutable (write) script against this database.
    ///
    /// # Errors
    /// Returns [`StoreError::Db`] if mnestic fails to run the script.
    fn run_mut(
        &self,
        query: &'static str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(query, params, ScriptMutability::Mutable)
            .map_err(db_error)
    }
}

/// Converts a mnestic `miette::Report` into a [`StoreError::Db`].
fn db_error(err: impl std::fmt::Debug) -> StoreError {
    StoreError::Db(format!("{err:?}"))
}

/// Builds the `$dossier_id`/`$as_of` parameter map shared by every
/// dossier-scoped query.
fn dossier_param(dossier_id: &str, as_of: &str) -> BTreeMap<String, DataValue> {
    let mut params = as_of_param(as_of);
    params.insert("dossier_id".to_string(), DataValue::from(dossier_id));
    params
}

/// Builds the `$as_of` parameter map shared by every as-of-aware query.
fn as_of_param(as_of: &str) -> BTreeMap<String, DataValue> {
    let mut params = BTreeMap::new();
    params.insert("as_of".to_string(), DataValue::from(as_of));
    params
}

/// Builds the `$q`/`$as_of` parameter map shared by both vector-similarity
/// queries: the 256-dimensional embedding of `query_text`.
fn query_param(query_text: &str, as_of: &str) -> BTreeMap<String, DataValue> {
    let mut params = as_of_param(as_of);
    params.insert("q".to_string(), embedding_list(&embed(query_text)));
    params
}

/// Converts an embedding vector into the `DataValue::List` form mnestic
/// expects for a `<F32; 256>` column or a `vec($q)` parameter.
fn embedding_list(embedding: &[f32]) -> DataValue {
    DataValue::List(
        embedding
            .iter()
            .map(|component| DataValue::from(f64::from(*component)))
            .collect(),
    )
}

/// Converts an `Option<f64>` into the `DataValue` mnestic expects for a
/// nullable `Float?` column: `DataValue::Null` for `None`.
fn opt_float(value: Option<f64>) -> DataValue {
    match value {
        Some(v) => DataValue::from(v),
        None => DataValue::Null,
    }
}

/// Converts an `Option<&str>` into the `DataValue` mnestic expects for a
/// nullable `String?` column.
fn opt_str(value: Option<&str>) -> DataValue {
    match value {
        Some(v) => DataValue::from(v),
        None => DataValue::Null,
    }
}

/// Parses a `SourceRow` from a 7-column `(id, title, url, provider,
/// published, retrieved_at, content_hash)` row.
fn source_row(row: &[DataValue], query: &'static str) -> Result<SourceRow, StoreError> {
    Ok(SourceRow {
        id: str_at(row, 0, query)?,
        title: str_at(row, 1, query)?,
        url: str_at(row, 2, query)?,
        provider: str_at(row, 3, query)?,
        published: str_at(row, 4, query)?,
        retrieved_at: str_at(row, 5, query)?,
        content_hash: str_at(row, 6, query)?,
    })
}

/// Joins already-fetched dossier-scoped rows into [`ClaimView`]s, ordered
/// by support descending (nulls last), ties broken by claim id.
fn build_claim_views(
    dossier_id: &str,
    claims: &[ClaimStanceRow],
    support: &[ClaimSupportRow],
    evidence: &[EvidenceRow],
    sources: &[SourceRow],
) -> Vec<ClaimView> {
    let support_by_claim: HashMap<&str, &ClaimSupportRow> =
        support.iter().map(|s| (s.claim_id.as_str(), s)).collect();
    let sources_by_id: HashMap<&str, &SourceRow> =
        sources.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut views: Vec<ClaimView> = claims
        .iter()
        .map(|claim| {
            let claim_support = support_by_claim.get(claim.id.as_str());
            let claim_evidence: Vec<ClaimEvidenceView> = evidence
                .iter()
                .filter(|e| e.claim_id == claim.id)
                .map(|e| {
                    let source = sources_by_id.get(e.source_id.as_str());
                    ClaimEvidenceView {
                        source_id: e.source_id.clone(),
                        title: source.map_or_else(String::new, |s| s.title.clone()),
                        url: source.map_or_else(String::new, |s| s.url.clone()),
                        provider: source.map_or_else(String::new, |s| s.provider.clone()),
                        published: source.map_or_else(String::new, |s| s.published.clone()),
                        fetched_at: source.map_or_else(String::new, |s| s.retrieved_at.clone()),
                        content_hash: source.map_or_else(String::new, |s| s.content_hash.clone()),
                        stance: e.stance.clone(),
                        excerpt: e.excerpt.clone(),
                    }
                })
                .collect();
            ClaimView {
                claim_id: claim.id.clone(),
                text: claim.text.clone(),
                kind: claim.kind.clone(),
                support: claim_support.and_then(|s| s.support),
                support_method: claim_support
                    .map_or_else(|| "none".to_string(), |s| s.method.clone()),
                dossier_id: dossier_id.to_string(),
                evidence: claim_evidence,
            }
        })
        .collect();

    views.sort_by(|a, b| {
        match (a.support, b.support) {
            (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.claim_id.cmp(&b.claim_id))
    });
    views
}

/// Builds the full ingestion parameter map for [`schema::INGEST_SCRIPT`]
/// from `payload`.
fn ingest_params(
    payload: &ExtractionPayload,
    dossier_id: &str,
    question_id: Option<&str>,
    created_at: &str,
) -> BTreeMap<String, DataValue> {
    let mut params = BTreeMap::new();

    params.insert(
        "dossier".to_string(),
        DataValue::List(vec![DataValue::List(vec![
            DataValue::from(dossier_id),
            DataValue::from(payload.question.as_str()),
            DataValue::from(created_at),
        ])]),
    );

    params.insert(
        "dossier_questions".to_string(),
        DataValue::List(match question_id {
            Some(question_id) => vec![DataValue::List(vec![
                DataValue::from(dossier_id),
                DataValue::from(question_id),
            ])],
            None => vec![],
        }),
    );

    params.insert(
        "dossier_items".to_string(),
        DataValue::List(
            payload
                .all_item_ids()
                .iter()
                .map(|item_id| {
                    DataValue::List(vec![
                        DataValue::from(dossier_id),
                        DataValue::from(item_id.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "entities".to_string(),
        DataValue::List(
            payload
                .entities
                .iter()
                .map(|entity| {
                    DataValue::List(vec![
                        DataValue::from(entity.id.as_str()),
                        DataValue::from(entity.name.as_str()),
                        DataValue::from(entity.kind.as_str()),
                        DataValue::from(entity.description.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "events".to_string(),
        DataValue::List(
            payload
                .events
                .iter()
                .map(|event| {
                    DataValue::List(vec![
                        DataValue::from(event.id.as_str()),
                        DataValue::from(event.name.as_str()),
                        DataValue::from(event.occurred_at.as_str()),
                        DataValue::from(event.description.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "event_actors".to_string(),
        DataValue::List(
            payload
                .events
                .iter()
                .flat_map(|event| {
                    event.actor_ids.iter().map(move |actor_id| {
                        DataValue::List(vec![
                            DataValue::from(dossier_id),
                            DataValue::from(event.id.as_str()),
                            DataValue::from(actor_id.as_str()),
                        ])
                    })
                })
                .collect(),
        ),
    );

    params.insert(
        "sources".to_string(),
        DataValue::List(
            payload
                .sources
                .iter()
                .map(|source| {
                    DataValue::List(vec![
                        DataValue::from(source.id.as_str()),
                        DataValue::from(source.title.as_str()),
                        DataValue::from(source.url.as_str()),
                        DataValue::from(source.provider.as_str()),
                        DataValue::from(source.published.as_str()),
                        DataValue::from(source.retrieved_at.as_str()),
                        DataValue::from(source.content_hash.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    let mut seen_blob_hashes = HashSet::new();
    params.insert(
        "blobs".to_string(),
        DataValue::List(
            payload
                .sources
                .iter()
                .filter(|source| seen_blob_hashes.insert(source.content_hash.clone()))
                .map(|source| {
                    DataValue::List(vec![
                        DataValue::from(source.content_hash.as_str()),
                        DataValue::from(source.content.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claims".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .map(|claim| {
                    DataValue::List(vec![
                        DataValue::from(claim.id.as_str()),
                        DataValue::from(claim.text.as_str()),
                        DataValue::from(claim.kind.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claim_vecs".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .map(|claim| {
                    DataValue::List(vec![
                        DataValue::from(claim.id.as_str()),
                        embedding_list(&embed(&claim.text)),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claim_subjects".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .flat_map(|claim| {
                    claim.subject_ids.iter().map(move |subject_id| {
                        DataValue::List(vec![
                            DataValue::from(dossier_id),
                            DataValue::from(claim.id.as_str()),
                            DataValue::from(subject_id.as_str()),
                        ])
                    })
                })
                .collect(),
        ),
    );

    params.insert(
        "evidence".to_string(),
        DataValue::List(
            payload
                .evidence
                .iter()
                .map(|evidence| {
                    DataValue::List(vec![
                        DataValue::from(evidence.id.as_str()),
                        DataValue::from(evidence.claim_id.as_str()),
                        DataValue::from(evidence.source_id.as_str()),
                        DataValue::from(evidence.stance.as_str()),
                        DataValue::from(evidence.excerpt.as_str()),
                        DataValue::from(evidence.quality),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "evidence_vecs".to_string(),
        DataValue::List(
            payload
                .evidence
                .iter()
                .map(|evidence| {
                    DataValue::List(vec![
                        DataValue::from(evidence.id.as_str()),
                        embedding_list(&embed(&evidence.excerpt)),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "causal_links".to_string(),
        DataValue::List(
            payload
                .causal_links
                .iter()
                .map(|link| {
                    DataValue::List(vec![
                        DataValue::from(dossier_id),
                        DataValue::from(link.cause_id.as_str()),
                        DataValue::from(link.effect_id.as_str()),
                        DataValue::from(link.mechanism.as_str()),
                        DataValue::from(link.confidence.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "temporal_relations".to_string(),
        DataValue::List(
            payload
                .temporal_relations
                .iter()
                .map(|relation| {
                    DataValue::List(vec![
                        DataValue::from(dossier_id),
                        DataValue::from(relation.before_id.as_str()),
                        DataValue::from(relation.after_id.as_str()),
                        DataValue::from(relation.relation.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params.insert(
        "claim_supports".to_string(),
        DataValue::List(
            payload
                .claims
                .iter()
                .map(|claim| {
                    DataValue::List(vec![
                        DataValue::from(dossier_id),
                        DataValue::from(claim.id.as_str()),
                        opt_float(claim.support),
                        DataValue::from(claim.support_method.as_str()),
                    ])
                })
                .collect(),
        ),
    );

    params
}

/// Reads a `String` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// string.
fn str_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<String, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_str)
        .map(str::to_string)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected string at column {idx}, row: {row:?}"),
        })
}

/// Reads an `Option<String>` from `row[idx]`: `None` for a null cell.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is neither
/// null nor a string.
fn opt_str_at(
    row: &[DataValue],
    idx: usize,
    query: &'static str,
) -> Result<Option<String>, StoreError> {
    match row.get(idx) {
        Some(DataValue::Null) => Ok(None),
        Some(other) => {
            other
                .get_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| StoreError::RowShape {
                    query,
                    detail: format!("expected nullable string at column {idx}, row: {row:?}"),
                })
        }
        None => Err(StoreError::RowShape {
            query,
            detail: format!("missing column {idx}, row: {row:?}"),
        }),
    }
}

/// Reads an `i64` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// number.
fn int_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<i64, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_int)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected int at column {idx}, row: {row:?}"),
        })
}

/// Reads an `f64` from `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is not a
/// number.
fn float_at(row: &[DataValue], idx: usize, query: &'static str) -> Result<f64, StoreError> {
    row.get(idx)
        .and_then(DataValue::get_float)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("expected float at column {idx}, row: {row:?}"),
        })
}

/// Reads an `Option<f64>` from `row[idx]`: `None` for a null cell.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing or is neither
/// null nor a number.
fn opt_float_at(
    row: &[DataValue],
    idx: usize,
    query: &'static str,
) -> Result<Option<f64>, StoreError> {
    match row.get(idx) {
        Some(DataValue::Null) => Ok(None),
        Some(other) => other
            .get_float()
            .map(Some)
            .ok_or_else(|| StoreError::RowShape {
                query,
                detail: format!("expected nullable float at column {idx}, row: {row:?}"),
            }),
        None => Err(StoreError::RowShape {
            query,
            detail: format!("missing column {idx}, row: {row:?}"),
        }),
    }
}

/// Reads a `serde_json::Value` from the `Json`-typed `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing.
fn json_at(
    row: &[DataValue],
    idx: usize,
    query: &'static str,
) -> Result<serde_json::Value, StoreError> {
    row.get(idx)
        .map(Into::into)
        .ok_or_else(|| StoreError::RowShape {
            query,
            detail: format!("missing column {idx}, row: {row:?}"),
        })
}

/// Reads a `Vec<String>` from the list-valued `row[idx]`.
///
/// # Errors
/// Returns [`StoreError::RowShape`] if the column is missing, is not a
/// list, or contains a non-string element.
fn str_list_at(
    row: &[DataValue],
    idx: usize,
    query: &'static str,
) -> Result<Vec<String>, StoreError> {
    let elements =
        row.get(idx)
            .and_then(DataValue::get_slice)
            .ok_or_else(|| StoreError::RowShape {
                query,
                detail: format!("expected list at column {idx}, row: {row:?}"),
            })?;
    elements
        .iter()
        .map(|element| {
            element
                .get_str()
                .map(str::to_string)
                .ok_or_else(|| StoreError::RowShape {
                    query,
                    detail: format!("expected string element in list at column {idx}"),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        CausalLink, Claim, Entity, Evidence, ExtractionPayload, HistoryItem, Source,
    };

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/rust/semiconductor_v2.json"
    ));
    /// Same dossier (same question, hence same `dossier_id`) as [`FIXTURE`],
    /// with exactly one source's `content`/`content_hash` changed, as if
    /// that page had been edited and the question re-researched. Used to
    /// prove the as-of/versioning behavior (`docs/contracts.md` decision 2).
    const FIXTURE_UPDATED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/rust/semiconductor_v2_updated.json"
    ));
    const CREATED_AT: &str = "2026-09-14T00:00:00Z";

    fn fixture_payload() -> ExtractionPayload {
        serde_json::from_str(FIXTURE).expect("fixture payload parses")
    }

    fn fixture_payload_updated() -> ExtractionPayload {
        serde_json::from_str(FIXTURE_UPDATED).expect("updated fixture payload parses")
    }

    /// Builds a v2 [`Source`] for `title`/`url` whose id and `content_hash`
    /// are correctly derived from `content`.
    fn make_source(provider: &str, title: &str, url: &str, content: &str) -> Source {
        let url_hash = &sha256_hex(url)[..16];
        Source {
            id: format!("src:{provider}:{url_hash}"),
            title: title.to_string(),
            url: url.to_string(),
            provider: provider.to_string(),
            published: String::new(),
            retrieved_at: CREATED_AT.to_string(),
            content_hash: sha256_hex(content),
            content: content.to_string(),
        }
    }

    fn small_payload(question: &str, entity_id: &str, entity_name: &str) -> ExtractionPayload {
        ExtractionPayload {
            schema_version: 2,
            question: question.to_string(),
            entities: vec![Entity {
                id: entity_id.to_string(),
                name: entity_name.to_string(),
                kind: "organization".to_string(),
                description: "A minimal handcrafted entity.".to_string(),
            }],
            events: vec![],
            sources: vec![],
            claims: vec![],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        }
    }

    #[test]
    fn init_schema_is_idempotent() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("first init");
        store.init_schema().expect("second init");
    }

    #[test]
    fn ingest_report_counts_match_fixture_lists() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        assert_eq!(report.entities, payload.entities.len());
        assert_eq!(report.events, payload.events.len());
        assert_eq!(report.sources, payload.sources.len());
        assert_eq!(report.claims, payload.claims.len());
        assert_eq!(report.evidence, payload.evidence.len());
        assert_eq!(report.causal_links, payload.causal_links.len());
        assert_eq!(report.temporal_relations, payload.temporal_relations.len());
        assert_eq!(report.dossier_id, dossier_id_for(&payload.question));
    }

    #[test]
    fn entities_events_sources_evidence_counts_match_fixture() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        assert_eq!(
            store
                .entities(&report.dossier_id, AS_OF_NOW)
                .expect("entities")
                .len(),
            payload.entities.len()
        );
        assert_eq!(
            store
                .events(&report.dossier_id, AS_OF_NOW)
                .expect("events")
                .len(),
            payload.events.len()
        );
        assert_eq!(
            store
                .sources(&report.dossier_id, AS_OF_NOW)
                .expect("sources")
                .len(),
            payload.sources.len()
        );
        assert_eq!(
            store
                .evidence(&report.dossier_id, AS_OF_NOW)
                .expect("evidence")
                .len(),
            payload.evidence.len()
        );
    }

    #[test]
    fn claims_with_stance_has_one_row_per_claim_zero_filled() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let rows = store
            .claims_with_stance(&report.dossier_id, AS_OF_NOW)
            .expect("claims_with_stance");
        assert_eq!(rows.len(), payload.claims.len());

        let evidence_less = rows
            .iter()
            .find(|row| row.id == "clm:chinese-fabs-cannot-access-secondhand-euv")
            .expect("evidence-less assumption claim present");
        assert_eq!(evidence_less.supports, 0);
        assert_eq!(evidence_less.contradicts, 0);
    }

    #[test]
    fn claim_support_round_trips_null_and_scored_claims() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let support = store
            .claim_support(&report.dossier_id, AS_OF_NOW)
            .expect("claim_support");
        assert_eq!(support.len(), payload.claims.len());

        let assumption = support
            .iter()
            .find(|row| row.claim_id == "clm:chinese-fabs-cannot-access-secondhand-euv")
            .expect("assumption claim's support row present");
        assert_eq!(assumption.support, None);
        assert_eq!(assumption.method, "none");

        let fact = support
            .iter()
            .find(|row| row.claim_id == "clm:smic-achieved-7nm-class-node-2023")
            .expect("fact claim's support row present");
        assert_eq!(fact.support, Some(0.9));
        assert_eq!(fact.method, "jev");
    }

    #[test]
    fn causal_chains_contains_a_path_of_at_least_four_nodes() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let chains = store
            .causal_chains(&report.dossier_id, AS_OF_NOW)
            .expect("causal_chains");
        assert!(
            chains.iter().any(|chain| chain.path.len() >= 4),
            "no chain of length >= 4 among {chains:?}"
        );
    }

    #[test]
    fn cruxes_returns_contested_claims_sorted_by_score_descending() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let cruxes = store.cruxes(&report.dossier_id, AS_OF_NOW).expect("cruxes");
        assert!(!cruxes.is_empty());
        assert!(cruxes.len() <= 3);
        for crux in &cruxes {
            assert!(crux.supports > 0 && crux.contradicts > 0);
        }
        let scores: Vec<i64> = cruxes.iter().map(|crux| crux.score).collect();
        let mut sorted_desc = scores.clone();
        sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(scores, sorted_desc);
    }

    #[test]
    fn consensus_returns_fact_claim_with_at_least_two_sources() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let consensus = store
            .consensus(&report.dossier_id, AS_OF_NOW)
            .expect("consensus");
        assert!(!consensus.is_empty());
        assert!(consensus
            .iter()
            .any(|row| row.id == "clm:smic-achieved-7nm-class-node-2023" && row.sources >= 2));
        for row in &consensus {
            assert!(row.sources >= 2);
        }
    }

    #[test]
    fn temporal_relations_count_matches_fixture() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let rows = store
            .temporal_relations(&report.dossier_id, AS_OF_NOW)
            .expect("temporal_relations");
        assert_eq!(rows.len(), payload.temporal_relations.len());
    }

    #[test]
    fn similar_evidence_top_hit_mentions_smic_or_mate_60() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let rows = store
            .similar_evidence("SMIC 7nm Huawei Mate 60", AS_OF_NOW)
            .expect("similar_evidence");
        assert_eq!(rows.len(), 8);
        for pair in rows.windows(2) {
            assert!(pair[0].distance <= pair[1].distance);
        }
        let top = &rows[0];
        assert!(
            top.excerpt.contains("SMIC") || top.excerpt.contains("Mate 60"),
            "top hit excerpt did not mention SMIC or Mate 60: {}",
            top.excerpt
        );
    }

    #[test]
    fn similar_claims_returns_five_rows_sorted_by_distance() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        let rows = store
            .similar_claims(
                "export controls semiconductor manufacturing China",
                AS_OF_NOW,
            )
            .expect("similar_claims");
        assert_eq!(rows.len(), 5);
        for pair in rows.windows(2) {
            assert!(pair[0].distance <= pair[1].distance);
        }
    }

    #[test]
    fn dossier_meta_round_trips_and_reingest_is_stable() {
        let payload = fixture_payload();
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("first ingest");

        assert_eq!(
            store
                .dossier_meta(&report.dossier_id, AS_OF_NOW)
                .expect("dossier_meta"),
            Some((payload.question.clone(), CREATED_AT.to_string()))
        );

        let second_report = store.ingest(&payload, None, CREATED_AT).expect("re-ingest");
        assert_eq!(second_report, report);
        assert_eq!(
            store
                .entities(&report.dossier_id, AS_OF_NOW)
                .expect("entities after re-ingest")
                .len(),
            payload.entities.len()
        );
        assert_eq!(
            store
                .evidence(&report.dossier_id, AS_OF_NOW)
                .expect("evidence after re-ingest")
                .len(),
            payload.evidence.len()
        );
    }

    #[test]
    fn minimal_payload_with_only_entities_ingests_fine() {
        let payload = small_payload("Does a minimal payload ingest?", "ent:only-one", "Only One");
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest minimal payload");
        assert_eq!(report.entities, 1);
        assert_eq!(report.evidence, 0);

        let entities = store
            .entities(&report.dossier_id, AS_OF_NOW)
            .expect("entities");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, "ent:only-one");
    }

    #[test]
    fn failed_block_in_chained_script_rolls_back_earlier_blocks() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let mut params: BTreeMap<String, DataValue> = BTreeMap::new();
        params.insert(
            "entities".to_string(),
            DataValue::List(vec![DataValue::List(vec![
                DataValue::from("ent:rollback-test"),
                DataValue::from("Rollback Test"),
                DataValue::from("organization"),
                DataValue::from("Should not survive the failed second block."),
            ])]),
        );

        let script = r#"
{ ?[id, name, kind, description] <- $entities
  :put entity {id => name, kind, description} }
{ ?[a, b] <- [[1, 2]]
  :put relation_that_does_not_exist {a, b} }
"#;

        let result = store
            .db
            .run_script(script, params, ScriptMutability::Mutable);
        assert!(result.is_err(), "expected the chained script to fail");

        let rows = store
            .run(
                "?[id, name, kind, description] := *entity{id, name, kind, description} \
                 :as_of $as_of",
                as_of_param(AS_OF_NOW),
            )
            .expect("query entity relation directly");
        assert!(
            rows.rows.is_empty(),
            "entity block should have been rolled back: {:?}",
            rows.rows
        );
    }

    #[test]
    fn open_sqlite_persists_across_reopen() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db_path = dir.path().join("iw.db");

        {
            let store = GraphStore::open_sqlite(&db_path).expect("open sqlite store");
            store.init_schema().expect("init schema");
            let payload = fixture_payload();
            store
                .ingest(&payload, None, CREATED_AT)
                .expect("ingest fixture");
        }

        let reopened = GraphStore::open_sqlite(&db_path).expect("reopen sqlite store");
        let dossier_id = dossier_id_for(&fixture_payload().question);
        assert_eq!(
            reopened
                .dossier_meta(&dossier_id, AS_OF_NOW)
                .expect("dossier_meta after reopen")
                .map(|(question, _)| question),
            Some(fixture_payload().question)
        );
    }

    #[test]
    fn dossiers_are_scoped_but_vector_search_is_global() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let payload_a = fixture_payload();
        let report_a = store
            .ingest(&payload_a, None, CREATED_AT)
            .expect("ingest A");

        let b_claim_text = "Dossier B's own minimal claim for cross-dossier vector testing.";
        let payload_b = ExtractionPayload {
            schema_version: 2,
            question: "Is unrelated dossier B fully isolated from dossier A?".to_string(),
            entities: vec![Entity {
                id: "ent:dossier-b-only".to_string(),
                name: "Dossier B Only Entity".to_string(),
                kind: "organization".to_string(),
                description: "A minimal handcrafted entity.".to_string(),
            }],
            events: vec![],
            sources: vec![],
            claims: vec![Claim {
                id: "clm:dossier-b-only".to_string(),
                text: b_claim_text.to_string(),
                kind: "assumption".to_string(),
                subject_ids: vec!["ent:dossier-b-only".to_string()],
                support: None,
                support_method: "none".to_string(),
            }],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        };
        let report_b = store
            .ingest(&payload_b, None, CREATED_AT)
            .expect("ingest B");

        let entities_a = store
            .entities(&report_a.dossier_id, AS_OF_NOW)
            .expect("entities A");
        assert!(!entities_a.iter().any(|row| row.id == "ent:dossier-b-only"));

        let entities_b = store
            .entities(&report_b.dossier_id, AS_OF_NOW)
            .expect("entities B");
        assert!(entities_b.iter().all(|row| row.id == "ent:dossier-b-only"));
        assert!(!entities_b
            .iter()
            .any(|row| payload_a.entities.iter().any(|entity| entity.id == row.id)));

        // Querying with dossier B's own claim text guarantees it is the
        // nearest match (distance 0), so the remaining slots in a 5-row
        // result must come from dossier A: proof that similar_claims does
        // not scope by dossier.
        let similar = store
            .similar_claims(b_claim_text, AS_OF_NOW)
            .expect("similar_claims across dossiers");
        assert_eq!(similar.len(), 5);
        assert!(similar.iter().any(|row| row.id == "clm:dossier-b-only"));
        assert!(similar
            .iter()
            .any(|row| payload_a.claims.iter().any(|claim| claim.id == row.id)));
    }

    /// Builds a handcrafted dossier B that re-declares one of dossier A's
    /// events (`evt:bis-export-controls-2022`) and one of A's claims
    /// (`clm:controls-durably-slow-china`) with identical stored fields,
    /// then adds B-only nodes and edges that touch those shared ids: an
    /// event actor and a claim subject pointing at a B-only entity, a
    /// causal link from the shared event to the shared claim, a temporal
    /// relation from the shared event to a B-only event, and a
    /// contradicting, quality-0.95 evidence item citing a B-only source.
    fn dossier_b_sharing_ids_with_fixture_a() -> ExtractionPayload {
        let b_only_source = make_source(
            "wikipedia",
            "B Only Source",
            "https://example.com/b-only-source",
            "B-only source content for the cross-dossier leak test.",
        );
        ExtractionPayload {
            schema_version: 2,
            question: "Did the October 2022 rule directly cause the B-only claim in this \
                        handcrafted dossier?"
                .to_string(),
            entities: vec![Entity {
                id: "ent:b-only-actor".to_string(),
                name: "B Only Actor".to_string(),
                kind: "organization".to_string(),
                description: "A dossier-B-only entity for the cross-dossier leak test.".to_string(),
            }],
            events: vec![
                crate::model::Event {
                    id: "evt:bis-export-controls-2022".to_string(),
                    name: "BIS publishes October 2022 export control rule".to_string(),
                    occurred_at: "2022-10-07".to_string(),
                    description: "The Bureau of Industry and Security issued a rule imposing \
                                   license requirements on exports to China of advanced logic \
                                   chips, high-bandwidth memory, and the equipment used to \
                                   manufacture them."
                        .to_string(),
                    actor_ids: vec!["ent:b-only-actor".to_string()],
                },
                crate::model::Event {
                    id: "evt:b-only-event".to_string(),
                    name: "B Only Event".to_string(),
                    occurred_at: "2024-01-01".to_string(),
                    description: "A dossier-B-only event for the cross-dossier leak test."
                        .to_string(),
                    actor_ids: vec![],
                },
            ],
            sources: vec![b_only_source.clone()],
            claims: vec![Claim {
                id: "clm:controls-durably-slow-china".to_string(),
                text: "Export controls durably slow China's access to advanced semiconductor \
                       manufacturing capability, rather than merely delaying it."
                    .to_string(),
                kind: "hypothesis".to_string(),
                subject_ids: vec!["ent:b-only-actor".to_string()],
                support: Some(0.2),
                support_method: "jev".to_string(),
            }],
            evidence: vec![Evidence {
                id: "evd:b-only-evidence".to_string(),
                claim_id: "clm:controls-durably-slow-china".to_string(),
                source_id: b_only_source.id,
                stance: "contradicts".to_string(),
                excerpt: "Dossier B's own evidence contradicting the shared claim.".to_string(),
                quality: 0.95,
            }],
            causal_links: vec![CausalLink {
                cause_id: "evt:bis-export-controls-2022".to_string(),
                effect_id: "clm:controls-durably-slow-china".to_string(),
                mechanism: "Dossier B's own causal link from the shared event to the shared \
                            claim."
                    .to_string(),
                confidence: "low".to_string(),
            }],
            temporal_relations: vec![crate::model::TemporalRelation {
                before_id: "evt:bis-export-controls-2022".to_string(),
                after_id: "evt:b-only-event".to_string(),
                relation: "before".to_string(),
            }],
            gate_log: vec![],
            dropped_sources: vec![],
        }
    }

    #[test]
    fn shared_ids_do_not_leak_across_dossiers() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let payload_a = fixture_payload();
        let report_a = store
            .ingest(&payload_a, None, CREATED_AT)
            .expect("ingest A");

        let event_actors_before = store
            .event_actors(&report_a.dossier_id, AS_OF_NOW)
            .expect("event_actors before B");
        let claim_subjects_before = store
            .claim_subjects(&report_a.dossier_id, AS_OF_NOW)
            .expect("claim_subjects before B");
        let claims_with_stance_before = store
            .claims_with_stance(&report_a.dossier_id, AS_OF_NOW)
            .expect("claims_with_stance before B");
        let causal_links_before = store
            .causal_links(&report_a.dossier_id, AS_OF_NOW)
            .expect("causal_links before B");
        let causal_chains_before = store
            .causal_chains(&report_a.dossier_id, AS_OF_NOW)
            .expect("causal_chains before B");
        let cruxes_before = store
            .cruxes(&report_a.dossier_id, AS_OF_NOW)
            .expect("cruxes before B");
        let consensus_before = store
            .consensus(&report_a.dossier_id, AS_OF_NOW)
            .expect("consensus before B");
        let temporal_before = store
            .temporal_relations(&report_a.dossier_id, AS_OF_NOW)
            .expect("temporal_relations before B");

        let payload_b = dossier_b_sharing_ids_with_fixture_a();
        payload_b.validate().expect("payload B is valid");
        store
            .ingest(&payload_b, None, CREATED_AT)
            .expect("ingest B");

        assert_eq!(
            store
                .event_actors(&report_a.dossier_id, AS_OF_NOW)
                .expect("event_actors after B"),
            event_actors_before
        );
        assert_eq!(
            store
                .claim_subjects(&report_a.dossier_id, AS_OF_NOW)
                .expect("claim_subjects after B"),
            claim_subjects_before
        );
        assert_eq!(
            store
                .claims_with_stance(&report_a.dossier_id, AS_OF_NOW)
                .expect("claims_with_stance after B"),
            claims_with_stance_before
        );
        assert_eq!(
            store
                .causal_links(&report_a.dossier_id, AS_OF_NOW)
                .expect("causal_links after B"),
            causal_links_before
        );
        assert_eq!(
            store
                .causal_chains(&report_a.dossier_id, AS_OF_NOW)
                .expect("causal_chains after B"),
            causal_chains_before
        );
        assert_eq!(
            store
                .cruxes(&report_a.dossier_id, AS_OF_NOW)
                .expect("cruxes after B"),
            cruxes_before
        );
        assert_eq!(
            store
                .consensus(&report_a.dossier_id, AS_OF_NOW)
                .expect("consensus after B"),
            consensus_before
        );
        assert_eq!(
            store
                .temporal_relations(&report_a.dossier_id, AS_OF_NOW)
                .expect("temporal_relations after B"),
            temporal_before
        );
    }

    /// A handcrafted three-node causal cycle (`clm:cycle-1 -> clm:cycle-2 ->
    /// clm:cycle-3 -> clm:cycle-1`), each claim contested (both supporting
    /// and contradicting evidence). Proves `CAUSAL_CHAINS`'s `!is_in(c, p)`
    /// guard stops a chain from revisiting a node, and `CRUXES`'s `a != c`
    /// guard in `reach` stops a crux from counting itself among its own
    /// downstream effects: each of the 3 nodes should reach exactly the
    /// other 2, never itself.
    #[test]
    fn causal_chains_do_not_revisit_nodes() {
        let source = make_source(
            "wikipedia",
            "Cycle Source",
            "https://example.com/cycle-source",
            "Cycle source content shared by every claim in the three-node cycle test.",
        );

        let mut claims = Vec::new();
        let mut evidence = Vec::new();
        for n in 1..=3 {
            let claim_id = format!("clm:cycle-{n}");
            claims.push(Claim {
                id: claim_id.clone(),
                text: format!("Contested claim {n} in a three-node causal cycle."),
                kind: "hypothesis".to_string(),
                subject_ids: vec![],
                support: Some(0.5),
                support_method: "jev".to_string(),
            });
            evidence.push(Evidence {
                id: format!("evd:cycle-{n}-supports"),
                claim_id: claim_id.clone(),
                source_id: source.id.clone(),
                stance: "supports".to_string(),
                excerpt: format!("Supporting excerpt for cycle claim {n}."),
                quality: 0.5,
            });
            evidence.push(Evidence {
                id: format!("evd:cycle-{n}-contradicts"),
                claim_id,
                source_id: source.id.clone(),
                stance: "contradicts".to_string(),
                excerpt: format!("Contradicting excerpt for cycle claim {n}."),
                quality: 0.5,
            });
        }

        let causal_links = vec![
            CausalLink {
                cause_id: "clm:cycle-1".to_string(),
                effect_id: "clm:cycle-2".to_string(),
                mechanism: "Cycle edge 1 -> 2.".to_string(),
                confidence: "low".to_string(),
            },
            CausalLink {
                cause_id: "clm:cycle-2".to_string(),
                effect_id: "clm:cycle-3".to_string(),
                mechanism: "Cycle edge 2 -> 3.".to_string(),
                confidence: "low".to_string(),
            },
            CausalLink {
                cause_id: "clm:cycle-3".to_string(),
                effect_id: "clm:cycle-1".to_string(),
                mechanism: "Cycle edge 3 -> 1.".to_string(),
                confidence: "low".to_string(),
            },
        ];

        let payload = ExtractionPayload {
            schema_version: 2,
            question: "Does a three-node causal cycle stay well-formed under the chain and \
                        crux queries?"
                .to_string(),
            entities: vec![],
            events: vec![],
            sources: vec![source],
            claims,
            evidence,
            causal_links,
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        };
        payload.validate().expect("cycle payload is valid");

        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest cycle payload");

        let chains = store
            .causal_chains(&report.dossier_id, AS_OF_NOW)
            .expect("causal_chains");
        assert!(!chains.is_empty());
        for chain in &chains {
            let mut seen = std::collections::HashSet::new();
            assert!(
                chain.path.iter().all(|id| seen.insert(id.clone())),
                "chain path revisits a node: {:?}",
                chain.path
            );
        }

        let cruxes = store.cruxes(&report.dossier_id, AS_OF_NOW).expect("cruxes");
        assert_eq!(
            cruxes.len(),
            3,
            "all three cycle claims should be contested"
        );
        for crux in &cruxes {
            assert_eq!(
                crux.downstream, 2,
                "crux {} should reach exactly the other two cycle nodes, not itself: {crux:?}",
                crux.id
            );
        }
    }

    // --- Schema v2 / bitemporality proofs -----------------------------

    /// `::hnsw create` on a `tt`-stamped relation is rejected by mnestic
    /// 0.18.0 (verified empirically against both `mem` and `sqlite` before
    /// this schema was written; see the build log): this is *why*
    /// `claim`/`evidence` embeddings live in the plain side relations
    /// `claim_vec`/`evidence_vec` instead of on `claim`/`evidence`
    /// themselves. This test keeps that fact under CI rather than only in
    /// the build log, so a future mnestic upgrade that lifted the
    /// restriction (or silently re-imposed a different one) would be
    /// caught here.
    #[test]
    fn hnsw_index_creation_is_rejected_on_a_txtime_relation() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cases: Vec<(&str, Option<std::path::PathBuf>)> = vec![
            ("mem", None),
            ("sqlite", Some(dir.path().join("hnsw-tt-probe.db"))),
        ];
        for (engine, path) in cases {
            let db = match &path {
                None => DbInstance::new(engine, "", "").expect("open db"),
                Some(path) => DbInstance::new(engine, path, "").expect("open db"),
            };
            db.run_script(
                "{:create probe_tt_vec {id: String, tt: TxTime => embedding: <F32; 4>}}",
                BTreeMap::new(),
                ScriptMutability::Mutable,
            )
            .expect("create tt-stamped relation");
            let result = db.run_script(
                "::hnsw create probe_tt_vec:idx {dim: 4, m: 16, dtype: F32, \
                 fields: [embedding], distance: Cosine, ef_construction: 64}",
                BTreeMap::new(),
                ScriptMutability::Mutable,
            );
            assert!(
                result.is_err(),
                "[{engine}] expected HNSW create on a TxTime relation to fail"
            );
            let message = format!("{:?}", result.unwrap_err());
            assert!(
                message.contains("TxTime") || message.contains("transaction-time"),
                "[{engine}] unexpected error message: {message}"
            );
        }
    }

    /// Re-ingesting a dossier whose source content changed keeps BOTH
    /// versions reachable: current state shows the new content_hash, and
    /// `as_of` a point between the two ingests shows the old one
    /// (`docs/contracts.md` decision 2, instruction (a)).
    #[test]
    fn reingest_with_changed_source_content_keeps_both_versions() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let original = fixture_payload();
        let updated = fixture_payload_updated();
        assert_eq!(
            dossier_id_for(&original.question),
            dossier_id_for(&updated.question),
            "fixtures must share a dossier id for this test to be meaningful"
        );
        let target_source_id = "src:wikipedia:c0d661fe10e6d398";
        let old_hash = original
            .sources
            .iter()
            .find(|s| s.id == target_source_id)
            .expect("target source in original fixture")
            .content_hash
            .clone();
        let new_hash = updated
            .sources
            .iter()
            .find(|s| s.id == target_source_id)
            .expect("target source in updated fixture")
            .content_hash
            .clone();
        assert_ne!(old_hash, new_hash, "fixtures must actually differ");

        let first_created_at = "2026-09-14T00:00:00Z";
        let report_1 = store
            .ingest(&original, None, first_created_at)
            .expect("first ingest");

        // A real wall-clock gap so the two ingests land in different
        // whole seconds: the public `as_of` contract is second-precision
        // RFC 3339, so a between-point at sub-second resolution could not
        // be expressed as an HTTP query parameter even if mnestic's
        // internal clock is finer-grained.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let between = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let second_created_at =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let report_2 = store
            .ingest(&updated, None, &second_created_at)
            .expect("second ingest");
        assert_eq!(report_1.dossier_id, report_2.dossier_id);

        let current_sources = store
            .sources(&report_2.dossier_id, AS_OF_NOW)
            .expect("current sources");
        let current = current_sources
            .iter()
            .find(|s| s.id == target_source_id)
            .expect("target source present currently");
        assert_eq!(
            current.content_hash, new_hash,
            "current state should be the new version"
        );

        let past_sources = store
            .sources(&report_2.dossier_id, &between)
            .expect("sources as of the between point");
        let past = past_sources
            .iter()
            .find(|s| s.id == target_source_id)
            .expect("target source present as of the between point");
        assert_eq!(
            past.content_hash, old_hash,
            "as_of between the two ingests should be the old version"
        );

        // The blob relation is content-addressed and keeps both bodies
        // (nothing was overwritten): both hashes resolve to their own
        // distinct content.
        let old_content = store
            .blob_content(&old_hash)
            .expect("blob_content old")
            .expect("old blob present");
        let new_content = store
            .blob_content(&new_hash)
            .expect("blob_content new")
            .expect("new blob present");
        assert_ne!(old_content, new_content);
    }

    /// A dossier queried `as_of` a point strictly before its first ingest
    /// does not exist yet: `dossier_meta` and every dossier-scoped query
    /// return empty, not an error (`docs/contracts.md` §C3: "Anything
    /// before the first write returns empty results, not an error";
    /// instruction (b), the non-HTTP half — the HTTP-level proof against
    /// `GET /api/dossiers/{id}/briefing?as_of` lives in `tests/roundtrip.rs`).
    #[test]
    fn as_of_before_first_ingest_is_empty_not_an_error() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let payload = fixture_payload();
        let before = "2000-01-01T00:00:00Z";
        std::thread::sleep(std::time::Duration::from_millis(50));
        let report = store
            .ingest(&payload, None, CREATED_AT)
            .expect("ingest fixture");

        assert_eq!(
            store
                .dossier_meta(&report.dossier_id, before)
                .expect("dossier_meta as_of before ingest"),
            None
        );
        assert!(store
            .entities(&report.dossier_id, before)
            .expect("entities as_of before ingest")
            .is_empty());
        assert!(store
            .claims_with_stance(&report.dossier_id, before)
            .expect("claims_with_stance as_of before ingest")
            .is_empty());
    }

    /// Family lifecycle: minting picks the base slug, then `-2`/`-3` on
    /// collision; merging is prospective (the absorbed family's own row is
    /// unaffected, only `merged_into` changes); `resolve_family_id` follows
    /// a multi-hop merge chain to the live root and stops on a
    /// self-referential cycle rather than looping.
    #[test]
    fn family_minting_and_merge_chain_resolution() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let first = store
            .mint_family_id("ECB rate decisions")
            .expect("mint first");
        assert_eq!(first, "fam:ecb-rate-decisions");
        store
            .insert_family(&first, "ECB rate decisions", "d1", CREATED_AT)
            .expect("insert first");

        let second = store
            .mint_family_id("ECB rate decisions")
            .expect("mint second");
        assert_eq!(second, "fam:ecb-rate-decisions-2");
        store
            .insert_family(&second, "ECB rate decisions", "d2", CREATED_AT)
            .expect("insert second");

        let third_label = "ECB Rate Decisions"; // same slug as the first two
        let third = store.mint_family_id(third_label).expect("mint third");
        assert_eq!(third, "fam:ecb-rate-decisions-3");
        store
            .insert_family(&third, third_label, "d3", CREATED_AT)
            .expect("insert third");

        // Multi-hop chain: third -> second -> first.
        store
            .merge_family(&third, &second)
            .expect("merge third into second");
        store
            .merge_family(&second, &first)
            .expect("merge second into first");

        let families = store.all_families(AS_OF_NOW).expect("all_families");
        assert_eq!(GraphStore::resolve_family_id(&families, &third), first);
        assert_eq!(GraphStore::resolve_family_id(&families, &second), first);
        assert_eq!(GraphStore::resolve_family_id(&families, &first), first);

        // The absorbed family's own label/description survive the merge
        // (only merged_into changed); only `merged_into` changed.
        let second_row = families
            .iter()
            .find(|f| f.id == second)
            .expect("second family row");
        assert_eq!(second_row.label, "ECB rate decisions");
        assert_eq!(second_row.description, "d2");
        assert_eq!(second_row.merged_into.as_deref(), Some(first.as_str()));

        // A self-referential cycle must not hang resolve_family_id.
        let mut cyclic = families.clone();
        cyclic.push(FamilyRow {
            id: "fam:cycle-a".to_string(),
            label: "Cycle A".to_string(),
            description: String::new(),
            created_at: CREATED_AT.to_string(),
            merged_into: Some("fam:cycle-b".to_string()),
        });
        cyclic.push(FamilyRow {
            id: "fam:cycle-b".to_string(),
            label: "Cycle B".to_string(),
            description: String::new(),
            created_at: CREATED_AT.to_string(),
            merged_into: Some("fam:cycle-a".to_string()),
        });
        let resolved = GraphStore::resolve_family_id(&cyclic, "fam:cycle-a");
        assert!(resolved == "fam:cycle-a" || resolved == "fam:cycle-b");
    }

    /// `dossiers_for_family` follows `dossier_question` -> `question_family`
    /// (resolved through merge chains) and orders newest dossier first;
    /// `claim_views_for_dossiers` concatenates in that order and truncates
    /// to the caller's limit. `documents`'s family filter uses the same
    /// resolution and returns only that family's sources (instruction (c)).
    #[test]
    fn family_scoped_reads_resolve_tags_order_and_filter_correctly() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        store
            .insert_family("fam:root", "Root Family", "root", CREATED_AT)
            .expect("insert root family");
        store
            .insert_family("fam:absorbed", "Absorbed Family", "absorbed", CREATED_AT)
            .expect("insert absorbed family");
        store
            .merge_family("fam:absorbed", "fam:root")
            .expect("merge absorbed into root");

        let older_payload = small_payload(
            "Older question tagged directly to the root family?",
            "ent:older",
            "Older Entity",
        );
        let older_created_at = "2026-01-01T00:00:00Z";
        let older_report = store
            .ingest(&older_payload, Some("q:older"), older_created_at)
            .expect("ingest older dossier");
        store
            .record_question_family(
                "q:older",
                "fam:root",
                Some(0.4),
                "jev",
                &older_payload.question,
            )
            .expect("tag older question to root");

        let newer_source = make_source(
            "wikipedia",
            "Newer Source",
            "https://example.com/newer-source",
            "Newer source content for the family-scoped documents filter test.",
        );
        let newer_payload = ExtractionPayload {
            schema_version: 2,
            question: "Newer question tagged to the absorbed family?".to_string(),
            entities: vec![],
            events: vec![],
            sources: vec![newer_source.clone()],
            claims: vec![Claim {
                id: "clm:newer-claim".to_string(),
                text: "A newer claim for the family-scoped ordering test.".to_string(),
                kind: "hypothesis".to_string(),
                subject_ids: vec![],
                support: Some(0.6),
                support_method: "jev".to_string(),
            }],
            evidence: vec![],
            causal_links: vec![],
            temporal_relations: vec![],
            gate_log: vec![],
            dropped_sources: vec![],
        };
        let newer_created_at = "2026-06-01T00:00:00Z";
        let newer_report = store
            .ingest(&newer_payload, Some("q:newer"), newer_created_at)
            .expect("ingest newer dossier");
        // Tagged to the now-absorbed family id, on purpose: resolution must
        // still land on the live root, both for the family-claims read and
        // for the documents filter.
        store
            .record_question_family(
                "q:newer",
                "fam:absorbed",
                Some(0.7),
                "jev",
                &newer_payload.question,
            )
            .expect("tag newer question to absorbed family");

        let dossier_ids = store
            .dossiers_for_family("fam:root", AS_OF_NOW)
            .expect("dossiers_for_family");
        assert_eq!(
            dossier_ids,
            vec![
                newer_report.dossier_id.clone(),
                older_report.dossier_id.clone()
            ],
            "newest dossier (by created_at) must come first"
        );

        let views = store
            .claim_views_for_dossiers(&dossier_ids, AS_OF_NOW, 1)
            .expect("claim_views_for_dossiers truncated to 1");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].claim_id, "clm:newer-claim");

        let documents = store
            .documents(AS_OF_NOW, Some("fam:root"), 50, false)
            .expect("documents scoped to fam:root");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].source_id, newer_source.id);

        // Querying by the absorbed id directly must resolve to the same
        // result as querying by the live root id.
        let documents_via_absorbed = store
            .documents(AS_OF_NOW, Some("fam:absorbed"), 50, false)
            .expect("documents scoped to fam:absorbed");
        assert_eq!(documents_via_absorbed, documents);
    }

    #[test]
    fn history_upserts_and_json_forecast_shapes_round_trip() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let binary = HistoryItem {
            id: "metaculus:41234".to_string(),
            kind: "personal".to_string(),
            title: "Will the ECB cut rates?".to_string(),
            url: "https://www.metaculus.com/questions/41234".to_string(),
            question_type: "binary".to_string(),
            forecast: serde_json::json!(0.42),
            resolution: None,
            resolved_at: None,
            family_id: Some("fam:ecb-rate-decisions".to_string()),
            note: String::new(),
        };
        let multiple_choice = HistoryItem {
            id: "metaculus:5678".to_string(),
            kind: "bot".to_string(),
            title: "Which candidate wins?".to_string(),
            url: "https://www.metaculus.com/questions/5678".to_string(),
            question_type: "multiple_choice".to_string(),
            forecast: serde_json::json!({"yes": 0.6, "no": 0.4}),
            resolution: Some("yes".to_string()),
            resolved_at: Some("2026-08-01T00:00:00Z".to_string()),
            family_id: None,
            note: "resolved".to_string(),
        };
        store
            .upsert_history(&[binary.clone(), multiple_choice.clone()])
            .expect("upsert_history");

        let all = store.all_history(AS_OF_NOW).expect("all_history");
        assert_eq!(all.len(), 2);
        let binary_row = all
            .iter()
            .find(|item| item.id == binary.id)
            .expect("binary row");
        assert_eq!(binary_row.forecast, serde_json::json!(0.42));
        assert_eq!(binary_row.resolution, None);
        let mc_row = all
            .iter()
            .find(|item| item.id == multiple_choice.id)
            .expect("multiple_choice row");
        assert_eq!(mc_row.forecast, serde_json::json!({"yes": 0.6, "no": 0.4}));
        assert_eq!(mc_row.resolution.as_deref(), Some("yes"));

        // Upserting the same id again writes a new tt version, not a
        // second row.
        let mut updated_binary = binary.clone();
        updated_binary.resolution = Some("yes".to_string());
        updated_binary.resolved_at = Some("2026-09-01T00:00:00Z".to_string());
        store
            .upsert_history(std::slice::from_ref(&updated_binary))
            .expect("re-upsert binary");
        let all_after = store
            .all_history(AS_OF_NOW)
            .expect("all_history after re-upsert");
        assert_eq!(all_after.len(), 2, "re-upsert must not duplicate the row");
        let binary_after = all_after
            .iter()
            .find(|item| item.id == binary.id)
            .expect("binary row after re-upsert");
        assert_eq!(binary_after.resolution.as_deref(), Some("yes"));
    }

    #[test]
    fn ingest_raw_documents_computes_content_addressed_ids_and_hashes() {
        let store = GraphStore::open_memory().expect("open store");
        store.init_schema().expect("init schema");

        let url = "https://example.com/outbox-document";
        let content = "Content spooled from the bot's outbox during a degraded-mode replay.";
        let document = RawDocument {
            url: url.to_string(),
            title: "Outbox Document".to_string(),
            provider: "asknews_news".to_string(),
            published: String::new(),
            fetched_at: CREATED_AT.to_string(),
            content: content.to_string(),
        };

        let ingested = store
            .ingest_raw_documents(std::slice::from_ref(&document))
            .expect("ingest_raw_documents");
        assert_eq!(ingested, 1);

        let expected_id = format!("src:asknews_news:{}", &sha256_hex(url)[..16]);
        let all = store.all_sources(AS_OF_NOW, 10).expect("all_sources");
        let row = all
            .iter()
            .find(|source| source.id == expected_id)
            .expect("raw document present with the expected content-addressed id");
        assert_eq!(row.content_hash, sha256_hex(content));
        assert_eq!(
            store
                .blob_content(&row.content_hash)
                .expect("blob_content")
                .as_deref(),
            Some(content)
        );
    }
}
